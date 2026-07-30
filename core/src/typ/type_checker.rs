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
    /// The statement the error was raised in.
    ///
    /// `Expr` carries no position, so an error about an expression can only be
    /// placed by searching the token stream for something that looks like it —
    /// which finds the *first* match, not this one (`let a = 1; let b = 1;`
    /// reported the second one's error on the first). The enclosing statement
    /// does have a position, and `Stmt::type_check` attaches it on the way out,
    /// innermost first.
    pub span: Option<Span>,
    pub function_name: Option<String>,
    pub parameter_name: Option<String>,
}

impl TypeError {
    /// Remember the statement this error came from, if it does not know already.
    ///
    /// Innermost wins: a nested statement attaches its own span before an outer
    /// one gets the chance, and the inner one is the smaller, truer range.
    pub fn attach_span(&mut self, span: &Span) {
        if self.span.is_none() {
            self.span = Some(span.clone());
        }
    }
}

impl core::fmt::Display for TypeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Type Error: {}", self.message)?;
        if let (Some(expected), Some(actual)) = (&self.expected, &self.actual) {
            write!(f, " (expected {}, got {})", expected.display(), actual.display())?;
        }
        // `TypeError` carries three things that say *where*: the offending
        // expression, the function it was an argument to, and the statement's
        // span. None of them were rendered, so `Argument 1 has the wrong type
        // (expected Int, got Int?)` was the whole message — and in a
        // four-thousand-line program that is not a diagnostic. Finding the one
        // real instance of it took a bisect script that then got fooled by
        // forward references.
        //
        // The expression is printed rather than the span because it is the field
        // that is actually populated on this path: `Stmt::Expr` (a bare call
        // statement, which is where the argument checks live) is the one
        // statement variant carrying no span at all. Naming that is a separate
        // piece of work; printing what we have is not blocked on it.
        if let Some(func) = &self.function_name {
            write!(f, " in `{func}`")?;
        }
        if let Some(expr) = &self.expr {
            write!(f, " at `{expr}`")?;
        }
        if let Some(span) = &self.span {
            write!(f, " ({})", span.start)?;
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

    /// Local variable types, as a stack of scopes — innermost last, index 0 the
    /// one that is always there.
    ///
    /// It was one flat map plus a **clone of it per scope**: entering a block
    /// copied every binding then in scope, so checking the n-th top-level
    /// function copied the n-1 declared before it. That is quadratic in the size
    /// of the file, and it showed — 250 functions type-checked in 0.04s, 500 in
    /// 0.17s, 1000 in 0.70s, 2000 in 3.03s, 4000 in 23s, while 2000 top-level
    /// `let`s (which declare nothing to copy) took no measurable time at all.
    ///
    /// Layers instead: entering a scope pushes an empty map and leaving it pops
    /// one, both O(1), and a lookup walks outward from the innermost — one or
    /// two layers in practice. The visible semantics are unchanged, including the
    /// part that matters: a binding added inside a scope, or an existing name
    /// rebound there, is gone when the scope ends, because it went into that
    /// scope's own layer.
    local_types: Vec<HashMap<String, Type>>,
    /// Const bindings, layered the same way and for the same reason.
    const_locals: Vec<HashSet<String>>,
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
    /// The declared return type of each open frame, when the callable wrote one.
    ///
    /// Pushed and popped in lockstep with `return_frames` — a `return`'s value
    /// has to be checked *against* the declaration, not merely compared with it
    /// afterwards, because a lambda typed in isolation does not match the
    /// function type written for it (`fn make() -> (Int) -> Int { return |x| …
    /// }` was rejected).
    declared_returns: Vec<Option<Type>>,
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
            span: None,
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
            span: None,
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
            local_types: alloc::vec![HashMap::new()],
            const_locals: alloc::vec![HashSet::new()],
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
            declared_returns: Vec::new(),
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

    /// Each function's inferred return type, by name.
    ///
    /// The signatures are already here — an editor showing `-> String` after a
    /// parameter list has no reason to re-derive it from the token stream.
    /// A `Vec` rather than a map so the caller picks its own container — this
    /// crate's `HashMap` is `hashbrown`'s, which is not the one the LSP holds.
    pub fn function_return_types(&self) -> Vec<(String, Type)> {
        self.function_sigs
            .iter()
            .filter_map(|(name, sig)| sig.return_type.clone().map(|ty| (name.clone(), ty)))
            .collect()
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
                self.lookup_local(name).map(|ty| ObservedBinding {
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
    ///
    /// Counted as the number of *pushed* scopes, so the always-present outermost
    /// layer does not show — the same number this answered when scopes were
    /// snapshots.
    pub fn scope_depth(&self) -> usize {
        self.local_types.len() - 1
    }

    /// Pop scopes until the stack is `depth` deep.
    ///
    /// A statement that fails mid-body leaves its scopes open — it returned
    /// through the `?` that would have popped them. Without this the *next*
    /// statement is checked inside the failed one's scope, and reports errors
    /// about bindings that are not in fact visible to it.
    pub fn unwind_scopes_to(&mut self, depth: usize) {
        while self.scope_depth() > depth {
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

    /// The key a method is registered and looked up under.
    ///
    /// A builtin container's element type is **erased** here, because the
    /// runtime erases it too: `heap_dispatch_type` reports every list as
    /// `List<Any>`, a `TypedList::Mixed` having nothing else to report. Keying
    /// on the static type instead meant `impl T for List` registered under
    /// `List<Any>` while a call on `[1, 2]` looked up `List<Int>` — so the
    /// method existed and could not be found, and the checker rejected the call
    /// before the runtime (which would have found it) ever ran. `String` and
    /// `Map` worked only because neither takes that path.
    fn method_sig_key(&self, receiver: &Type, name: &str) -> (String, String) {
        (
            Self::dispatch_type(&self.resolve_aliases(receiver)).display(),
            name.to_string(),
        )
    }

    /// The type a receiver dispatches on — see [`Self::method_sig_key`].
    pub(crate) fn dispatch_type(resolved: &Type) -> Type {
        match resolved {
            Type::List(_) => Type::List(Box::new(Type::Any)),
            Type::Map(_, _) => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
            Type::Set(_) => Type::Set(Box::new(Type::Any)),
            other => other.clone(),
        }
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

    /// Reject a type annotation naming a type nothing declares.
    ///
    /// `Type::Named` is the parser's answer for any identifier in type
    /// position, so a typo used to become a type: `let x: Strng = "a";`
    /// reported "expected Strng, but expression has type String" — an error
    /// about the *value*, pointing away from the misspelling. Worse in a
    /// signature: `fn f(v: Nonexistent)` made the function uncallable and
    /// blamed every caller ("Argument 1 has the wrong type").
    ///
    /// Known names are the declared ones — structs, traits, aliases — plus the
    /// runtime handles the standard library documents, which have no `Type`
    /// variant of their own.
    pub fn check_type_annotation(&self, ty: &Type, context: &str) -> Result<()> {
        match ty {
            Type::Named(name) => {
                // No escape hatch for a bare `T`: LK has no generic parameter
                // syntax (`fn f<T>(…)` does not parse), so a single uppercase
                // letter in type position is an undeclared name like any
                // other. Exempting it was a guess, and it let exactly the
                // errors this check exists to replace through — `fn f(v: T)`
                // still said "Argument 1 has the wrong type (expected T)".
                if self.registry.resolve_type(name).is_some() || crate::typ::stdlib_sig::is_documented_handle_type(name)
                {
                    return Ok(());
                }
                let hint = self.suggest_type_name(name);
                Err(Self::type_err(
                    &alloc::format!("Unknown type '{name}' in {context}{hint}"),
                    None,
                    None,
                    None,
                ))
            }
            Type::List(inner) | Type::Optional(inner) | Type::Set(inner) | Type::Boxed(inner) => {
                self.check_type_annotation(inner, context)
            }
            Type::Map(key, value) => {
                self.check_type_annotation(key, context)?;
                self.check_type_annotation(value, context)
            }
            Type::Union(variants) | Type::Tuple(variants) => {
                variants.iter().try_for_each(|v| self.check_type_annotation(v, context))
            }
            // A trait's method signatures arrive as one of these, and were the
            // last annotation nothing looked inside: a trait could promise a
            // type that does not exist, and every impl of it was then measured
            // against nothing.
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                params.iter().try_for_each(|p| self.check_type_annotation(p, context))?;
                named_params
                    .iter()
                    .try_for_each(|p| self.check_type_annotation(&p.ty, context))?;
                self.check_type_annotation(return_type, context)
            }
            Type::Task(inner) | Type::Channel(inner) => self.check_type_annotation(inner, context),
            _ => Ok(()),
        }
    }

    /// What the writer probably meant, as a trailing ` — did you mean …` or
    /// the empty string.
    ///
    /// A bare "Unknown type 'bool'" is accurate and useless: LK spells it
    /// `Bool`, and someone arriving from Rust or Python writes `bool`, `str`,
    /// `int` by reflex. The near-misses that matter are a case difference, a
    /// typo, and a handful of names from other languages that LK deliberately
    /// does not have — `f32` among them, which is a decision (one float type,
    /// spelled `Float` or `f64`) rather than an omission.
    fn suggest_type_name(&self, name: &str) -> alloc::string::String {
        // Spellings LK deliberately does not have, and what to write instead.
        const FOREIGN: &[(&str, &str)] = &[
            ("str", "String"),
            ("char", "String"),
            ("void", "Nil"),
            ("none", "Nil"),
            ("null", "Nil"),
            ("f32", "Float"),
            ("f64", "Float"),
            ("i128", "Int"),
            ("u128", "Int"),
            ("boolean", "Bool"),
            ("dict", "Map"),
            ("array", "List"),
            ("vec", "List"),
        ];

        let mut candidates: Vec<alloc::string::String> = crate::val::PRIMITIVE_TYPES
            .iter()
            .map(|(spelling, _)| (*spelling).to_string())
            .chain(
                crate::val::TYPE_SPELLINGS
                    .iter()
                    .map(|(spelling, _)| (*spelling).to_string()),
            )
            .chain(core::iter::once(crate::val::NUMBER_TYPE_NAME.to_string()))
            .chain(crate::val::IntKind::ALL.iter().map(|kind| kind.name().to_string()))
            .collect();
        candidates.extend(self.registry.declared_type_names());

        // A case difference first: it is the likeliest mistake and the surest
        // answer.
        if let Some(exact) = candidates.iter().find(|candidate| candidate.eq_ignore_ascii_case(name)) {
            return alloc::format!(" — did you mean `{exact}`?");
        }
        if let Some((_, replacement)) = FOREIGN.iter().find(|(foreign, _)| foreign.eq_ignore_ascii_case(name)) {
            return alloc::format!(" — LK spells that `{replacement}`");
        }
        // Then a typo, measured rather than guessed: one edit for a short name,
        // two for a longer one, so `Strng` finds `String` and `Foo` does not
        // find `Int`.
        let budget = if name.len() <= 4 { 1 } else { 2 };
        let mut best: Option<(usize, &alloc::string::String)> = None;
        for candidate in &candidates {
            let distance = edit_distance(name, candidate);
            if distance <= budget && best.is_none_or(|(previous, _)| distance < previous) {
                best = Some((distance, candidate));
            }
        }
        match best {
            Some((_, candidate)) => alloc::format!(" — did you mean `{candidate}`?"),
            None => alloc::string::String::new(),
        }
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
        self.lookup_local(name)
    }

    /// Add a type annotation for a local variable
    pub fn add_local_type(&mut self, name: String, typ: Type) {
        self.add_local_binding(name, typ, false);
    }

    /// Add a type annotation with mutability information
    pub fn add_local_binding(&mut self, name: String, typ: Type, is_const: bool) {
        let normalized = self.resolve_aliases(&typ);
        // Into the innermost scope: rebinding a name from an outer one shadows it
        // for the rest of this scope and leaves it alone afterwards, which is
        // what the snapshot-and-restore did.
        let scope = self
            .local_types
            .last_mut()
            .expect("the outermost scope is never popped");
        scope.insert(name.clone(), normalized);
        let consts = self
            .const_locals
            .last_mut()
            .expect("the outermost scope is never popped");
        if is_const {
            consts.insert(name);
        } else {
            consts.remove(name.as_str());
        }
    }

    /// Check whether a local binding is const
    pub fn is_const_local(&self, name: &str) -> bool {
        // Innermost first, like a type lookup: a name rebound in this scope is
        // this scope's binding, const or not.
        self.const_locals
            .iter()
            .rev()
            .zip(self.local_types.iter().rev())
            .find(|(_, types)| types.contains_key(name))
            .is_some_and(|(consts, _)| consts.contains(name))
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
    pub fn push_return_frame(&mut self, declared: Option<Type>) {
        self.return_frames.push(Vec::new());
        self.declared_returns.push(declared);
    }

    /// The declared return type of the innermost open frame.
    pub fn declared_return(&self) -> Option<Type> {
        self.declared_returns.last().cloned().flatten()
    }

    /// The kind of declaration already bound to `name` at the top level, if any.
    ///
    /// A `fn` and a type declaration are **hoisted**: mutual recursion works, so
    /// a `fn` is visible before the line it is written on. Source order
    /// therefore does not apply to them, and "a `let` shadows it" has no
    /// coherent meaning — which showed as `fn pick() {…}` then `let pick = …;`
    /// resolving to the `let` *in either order*, silently. Two `fn`s of one name
    /// were already refused; this is the same collision.
    pub fn top_level_declaration_kind(&self, name: &str) -> Option<&'static str> {
        if self.get_function_sig(name).is_some() {
            return Some("function");
        }
        if self.registry.get_struct(name).is_some() {
            return Some("struct");
        }
        if self.registry.get_type_alias(name).is_some() {
            return Some("type alias");
        }
        None
    }

    /// Whether the walk is currently inside a function or closure body.    /// Whether the walk is currently inside a function or closure body.
    ///
    /// The return frames answer this exactly — one is open for the duration of
    /// every callable body and nothing else — so there is no second piece of
    /// bookkeeping to keep in step.
    pub fn inside_callable_body(&self) -> bool {
        !self.return_frames.is_empty()
    }

    /// Closes the innermost frame and yields the return types seen in it.
    pub fn pop_return_frame(&mut self) -> Vec<Type> {
        self.declared_returns.pop();
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
        self.local_types.push(HashMap::new());
        self.const_locals.push(HashSet::new());
    }

    /// Exit the current scope, discarding what it bound.
    ///
    /// The outermost layer is never popped: it is the scope every check starts
    /// in, and an unbalanced `pop_scope` used to silently leave the checker with
    /// the *previous* snapshot instead.
    pub fn pop_scope(&mut self) {
        if self.local_types.len() > 1 {
            self.local_types.pop();
        }
        if self.const_locals.len() > 1 {
            self.const_locals.pop();
        }
    }

    /// The type bound to `name`, searching from the innermost scope outward.
    fn lookup_local(&self, name: &str) -> Option<&Type> {
        self.local_types.iter().rev().find_map(|scope| scope.get(name))
    }

    fn apply_substitutions_to_environment(&mut self, subs: &HashMap<String, Type>) {
        // Every layer: this runs once, after the whole program, when only the
        // outermost is left — writing to all of them keeps that true if it ever
        // runs somewhere deeper.
        for ty in self.local_types.iter_mut().flat_map(|scope| scope.values_mut()) {
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
        // The check this defers is the strict-Any one, so a non-strict run has
        // nothing to do here. It matters now that *both* modes defer: the
        // deferral used to imply strictness, and it no longer does.
        if !self.strict_any() {
            return Ok(());
        }
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

/// Levenshtein distance, for the "did you mean" hint on an unknown type name.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, l) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, r) in right.iter().enumerate() {
            let substitute = previous[j] + usize::from(l != r);
            current[j + 1] = substitute.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        core::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// Whether a value of this type could ever be a map key or a set member.
///
/// The rule is `RuntimeMapKey::from_value`'s, and it is one rule for both
/// questions because a set *is* a map's key set: nil, Bool, Int and String, and
/// nothing else. Float is out because `0.0 == -0.0` while their bits differ and
/// NaN is not equal to itself; containers are out because a key that can be
/// mutated is a record you can no longer find. See `docs/semantics.md`.
///
/// The runtime enforced it and the checker did not, so `Set([1.5])` and
/// `{1.5: "a"}` type-checked and raised at run time — with the key type sitting
/// right there in the literal.
///
/// Answers `false` only when the type is *certainly* unusable. `Any`, a type
/// variable and any union pass: a `Int | Float` value may well be the Int at run
/// time, and refusing a working program is worse than letting the runtime have
/// the last word on one that is not.
pub(crate) fn type_is_certainly_not_a_key(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Float | Type::List(_) | Type::Map(_, _) | Type::Set(_) | Type::Tuple(_)
    )
}

/// Collapses distributed alternatives: identical types stay themselves, `Any`
/// anywhere swallows the rest (nothing is known), otherwise a union.
///
/// Lives here rather than beside one of its callers because there are now two
/// of them in different modules — pattern distribution and a closure's return
/// type — and "collapse a set of alternatives" is one rule, not two.
pub(crate) fn union_of(types: impl IntoIterator<Item = Type>) -> Type {
    let mut out: Vec<Type> = Vec::new();
    for ty in types {
        if ty == Type::Any {
            return Type::Any;
        }
        if !out.contains(&ty) {
            out.push(ty);
        }
    }
    match out.len() {
        0 => Type::Any,
        1 => out.pop().expect("checked len"),
        _ => Type::Union(out),
    }
}
