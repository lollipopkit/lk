use crate::{
    expr::Expr,
    typ::{TypeInferenceEngine, TypeRegistry},
    val::{FunctionNamedParamType, Type},
};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

mod expressions;
mod patterns;

#[cfg(test)]
mod tests;

/// Options that influence type checking behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TypeCheckerOptions {
    /// When enabled, implicit fallbacks to `Any` (e.g. missing annotations) are treated as errors unless constrained.
    pub strict_any: bool,
}

impl TypeCheckerOptions {
    pub const fn strict() -> Self {
        Self { strict_any: true }
    }
}

/// Type checking error with location information
#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: String,
    pub expected: Option<Type>,
    pub actual: Option<Type>,
    pub expr: Option<Expr>,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Type Error: {}", self.message)?;
        if let (Some(expected), Some(actual)) = (&self.expected, &self.actual) {
            write!(f, " (expected {}, got {})", expected.display(), actual.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for TypeError {}

/// Type checker for LKR expressions
#[derive(Debug, Clone, PartialEq)]
pub struct TypeChecker {
    /// Type registry for custom types and traits
    registry: TypeRegistry,

    /// Type inference engine
    inference_engine: TypeInferenceEngine,

    /// Local variable types
    local_types: HashMap<String, Type>,
    /// Tracks const bindings in current scope
    const_locals: HashSet<String>,
    /// Snapshot stack for simple scope management
    scope_stack: Vec<HashMap<String, Type>>,
    const_stack: Vec<HashSet<String>>,
    /// Function signatures indexed by name (for static checking of CallNamed)
    function_sigs: HashMap<String, FunctionSig>,
    /// Behaviour options
    options: TypeCheckerOptions,
    /// Optional cache of inferred expression types keyed by Expr pointer
    expr_types: HashMap<usize, Type>,
    /// Active `impl` target type for the current method being checked
    impl_self_type: Option<Type>,
    /// Recorded method signatures keyed by (receiver_type, method_name)
    method_sigs: HashMap<(String, String), Type>,
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeChecker {
    fn type_err(message: &str, expected: Option<Type>, actual: Option<Type>, expr: Option<Expr>) -> anyhow::Error {
        let te = TypeError {
            message: message.to_string(),
            expected,
            actual,
            expr,
        };
        anyhow::Error::new(te)
    }
    /// Create a new type checker with default (non-strict) behaviour
    pub fn new() -> Self {
        Self::with_options(TypeCheckerOptions::default())
    }

    /// Create a new type checker with strict fallback handling enabled
    pub fn new_strict() -> Self {
        Self::with_options(TypeCheckerOptions::strict())
    }

    /// Create a new type checker using the provided options
    pub fn with_options(options: TypeCheckerOptions) -> Self {
        let registry = TypeRegistry::new();
        Self::with_registry_and_options(registry, options)
    }

    /// Create a type checker with existing registry
    pub fn with_registry(registry: TypeRegistry) -> Self {
        Self::with_registry_and_options(registry, TypeCheckerOptions::default())
    }

    /// Create a type checker with existing registry and custom options
    pub fn with_registry_and_options(registry: TypeRegistry, options: TypeCheckerOptions) -> Self {
        let inference_engine = TypeInferenceEngine::new(registry.clone());

        Self {
            registry,
            inference_engine,
            local_types: HashMap::new(),
            const_locals: HashSet::new(),
            scope_stack: Vec::new(),
            const_stack: Vec::new(),
            function_sigs: HashMap::new(),
            options,
            expr_types: HashMap::new(),
            impl_self_type: None,
            method_sigs: HashMap::new(),
        }
    }

    /// Return true when implicit Any fallbacks should be treated strictly
    pub fn strict_any(&self) -> bool {
        self.options.strict_any
    }

    fn record_expr_type(&mut self, expr: &Expr, ty: &Type) {
        let key = expr as *const Expr as usize;
        self.expr_types.insert(key, ty.clone());
    }

    pub fn take_expr_types(&mut self) -> HashMap<usize, Type> {
        std::mem::take(&mut self.expr_types)
    }

    /// Get the active impl `self` type when type-checking trait implementations.
    pub fn current_impl_self_type(&self) -> Option<&Type> {
        self.impl_self_type.as_ref()
    }

    /// Set the active impl `self` type, returning the previous value for restoration.
    pub fn set_impl_self_type(&mut self, ty: Option<Type>) -> Option<Type> {
        std::mem::replace(&mut self.impl_self_type, ty)
    }

    fn method_sig_key(&self, receiver: &Type, name: &str) -> (String, String) {
        (self.resolve_aliases(receiver).display(), name.to_string())
    }

    pub fn add_method_sig(&mut self, receiver: &Type, name: &str, sig: Type) {
        let key = self.method_sig_key(receiver, name);
        self.method_sigs.insert(key, sig);
    }

    pub fn get_method_sig(&self, receiver: &Type, name: &str) -> Option<Type> {
        let key = self.method_sig_key(receiver, name);
        self.method_sigs.get(&key).cloned()
    }

    /// Resolve all type aliases contained in `ty`, returning a canonical representation.
    pub fn resolve_aliases(&self, ty: &Type) -> Type {
        let mut visiting = HashSet::new();
        self.resolve_aliases_internal(ty, &mut visiting)
    }

    fn resolve_aliases_internal(&self, ty: &Type, visiting: &mut HashSet<String>) -> Type {
        match ty {
            Type::Named(name) => {
                if let Some(alias) = self.registry.get_type_alias(name) {
                    if !visiting.insert(name.clone()) {
                        return Type::Any;
                    }
                    let resolved = self.resolve_aliases_internal(&alias.target_type, visiting);
                    visiting.remove(name);
                    resolved
                } else {
                    Type::Named(name.clone())
                }
            }
            Type::List(inner) => Type::List(Box::new(self.resolve_aliases_internal(inner, visiting))),
            Type::Map(key, value) => Type::Map(
                Box::new(self.resolve_aliases_internal(key, visiting)),
                Box::new(self.resolve_aliases_internal(value, visiting)),
            ),
            Type::Tuple(items) => {
                let mapped = items
                    .iter()
                    .map(|t| self.resolve_aliases_internal(t, visiting))
                    .collect();
                Type::Tuple(mapped)
            }
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                let mapped_params = params
                    .iter()
                    .map(|t| self.resolve_aliases_internal(t, visiting))
                    .collect();
                let mapped_named = named_params
                    .iter()
                    .map(|np| FunctionNamedParamType {
                        name: np.name.clone(),
                        ty: self.resolve_aliases_internal(&np.ty, visiting),
                        has_default: np.has_default,
                    })
                    .collect();
                let mapped_return = self.resolve_aliases_internal(return_type, visiting);
                Type::Function {
                    params: mapped_params,
                    named_params: mapped_named,
                    return_type: Box::new(mapped_return),
                }
            }
            Type::Task(inner) => Type::Task(Box::new(self.resolve_aliases_internal(inner, visiting))),
            Type::Channel(inner) => Type::Channel(Box::new(self.resolve_aliases_internal(inner, visiting))),
            Type::Union(items) => {
                let mapped = items
                    .iter()
                    .map(|t| self.resolve_aliases_internal(t, visiting))
                    .collect();
                Type::Union(mapped)
            }
            Type::Optional(inner) => Type::Optional(Box::new(self.resolve_aliases_internal(inner, visiting))),
            Type::Generic { name, params } => {
                let mapped_params = params
                    .iter()
                    .map(|t| self.resolve_aliases_internal(t, visiting))
                    .collect();
                Type::Generic {
                    name: name.clone(),
                    params: mapped_params,
                }
            }
            Type::Boxed(inner) => Type::Boxed(Box::new(self.resolve_aliases_internal(inner, visiting))),
            Type::Any | Type::Int | Type::Float | Type::String | Type::Bool | Type::Nil | Type::Variable(_) => {
                ty.clone()
            }
        }
    }

    /// Check assignability between two types after resolving aliases.
    pub fn is_assignable(&self, from: &Type, to: &Type) -> bool {
        let lhs = self.resolve_aliases(from);
        let rhs = self.resolve_aliases(to);
        lhs.is_assignable_to(&rhs)
    }

    /// Register a function signature for static checking by name
    pub fn add_function_sig(&mut self, name: String, sig: FunctionSig) {
        self.function_sigs.insert(name, sig);
    }

    /// Retrieve a function signature by name
    pub fn get_function_sig(&self, name: &str) -> Option<&FunctionSig> {
        self.function_sigs.get(name)
    }

    /// Solve type constraints and return final types
    pub fn solve_constraints(&mut self) -> Result<HashMap<String, Type>> {
        self.inference_engine.solve_constraints()
    }

    /// Get the inferred type for a local variable
    pub fn get_local_type(&self, name: &str) -> Option<&Type> {
        self.local_types.get(name)
    }

    /// Add a type annotation for a local variable
    pub fn add_local_type(&mut self, name: String, typ: Type) {
        self.add_local_binding(name, typ, false);
    }

    /// Add a type annotation with mutability information
    pub fn add_local_binding(&mut self, name: String, typ: Type, is_const: bool) {
        let normalized = self.resolve_aliases(&typ);
        self.local_types.insert(name.clone(), normalized);
        if is_const {
            self.const_locals.insert(name);
        } else {
            self.const_locals.remove(name.as_str());
        }
    }

    /// Check whether a local binding is const
    pub fn is_const_local(&self, name: &str) -> bool {
        self.const_locals.contains(name)
    }

    /// Get the type registry
    pub fn registry(&self) -> &TypeRegistry {
        &self.registry
    }

    /// Get mutable access to the type registry
    pub fn registry_mut(&mut self) -> &mut TypeRegistry {
        &mut self.registry
    }

    /// Enter a new scope for local variables
    pub fn push_scope(&mut self) {
        // Snapshot current locals; modifications in the new scope are discarded on pop
        self.scope_stack.push(self.local_types.clone());
        self.const_stack.push(self.const_locals.clone());
    }

    /// Exit the current scope
    pub fn pop_scope(&mut self) {
        if let Some(prev) = self.scope_stack.pop() {
            self.local_types = prev;
        }
        if let Some(prev) = self.const_stack.pop() {
            self.const_locals = prev;
        }
    }
}

/// Function signature for static checking (positional + named)
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSig {
    pub positional: Vec<Type>,
    pub named: Vec<NamedParamSig>,
    pub return_type: Option<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedParamSig {
    pub name: String,
    pub ty: Type,
    pub has_default: bool,
}
