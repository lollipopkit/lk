#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::val::Type;
use anyhow::{Result, anyhow};
use hashbrown::HashMap;

/// Struct definition with field types
#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub fields: HashMap<String, Type>,
}

/// Trait definition with method signatures
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    pub name: String,
    pub methods: HashMap<String, Type>, // method_name -> function_type
}

/// Implementation of a trait for a specific type.
///
/// Methods are identified by the **index of their compiled body**, not by a
/// runtime closure. That keeps this table free of heap handles, which matters
/// for two reasons: the registry is not a GC root (so holding handles here was
/// only safe by accident, while the closure happened to still be live in a
/// register), and it lets the table be built directly from the artifact's
/// [`crate::vm::TypeInfo`] instead of by executing registration calls.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitImpl {
    pub trait_name: String,
    pub target_type: Type,
    /// `method_name -> (index into the module's function table, declared type)`
    pub methods: HashMap<String, (u32, Option<Type>)>,
}

/// Type alias definition
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlias {
    pub name: String,
    pub target_type: Type,
}

/// Registry for managing custom types, traits, and implementations
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeRegistry {
    /// Type aliases: type UserId = Int
    type_aliases: HashMap<String, TypeAlias>,

    /// Struct definitions
    structs: HashMap<String, StructDef>,

    /// Names among `structs` that came from **another module**, not from the
    /// program being checked.
    ///
    /// The two are registered into one table on purpose — an imported type's
    /// fields have to be known to check `m.P { x: 1 }` — but a *bare* `P { … }`
    /// is a different question: it names a type declared here, and building one
    /// for a name that is only imported produced a value that renders `P{x:4}`
    /// and reports `typeof` `P` while carrying none of `P`'s methods (the
    /// runtime stamps the *constructing* module's `TypeScope`, and the method
    /// table is keyed by the declaring one). So the table says which is which.
    imported_structs: crate::compat::collections::HashSet<String>,

    /// Imported structs a bare `P { … }` may still build: the ones brought in
    /// **by name** (`use { P } from "m"`), keyed by the name they are bound
    /// under and holding the name the declaring module gave them.
    ///
    /// That import binds `P` to the constructor the declaring module generates
    /// beside the type (`stmt::struct_ctors`), so the literal has something to
    /// call and the object is built by `m` — identity, field order and dispatch
    /// all right. A type merely *visible* through a namespace import
    /// (`use "m"`) binds no such name, so there the literal has to be written
    /// `m.P { … }`.
    ///
    /// The two names differ under `use { P as Q } from "m"`: the schema, the
    /// methods and the value's own `typeof` are all `P`'s, and only the
    /// spelling at the construction site is `Q` — so a literal written `Q` is
    /// checked, and answers, as a `P`.
    constructible_imports: HashMap<String, String>,

    /// Trait definitions
    traits: HashMap<String, TraitDef>,

    /// Trait implementations per type
    implementations: HashMap<String, Vec<TraitImpl>>, // type_name -> implementations

    /// Type variable counter for fresh variable generation
    type_var_counter: u32,
}

