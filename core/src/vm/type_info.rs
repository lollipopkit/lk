//! Static type declarations carried from the compiler to every back end.
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
}

/// One `impl Trait for Type` block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImplDecl {
    pub trait_name: String,
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
}

impl TypeInfo {
    /// Whether the module declared no traits or impls — the common case, kept
    /// cheap so callers can skip work entirely.
    pub fn is_empty(&self) -> bool {
        self.traits.is_empty() && self.impls.is_empty()
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
