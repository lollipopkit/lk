//! Static `trait`/`impl`/`struct` declarations carried from the compiler to
//! every back end.
//!
//! # Why this exists
//!
//! `trait`/`impl` declarations are known in full at compile time — the
//! compiler holds the trait name, the target type, each method name and the
//! function index it compiled the body into. Before this module that
//! structured knowledge was immediately thrown away: `lower_impl_decl`
//! serialized it into *string literals* plus a runtime
//! `__lk_register_trait_impl` call, and every consumer then reconstructed it
//! from a lower-level form —
//!
//! - the VM re-parsed the strings back into `Type`s while executing the
//!   registration calls, and
//! - the AOT lowering ran a 174-line abstract interpreter over the entry
//!   function's bytecode to recover the same table.
//!
//! [`TypeInfo`] is that knowledge kept in its structured form and carried
//! through `ModuleArtifact`, so a back end reads it instead of rebuilding it.
//!
//! A value's own type identity is **not** here: see [`crate::val::DeclaredType`].
//! The two halves shared this file and referenced each other **not once** —
//! which is how the value half ended up under `vm/` unnoticed.
//!
//! # Representation notes
//!
//! Types are stored as their `Type::display()` text rather than a structured
//! `Type`. That is deliberate for now: it is exactly what the previous
//! string-literal encoding carried, so this change moves *where* the data
//! lives without also changing *what* it says. Making `Type` itself
//! serializable is a separate step.
//!
//! Methods reference their compiled body by **function index**, never by a
//! runtime value — that is what makes this serializable at all, and what lets
//! the AOT path use it without a VM in the picture.
//!
//! # This is now the only source
//!
//! The runtime `__lk_register_trait{,_impl}` calls are gone: the compiler emits
//! no instructions for a declaration, the VM builds its method table from here
//! (`VmContext::register_module_types`) before any user code runs, and the AOT
//! lowering reads it directly.
//!
//! The GC hazard that blocked this is gone with it rather than being patched
//! around. `TypeRegistry` used to hold method closures as `RuntimeVal` heap
//! handles while **not being a GC root** (`ExecutorState::gc_roots` covers
//! globals, stack, pending raise, and host roots) — safe only by accident,
//! because a method was registered while its closure still sat in a register.
//! The registry now stores function *indices*, so it holds no handles at all
//! and the question does not arise. Nothing is materialized on the way to a
//! call either: `vm::call_trait_method` dispatches straight from the table
//! entry, because an index plus its module is already everything a call needs.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use serde::{Deserialize, Serialize};

/// One `trait` declaration: the method names it requires and their declared
/// types (as display text).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TraitDecl {
    pub name: String,
    /// `(method name, declared type text)`, in declaration order.
    pub methods: Vec<(String, String)>,
}

/// One method of an `impl` block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImplMethod {
    pub name: String,
    /// Index into `Module::functions` of the compiled body.
    pub function: u32,
    /// The method's type as display text (receiver included).
    pub ty: String,
    /// Whether this body — or anything transitively reachable from it through
    /// `CallDirect`/`MakeClosure` — executes a `SetGlobal`, **or** can reach
    /// code the walk cannot see (an indirect call).
    ///
    /// Together with [`Self::reads_globals`] this is what decides whether the
    /// method can be dispatched from a *frame belonging to another module*.
    /// Such a call runs the body against the current heap with the declaring
    /// module's globals swapped in, so a *write* would land in that temporary
    /// table and be dropped on restore — the module's state would silently
    /// diverge. Refused instead.
    ///
    /// Computed as a post-pass over the finished function table
    /// (`Compiler::record_impl_method_global_use`) because a method may call a
    /// function compiled after it.
    #[serde(default)]
    pub writes_globals: bool,
    /// The global slots this body's reachable subtree reads, sorted and
    /// deduplicated. Meaningful only when [`Self::writes_globals`] is `false`,
    /// which is also what makes it *complete*: the same walk treats an
    /// unresolvable call as writing, so a method that passes that test has no
    /// unseen code left to read a slot this list omits.
    ///
    /// A cross-module dispatch seeds exactly these slots. Seeding the whole
    /// table is not an option: a module's globals include its imports, and
    /// importing one means reading the exporting module's heap — which is
    /// checked out of its `Arc<Mutex>` whenever that module is the one
    /// executing, i.e. exactly the situation a cross-module dispatch is in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads_globals: Vec<u16>,
}

/// One `impl Trait for Type` block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImplDecl {
    /// `None` for an inherent `impl Type { … }`: the methods belong to the
    /// type, not to a trait it satisfies. Dispatch never needed the trait — it
    /// keys on the target type — which is why an *empty* trait plus an impl of
    /// it was the workaround before the syntax existed.
    pub trait_name: Option<String>,
    /// Target type as display text (the key both back ends dispatch on).
    pub type_name: String,
    pub methods: Vec<ImplMethod>,
}

/// The compiler's static type declarations for a module.
///
/// Order is significant and is the compiler's declaration order: the runtime
/// type ids the AOT backend assigns (and the VM's registration order) derive
/// from it, so round-tripping must preserve it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TypeInfo {
    pub traits: Vec<TraitDecl>,
    pub impls: Vec<ImplDecl>,
    /// Each `struct` this module declares, with its field names in declaration
    /// order — what `display` prints them in. See [`DeclaredType::fields`].
    #[serde(default)]
    pub structs: Vec<StructDecl>,
}

/// One `struct` declaration: its name and its field names, in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StructDecl {
    pub name: String,
    /// Field names, in declaration order.
    pub fields: Vec<String>,
}

impl TypeInfo {
    /// Whether the module declared no traits or impls — the common case, kept
    /// cheap so callers can skip work entirely.
    pub fn is_empty(&self) -> bool {
        self.traits.is_empty() && self.impls.is_empty() && self.structs.is_empty()
    }

    /// The field order of a `struct` this module declares.
    pub fn struct_fields(&self, name: &str) -> Option<&[String]> {
        self.structs
            .iter()
            .find(|decl| decl.name == name)
            .map(|decl| decl.fields.as_slice())
    }

    /// The declaration of whichever impl method compiled to `function`.
    ///
    /// Used on the cross-module dispatch path to recover the global-use facts
    /// that `MethodImpl::Local` deliberately does not carry. A linear scan is
    /// right here: the path is cold, and a module has a handful of impls.
    pub fn method_by_function(&self, function: u32) -> Option<&ImplMethod> {
        self.impls
            .iter()
            .flat_map(|decl| decl.methods.iter())
            .find(|method| method.function == function)
    }

    /// Looks up the compiled body of `type_name::method_name`, searching the
    /// impls in declaration order (matching `TypeRegistry::get_method`, which
    /// returns the first matching impl).
    pub fn impl_method(&self, type_name: &str, method_name: &str) -> Option<u32> {
        self.impls
            .iter()
            .filter(|decl| decl.type_name == type_name)
            .find_map(|decl| {
                decl.methods
                    .iter()
                    .find(|method| method.name == method_name)
                    .map(|method| method.function)
            })
    }
}