/// Whether an `impl`'s method signature satisfies the trait's declaration.
///
/// One rule, two callers: [`TypeRegistry::validate_trait_impl`], which the VM
/// runs when it registers an impl, and the `impl` statement's own check, which
/// `lk check` runs. The *presence* half was moved to the checker on its own
/// and left this behind — so a method that took the wrong number of arguments,
/// or returned the wrong type, passed `lk check` and failed the moment the
/// program ran, with the pre-flight command saying nothing.
///
/// Parameters are contravariant and the return type covariant, which is the
/// ordinary rule for a signature that has to stand in for another.
pub fn trait_method_conformance(method_name: &str, trait_name: &str, expected: &Type, actual: &Type) -> Result<()> {
    let (
        Type::Function {
            params: exp_params,
            named_params: exp_named,
            return_type: exp_ret,
        },
        Type::Function {
            params: act_params,
            named_params: act_named,
            return_type: act_ret,
        },
    ) = (expected, actual)
    else {
        return Ok(());
    };
    if exp_params.len() != act_params.len() {
        return Err(anyhow!(
            "Method '{}' arity mismatch for trait '{}': expected {}, got {}",
            method_name,
            trait_name,
            exp_params.len(),
            act_params.len()
        ));
    }
    if exp_named.len() != act_named.len() {
        return Err(anyhow!(
            "Method '{}' named parameter count mismatch for trait '{}': expected {}, got {}",
            method_name,
            trait_name,
            exp_named.len(),
            act_named.len()
        ));
    }
    let params_ok = exp_params
        .iter()
        .zip(act_params.iter())
        .all(|(e, a)| a.is_assignable_to(e));
    let named_ok = exp_named.iter().all(|exp_np| {
        act_named
            .iter()
            .find(|act_np| act_np.name == exp_np.name)
            .map(|act_np| act_np.has_default == exp_np.has_default && act_np.ty.is_assignable_to(&exp_np.ty))
            .unwrap_or(false)
    });
    let ret_ok = act_ret.is_assignable_to(exp_ret);
    if !params_ok || !named_ok || !ret_ok {
        return Err(anyhow!(
            "Method '{}' signature mismatch for trait '{}'",
            method_name,
            trait_name
        ));
    }
    Ok(())
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a type alias
    pub fn register_type_alias(&mut self, alias: TypeAlias) {
        self.type_aliases.insert(alias.name.clone(), alias);
    }

    /// Retrieve a previously registered type alias by name
    pub fn get_type_alias(&self, name: &str) -> Option<&TypeAlias> {
        self.type_aliases.get(name)
    }

    /// Register a struct the program being checked declares.
    pub fn register_struct(&mut self, s: StructDef) {
        // A local declaration wins over an imported name of the same spelling:
        // imports are seeded first, and this is what un-marks the entry. Both
        // tables, and for the same reason — under an alias the constructible
        // entry is keyed by the *bound* name, so `use { P as Q }` beside a
        // local `struct Q` left `Q { … }` checked against `P`'s schema while
        // the compiler (which prefers the local `Q$new`) built the local one.
        self.imported_structs.remove(&s.name);
        self.constructible_imports.remove(&s.name);
        self.structs.insert(s.name.clone(), s);
    }

    /// Register a struct **another module** declares.
    pub fn register_imported_struct(&mut self, s: StructDef) {
        self.imported_structs.insert(s.name.clone());
        self.structs.insert(s.name.clone(), s);
    }

    /// Whether `name` is known only because another module declares it.
    pub fn is_imported_struct(&self, name: &str) -> bool {
        self.imported_structs.contains(name)
    }

    /// Marks an imported struct as brought in by name, so a bare literal builds
    /// it through the declaring module's constructor. `bound` is the name this
    /// file writes; `declared` is the name its module gave it.
    pub fn mark_constructible_import(&mut self, bound: &str, declared: &str) {
        self.constructible_imports
            .insert(bound.to_string(), declared.to_string());
    }

    /// The declaring module's name for a type a bare `bound { … }` may build.
    pub fn constructible_import_target(&self, bound: &str) -> Option<&str> {
        self.constructible_imports.get(bound).map(String::as_str)
    }

    /// Get struct definition by name
    pub fn get_struct(&self, name: &str) -> Option<&StructDef> {
        self.structs.get(name)
    }

    /// Register a trait definition
    /// A declared trait, by name.
    pub fn get_trait(&self, name: &str) -> Option<&TraitDef> {
        self.traits.get(name)
    }

    pub fn register_trait(&mut self, trait_def: TraitDef) {
        self.traits.insert(trait_def.name.clone(), trait_def);
    }

    /// Register a trait implementation.
    ///
    /// Re-registering `impl Trait for Type` **replaces** the existing entry
    /// instead of stacking another copy behind it. A registry is reused across
    /// runs (the REPL's context, the hybrid bridge's process-lifetime context),
    /// and every lookup here is a linear scan of the type's impl list, so
    /// pushing made both memory and dispatch cost grow with the number of runs
    /// while the extra copies could never be reached.
    pub fn register_trait_impl(&mut self, impl_def: TraitImpl) {
        let type_name = Self::type_to_string(&impl_def.target_type);
        let impls = self.implementations.entry(type_name).or_default();
        match impls
            .iter_mut()
            .find(|existing| existing.trait_name == impl_def.trait_name)
        {
            Some(existing) => *existing = impl_def,
            None => impls.push(impl_def),
        }
    }

    /// Every type name this program declares — structs, traits, aliases.
    ///
    /// Used by the unknown-type diagnostic to suggest a near miss, so a typo in
    /// a *user's* type name is caught the same way one in a builtin's is.
    pub fn declared_type_names(&self) -> Vec<String> {
        self.type_aliases
            .keys()
            .chain(self.structs.keys())
            .chain(self.traits.keys())
            .cloned()
            .collect()
    }

    /// Resolve a named type to its concrete type
    pub fn resolve_type(&self, name: &str) -> Option<Type> {
        // Check if it's a type alias
        if let Some(alias) = self.type_aliases.get(name) {
            return Some(alias.target_type.clone());
        }

        // Check if it's a struct type
        if self.structs.contains_key(name) {
            return Some(Type::Named(name.to_string()));
        }

        // Check if it's a trait (traits can be used as types in some contexts)
        if self.traits.contains_key(name) {
            return Some(Type::Named(name.to_string()));
        }

        None
    }

    /// How many impls are registered against `typ`. Every dispatch lookup is a
    /// linear scan of this list, so it staying flat across repeated
    /// registrations is a property worth asserting.
    pub fn trait_impl_count(&self, typ: &Type) -> usize {
        self.implementations
            .get(&Self::type_to_string(typ))
            .map_or(0, |impls| impls.len())
    }

    /// Check if a type implements a trait
    pub fn implements_trait(&self, typ: &Type, trait_name: &str) -> bool {
        let type_name = Self::type_to_string(typ);
        if let Some(impls) = self.implementations.get(&type_name) {
            impls.iter().any(|impl_def| impl_def.trait_name == trait_name)
        } else {
            false
        }
    }

    /// Get the method implementation for a type and method name
    /// The compiled body index of `typ::method_name`, searching impls in
    /// registration order (first match wins). The index is only meaningful
    /// against the module that compiled it; the runtime dispatch table
    /// (`VmContext::methods`) is what carries that module alongside it.
    pub fn get_method(&self, typ: &Type, method_name: &str) -> Option<u32> {
        let type_name = Self::type_to_string(typ);
        let impls = self.implementations.get(&type_name)?;
        impls
            .iter()
            .find_map(|impl_def| impl_def.methods.get(method_name).map(|(function, _sig)| *function))
    }

    /// Generate a fresh type variable
    pub fn fresh_type_var(&mut self) -> Type {
        let var_name = format!("T{}", self.type_var_counter);
        self.type_var_counter += 1;
        Type::Variable(var_name)
    }

    /// Convert a type to a string representation for indexing
    fn type_to_string(typ: &Type) -> String {
        match typ {
            Type::Named(name) => name.clone(),
            Type::Unknown => "_".to_string(),
            Type::Int => "Int".to_string(),
            Type::MachineInt(kind) => kind.name().to_string(),
            Type::Ptr { pointee, mutable } => {
                let inner = Self::type_to_string(pointee);
                if *mutable {
                    format!("*mut {inner}")
                } else {
                    format!("*{inner}")
                }
            }
            Type::Float => "Float".to_string(),
            Type::String => "String".to_string(),
            Type::Bool => "Bool".to_string(),
            Type::Nil => "Nil".to_string(),
            Type::List(inner) => format!("List<{}>", Self::type_to_string(inner)),
            Type::Map(k, v) => format!("Map<{}, {}>", Self::type_to_string(k), Self::type_to_string(v)),
            Type::Set(inner) => format!("Set<{}>", Self::type_to_string(inner)),
            Type::Tuple(elems) => {
                if elems.is_empty() {
                    "Tuple<>".to_string()
                } else {
                    let names: Vec<String> = elems.iter().map(Self::type_to_string).collect();
                    format!("Tuple<{}>", names.join(", "))
                }
            }
            Type::Function { .. } => "Function".to_string(),
            Type::Task(inner) => format!("Task<{}>", Self::type_to_string(inner)),
            Type::Channel(inner) => format!("Channel<{}>", Self::type_to_string(inner)),
            Type::Union(types) => {
                let type_names: Vec<String> = types.iter().map(Self::type_to_string).collect();
                format!("({})", type_names.join(" | "))
            }
            Type::Optional(inner) => format!("{}?", Self::type_to_string(inner)),
            Type::Variable(name) => format!("'{}", name),
            Type::Generic { name, params } => {
                if params.is_empty() {
                    name.clone()
                } else {
                    let param_names: Vec<String> = params.iter().map(Self::type_to_string).collect();
                    format!("{}<{}>", name, param_names.join(", "))
                }
            }
            Type::Boxed(inner) => format!("Box<{}>", Self::type_to_string(inner)),
            Type::Any => "Any".to_string(),
        }
    }

    /// Validate that a trait implementation is correct
    pub fn validate_trait_impl(&self, impl_def: &TraitImpl) -> Result<()> {
        // Check that the trait exists
        let trait_def = self
            .traits
            .get(&impl_def.trait_name)
            .ok_or_else(|| anyhow!("Trait '{}' not found", impl_def.trait_name))?;

        // Check that all required methods are implemented and signatures match
        for (method_name, expected_ty) in &trait_def.methods {
            let Some((val, sig)) = impl_def.methods.get(method_name) else {
                return Err(anyhow!(
                    "Method '{}' required by trait '{}' not implemented for type '{}'",
                    method_name,
                    impl_def.trait_name,
                    Self::type_to_string(&impl_def.target_type)
                ));
            };

            // A registered method is always a compiled function body (the
            // compiler only records indices for `fn` items in an impl block),
            // so there is no "is this callable" question left to ask here.
            let _ = val;
            let mut actual_ty = Type::Function {
                params: Vec::new(),
                named_params: Vec::new(),
                return_type: Box::new(Type::Any),
            };

            // Prefer declared signature if provided for strict matching
            if let Some(declared) = sig {
                actual_ty = declared.clone();
            }

            // If expected is a function, check arity
            if let Type::Function { .. } = expected_ty {
                if let Type::Function { .. } = &actual_ty {
                    trait_method_conformance(method_name, &impl_def.trait_name, expected_ty, &actual_ty)?;
                } else {
                    // Should not happen given construction above
                    return Err(anyhow!(
                        "Method '{}' must be a function for trait '{}'",
                        method_name,
                        impl_def.trait_name
                    ));
                }
            }
        }

        // A method the trait never declared does not belong here. It used to be
        // accepted, and it had to be: `impl Type { … }` was a syntax error and
        // there is no UFCS, so a trait impl was the only place a method could
        // live — programs declared an empty trait and hung everything off it.
        // Now that a type can carry its own methods, an undeclared one in a
        // *trait* impl is a mistake with an obvious fix, and saying so is what
        // keeps the trait's method list meaning something.
        for method_name in impl_def.methods.keys() {
            if !trait_def.methods.iter().any(|(declared, _)| declared == method_name) {
                return Err(anyhow!(
                    "Method '{}' is not declared by trait '{}' — put it in `impl {} {{ … }}`, \
                     which is where a type's own methods go",
                    method_name,
                    impl_def.trait_name,
                    Self::type_to_string(&impl_def.target_type)
                ));
            }
        }

        Ok(())
    }
}

