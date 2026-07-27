use crate::compat::collections::HashSet;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::Expr,
    token::Span,
    typ::{TypeInferenceEngine, TypeRegistry},
    val::{FunctionNamedParamType, Type},
};
use anyhow::Result;
use hashbrown::HashMap;

mod expressions;
pub use expressions::builtin_machine_result;
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

/// A binding and the type it was bound to, at the position it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedBinding {
    /// The `let`/`const` keyword through the end of the pattern.
    pub span: Span,
    pub name: String,
    pub ty: Type,
    /// True when the source already spells this type out.
    ///
    /// An inlay hint exists to show what was left unwritten, so it skips these;
    /// hover wants them all.
    pub annotated: bool,
}

/// Type checking error with location information
#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: String,
    pub expected: Option<Type>,
    pub actual: Option<Type>,
    pub expr: Option<Expr>,
    pub function_name: Option<String>,
    pub parameter_name: Option<String>,
}

impl core::fmt::Display for TypeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Type Error: {}", self.message)?;
        if let (Some(expected), Some(actual)) = (&self.expected, &self.actual) {
            write!(f, " (expected {}, got {})", expected.display(), actual.display())?;
        }
        Ok(())
    }
}

impl core::error::Error for TypeError {}

/// Type checker for LK expressions
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
    /// Active `impl` target type for the current method being checked
    impl_self_type: Option<Type>,
    /// Nesting depth of enclosing `unsafe` blocks.
    ///
    /// A depth rather than a flag because `unsafe` blocks nest, and leaving one
    /// must restore the enclosing state rather than clear it outright.
    unsafe_depth: usize,
    /// Top-level bindings declared *below* the statement being checked.
    ///
    /// The top level runs in order, so a statement there cannot read a `const`
    /// or `let` that comes after it — it reads nil, and what surfaces is
    /// whatever the nil then breaks ("Add expected numbers, got Nil and Int"),
    /// naming neither the binding nor the order. A function body is the
    /// opposite case and must not be caught by this: it runs after the whole
    /// top level, so reading a `const` declared below it is ordinary. Bodies
    /// therefore take this set away for their duration and give it back.
    pending_top_level: HashSet<String>,
    /// Recorded method signatures keyed by (receiver_type, method_name)
    method_sigs: HashMap<(String, String), Type>,
    /// Function strict-Any checks delayed until the whole program contributes call-site constraints.
    pending_strict_functions: Vec<PendingStrictFunction>,
    /// Program-level type checking enables this so later call sites can refine earlier function declarations.
    defer_strict_function_checks: bool,
    /// Members of a namespace bound by `use "lib";` or `use * as m from "lib";`,
    /// keyed by the binding then the member name.
    ///
    /// Separate from `function_sigs` because the name that reaches the checker
    /// is `m.f`, not `f`: two namespaces may each export an `f`, and neither of
    /// them is a free function.
    imported_members: HashMap<String, HashMap<String, Type>>,
    /// Bindings recorded as they are bound, when a caller asked to be told.
    ///
    /// `None` for the compiler's own runs: a check exists to produce an error or
    /// nothing, and recording every binding would be work for a result nobody
    /// reads. An editor wants exactly the opposite — the types, at their
    /// positions, for a file that may not even check cleanly.
    ///
    /// Recorded *during* the walk for the same reason `return_frames` is: it is
    /// the only time the binding's scope is still live. A traversal afterwards
    /// sees every nested scope already popped and every local gone with it.
    observations: Option<Vec<ObservedBinding>>,
    /// Return types observed while checking the body of the function (or closure)
    /// currently being checked, innermost last.
    ///
    /// Collected *during* the walk because that is the only time a `return`'s
    /// scope is still live. A second traversal afterwards sees every nested
    /// `if`/`while`/`for`/`try` scope already popped, so
    /// `fn f() -> Int { if c { let r: Int = 7; return r; } … }` inferred `r` as a
    /// fresh type variable and rejected valid code.
    return_frames: Vec<Vec<Type>>,
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
            function_name: None,
            parameter_name: None,
        };
        anyhow::Error::new(te)
    }

    pub fn implicit_any_type_err(
        function_name: &str,
        issues: &[String],
        parameter_name: Option<&str>,
    ) -> anyhow::Error {
        let message = format!(
            "Function '{}' infers implicit Any for {}; add explicit annotations",
            function_name,
            issues.join(", ")
        );
        anyhow::Error::new(TypeError {
            message,
            expected: None,
            actual: None,
            expr: None,
            function_name: Some(function_name.to_string()),
            parameter_name: parameter_name.map(str::to_string),
        })
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
            impl_self_type: None,
            unsafe_depth: 0,
            pending_top_level: HashSet::new(),
            method_sigs: HashMap::new(),
            pending_strict_functions: Vec::new(),
            defer_strict_function_checks: false,
            imported_members: HashMap::new(),
            observations: None,
            return_frames: Vec::new(),
        }
    }

    /// Record what `namespace.member` is, for a namespace bound by an import.
    pub fn add_imported_member(&mut self, namespace: &str, member: String, ty: Type) {
        self.imported_members
            .entry(namespace.to_string())
            .or_default()
            .insert(member, ty);
    }

    /// The type of `namespace.member`, if the namespace was imported.
    pub fn imported_member_type(&self, namespace: &str, member: &str) -> Option<Type> {
        self.imported_members.get(namespace)?.get(member).cloned()
    }

    /// Start recording every binding this checker binds, with its position.
    pub fn observe_bindings(&mut self) {
        self.observations = Some(Vec::new());
    }

    /// Take what was recorded, leaving recording on.
    pub fn take_observations(&mut self) -> Vec<ObservedBinding> {
        match &mut self.observations {
            Some(observations) => core::mem::take(observations),
            None => Vec::new(),
        }
    }

    /// Record the types just bound by a pattern at `span`.
    ///
    /// Reads the types back out of the environment rather than taking one: a
    /// pattern distributes its value's type over its names, so `let [a, b] = f()`
    /// binds two different types and neither of them is the type of `f()`.
    pub fn record_bindings<'a>(&mut self, span: &Span, annotated: bool, names: impl Iterator<Item = &'a str>) {
        if self.observations.is_none() {
            return;
        }
        let recorded: Vec<ObservedBinding> = names
            .filter_map(|name| {
                self.local_types.get(name).map(|ty| ObservedBinding {
                    span: span.clone(),
                    name: name.to_string(),
                    ty: ty.clone(),
                    annotated,
                })
            })
            .collect();
        if let Some(observations) = &mut self.observations {
            observations.extend(recorded);
        }
    }

    /// How deep the scope stack is, for unwinding after a failed statement.
    pub fn scope_depth(&self) -> usize {
        self.scope_stack.len()
    }

    /// Pop scopes until the stack is `depth` deep.
    ///
    /// A statement that fails mid-body leaves its scopes open — it returned
    /// through the `?` that would have popped them. Without this the *next*
    /// statement is checked inside the failed one's scope, and reports errors
    /// about bindings that are not in fact visible to it.
    pub fn unwind_scopes_to(&mut self, depth: usize) {
        while self.scope_stack.len() > depth {
            self.pop_scope();
        }
    }

    /// Return true when implicit Any fallbacks should be treated strictly
    pub fn strict_any(&self) -> bool {
        self.options.strict_any
    }

    /// Get the active impl `self` type when type-checking trait implementations.
    pub fn current_impl_self_type(&self) -> Option<&Type> {
        self.impl_self_type.as_ref()
    }

    /// Set the active impl `self` type, returning the previous value for restoration.
    pub fn set_impl_self_type(&mut self, ty: Option<Type>) -> Option<Type> {
        core::mem::replace(&mut self.impl_self_type, ty)
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
    /// Whether the checker is currently inside an `unsafe` block.
    ///
    /// The unchecked operations (raw pointer access, volatile, inline assembly)
    /// consult this and reject outside one, so they cannot appear by accident.
    pub fn in_unsafe(&self) -> bool {
        self.unsafe_depth > 0
    }

    /// Records the top-level bindings not yet reached (see the field docs).
    pub fn set_pending_top_level(&mut self, names: HashSet<String>) {
        self.pending_top_level = names;
    }

    /// Marks a top-level binding as reached, so later statements may read it.
    pub fn define_top_level(&mut self, name: &str) {
        self.pending_top_level.remove(name);
    }

    /// Takes the set away for the duration of a function or closure body, which
    /// runs after the top level and may read anything it declares.
    pub fn suspend_pending_top_level(&mut self) -> HashSet<String> {
        core::mem::take(&mut self.pending_top_level)
    }

    pub fn restore_pending_top_level(&mut self, pending: HashSet<String>) {
        self.pending_top_level = pending;
    }

    pub(crate) fn is_pending_top_level(&self, name: &str) -> bool {
        self.pending_top_level.contains(name)
    }

    pub fn enter_unsafe(&mut self) {
        self.unsafe_depth += 1;
    }

    pub fn exit_unsafe(&mut self) {
        self.unsafe_depth = self.unsafe_depth.saturating_sub(1);
    }

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
            Type::Ptr { pointee, mutable } => Type::Ptr {
                pointee: Box::new(self.resolve_aliases_internal(pointee, visiting)),
                mutable: *mutable,
            },
            Type::Map(key, value) => Type::Map(
                Box::new(self.resolve_aliases_internal(key, visiting)),
                Box::new(self.resolve_aliases_internal(value, visiting)),
            ),
            Type::Set(inner) => Type::Set(Box::new(self.resolve_aliases_internal(inner, visiting))),
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
            Type::Any
            | Type::Int
            | Type::MachineInt(_)
            | Type::Float
            | Type::String
            | Type::Bool
            | Type::Nil
            | Type::Variable(_) => ty.clone(),
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

    /// Add a type constraint via the inference engine (for use by external type-checking passes).
    pub fn add_constraint(&mut self, a: Type, b: Type) {
        self.inference_engine.add_constraint(a, b);
    }

    pub fn defer_strict_function_checks(&self) -> bool {
        self.defer_strict_function_checks
    }

    pub fn begin_deferred_strict_function_checks(&mut self) -> bool {
        let previous = self.defer_strict_function_checks;
        self.defer_strict_function_checks = true;
        previous
    }

    pub fn restore_deferred_strict_function_checks(&mut self, previous: bool) {
        self.defer_strict_function_checks = previous;
    }

    pub fn add_pending_strict_function(&mut self, pending: PendingStrictFunction) {
        self.pending_strict_functions.push(pending);
    }

    pub fn finalize_deferred_strict_function_checks(&mut self) -> Result<()> {
        let pending = core::mem::take(&mut self.pending_strict_functions);
        let subs = self.solve_constraints()?;
        self.apply_substitutions_to_environment(&subs);
        self.check_pending_strict_functions(&pending, &subs)
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

    /// Opens a return-collection frame for a function or closure body.
    pub fn push_return_frame(&mut self) {
        self.return_frames.push(Vec::new());
    }

    /// Closes the innermost frame and yields the return types seen in it.
    pub fn pop_return_frame(&mut self) -> Vec<Type> {
        self.return_frames.pop().unwrap_or_default()
    }

    /// Records a `return`'s type against the innermost frame. Outside any
    /// function body (a top-level `return`) there is nothing to collect.
    pub fn record_return(&mut self, ty: Type) {
        if let Some(frame) = self.return_frames.last_mut() {
            frame.push(ty);
        }
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

    fn apply_substitutions_to_environment(&mut self, subs: &HashMap<String, Type>) {
        for ty in self.local_types.values_mut() {
            *ty = ty.substitute(subs);
        }
        for sig in self.function_sigs.values_mut() {
            sig.apply_substitutions(subs);
        }
        for ty in self.method_sigs.values_mut() {
            *ty = ty.substitute(subs);
        }
    }

    fn check_pending_strict_functions(
        &self,
        pending_functions: &[PendingStrictFunction],
        subs: &HashMap<String, Type>,
    ) -> Result<()> {
        for pending in pending_functions {
            let mut issues = Vec::new();
            let mut first_param_name = None;
            for param in &pending.positional {
                let resolved = param.ty.substitute(subs);
                if !param.annotated && Self::type_is_strict_any_unresolved(&resolved) {
                    first_param_name.get_or_insert_with(|| param.name.clone());
                    issues.push(format!("parameter '{}'", param.name));
                }
            }
            for param in &pending.named {
                let resolved = param.ty.substitute(subs);
                if !param.annotated && Self::type_is_strict_any_unresolved(&resolved) {
                    first_param_name.get_or_insert_with(|| param.name.clone());
                    issues.push(format!("named parameter '{}'", param.name));
                }
            }
            let resolved_return = pending.return_type.substitute(subs);
            if !pending.return_annotated && Self::type_is_strict_any_unresolved(&resolved_return) {
                issues.push("return type".to_string());
            }
            if !issues.is_empty() {
                return Err(Self::implicit_any_type_err(
                    &pending.name,
                    &issues,
                    first_param_name.as_deref(),
                ));
            }
        }
        Ok(())
    }

    pub fn type_is_strict_any_unresolved(ty: &Type) -> bool {
        matches!(ty, Type::Any) || ty.contains_variables()
    }
}

/// Function signature for static checking (positional + named)
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSig {
    pub positional: Vec<Type>,
    pub named: Vec<NamedParamSig>,
    pub return_type: Option<Type>,
    /// Which positional parameters the source *annotated*, in order.
    ///
    /// Only those can be checked against at a call site. An unannotated
    /// parameter still ends up with a type — inference gives it one from the
    /// body — but that type is a derivation, not a claim: `fn scale(x) { return
    /// x * 2.5; }` may settle on `Int` for `x`, and rejecting `scale(4.0)`
    /// against it would be rejecting on something the program never said.
    pub annotated: Vec<bool>,
}

impl FunctionSig {
    fn apply_substitutions(&mut self, subs: &HashMap<String, Type>) {
        for ty in &mut self.positional {
            *ty = ty.substitute(subs);
        }
        for param in &mut self.named {
            param.ty = param.ty.substitute(subs);
        }
        if let Some(return_type) = &mut self.return_type {
            *return_type = return_type.substitute(subs);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedParamSig {
    pub name: String,
    pub ty: Type,
    pub has_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingStrictFunction {
    pub name: String,
    pub positional: Vec<PendingStrictParam>,
    pub named: Vec<PendingStrictParam>,
    pub return_type: Type,
    pub return_annotated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingStrictParam {
    pub name: String,
    pub ty: Type,
    pub annotated: bool,
}
