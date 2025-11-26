use crate::val::{Type, Val};
use anyhow::{Result, anyhow};
use std::collections::HashMap;

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

/// Implementation of a trait for a specific type
#[derive(Debug, Clone, PartialEq)]
pub struct TraitImpl {
    pub trait_name: String,
    pub target_type: Type,
    // method_name -> (function_value, declared_type)
    pub methods: HashMap<String, (Val, Option<Type>)>,
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

    /// Trait definitions
    traits: HashMap<String, TraitDef>,

    /// Trait implementations per type
    implementations: HashMap<String, Vec<TraitImpl>>, // type_name -> implementations

    /// Type variable counter for fresh variable generation
    type_var_counter: u32,
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

    /// Register a struct definition
    pub fn register_struct(&mut self, s: StructDef) {
        self.structs.insert(s.name.clone(), s);
    }

    /// Get struct definition by name
    pub fn get_struct(&self, name: &str) -> Option<&StructDef> {
        self.structs.get(name)
    }

    /// Register a trait definition
    pub fn register_trait(&mut self, trait_def: TraitDef) {
        self.traits.insert(trait_def.name.clone(), trait_def);
    }

    /// Register a trait implementation
    pub fn register_trait_impl(&mut self, impl_def: TraitImpl) {
        let type_name = Self::type_to_string(&impl_def.target_type);
        self.implementations.entry(type_name).or_default().push(impl_def);
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
    pub fn get_method(&self, typ: &Type, method_name: &str) -> Option<&Val> {
        let type_name = Self::type_to_string(typ);
        if let Some(impls) = self.implementations.get(&type_name) {
            for impl_def in impls {
                if let Some((method, _sig)) = impl_def.methods.get(method_name) {
                    return Some(method);
                }
            }
        }
        None
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
            Type::Int => "Int".to_string(),
            Type::Float => "Float".to_string(),
            Type::String => "String".to_string(),
            Type::Bool => "Bool".to_string(),
            Type::Nil => "Nil".to_string(),
            Type::List(inner) => format!("List<{}>", Self::type_to_string(inner)),
            Type::Map(k, v) => format!("Map<{}, {}>", Self::type_to_string(k), Self::type_to_string(v)),
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
            Type::Optional(inner) => format!("?{}", Self::type_to_string(inner)),
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

            // Only function values are valid implementations
            let mut actual_ty = match val {
                Val::Closure(closure) => Type::Function {
                    params: vec![Type::Any; closure.params.len()], // Without annotations, conservatively Any
                    named_params: Vec::new(),
                    return_type: Box::new(Type::Any),
                },
                Val::RustFunction(_) | Val::RustFunctionNamed(_) => {
                    // Native function type info not carried; accept for now as Function Any
                    Type::Function {
                        params: vec![],
                        named_params: Vec::new(),
                        return_type: Box::new(Type::Any),
                    }
                }
                _ => {
                    return Err(anyhow!(
                        "Method '{}' for trait '{}' must be a function, got {:?}",
                        method_name,
                        impl_def.trait_name,
                        val
                    ));
                }
            };

            // Prefer declared signature if provided for strict matching
            if let Some(declared) = sig {
                actual_ty = declared.clone();
            }

            // If expected is a function, check arity
            if let Type::Function {
                params: exp_params,
                named_params: exp_named,
                return_type: exp_ret,
            } = expected_ty
            {
                if let Type::Function {
                    params: act_params,
                    named_params: act_named,
                    return_type: act_ret,
                } = &actual_ty
                {
                    if exp_params.len() != act_params.len() {
                        return Err(anyhow!(
                            "Method '{}' arity mismatch for trait '{}': expected {}, got {}",
                            method_name,
                            impl_def.trait_name,
                            exp_params.len(),
                            act_params.len()
                        ));
                    }
                    if exp_named.len() != act_named.len() {
                        return Err(anyhow!(
                            "Method '{}' named parameter count mismatch for trait '{}': expected {}, got {}",
                            method_name,
                            impl_def.trait_name,
                            exp_named.len(),
                            act_named.len()
                        ));
                    }
                    // When signatures are concrete, ensure contravariant params and covariant return
                    let params_ok = exp_params
                        .iter()
                        .zip(act_params.iter())
                        .all(|(e, a)| a.is_assignable_to(e));
                    let named_ok = exp_named.iter().all(|exp_np| {
                        act_named
                            .iter()
                            .find(|act_np| act_np.name == exp_np.name)
                            .map(|act_np| {
                                act_np.has_default == exp_np.has_default && act_np.ty.is_assignable_to(&exp_np.ty)
                            })
                            .unwrap_or(false)
                    });
                    let ret_ok = act_ret.is_assignable_to(exp_ret);
                    if !params_ok || !named_ok || !ret_ok {
                        return Err(anyhow!(
                            "Method '{}' signature mismatch for trait '{}'",
                            method_name,
                            impl_def.trait_name
                        ));
                    }
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

    /// Registry for custom types
    registry: TypeRegistry,
}

impl TypeInferenceEngine {
    pub fn new(registry: TypeRegistry) -> Self {
        Self {
            substitutions: HashMap::new(),
            constraints: Vec::new(),
            registry,
        }
    }

    /// Generate a fresh type variable
    pub fn fresh_type_var(&mut self) -> Type {
        self.registry.fresh_type_var()
    }

    /// Add a constraint that two types must be equal
    pub fn add_constraint(&mut self, t1: Type, t2: Type) {
        self.constraints.push((t1, t2));
    }

    /// Solve all constraints using unification
    pub fn solve_constraints(&mut self) -> Result<HashMap<String, Type>> {
        while let Some((t1, t2)) = self.constraints.pop() {
            self.unify(t1, t2)?;
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
        use std::collections::BTreeSet;
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

    fn unify(&mut self, t1: Type, t2: Type) -> Result<()> {
        let t1 = Self::normalize_union(self.apply_substitution(&t1));
        let t2 = Self::normalize_union(self.apply_substitution(&t2));

        match (t1.clone(), t2.clone()) {
            // Same types unify
            (a, b) if a == b => Ok(()),

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
            (Type::List(a), Type::List(b)) => self.unify(*a, *b),
            (Type::Map(ak, av), Type::Map(bk, bv)) => {
                self.unify(*ak, *bk)?;
                self.unify(*av, *bv)
            }
            (Type::Tuple(a), Type::Tuple(b)) => {
                if a.len() != b.len() {
                    return Err(anyhow!("Tuple arity mismatch"));
                }
                for (x, y) in a.into_iter().zip(b.into_iter()) {
                    self.unify(x, y)?;
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
                for (a_param, b_param) in a_params.into_iter().zip(b_params.into_iter()) {
                    self.unify(a_param, b_param)?;
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
                    self.unify(a_ty, b_ty)?;
                }
                self.unify(*a_ret, *b_ret)
            }
            (Type::Optional(a), Type::Optional(b)) => self.unify(*a, *b),
            (Type::Task(a), Type::Task(b)) => self.unify(*a, *b),
            (Type::Channel(a), Type::Channel(b)) => self.unify(*a, *b),
            (Type::Boxed(a), Type::Boxed(b)) => self.unify(*a, *b),
            (Type::Boxed(inner), other) | (other, Type::Boxed(inner)) => self.unify(*inner, other),

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
                } else {
                    // Attempt to find members compatible with t
                    let compatibles: Vec<Type> = types
                        .into_iter()
                        .filter(|u| u.is_assignable_to(&t) || t.is_assignable_to(u))
                        .collect();
                    if compatibles.is_empty() {
                        Err(anyhow!("Cannot unify {} with union type", t.display()))
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
                    self.unify(a_param.clone(), b_param.clone())?;
                }
                Ok(())
            }

            // Type mismatch
            _ => Err(anyhow!("Cannot unify {} with {}", t1.display(), t2.display())),
        }
    }

    /// Apply current substitutions to a type
    fn apply_substitution(&self, typ: &Type) -> Type {
        typ.substitute(&self.substitutions)
    }

    /// Occurs check to prevent infinite types
    fn occurs_check(var: &str, typ: &Type) -> bool {
        match typ {
            Type::Variable(v) => v == var,
            Type::List(inner) | Type::Optional(inner) | Type::Task(inner) | Type::Channel(inner) => {
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
        let mut engine = TypeInferenceEngine::new(registry);

        let var1 = engine.fresh_type_var();
        let var2 = engine.fresh_type_var();

        // Add constraint: T0 = Int
        engine.add_constraint(var1.clone(), Type::Int);
        // Add constraint: T1 = T0
        engine.add_constraint(var2.clone(), var1.clone());

        let substitutions = engine.solve_constraints().unwrap();

        // Both variables should resolve to Int
        if let Type::Variable(name1) = &var1 {
            assert_eq!(substitutions.get(name1), Some(&Type::Int));
        }
        if let Type::Variable(name2) = &var2 {
            assert_eq!(substitutions.get(name2), Some(&Type::Int));
        }
    }
}