/// Type inference engine using unification
#[derive(Debug, Clone, PartialEq)]
pub struct TypeInferenceEngine {
    /// Current substitutions for type variables
    substitutions: HashMap<String, Type>,

    /// Constraints to be solved
    constraints: Vec<(Type, Type)>,

    /// Next `T{n}` this engine hands out.
    ///
    /// It used to own a whole [`TypeRegistry`] for this counter — a *clone*
    /// taken when the checker was built, so every declaration made afterwards
    /// was invisible to unification. The two facts unification needs about
    /// declarations (which names are traits, and which types implement them)
    /// therefore could not be asked at all; they arrive as a parameter now, and
    /// the copy is gone.
    type_var_counter: u32,
}

impl Default for TypeInferenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeInferenceEngine {
    pub fn new() -> Self {
        Self {
            substitutions: HashMap::new(),
            constraints: Vec::new(),
            type_var_counter: 0,
        }
    }

    /// Generate a fresh type variable
    pub fn fresh_type_var(&mut self) -> Type {
        let var_name = format!("T{}", self.type_var_counter);
        self.type_var_counter += 1;
        Type::Variable(var_name)
    }

    /// Add a constraint that two types must be equal
    pub fn add_constraint(&mut self, t1: Type, t2: Type) {
        self.constraints.push((t1, t2));
    }

