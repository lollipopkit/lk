//! A value's **type identity**: which module declared a named type, and its name.
//!
//! `struct Point` in `a.lk` and `struct Point` in `b.lk` are different types,
//! so a name alone does not identify one — see [`TypeScope`].
//!
//! This lived under `vm/`, bundled with the compiler's `trait`/`impl` tables
//! (`TypeInfo`, still there) purely because both were "type information". They
//! are not the same thing: those tables are a *module artifact* the compiler
//! hands to a back end, while this is a property of a **value** —
//! `RuntimeObject` embeds an `Arc<DeclaredType>`. Being in `vm` made `val`
//! name `vm`, which is half of the `val` <-> `vm` cycle recorded in
//! `CLAUDE.md`; the remaining half is the callable payload, and that one is a
//! real redesign rather than a move.

use alloc::sync::Arc;
use serde::{Deserialize, Serialize};

/// Identity of the module that *declares* a named type.
///
/// # Why a declared type needs more than its name
///
/// `struct Point` in `a.lk` and `struct Point` in `b.lk` are different types.
/// The runtime used to disagree: an object carried only `"Point"` and the
/// dispatch table was keyed by that bare string, so whichever module registered
/// last owned the name for the whole context — `a.mk(1).tag()` returned `b`'s
/// answer. The same missing half made a *transitive* import fail outright: the
/// importer collected impls one level deep, so a value built by a module its
/// own dependency imported had no reachable methods at all.
///
/// Both are the same hole: identity lived in a name, and a name is only unique
/// inside one module.
///
/// # Why the declaring module is the right scope
///
/// A struct literal can only name a type declared in the same compilation unit
/// — an imported struct is not constructible (`Point { .. }` in the importer is
/// "Unknown struct 'Point'") and not nameable in an annotation. So the module
/// executing the construction *is* the module that declared the type, and
/// stamping the object at construction needs no extra compiler plumbing.
///
/// # Representation
///
/// The normalized source path for a file module, so the identity is stable
/// across processes and can ride in a `ModuleArtifact`. Modules with no file
/// behind them (the entry program, `eval`-style sources, tests) get
/// [`TypeScope::anonymous`], which is distinct from every path and from other
/// anonymous scopes only by being the single scope of that run — good enough,
/// because nothing can import them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TypeScope(Arc<str>);

impl TypeScope {
    /// The scope of a module loaded from `path` (already normalized by the
    /// resolver).
    pub fn from_path(path: &str) -> Self {
        Self(Arc::<str>::from(path))
    }

    /// The scope of a module with no file behind it.
    pub fn anonymous() -> Self {
        Self(Arc::<str>::from("<anon>"))
    }

    /// The one scope shared by every `impl` whose target is a **builtin** type
    /// (`impl Doubler for Int`).
    ///
    /// A builtin type is not declared by anybody, so it has no declaring module
    /// to be scoped to and every module means the same `Int`. Filing those
    /// impls per-module would be wrong in the other direction: the receiver is
    /// a bare `5` with no module attached, so the lookup could never find them.
    ///
    /// TODO(coherence): two modules that both `impl Doubler for Int` still
    /// collide here, last registration winning, because a global type genuinely
    /// admits only one impl. Rejecting the overlap needs an orphan rule, which
    /// is a language decision rather than a dispatch fix.
    pub fn builtin() -> Self {
        Self(Arc::<str>::from("<builtin>"))
    }

    /// Whether this is the shared scope for builtin types (see
    /// [`Self::builtin`]), which admits only one impl per trait.
    pub fn is_builtin(&self) -> bool {
        self.0.as_ref() == "<builtin>"
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Pointer identity — the same scope value, not merely an equal one.
    ///
    /// Every module hands out clones of one `Arc`, so this answers "still the
    /// same module?" without a string compare. The executor asks that on every
    /// activation, which is why it is worth not spelling `==` there.
    #[inline]
    pub fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for TypeScope {
    fn default() -> Self {
        Self::anonymous()
    }
}

/// The full identity of a declared type: which module declared it, and its
/// name. Neither half identifies a type on its own.
///
/// Kept as one heap-allocated value that instances share by `Arc`, rather than
/// as two fields on every object. `RuntimeObject` is the largest `HeapValue`
/// variant and therefore sets the size of *every* heap cell — list, map, string
/// and all — so widening it by a second fat pointer measurably slowed programs
/// that contain no structs at all (~1.3% on the workload suite). One thin
/// pointer instead of the previous bare `Arc<str>` name makes objects smaller
/// than they were before scoping.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeclaredType {
    pub scope: TypeScope,
    pub name: Arc<str>,
    /// The declaration's fields, in the order they were written — empty when
    /// the declaration is not in reach (a struct from another module, or an
    /// object built by a host).
    ///
    /// `display` reads the order, to print a value's fields the way its type
    /// was written: fields live in a map on the object, so without this the
    /// order was the hasher's — `struct Range { start, end }` printed `end`
    /// first, and a hasher change would have silently permuted every struct in
    /// the language. The *declared type* rides along so a store can be checked
    /// against it. It costs nothing per object — instances share one
    /// `DeclaredType` by `Arc`.
    pub fields: Arc<[DeclaredField]>,
    /// Whether any field was written with a type — precomputed because the
    /// answer decides whether a store has to look at all, and a store that
    /// scanned the field list to find out made construction quadratic in the
    /// field count for the (common) type that declares none.
    typed_fields: bool,
}

/// One field of a declared type: the name it was written with, and the type it
/// was written with when it had one.
///
/// `Eq`/`Hash` are over the **name** alone, which the derive cannot do (a
/// `Type` is neither). That is not a shortcut: a declared type is identified by
/// its scope and name, and within one of those a field name occurs once — two
/// fields of one type that agree on the name are the same field.
#[derive(Clone, Debug)]
pub struct DeclaredField {
    pub name: Arc<str>,
    pub ty: Option<crate::val::Type>,
}

impl PartialEq for DeclaredField {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for DeclaredField {}

impl core::hash::Hash for DeclaredField {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl DeclaredField {
    pub fn new(name: Arc<str>, ty: Option<crate::val::Type>) -> Self {
        Self { name, ty }
    }
}

impl DeclaredType {
    pub fn new(scope: TypeScope, name: Arc<str>) -> Self {
        Self {
            scope,
            name,
            fields: Arc::from([] as [DeclaredField; 0]),
            typed_fields: false,
        }
    }

    pub fn with_fields(scope: TypeScope, name: Arc<str>, fields: Arc<[DeclaredField]>) -> Self {
        let typed_fields = fields.iter().any(|field| field.ty.is_some());
        Self {
            scope,
            name,
            fields,
            typed_fields,
        }
    }

    /// The declared type of `field`, when the declaration is in reach and the
    /// field was written with one.
    pub fn field_type(&self, field: &str) -> Option<&crate::val::Type> {
        if !self.typed_fields {
            return None;
        }
        self.fields
            .iter()
            .find(|declared| &*declared.name == field)
            .and_then(|declared| declared.ty.as_ref())
    }
}

impl core::fmt::Display for TypeScope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}