    /// Solve all constraints using unification
    pub fn solve_constraints(&mut self, registry: &TypeRegistry) -> Result<HashMap<String, Type>> {
        while let Some((t1, t2)) = self.constraints.pop() {
            self.unify(t1, t2, registry)?;
        }
        Ok(self.substitutions.clone())
    }

    /// Unify two types
    fn normalize_union(t: Type) -> Type {
        // Flatten nested unions and remove duplicates; also collapse Optional(T) into Union(T|Nil)
        fn collect(t: Type, acc: &mut Vec<Type>) {
            match t {
                Type::Union(vs) => {
                    for u in vs {
                        collect(u, acc);
                    }
                }
                Type::Optional(inner) => {
                    collect(*inner, acc);
                    acc.push(Type::Nil);
                }
                other => acc.push(other),
            }
        }
        let mut items = Vec::new();
        collect(t, &mut items);
        // Deduplicate by display string to be stable
        use alloc::collections::BTreeSet;
        let mut seen = BTreeSet::new();
        let mut uniq = Vec::new();
        for ty in items {
            let key = ty.display();
            if seen.insert(key) {
                uniq.push(ty);
            }
        }
        match uniq.len() {
            0 => Type::Nil,
            1 => uniq.into_iter().next().unwrap(),
            _ => Type::Union(uniq),
        }
    }

    fn unify(&mut self, t1: Type, t2: Type, registry: &TypeRegistry) -> Result<()> {
        // Before substitution, because substitution is what hides this case: a
        // variable already bound to one type, now required to be another.
        //
        // `let l = []; l.push(1); l.push("a");` is the shape. The element type
        // starts as a variable, the first push binds it to `Int`, and by the
        // second push substitution has already turned the variable into `Int`
        // — so what reaches the match below is `Int` against `String`, with no
        // sign that a *variable* is what disagrees. LK's lists are
        // heterogeneous, so the answer is not a conflict: the variable is both,
        // and widening it to `Int | String` says so.
        if let Some(()) = self.widen_rebound_variable(&t1, &t2) {
            return Ok(());
        }

        let t1 = Self::normalize_union(self.apply_substitution(&t1));
        let t2 = Self::normalize_union(self.apply_substitution(&t2));

        match (t1.clone(), t2.clone()) {
            // Same types unify
            (a, b) if a == b => Ok(()),

            // `_` — the element type of a read-only container view — says
            // nothing about what it stands for, so it constrains nothing. It
            // reaches the unifier from a declared parameter (`List<_>`) meeting
            // an argument (`List<Int>`), which is exactly the case it exists to
            // accept.
            (Type::Unknown, _) | (_, Type::Unknown) => Ok(()),

            // `Any` is a weak gradual-typing constraint. It must not bind an
            // otherwise fresh type variable, because later concrete call-site
            // constraints should still be able to refine that variable.
            (Type::Any, _) | (_, Type::Any) => Ok(()),

            // Variable unification
            (Type::Variable(var), typ) | (typ, Type::Variable(var)) => {
                if Self::occurs_check(&var, &typ) {
                    Err(anyhow!("Occurs check failed: {} occurs in {}", var, typ.display()))
                } else {
                    // Apply the new substitution to existing substitutions
                    let new_substitution = typ.clone();
                    let mut updated_substitutions = HashMap::new();
                    for (existing_var, existing_type) in &self.substitutions {
                        let updated_type =
                            existing_type.substitute(&[(var.clone(), new_substitution.clone())].into_iter().collect());
                        updated_substitutions.insert(existing_var.clone(), updated_type);
                    }
                    // Apply to the substitution itself recursively
                    let final_substitution = new_substitution.substitute(&updated_substitutions);

                    // Update all substitutions
                    for (k, v) in updated_substitutions {
                        self.substitutions.insert(k, v);
                    }
                    self.substitutions.insert(var.clone(), final_substitution);
                    Ok(())
                }
            }

            // Structural unification
            (Type::List(a), Type::List(b)) => self.unify(*a, *b, registry),
            // A tuple is a list whose element types are known one by one —
            // there is no tuple at runtime, `HeapValue` has only `List`. This
            // mirrors the rule in `is_assignable_to`; without it the two
            // disagreed, and the disagreement was invisible only because the
            // concrete-concrete rule below swallows whatever reaches it.
            (Type::Tuple(elems), Type::List(elem)) | (Type::List(elem), Type::Tuple(elems)) => {
                for tuple_elem in elems {
                    self.unify(tuple_elem, (*elem).clone(), registry)?;
                }
                Ok(())
            }
            (Type::Set(a), Type::Set(b)) => self.unify(*a, *b, registry),
            (Type::Map(ak, av), Type::Map(bk, bv)) => {
                self.unify(*ak, *bk, registry)?;
                self.unify(*av, *bv, registry)
            }
            (Type::Tuple(a), Type::Tuple(b)) => {
                if a.len() != b.len() {
                    return Err(anyhow!("Tuple arity mismatch"));
                }
                for (x, y) in a.into_iter().zip(b) {
                    self.unify(x, y, registry)?;
                }
                Ok(())
            }
            (
                Type::Function {
                    params: a_params,
                    named_params: a_named,
                    return_type: a_ret,
                },
                Type::Function {
                    params: b_params,
                    named_params: b_named,
                    return_type: b_ret,
                },
            ) => {
                if a_params.len() != b_params.len() {
                    return Err(anyhow!("Function arity mismatch"));
                }
                for (a_param, b_param) in a_params.into_iter().zip(b_params) {
                    self.unify(a_param, b_param, registry)?;
                }
                if a_named.len() != b_named.len() {
                    return Err(anyhow!("Function named parameter count mismatch"));
                }
                let mut a_map: HashMap<String, (Type, bool)> = HashMap::with_capacity(a_named.len());
                for np in a_named.into_iter() {
                    a_map.insert(np.name, (np.ty, np.has_default));
                }
                let mut b_map: HashMap<String, (Type, bool)> = HashMap::with_capacity(b_named.len());
                for np in b_named.into_iter() {
                    b_map.insert(np.name, (np.ty, np.has_default));
                }
                for (name, (a_ty, a_default)) in a_map.into_iter() {
                    let Some((b_ty, b_default)) = b_map.remove(&name) else {
                        return Err(anyhow!("Function named parameter '{}' mismatch", name));
                    };
                    if a_default != b_default {
                        return Err(anyhow!("Function named parameter '{}' default mismatch", name));
                    }
                    self.unify(a_ty, b_ty, registry)?;
                }
                self.unify(*a_ret, *b_ret, registry)
            }
            (Type::Optional(a), Type::Optional(b)) => self.unify(*a, *b, registry),
            (Type::Task(a), Type::Task(b)) => self.unify(*a, *b, registry),
            (Type::Channel(a), Type::Channel(b)) => self.unify(*a, *b, registry),
            (Type::Boxed(a), Type::Boxed(b)) => self.unify(*a, *b, registry),
            (Type::Boxed(inner), other) | (other, Type::Boxed(inner)) => self.unify(*inner, other, registry),

            // Union type unification
            (Type::Union(a_types), Type::Union(b_types)) => {
                // Intersect the two unions by assignability; if intersection empty, error
                let mut result = Vec::new();
                for at in a_types.iter() {
                    for bt in b_types.iter() {
                        if at.is_assignable_to(bt) || bt.is_assignable_to(at) {
                            result.push(at.clone().clone());
                            break;
                        }
                    }
                }
                if result.is_empty() {
                    return Err(anyhow!(
                        "Union types are disjoint: {} vs {}",
                        t1.display(),
                        t2.display()
                    ));
                }
                // Constrain to the normalized intersection
                let norm = Self::normalize_union(Type::Union(result));
                // Bind both sides to intersection to progress inference
                self.add_constraint(norm.clone(), t1.clone());
                self.add_constraint(norm, t2.clone());
                Ok(())
            }
            (Type::Union(types), t) | (t, Type::Union(types)) => {
                // If t is assignable to any, OK; otherwise try to narrow union by t
                if types.iter().any(|u| t.is_assignable_to(u)) {
                    Ok(())
                } else if let Some(var) = types.iter().find(|u| matches!(u, Type::Variable(_))).cloned() {
                    self.add_constraint(var, t);
                    Ok(())
                } else {
                    let union_display = Type::Union(types.clone()).display();
                    // Attempt to find members compatible with t
                    let compatibles: Vec<Type> = types
                        .into_iter()
                        .filter(|u| u.is_assignable_to(&t) || t.is_assignable_to(u))
                        .collect();
                    if compatibles.is_empty() {
                        Err(anyhow!(
                            "Cannot unify {} with union type {}",
                            t.display(),
                            union_display
                        ))
                    } else {
                        let narrowed = Self::normalize_union(Type::Union(compatibles));
                        self.add_constraint(narrowed, t.clone());
                        Ok(())
                    }
                }
            }

            // Generic type unification
            (
                Type::Generic {
                    name: a_name,
                    params: a_params,
                },
                Type::Generic {
                    name: b_name,
                    params: b_params,
                },
            ) => {
                if a_name != b_name || a_params.len() != b_params.len() {
                    return Err(anyhow!("Generic type mismatch"));
                }
                for (a_param, b_param) in a_params.iter().zip(b_params.iter()) {
                    self.unify(a_param.clone(), b_param.clone(), registry)?;
                }
                Ok(())
            }

            // Numeric hierarchy: Int ≤ Float ≤ Boxed — compatible numeric types can be unified.
            // This handles cases where arithmetic on typed variables creates subtype constraints.
            (ref lhs, ref rhs) if lhs.numeric_class().is_some() && rhs.numeric_class().is_some() => Ok(()),

            // A machine int meets a plain `Int` wherever a literal appears in a
            // machine-width context — `match ALL_ONES { 0xFFFFFFFFFFFFFFFF => … }`,
            // or `reg + 1`. The literal takes the width, which is the rule
            // `Stmt::Let` already applies to `let x: u8 = 5`.
            //
            // `NumericHierarchy::classify` deliberately does not rank machine
            // ints (they convert only explicitly, and *assignability* still
            // refuses both directions). That is a question about values;
            // unification is asking a different one, about which type a literal
            // takes. Two machine widths still do not unify with each other.
            (Type::MachineInt(_), Type::Int) | (Type::Int, Type::MachineInt(_)) => Ok(()),

            // Type mismatch.
            //
            // Two disagreeing concrete types used to be accepted here, on the
            // grounds that a gradually-typed language legitimately holds
            // different concrete types in one context at different call sites.
            // Measured, that described four fixable gaps rather than the
            // language, and each is now closed:
            //
            //   - a binding that started `nil` kept the type `Nil` after being
            //     assigned (`Stmt::Assign` widens it),
            //   - `==` constrained its operands to be the *same* type, so
            //     `x == nil` was a conflict (`check_binary_op` no longer does),
            //   - a type variable bound once could not be bound again, so
            //     `l.push(1); l.push("a")` conflicted on a heterogeneous list
            //     (`widen_rebound_variable`),
            //   - constraints were solved at the end of every function against
            //     a global pool, so one function's leftovers met the next one's
            //     (`Program::type_check` defers in both modes now).
            // A trait meets an implementor. Assignability already says an
            // implementor may stand where the trait is expected; a declared
            // return type is checked by *unification* instead, so
            // `fn pick() -> Show { return P { … }; }` was rejected while
            // `fn render(v: Show)` was accepted — the same question answered
            // two ways.
            (ref t, Type::Named(ref name)) | (Type::Named(ref name), ref t)
                if registry.get_trait(name).is_some() && registry.implements_trait(t, name) =>
            {
                Ok(())
            }
            _ => Err(anyhow!("Cannot unify {} with {}", t1.display(), t2.display())),
        }
    }

    /// A variable bound to one concrete type and now required to be another:
    /// rebind it to both. Returns `Some(())` when it did.
    ///
    /// Only for a variable against a concrete type. Two variables, or anything
    /// still undetermined, is ordinary unification's business — widening there
    /// would decide a type that inference has not finished deciding.
    fn widen_rebound_variable(&mut self, t1: &Type, t2: &Type) -> Option<()> {
        let (var, incoming) = match (t1, t2) {
            (Type::Variable(var), other) | (other, Type::Variable(var)) => (var, other),
            _ => return None,
        };
        let incoming = Self::normalize_union(self.apply_substitution(incoming));
        let bound = Self::normalize_union(self.apply_substitution(&self.substitutions.get(var)?.clone()));
        if bound == incoming {
            return None;
        }
        // A *bare* variable is what "inference has not finished deciding"
        // means: it has no shape yet, so widening against it would decide
        // something ordinary unification is still entitled to decide.
        //
        // A **constructed** type that merely contains variables is a different
        // thing, and both sides used to be refused for it. `List<'T2>` — what an
        // empty literal gives — has its shape settled: it is a list, and a list
        // never unifies with an `Int` however `'T2` turns out. So refusing did
        // not defer a decision, it reported a conflict. `f([]); f(5)` failed
        // with `Cannot unify Int with List<'T2>` while `f([1]); f(5)` was
        // accepted — the same program with one element in it, and the
        // difference decided by which of the two constraints the solver
        // happened to pop first. The inner variable survives into the union and
        // is substituted later like any other.
        if matches!(bound, Type::Variable(_)) || matches!(incoming, Type::Variable(_)) {
            return None;
        }
        // `Any` already accepts everything; widening it says nothing new.
        if bound == Type::Any || incoming == Type::Any {
            return None;
        }
        let widened = Self::normalize_union(Type::Union(vec![bound, incoming]));
        self.substitutions.insert(var.clone(), widened);
        Some(())
    }

    /// Apply current substitutions to a type
    fn apply_substitution(&self, typ: &Type) -> Type {
        typ.substitute(&self.substitutions)
    }

    /// Occurs check to prevent infinite types
    fn occurs_check(var: &str, typ: &Type) -> bool {
        match typ {
            Type::Variable(v) => v == var,
            Type::List(inner) | Type::Set(inner) | Type::Optional(inner) | Type::Task(inner) | Type::Channel(inner) => {
                Self::occurs_check(var, inner)
            }
            Type::Map(k, v) => Self::occurs_check(var, k) || Self::occurs_check(var, v),
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                params.iter().any(|p| Self::occurs_check(var, p))
                    || named_params.iter().any(|np| Self::occurs_check(var, &np.ty))
                    || Self::occurs_check(var, return_type)
            }
            Type::Union(types) => types.iter().any(|t| Self::occurs_check(var, t)),
            Type::Generic { params, .. } => params.iter().any(|p| Self::occurs_check(var, p)),
            _ => false,
        }
    }
}

/// The registry is the answer to the assignability walk's trait question
/// ([`crate::val::TraitOracle`]).
///
/// Only a *declared* trait counts: `Type::Named` covers struct names too, and
/// `implements_trait` would answer `false` for those anyway — asking `get_trait`
/// first says why, and keeps a struct name from being read as a bound.
impl crate::val::TraitOracle for TypeRegistry {
    fn implements(&self, ty: &Type, trait_name: &str) -> bool {
        self.get_trait(trait_name).is_some() && self.implements_trait(ty, trait_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_registry() {
        let mut registry = TypeRegistry::new();

        // Register a type alias
        let alias = TypeAlias {
            name: "UserId".to_string(),
            target_type: Type::Int,
        };
        registry.register_type_alias(alias);

        assert_eq!(registry.resolve_type("UserId"), Some(Type::Int));
        assert_eq!(registry.resolve_type("UnknownType"), None);
    }

    #[test]
    fn test_trait_system() {
        let mut registry = TypeRegistry::new();

        // Define a trait
        let mut methods = HashMap::new();
        methods.insert(
            "display".to_string(),
            Type::Function {
                params: vec![],
                named_params: Vec::new(),
                return_type: Box::new(Type::String),
            },
        );

        let trait_def = TraitDef {
            name: "Display".to_string(),
            methods,
        };
        registry.register_trait(trait_def);

        assert!(registry.traits.contains_key("Display"));
    }

    #[test]
    fn test_type_inference() {
        let registry = TypeRegistry::new();
        let mut engine = TypeInferenceEngine::new();

        let var1 = engine.fresh_type_var();
        let var2 = engine.fresh_type_var();

        // Add constraint: T0 = Int
        engine.add_constraint(var1.clone(), Type::Int);
        // Add constraint: T1 = T0
        engine.add_constraint(var2.clone(), var1.clone());

        let substitutions = engine.solve_constraints(&registry).unwrap();

        // Both variables should resolve to Int
        if let Type::Variable(name1) = &var1 {
            assert_eq!(substitutions.get(name1), Some(&Type::Int));
        }
        if let Type::Variable(name2) = &var2 {
            assert_eq!(substitutions.get(name2), Some(&Type::Int));
        }
    }
}
