#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
mod calls;
mod literals;
mod stdlib;

use super::{NamedParamSig, TypeChecker};
use crate::expr::Expr;
use crate::operator::{BinOp, UnaryOp};
use crate::typ::{NumericClass, NumericHierarchy};
use crate::val::{FunctionNamedParamType, LiteralVal, Type};
use anyhow::{Result, anyhow};
use hashbrown::HashMap;

impl TypeChecker {
    fn enforce_int_type(&mut self, expr: &Expr, ty: Type, context: &str) -> Result<()> {
        let resolved = self.resolve_aliases(&ty);
        match resolved {
            Type::Int => Ok(()),
            Type::Variable(_) => {
                self.inference_engine.add_constraint(ty, Type::Int);
                Ok(())
            }
            Type::Any => {
                if self.strict_any() {
                    Err(Self::type_err(
                        &format!("{context} must be Int"),
                        Some(Type::Int),
                        Some(Type::Any),
                        Some(expr.clone()),
                    ))
                } else {
                    Ok(())
                }
            }
            // Box<T> from numeric-hierarchy arithmetic on Any values — unwrap and re-check inner.
            // Box<Any> results from arithmetic like `native_fn() - native_fn()` where the native
            // return type is unresolvable at compile time; treat it the same as Any.
            Type::Boxed(inner) => self.enforce_int_type(expr, *inner, context),
            other => Err(Self::type_err(
                &format!("{context} must be Int"),
                Some(Type::Int),
                Some(other),
                Some(expr.clone()),
            )),
        }
    }

    /// Enforce that a type is Bool, adding a constraint for type variables.
    fn enforce_bool_type(&mut self, ty: &Type, expr: &Expr) -> Result<()> {
        let resolved = self.resolve_aliases(ty);
        match &resolved {
            Type::Bool => Ok(()),
            Type::Variable(_) => {
                self.inference_engine.add_constraint(ty.clone(), Type::Bool);
                Ok(())
            }
            Type::Any => {
                if self.strict_any() {
                    Err(Self::type_err(
                        "Expected boolean type",
                        Some(Type::Bool),
                        Some(ty.clone()),
                        Some(expr.clone()),
                    ))
                } else {
                    Ok(())
                }
            }
            Type::Union(variants) => {
                // Accept union if any variant is Bool, Nil, or Variable (falsy-aware)
                if variants
                    .iter()
                    .any(|v| matches!(v, Type::Bool | Type::Nil | Type::Variable(_) | Type::Any))
                {
                    // Add constraints for each Variable variant
                    for v in variants {
                        if matches!(v, Type::Variable(_)) {
                            self.inference_engine.add_constraint(v.clone(), Type::Bool);
                        }
                    }
                    Ok(())
                } else {
                    Err(Self::type_err(
                        "Expected boolean type",
                        Some(Type::Bool),
                        Some(ty.clone()),
                        Some(expr.clone()),
                    ))
                }
            }
            other => Err(Self::type_err(
                "Expected boolean type",
                Some(Type::Bool),
                Some(other.clone()),
                Some(expr.clone()),
            )),
        }
    }

    /// Type check an expression.
    pub fn check_expr(&mut self, expr: &Expr) -> Result<Type> {
        self.check_expr_inner(expr)
    }

    /// `expr as T`.
    ///
    /// Casts are only allowed where a bit-level reinterpretation is meaningful:
    /// between numbers, and between numbers and `Bool`. Casting a `List` to a
    /// `u8` is a mistake, not a reinterpretation, so it stays an error rather
    /// than silently producing something.
    ///
    /// The source type is checked but does not otherwise constrain the result:
    /// a cast's whole job is to produce the target type. Range is deliberately
    /// *not* checked — `300 as u8` is 44, because a driver author writing a
    /// cast is asking for those bits at that width, not for a bounds check.
    /// The range check lives on the annotation path instead (`let x: u8 = 300`
    /// is an error), which is where a mistake is more likely than an intent.
    fn check_cast(&mut self, inner: &Expr, target: &Type) -> Result<Type> {
        let source = self.check_expr(inner)?;
        if !cast_is_meaningful(&source, target) {
            return Err(anyhow!(
                "cannot cast {} to {}: casts are only defined between numeric types",
                source.display(),
                target.display()
            ));
        }
        Ok(target.clone())
    }

    /// Checks an `unsafe` block's contents.
    ///
    /// `Expr::Block` on its own type-checks to `Any` without looking inside —
    /// blocks are mostly produced by desugars, which are checked before they
    /// are built. That is fine for those, but it would make `unsafe { … }` a
    /// hole in the type system: precisely the construct that needs *more*
    /// scrutiny would get none. So the statements are checked here.
    ///
    /// The block's own type stays `Any` for now, matching `Expr::Block`; a
    /// block that evaluates to a typed value is a separate change.
    fn check_unsafe_body(&mut self, inner: &Expr) -> Result<Type> {
        let Expr::Block(statements) = inner else {
            return self.check_expr(inner);
        };
        for stmt in statements {
            stmt.type_check(self)?;
        }
        Ok(Type::Any)
    }

    /// The `cpu_*` intrinsics: barriers, interrupt masking, wait-for-interrupt.
    ///
    /// These need `unsafe` for a different reason than pointers do — nothing
    /// here can corrupt memory. Masking interrupts or parking the core changes
    /// the machine's state in a way the rest of the program's correctness may
    /// depend on, and getting the nesting wrong deadlocks rather than crashes.
    /// Marking it makes the region auditable.
    fn check_cpu_builtin(&mut self, name: &str, args: &[Box<Expr>]) -> Result<Option<Type>> {
        let (arity, result) = match name {
            "cpu_barrier" | "cpu_compiler_barrier" | "cpu_wait_for_interrupt" => (0, Type::Nil),
            "cpu_irq_save" => (0, Type::Int),
            "cpu_irq_restore" => (1, Type::Nil),
            _ => return Ok(None),
        };
        if args.len() != arity {
            return Err(anyhow!("{name} expects {arity} argument(s), got {}", args.len()));
        }
        if !self.in_unsafe() {
            return Err(anyhow!(
                "{name} requires an `unsafe` block: it changes machine state the rest of the \
                 program's correctness can depend on"
            ));
        }
        if name == "cpu_irq_restore" {
            let saved = self.check_expr(&args[0])?;
            if !self.is_assignable(&saved, &Type::Int) {
                return Err(anyhow!(
                    "cpu_irq_restore expects the value returned by cpu_irq_save, got {}",
                    saved.display()
                ));
            }
        }
        Ok(Some(result))
    }

    /// `volatile_read_uN(ptr)` / `volatile_write_uN(ptr, value)`.
    ///
    /// Returns `None` for any other name, so ordinary calls fall through.
    ///
    /// These are intrinsics rather than syntax on purpose. `*p` would need the
    /// *compiler* to know the pointee's width to emit the right load, and the
    /// compiler has no access to the type checker — the width lives in the name
    /// instead. It also makes the volatile-ness explicit, which `*p` never is
    /// in any language.
    fn check_volatile_builtin(&mut self, name: &str, args: &[Box<Expr>]) -> Result<Option<Type>> {
        if let Some(result) = self.check_cpu_builtin(name, args)? {
            return Ok(Some(result));
        }
        let Some((is_write, kind)) = parse_volatile_builtin(name) else {
            return Ok(None);
        };
        let expected_args = if is_write { 2 } else { 1 };
        if args.len() != expected_args {
            return Err(anyhow!(
                "{name} expects {expected_args} argument(s), got {}",
                args.len()
            ));
        }
        if !self.in_unsafe() {
            return Err(anyhow!(
                "{name} requires an `unsafe` block: the compiler cannot check that the address is \
                 mapped, aligned, or safe to access"
            ));
        }

        let value_ty = Type::MachineInt(kind);
        let ptr_ty = self.check_expr(&args[0])?;
        let resolved = self.resolve_aliases(&ptr_ty);
        let Type::Ptr { pointee, mutable } = &resolved else {
            return Err(anyhow!(
                "{name} expects a pointer as its first argument, got {}",
                ptr_ty.display()
            ));
        };
        if pointee.as_ref() != &value_ty {
            return Err(anyhow!(
                "{name} expects a pointer to {}, got {}",
                value_ty.display(),
                ptr_ty.display()
            ));
        }
        if is_write && !mutable {
            return Err(anyhow!(
                "{name} needs a `*mut` pointer; {} is read-only",
                ptr_ty.display()
            ));
        }

        if is_write {
            let written = self.check_expr(&args[1])?;
            if !self.is_assignable(&written, &value_ty) {
                return Err(anyhow!(
                    "{name} expects a {} value, got {}",
                    value_ty.display(),
                    written.display()
                ));
            }
            return Ok(Some(Type::Nil));
        }
        Ok(Some(value_ty))
    }

    /// Internal expression checker without recording.
    fn check_expr_inner(&mut self, expr: &Expr) -> Result<Type> {
        match expr {
            // Literals (via LiteralVal enum)
            Expr::Literal(val) => self.check_literal(val),

            // Variables
            Expr::Var(name) => self.check_identifier(name),

            // Explicit conversion
            Expr::Cast(inner, target) => self.check_cast(inner, target),

            // `unsafe { … }` — the block's own type, checked with the
            // unchecked operations permitted inside it.
            Expr::Unsafe(inner) => {
                self.enter_unsafe();
                let result = self.check_unsafe_body(inner);
                self.exit_unsafe();
                result
            }

            // Binary operations
            Expr::Bin(_, _, _) => self.check_binary_op_iter(expr),
            Expr::And(left, right) => self.check_logical_op(left, right, Type::Bool),
            Expr::Or(left, right) => self.check_logical_op(left, right, Type::Bool),

            // Unary operations
            Expr::Unary(op, expr) => self.check_unary_op(op, expr),

            // Collections
            Expr::List(items) => self.check_list(items),
            Expr::Map(pairs) => self.check_map(pairs),
            Expr::StructLiteral { name, fields } => {
                // If struct is known, enforce field presence and types; otherwise, accept as named type
                if let Some(sd) = self.registry.get_struct(name) {
                    let schema = sd.fields.clone();
                    // Provided -> check existence and type
                    for (fname, fexpr) in fields {
                        let expected = schema.get(fname).cloned();
                        let at = self.check_expr(fexpr)?;
                        if let Some(expected) = expected {
                            if !self.is_assignable(&at, &expected) {
                                return Err(Self::type_err(
                                    &format!("Field '{}' type mismatch in struct '{}'", fname, name),
                                    Some(expected.clone()),
                                    Some(at),
                                    Some(fexpr.as_ref().clone()),
                                ));
                            }
                        } else {
                            return Err(Self::type_err(
                                &format!("Unknown field '{}' for struct '{}'", fname, name),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    // Missing required fields (non-optional)
                    use Type as T;
                    for (req_name, req_ty) in &schema {
                        let provided = fields.iter().any(|(n, _)| n == req_name);
                        if !provided {
                            let optional = matches!(req_ty, T::Optional(_))
                                || matches!(req_ty, T::Union(v) if v.contains(&T::Nil));
                            if !optional {
                                return Err(Self::type_err(
                                    &format!("Missing required field '{}' for struct '{}'", req_name, name),
                                    None,
                                    None,
                                    None,
                                ));
                            }
                        }
                    }
                }
                Ok(Type::Named(name.clone()))
            }

            // Access operations
            Expr::Access(expr, field) => self.check_access(expr, field),
            Expr::NullishCoalescing(expr, default) => self.check_nullish_coalescing(expr, default),
            Expr::OptionalAccess(expr, field) => self.check_optional_chaining(expr, field),
            Expr::Conditional(cond, then_expr, else_expr) => {
                // condition must be Bool
                let cond_ty = self.check_expr(cond)?;
                if cond_ty != Type::Bool {
                    return Err(Self::type_err(
                        "Ternary condition must be Bool",
                        Some(Type::Bool),
                        Some(cond_ty),
                        Some(*cond.clone()),
                    ));
                }
                let then_ty = self.check_expr(then_expr)?;
                let else_ty = self.check_expr(else_expr)?;
                // unify then/else types; return the unified type (prefer then_ty)
                self.inference_engine.add_constraint(then_ty.clone(), else_ty.clone());
                Ok(then_ty)
            }
            // Functions - handle both Call (string name) and CallExpr (expression)
            Expr::Call(func, args) => {
                // Volatile access is checked here rather than through an
                // ordinary signature: its argument must be a *pointer of the
                // matching width*, which a plain `(usize) -> u32` signature
                // cannot express, and it has to demand `unsafe`.
                if let Some(result) = self.check_volatile_builtin(func, args)? {
                    return Ok(result);
                }
                // For Call with string name, create a variable expression for the function
                let func_expr = Expr::Var(func.clone());
                self.check_function_call(&func_expr, args)
            }
            Expr::CallExpr(func_expr, args) => {
                // Source-level calls parse to `CallExpr`; `Call` is only built
                // by internal desugars.
                if let Expr::Var(name) = func_expr.as_ref()
                    && let Some(result) = self.check_volatile_builtin(name, args)?
                {
                    return Ok(result);
                }
                self.check_function_call(func_expr, args)
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                // Struct constructor sugar: TypeName(field: expr, ...)
                if let Expr::Var(name) = callee.as_ref()
                    && let Some(sd) = self.registry.get_struct(name)
                {
                    let schema = sd.fields.clone();
                    if !pos_args.is_empty() {
                        return Err(Self::type_err(
                            &format!("Struct constructor '{}' does not accept positional arguments", name),
                            None,
                            None,
                            None,
                        ));
                    }
                    use crate::compat::collections::HashSet;
                    let mut seen_names: HashSet<&str> = HashSet::with_capacity(named_args.len());
                    // Check named arguments
                    for (n, e) in named_args {
                        let key = n.as_str();
                        if !seen_names.insert(key) {
                            return Err(Self::type_err(
                                &format!("Duplicate field '{}' for struct '{}'", n, name),
                                None,
                                None,
                                Some(e.as_ref().clone()),
                            ));
                        }
                        // Unknown field
                        if !schema.contains_key(n) {
                            return Err(Self::type_err(
                                &format!("Unknown field '{}' for struct '{}'", n, name),
                                None,
                                None,
                                Some(e.as_ref().clone()),
                            ));
                        }
                        let at = self.check_expr(e)?;
                        if let Some(expected) = schema.get(n)
                            && !self.is_assignable(&at, expected)
                        {
                            return Err(Self::type_err(
                                &format!("Field '{}' type mismatch in struct '{}'", n, name),
                                Some(expected.clone()),
                                Some(at),
                                Some(e.as_ref().clone()),
                            ));
                        }
                    }
                    // Missing required fields
                    use Type as T;
                    for (req_name, req_ty) in &schema {
                        let provided = seen_names.contains(req_name.as_str());
                        if !provided {
                            let optional = matches!(req_ty, T::Optional(_))
                                || matches!(req_ty, T::Union(v) if v.contains(&T::Nil));
                            if !optional {
                                return Err(Self::type_err(
                                    &format!("Missing required field '{}' for struct '{}'", req_name, name),
                                    None,
                                    None,
                                    None,
                                ));
                            }
                        }
                    }

                    return Ok(Type::Named(name.clone()));
                }
                if let Some(return_type) = self.check_stdlib_named_function_call(callee, pos_args, named_args)? {
                    return Ok(return_type);
                }
                // Type-check callee first
                let callee_type = self.check_expr(callee)?;

                // Type-check argument expressions and keep their types
                let mut pos_types: Vec<Type> = Vec::with_capacity(pos_args.len());
                for a in pos_args {
                    pos_types.push(self.check_expr(a)?);
                }
                let mut named_types: Vec<(String, Type)> = Vec::with_capacity(named_args.len());
                for (n, e) in named_args {
                    named_types.push((n.clone(), self.check_expr(e)?));
                }

                // If callee is a variable and we have a signature, enforce named rules
                if let Expr::Var(name) = callee.as_ref()
                    && let Some(sig) = self.get_function_sig(name).cloned()
                {
                    // Check positional arity
                    if sig.positional.len() != pos_types.len() {
                        return Err(Self::type_err(
                            &format!(
                                "Function '{}' expects {} positional args, got {}",
                                name,
                                sig.positional.len(),
                                pos_types.len()
                            ),
                            None,
                            None,
                            None,
                        ));
                    }
                    // Constrain positional types
                    for (pt, at) in sig.positional.iter().zip(pos_types.iter()) {
                        self.inference_engine.add_constraint(pt.clone(), at.clone());
                    }

                    // Duplicate/unknown
                    use crate::compat::collections::{HashMap as Map, HashSet};
                    let mut sig_lookup: Map<&str, &NamedParamSig> = Map::with_capacity(sig.named.len());
                    for decl in &sig.named {
                        sig_lookup.insert(decl.name.as_str(), decl);
                    }
                    let mut seen: HashSet<&str> = HashSet::with_capacity(named_types.len());
                    for (n, _) in &named_types {
                        let key = n.as_str();
                        if !seen.insert(key) {
                            return Err(Self::type_err(
                                &format!("Duplicate named argument: {}", n),
                                None,
                                None,
                                None,
                            ));
                        }
                        if !sig_lookup.contains_key(key) {
                            return Err(Self::type_err(
                                &format!("Unknown named argument: {}", n),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    // Required named presence
                    for decl in &sig.named {
                        let is_optional = matches!(decl.ty, Type::Optional(_));
                        if !is_optional && !decl.has_default && !seen.contains(decl.name.as_str()) {
                            return Err(Self::type_err(
                                &format!("Missing required named argument: {}", decl.name),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    // Type constraints for provided named
                    let mut name_to_ty: Map<&str, Type> = Map::with_capacity(sig.named.len());
                    for d in &sig.named {
                        name_to_ty.insert(d.name.as_str(), d.ty.clone());
                    }
                    for (n, at) in &named_types {
                        if let Some(decl_ty) = name_to_ty.get(n.as_str()) {
                            self.inference_engine.add_constraint(decl_ty.clone(), at.clone());
                        }
                    }
                }

                // Fall back to callee function type for return
                match callee_type {
                    Type::Function {
                        params,
                        named_params,
                        return_type,
                    } => {
                        // Basic positional arity check when no signature is available
                        if params.len() != pos_types.len() {
                            return Err(Self::type_err(
                                &format!("Function expects {} positional arguments", params.len()),
                                None,
                                None,
                                None,
                            ));
                        }
                        for (pt, at) in params.iter().zip(pos_types.iter()) {
                            self.inference_engine.add_constraint(pt.clone(), at.clone());
                        }
                        if !named_params.is_empty() || !named_types.is_empty() {
                            use crate::compat::collections::{HashMap as Map, HashSet};
                            let decl_map: Map<&str, &FunctionNamedParamType> =
                                named_params.iter().map(|np| (np.name.as_str(), np)).collect();
                            let mut provided: HashSet<&str> = HashSet::with_capacity(named_types.len());
                            for (n, ty) in &named_types {
                                let key = n.as_str();
                                if !decl_map.contains_key(key) {
                                    return Err(Self::type_err(
                                        &format!("Unknown named argument: {}", n),
                                        None,
                                        None,
                                        None,
                                    ));
                                }
                                provided.insert(key);
                                let decl_ty = &decl_map[key].ty;
                                self.inference_engine.add_constraint(decl_ty.clone(), ty.clone());
                            }
                            for decl in &named_params {
                                let is_optional = matches!(decl.ty, Type::Optional(_)) || decl.has_default;
                                if !is_optional && !provided.contains(decl.name.as_str()) {
                                    return Err(Self::type_err(
                                        &format!("Missing required named argument: {}", decl.name),
                                        None,
                                        None,
                                        None,
                                    ));
                                }
                            }
                        }
                        Ok(*return_type)
                    }
                    Type::Any | Type::Variable(_) => Ok(Type::Any),
                    Type::Map(_, _) => Ok(Type::Any),
                    Type::Union(_) => Ok(Type::Any),
                    other => Err(Self::type_err("Cannot call non-function type", None, Some(other), None)),
                }
            }

            // Complex expressions
            Expr::TemplateString(parts) => self.check_template_string(parts),

            // Range expressions behave like synthetic Int lists
            Expr::Range {
                start,
                end,
                inclusive: _,
                step,
            } => {
                if let Some(start_expr) = start {
                    let start_ty = self.check_expr(start_expr)?;
                    self.enforce_int_type(start_expr.as_ref(), start_ty, "Range start")?;
                }
                if let Some(end_expr) = end {
                    let end_ty = self.check_expr(end_expr)?;
                    self.enforce_int_type(end_expr.as_ref(), end_ty, "Range end")?;
                }
                if let Some(step_expr) = step {
                    let step_ty = self.check_expr(step_expr)?;
                    self.enforce_int_type(step_expr.as_ref(), step_ty, "Range step")?;
                }
                Ok(Type::List(Box::new(Type::Int)))
            }
            Expr::Closure { params, body } => {
                // Infer closure as a function type with param type variables and an inferred return
                let mut param_types = Vec::with_capacity(params.len());
                for _ in params {
                    param_types.push(self.inference_engine.fresh_type_var());
                }
                // Body type is inferred by checking the body expression. Its own
                // return frame: a `return` inside a closure body belongs to the
                // closure, and must not be collected as a return of the enclosing
                // function (whose declared type it would then have to satisfy).
                self.push_return_frame();
                let ret_type = self.check_expr(body);
                let _ = self.pop_return_frame();
                let ret_type = ret_type?;
                Ok(Type::Function {
                    params: param_types,
                    named_params: Vec::new(),
                    return_type: Box::new(ret_type),
                })
            }
            Expr::Match { value, arms } => {
                // Check the matched value type
                let value_type = self.check_expr(value)?;

                if arms.is_empty() {
                    return Err(Self::type_err(
                        "Match expression must have at least one arm",
                        None,
                        None,
                        Some(expr.clone()),
                    ));
                }

                // Check all arms have compatible types
                let mut result_type: Option<Type> = None;
                for arm in arms {
                    // Ensure pattern is compatible with the matched value type
                    self.check_pattern_against_type(&arm.pattern, &value_type)?;

                    // Arm body is checked in a scope with pattern bindings available
                    let locals_snapshot = self.local_types.clone();
                    self.add_bindings_for_pattern(&arm.pattern, &value_type)?;
                    let arm_type = self.check_expr(&arm.body)?;
                    self.local_types = locals_snapshot;

                    if let Some(existing_type) = &result_type {
                        // Add constraint that all arms should return the same type
                        self.inference_engine
                            .add_constraint(existing_type.clone(), arm_type.clone());
                    } else {
                        result_type = Some(arm_type);
                    }
                }

                result_type
                    .ok_or_else(|| Self::type_err("Match expression has no arms", None, None, Some(expr.clone())))
            }
            Expr::Paren(expr) => self.check_expr(expr),
            Expr::Block(_) => Ok(Type::Any),
        }
    }

    /// Type check an expression and return a type with current constraints solved
    pub fn infer_resolved_type(&mut self, expr: &Expr) -> Result<Type> {
        let ty = self.check_expr(expr)?;
        // Attempt to solve constraints and substitute into the resulting type
        match self.inference_engine.solve_constraints() {
            Ok(subs) => Ok(ty.substitute(&subs)),
            Err(_) => Ok(ty), // On failure, return the unsolved type to avoid hard errors in tooling
        }
    }

    /// Generate a fresh type variable from the inference engine
    pub fn fresh_type_var(&mut self) -> Type {
        self.inference_engine.fresh_type_var()
    }

    /// Apply the given substitution map to a type
    pub fn apply_substitutions(&self, ty: Type, subs: &HashMap<String, Type>) -> Type {
        ty.substitute(subs)
    }

    /// Check identifier type
    fn check_identifier(&mut self, name: &str) -> Result<Type> {
        // Check local variables first
        if let Some(typ) = self.local_types.get(name) {
            return Ok(typ.clone());
        }

        // Check type registry for named types
        if let Some(typ) = self.registry.resolve_type(name) {
            return Ok(typ);
        }

        // Otherwise, assume it's a dynamic variable (type inference needed)
        let var_type = self.inference_engine.fresh_type_var();
        self.local_types.insert(name.to_string(), var_type.clone());
        Ok(var_type)
    }

    // Removed '@' context access.

    /// Check binary operation types
    fn check_binary_op_with_types(
        &mut self,
        left_expr: &Expr,
        left_type: Type,
        op: &BinOp,
        right_expr: &Expr,
        right_type: Type,
    ) -> Result<Type> {
        match op {
            BinOp::Add => {
                let left_resolved = self.resolve_aliases(&left_type);
                let right_resolved = self.resolve_aliases(&right_type);
                if matches!(left_resolved, Type::List(_)) || matches!(right_resolved, Type::List(_)) {
                    return self.check_list_addition(left_expr, &left_type, right_expr, &right_type);
                }
                if self.is_string_like(&left_type) || self.is_string_like(&right_type) {
                    self.check_string_addition(left_expr, &left_type, right_expr, &right_type)
                } else {
                    self.check_numeric_bin_op(left_expr, &left_type, right_expr, &right_type, op)
                }
            }
            BinOp::Mul if self.is_string_like(&left_type) || self.is_string_like(&right_type) => {
                let left_string = self.is_string_like(&left_type);
                let right_string = self.is_string_like(&right_type);
                let left_int = matches!(self.resolve_aliases(&left_type), Type::Int);
                let right_int = matches!(self.resolve_aliases(&right_type), Type::Int);
                if (left_string && right_int) || (left_int && right_string) {
                    Ok(Type::String)
                } else {
                    self.check_numeric_bin_op(left_expr, &left_type, right_expr, &right_type, op)
                }
            }
            BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                self.check_numeric_bin_op(left_expr, &left_type, right_expr, &right_type, op)
            }
            BinOp::Eq | BinOp::Ne => {
                self.inference_engine
                    .add_constraint(left_type.clone(), right_type.clone());
                Ok(Type::Bool)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.ensure_numeric_operand(&left_type, left_expr, "左侧")?;
                self.ensure_numeric_operand(&right_type, right_expr, "右侧")?;
                Ok(Type::Bool)
            }
            BinOp::In => match self.resolve_aliases(&right_type) {
                Type::List(_) | Type::Map(_, _) | Type::Set(_) => Ok(Type::Bool),
                other => Err(Self::type_err(
                    "'in' operator requires container type",
                    Some(Type::List(Box::new(Type::Any))),
                    Some(other),
                    Some(Expr::Bin(
                        Box::new(left_expr.clone()),
                        op.clone(),
                        Box::new(right_expr.clone()),
                    )),
                )),
            },
        }
    }

    fn check_binary_op_iter(&mut self, root: &Expr) -> Result<Type> {
        let mut chain: Vec<(&Expr, &Expr, &BinOp, &Expr)> = Vec::new();
        let mut current = root;
        while let Expr::Bin(left, op, right) = current {
            chain.push((current, left.as_ref(), op, right.as_ref()));
            current = left;
        }

        let mut acc_type = self.check_expr(current)?;
        for (_, left_expr, op, right_expr) in chain.into_iter().rev() {
            let right_type = self.check_expr(right_expr)?;
            acc_type = self.check_binary_op_with_types(left_expr, acc_type, op, right_expr, right_type)?;
        }
        Ok(acc_type)
    }

    fn is_string_like(&self, ty: &Type) -> bool {
        matches!(self.resolve_aliases(ty), Type::String)
    }

    fn coerce_to_string(&mut self, ty: &Type) {
        if matches!(ty, Type::Variable(_)) {
            self.inference_engine.add_constraint(ty.clone(), Type::String);
        }
    }

    fn check_string_addition(
        &mut self,
        _left_expr: &Expr,
        left_ty: &Type,
        _right_expr: &Expr,
        right_ty: &Type,
    ) -> Result<Type> {
        self.coerce_to_string(left_ty);
        self.coerce_to_string(right_ty);
        Ok(Type::String)
    }

    fn check_list_addition(
        &mut self,
        left_expr: &Expr,
        left_ty: &Type,
        right_expr: &Expr,
        right_ty: &Type,
    ) -> Result<Type> {
        let left_resolved = self.resolve_aliases(left_ty);
        let right_resolved = self.resolve_aliases(right_ty);
        match (left_resolved, right_resolved) {
            (Type::List(left_inner), Type::List(right_inner)) => {
                let elem_ty = if self.is_assignable(left_inner.as_ref(), right_inner.as_ref()) {
                    (*left_inner).clone()
                } else if self.is_assignable(right_inner.as_ref(), left_inner.as_ref()) {
                    (*right_inner).clone()
                } else {
                    Type::Any
                };
                Ok(Type::List(Box::new(elem_ty)))
            }
            (Type::List(_), other) => Err(Self::type_err(
                "List concatenation requires both operands to be lists",
                Some(Type::List(Box::new(Type::Any))),
                Some(other),
                Some(Expr::Bin(
                    Box::new(left_expr.clone()),
                    BinOp::Add,
                    Box::new(right_expr.clone()),
                )),
            )),
            (other, Type::List(_)) => Err(Self::type_err(
                "List concatenation requires both operands to be lists",
                Some(Type::List(Box::new(Type::Any))),
                Some(other),
                Some(Expr::Bin(
                    Box::new(left_expr.clone()),
                    BinOp::Add,
                    Box::new(right_expr.clone()),
                )),
            )),
            _ => Err(Self::type_err(
                "List concatenation requires both operands to be lists",
                Some(Type::List(Box::new(Type::Any))),
                None,
                Some(Expr::Bin(
                    Box::new(left_expr.clone()),
                    BinOp::Add,
                    Box::new(right_expr.clone()),
                )),
            )),
        }
    }

    fn check_numeric_bin_op(
        &mut self,
        left_expr: &Expr,
        left_ty: &Type,
        right_expr: &Expr,
        right_ty: &Type,
        op: &BinOp,
    ) -> Result<Type> {
        let resolved_left = self.resolve_aliases(left_ty);
        let resolved_right = self.resolve_aliases(right_ty);

        // Machine integers stay in their own width: `u8 + u8` is `u8`, wrapping
        // on overflow. Mixing widths or signedness is an error rather than a
        // promotion — the same reason they do not convert implicitly. Promoting
        // to `Int` here would silently give the operation 64-bit semantics,
        // which is exactly what a driver author asked not to have.
        if let (Type::MachineInt(left_kind), Type::MachineInt(right_kind)) = (&resolved_left, &resolved_right) {
            if left_kind != right_kind {
                return Err(Self::type_err(
                    "machine integer operands must have the same type",
                    Some(Type::MachineInt(*left_kind)),
                    Some(Type::MachineInt(*right_kind)),
                    Some(right_expr.clone()),
                ));
            }
            // Division still yields the same width, unlike `Int / Int -> Float`:
            // a fixed-width type has no float to promote to, and integer
            // division is what the hardware does.
            return Ok(Type::MachineInt(*left_kind));
        }
        // A machine integer on one side only is a width mistake, not a promotion.
        if matches!(resolved_left, Type::MachineInt(_)) || matches!(resolved_right, Type::MachineInt(_)) {
            let (offending, expr) = if matches!(resolved_left, Type::MachineInt(_)) {
                (&resolved_right, right_expr)
            } else {
                (&resolved_left, left_expr)
            };
            return Err(Self::type_err(
                "machine integers do not mix with other numeric types; cast explicitly",
                Some(Type::MachineInt(lk_values::IntKind::U8)),
                Some(offending.clone()),
                Some(expr.clone()),
            ));
        }

        let left_class = self.classify_numeric_operand(left_ty, &resolved_left, left_expr, "左侧")?;
        let right_class = self.classify_numeric_operand(right_ty, &resolved_right, right_expr, "右侧")?;

        let mut result_class = NumericHierarchy::result(left_class, right_class);
        if matches!(op, BinOp::Div) && result_class == NumericClass::Int {
            result_class = NumericClass::Float;
        }

        Ok(NumericHierarchy::to_type(result_class))
    }

    fn classify_numeric_operand(
        &mut self,
        original: &Type,
        resolved: &Type,
        expr: &Expr,
        label: &'static str,
    ) -> Result<NumericClass> {
        if let Some(class) = NumericHierarchy::classify(resolved) {
            return Ok(class);
        }
        if original.contains_variables() {
            self.inference_engine.add_constraint(original.clone(), Type::Int);
            return Ok(NumericClass::Int);
        }
        Err(Self::type_err(
            &format!("{label} must by numeric types"),
            Some(NumericHierarchy::expected_type()),
            Some(resolved.clone()),
            Some(expr.clone()),
        ))
    }

    fn ensure_numeric_operand(&mut self, ty: &Type, expr: &Expr, label: &'static str) -> Result<NumericClass> {
        let resolved = self.resolve_aliases(ty);
        self.classify_numeric_operand(ty, &resolved, expr, label)
    }

    /// Check logical operation types (&&, ||)
    fn check_logical_op(&mut self, left: &Expr, right: &Expr, result_type: Type) -> Result<Type> {
        let left_type = self.check_expr(left)?;
        let right_type = self.check_expr(right)?;

        // Both operands must be boolean
        self.enforce_bool_type(&left_type, left)?;
        self.enforce_bool_type(&right_type, right)?;

        Ok(result_type)
    }

    /// Check unary operation types
    fn check_unary_op(&mut self, op: &UnaryOp, expr: &Expr) -> Result<Type> {
        let expr_type = self.check_expr(expr)?;

        match op {
            UnaryOp::Not => {
                if matches!(self.resolve_aliases(&expr_type), Type::Variable(_)) {
                    self.inference_engine.add_constraint(expr_type, Type::Any);
                }
                Ok(Type::Bool)
            }
        }
    }

    /// Check list literal type
    fn check_list(&mut self, items: &[Box<Expr>]) -> Result<Type> {
        if items.is_empty() {
            // Empty list, infer element type later
            let elem_type = self.inference_engine.fresh_type_var();
            return Ok(Type::List(Box::new(elem_type)));
        }

        let mut item_types: Vec<Type> = Vec::with_capacity(items.len());
        for item in items {
            item_types.push(self.check_expr(item)?);
        }
        if item_types.windows(2).any(|pair| pair[0] != pair[1]) {
            return Ok(Type::Tuple(item_types));
        }

        // Deduplicate and produce a stable order by display string
        use alloc::collections::BTreeMap;
        let mut by_key: BTreeMap<String, Type> = BTreeMap::new();
        for ty in item_types {
            if let Type::Union(types) = ty {
                for inner in types {
                    by_key.entry(inner.display()).or_insert(inner);
                }
                continue;
            }
            by_key.entry(ty.display()).or_insert(ty);
        }
        let mut uniq: Vec<Type> = by_key.into_values().collect();
        let elem_type = if uniq.len() == 1 {
            uniq.remove(0)
        } else {
            Type::Union(uniq)
        };

        Ok(Type::List(Box::new(elem_type)))
    }

    /// Check map literal type
    fn check_map(&mut self, pairs: &[(Box<Expr>, Box<Expr>)]) -> Result<Type> {
        if pairs.is_empty() {
            // Empty map, infer key/value types later
            let key_type = self.inference_engine.fresh_type_var();
            let value_type = self.inference_engine.fresh_type_var();
            return Ok(Type::Map(Box::new(key_type), Box::new(value_type)));
        }

        // Collect key/value types and build normalized unions when heterogeneous
        let mut key_tys: Vec<Type> = Vec::with_capacity(pairs.len());
        let mut val_tys: Vec<Type> = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let kt = self.check_expr(k)?;
            let vt = self.check_expr(v)?;
            match kt {
                Type::Union(ts) => key_tys.extend(ts),
                other => key_tys.push(other),
            }
            match vt {
                Type::Union(ts) => val_tys.extend(ts),
                other => val_tys.push(other),
            }
        }

        use alloc::collections::BTreeMap;
        let mut key_by_str: BTreeMap<String, Type> = BTreeMap::new();
        for t in key_tys {
            key_by_str.entry(t.display()).or_insert(t);
        }
        let mut val_by_str: BTreeMap<String, Type> = BTreeMap::new();
        for t in val_tys {
            val_by_str.entry(t.display()).or_insert(t);
        }

        let mut keys: Vec<Type> = key_by_str.into_values().collect();
        let mut vals: Vec<Type> = val_by_str.into_values().collect();

        let key_type = if keys.len() == 1 {
            keys.remove(0)
        } else {
            Type::Union(keys)
        };
        let value_type = if vals.len() == 1 {
            vals.remove(0)
        } else {
            Type::Union(vals)
        };

        Ok(Type::Map(Box::new(key_type), Box::new(value_type)))
    }

    fn struct_field_type(&self, struct_name: &str, field: &Expr) -> Result<Type> {
        let Some(def) = self.registry.get_struct(struct_name) else {
            return Err(Self::type_err(
                &format!("Unknown struct '{}'", struct_name),
                None,
                None,
                Some(field.clone()),
            ));
        };

        let field_name = match field {
            Expr::Literal(val) if val.as_str().is_some() => val.as_str().unwrap().to_string(),
            Expr::Literal(LiteralVal::Int(idx)) => idx.to_string(),
            _ => {
                return Err(Self::type_err(
                    "Struct field access requires a literal field name",
                    None,
                    None,
                    Some(field.clone()),
                ));
            }
        };

        if let Some(field_ty) = def.fields.get(&field_name) {
            return Ok(field_ty.clone());
        }

        if let Some(method_ty) = self.get_method_sig(&Type::Named(struct_name.to_string()), &field_name) {
            return Ok(method_ty);
        }

        Err(Self::type_err(
            &format!("Struct '{}' has no field '{}'", struct_name, field_name),
            None,
            None,
            Some(field.clone()),
        ))
    }

    /// Check access type (expr.field or expr[index])
    fn check_access(&mut self, expr: &Expr, field: &Expr) -> Result<Type> {
        if let Some(function_type) = self.stdlib_access_function_type(expr, field) {
            return Ok(function_type);
        }

        let expr_type = self.check_expr(expr)?;
        let field_type = self.check_expr(field)?;
        let resolved_expr_type = self.resolve_aliases(&expr_type);

        match &resolved_expr_type {
            Type::List(elem_type) => {
                // Slice: list[range] returns same list type
                if matches!(&field, Expr::Range { .. }) {
                    return Ok(Type::List(elem_type.clone()));
                }
                // Field must be integer index (Any/Box<Any> accepted for dynamic dispatch)
                if !self.is_assignable(&field_type, &Type::Int) {
                    return Err(Self::type_err(
                        "List index must be integer",
                        Some(Type::Int),
                        Some(field_type),
                        None,
                    ));
                }
                Ok((**elem_type).clone())
            }
            Type::Tuple(elems) => {
                // Field must be integer index; if it's a literal index, pick that element
                if !self.is_assignable(&field_type, &Type::Int) {
                    return Err(Self::type_err(
                        "Tuple index must be integer",
                        Some(Type::Int),
                        Some(field_type),
                        None,
                    ));
                }
                // Try literal extraction
                if let Expr::Literal(LiteralVal::Int(i)) = field {
                    let idx = *i as usize;
                    if idx < elems.len() {
                        return Ok(elems[idx].clone());
                    }
                }
                // Fallback: unknown index -> union of all element types
                let u = Type::Union(elems.to_vec());
                Ok(u)
            }
            Type::Map(key_type, value_type) => {
                // Field must match key type
                self.inference_engine.add_constraint((**key_type).clone(), field_type);
                Ok((**value_type).clone())
            }
            Type::String => {
                // Slice: str[range] returns String
                if matches!(&field, Expr::Range { .. }) {
                    return Ok(Type::String);
                }
                // Char access: str[idx] returns String
                if self.is_assignable(&field_type, &Type::Int) {
                    return Ok(Type::String);
                }
                Err(Self::type_err(
                    "String index must be integer or range",
                    Some(Type::Int),
                    Some(field_type),
                    None,
                ))
            }
            Type::Named(name) => self.struct_field_type(name, field),
            Type::Variable(_) => {
                if matches!(&field, Expr::Range { .. }) {
                    let elem_type = self.inference_engine.fresh_type_var();
                    self.inference_engine
                        .add_constraint(expr_type, Type::List(Box::new(elem_type.clone())));
                    return Ok(Type::List(Box::new(elem_type)));
                }
                if self.is_assignable(&field_type, &Type::Int) || field_type.contains_variables() {
                    let elem_type = self.inference_engine.fresh_type_var();
                    self.inference_engine
                        .add_constraint(expr_type, Type::List(Box::new(elem_type.clone())));
                    self.enforce_int_type(field, field_type, "List index")?;
                    return Ok(elem_type);
                }
                Ok(Type::Any)
            }
            Type::Any | Type::Nil => Ok(Type::Any),
            Type::Union(variants) => {
                let mut collected: Vec<Type> = Vec::new();
                for variant in variants {
                    match variant {
                        Type::Named(name) => collected.push(self.struct_field_type(name, field)?),
                        _ => return Ok(Type::Any),
                    }
                }
                match collected.len() {
                    0 => Ok(Type::Any),
                    1 => Ok(collected.remove(0)),
                    _ => Ok(Type::Union(collected)),
                }
            }
            _ => Err(Self::type_err(
                "Cannot access field on type",
                None,
                Some(expr_type),
                None,
            )),
        }
    }

    fn check_builtin_container_method(
        &mut self,
        receiver_ty: &Type,
        method: &str,
        args: &[Box<Expr>],
        func: &Expr,
    ) -> Result<Option<Type>> {
        match method {
            "len" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                let known_container = matches!(
                    &resolved_receiver,
                    Type::List(_) | Type::Map(_, _) | Type::Set(_) | Type::String | Type::Tuple(_) | Type::Variable(_)
                );
                if !known_container {
                    return Ok(None);
                }
                if !args.is_empty() {
                    if matches!(&resolved_receiver, Type::Variable(_)) {
                        return Ok(None);
                    }
                    return Err(Self::type_err(
                        "Method len expects 0 arguments",
                        None,
                        None,
                        Some(func.clone()),
                    ));
                }
                Ok(Some(Type::Int))
            }
            "skip" | "take" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                if !matches!(&resolved_receiver, Type::List(_) | Type::Variable(_)) {
                    return Ok(None);
                }
                if args.len() != 1 {
                    if matches!(&resolved_receiver, Type::Variable(_)) {
                        return Ok(None);
                    }
                    return Err(Self::type_err(
                        &format!("Method {method} expects 1 argument"),
                        None,
                        None,
                        Some(func.clone()),
                    ));
                }
                let count_ty = self.check_expr(&args[0])?;
                self.enforce_int_type(args[0].as_ref(), count_ty, "List slice count")?;

                match resolved_receiver {
                    Type::List(elem_type) => Ok(Some(Type::List(elem_type))),
                    Type::Variable(_) => {
                        let elem_type = self.inference_engine.fresh_type_var();
                        self.inference_engine
                            .add_constraint(receiver_ty.clone(), Type::List(Box::new(elem_type.clone())));
                        Ok(Some(Type::List(Box::new(elem_type))))
                    }
                    _ => Ok(None),
                }
            }
            "is_empty" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                let known_container = matches!(
                    &resolved_receiver,
                    Type::List(_) | Type::Map(_, _) | Type::Set(_) | Type::String | Type::Tuple(_) | Type::Variable(_)
                );
                if !known_container {
                    return Ok(None);
                }
                if !args.is_empty() {
                    if matches!(&resolved_receiver, Type::Variable(_)) {
                        return Ok(None);
                    }
                    return Err(Self::type_err(
                        "Method is_empty expects 0 arguments",
                        None,
                        None,
                        Some(func.clone()),
                    ));
                }
                Ok(Some(Type::Bool))
            }
            "get" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                match resolved_receiver {
                    Type::Map(key_type, value_type) => {
                        if args.is_empty() || args.len() > 2 {
                            return Err(Self::type_err(
                                "Method get expects 1 or 2 arguments",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*key_type).clone(), arg_ty);
                        if let Some(default) = args.get(1) {
                            let default_ty = self.check_expr(default)?;
                            self.inference_engine.add_constraint((*value_type).clone(), default_ty);
                        }
                        Ok(Some(*value_type))
                    }
                    Type::List(elem_type) => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                "Method get expects 1 argument",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let index_ty = self.check_expr(&args[0])?;
                        self.enforce_int_type(args[0].as_ref(), index_ty, "List index")?;
                        Ok(Some(*elem_type))
                    }
                    Type::Variable(_) => Ok(None),
                    _ => Ok(None),
                }
            }
            "set" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                match resolved_receiver {
                    Type::Map(key_type, value_type) => {
                        if args.len() != 2 {
                            return Err(Self::type_err(
                                "Method set expects 2 arguments",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let key_ty = self.check_expr(&args[0])?;
                        let value_ty = self.check_expr(&args[1])?;
                        self.inference_engine.add_constraint((*key_type).clone(), key_ty);
                        self.inference_engine.add_constraint((*value_type).clone(), value_ty);
                        Ok(Some(Type::Nil))
                    }
                    Type::List(elem_type) => {
                        if args.len() != 2 {
                            return Err(Self::type_err(
                                "Method set expects 2 arguments",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let index_ty = self.check_expr(&args[0])?;
                        let value_ty = self.check_expr(&args[1])?;
                        self.enforce_int_type(args[0].as_ref(), index_ty, "List index")?;
                        self.inference_engine.add_constraint((*elem_type).clone(), value_ty);
                        Ok(Some(Type::Nil))
                    }
                    Type::Variable(_) => Ok(None),
                    _ => Ok(None),
                }
            }
            "has" | "contains" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                match resolved_receiver {
                    Type::Map(key_type, _) => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                &format!("Method {method} expects 1 argument"),
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*key_type).clone(), arg_ty);
                        Ok(Some(Type::Bool))
                    }
                    // List membership is a value-equality linear scan at runtime; Set membership
                    // uses the same key equality semantics but is the preferred O(1) path.
                    Type::Set(elem_type) | Type::List(elem_type) => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                &format!("Method {method} expects 1 argument"),
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*elem_type).clone(), arg_ty);
                        Ok(Some(Type::Bool))
                    }
                    Type::Variable(_) => Ok(None),
                    _ => Ok(None),
                }
            }
            "delete" | "remove" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                match resolved_receiver {
                    Type::Map(key_type, value_type) => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                &format!("Method {method} expects 1 argument"),
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*key_type).clone(), arg_ty);
                        Ok(Some(*value_type))
                    }
                    Type::Set(elem_type) => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                &format!("Method {method} expects 1 argument"),
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*elem_type).clone(), arg_ty);
                        Ok(Some(Type::Bool))
                    }
                    Type::List(elem_type) if method == "remove" => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                "Method remove expects 1 argument",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*elem_type).clone(), arg_ty);
                        Ok(Some(Type::List(elem_type)))
                    }
                    Type::Variable(_) => Ok(None),
                    _ => Ok(None),
                }
            }
            "add" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                match resolved_receiver {
                    Type::Set(elem_type) => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                "Method add expects 1 argument",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*elem_type).clone(), arg_ty);
                        Ok(Some(Type::Bool))
                    }
                    Type::Variable(_) => Ok(None),
                    _ => Ok(None),
                }
            }
            "push" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                match resolved_receiver {
                    Type::List(elem_type) => {
                        if args.len() != 1 {
                            return Err(Self::type_err(
                                "Method push expects 1 argument",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        self.inference_engine.add_constraint((*elem_type).clone(), arg_ty);
                        Ok(Some(Type::Nil))
                    }
                    Type::Variable(_) => Ok(None),
                    _ => Ok(None),
                }
            }
            "keys" | "values" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                match resolved_receiver {
                    Type::Map(key_type, value_type) => {
                        if !args.is_empty() {
                            return Err(Self::type_err(
                                &format!("Method {method} expects 0 arguments"),
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        let elem = if method == "keys" { *key_type } else { *value_type };
                        Ok(Some(Type::List(Box::new(elem))))
                    }
                    Type::Set(elem_type) if method == "values" => {
                        if !args.is_empty() {
                            return Err(Self::type_err(
                                "Method values expects 0 arguments",
                                None,
                                None,
                                Some(func.clone()),
                            ));
                        }
                        Ok(Some(Type::List(elem_type)))
                    }
                    Type::Variable(_) => Ok(None),
                    _ => Ok(None),
                }
            }
            "clear" => {
                let resolved_receiver = self.resolve_aliases(receiver_ty);
                if matches!(&resolved_receiver, Type::Map(_, _) | Type::Set(_) | Type::List(_)) {
                    if !args.is_empty() {
                        return Err(Self::type_err(
                            "Method clear expects 0 arguments",
                            None,
                            None,
                            Some(func.clone()),
                        ));
                    }
                    return Ok(Some(Type::Nil));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Check nullish coalescing type (expr ?? default)
    fn check_nullish_coalescing(&mut self, expr: &Expr, default: &Expr) -> Result<Type> {
        let expr_type = self.check_expr(expr)?;
        let default_type = self.check_expr(default)?;

        // Expression can be optional, default should be the base type
        match expr_type {
            Type::Optional(inner) => {
                self.inference_engine.add_constraint((*inner).clone(), default_type);
                Ok((*inner).clone())
            }
            Type::Nil => Ok(default_type),
            _ => {
                self.inference_engine.add_constraint(expr_type.clone(), default_type);
                Ok(expr_type)
            }
        }
    }

    /// Check optional chaining type (expr?.field)
    fn check_optional_chaining(&mut self, expr: &Expr, field: &Expr) -> Result<Type> {
        let expr_type = self.check_expr(expr)?;

        match expr_type.clone() {
            Type::Optional(inner) => {
                let resolved_inner = self.resolve_aliases(inner.as_ref());
                match resolved_inner {
                    Type::List(elem_type) => {
                        let field_ty = self.check_expr(field)?;
                        if !self.is_assignable(&field_ty, &Type::Int) {
                            return Err(Self::type_err(
                                "List index must be integer",
                                Some(Type::Int),
                                Some(field_ty),
                                None,
                            ));
                        }
                        Ok(Type::Optional(elem_type))
                    }
                    Type::Map(key_type, value_type) => {
                        let field_ty = self.check_expr(field)?;
                        self.inference_engine.add_constraint((*key_type).clone(), field_ty);
                        Ok(Type::Optional(value_type))
                    }
                    Type::Tuple(elems) => {
                        let field_ty = self.check_expr(field)?;
                        if !self.is_assignable(&field_ty, &Type::Int) {
                            return Err(Self::type_err(
                                "Tuple index must be integer",
                                Some(Type::Int),
                                Some(field_ty),
                                None,
                            ));
                        }
                        if let Expr::Literal(LiteralVal::Int(i)) = field {
                            let idx = *i as usize;
                            if idx < elems.len() {
                                return Ok(Type::Optional(Box::new(elems[idx].clone())));
                            }
                        }
                        let u = Type::Union(elems);
                        Ok(Type::Optional(Box::new(u)))
                    }
                    Type::Named(name) => {
                        let field_ty = self.struct_field_type(&name, field)?;
                        Ok(Type::Optional(Box::new(field_ty)))
                    }
                    Type::Any | Type::Variable(_) => {
                        self.check_expr(field)?;
                        Ok(Type::Any)
                    }
                    other => Err(Self::type_err("Cannot access field on type", None, Some(other), None)),
                }
            }
            Type::Nil => Ok(Type::Nil),
            _ => self.check_access(expr, field),
        }
    }
}

/// Whether reinterpreting `source` as `target` has a defined meaning.
///
/// `Any` is on both sides because it is the dynamic escape hatch — a value of
/// unknown type has to be castable, or nothing dynamic could ever reach a
/// machine-typed boundary.
fn cast_is_meaningful(source: &Type, target: &Type) -> bool {
    fn is_scalar(ty: &Type) -> bool {
        matches!(
            ty,
            Type::Int | Type::MachineInt(_) | Type::Float | Type::Bool | Type::Any
        )
    }
    fn is_integral(ty: &Type) -> bool {
        matches!(ty, Type::Int | Type::MachineInt(_) | Type::Any)
    }

    match (source, target) {
        // Address ↔ pointer. This is how a hardware register gets named at all:
        // `0x3F20_0000 as *mut u32`. Only integers convert — a `Float` address
        // is meaningless, and building one from a `Bool` is a mistake.
        (integral, Type::Ptr { .. }) if is_integral(integral) => true,
        (Type::Ptr { .. }, integral) if is_integral(integral) => true,
        // Retyping a pointer: `*u8` to `*mut u32`. The pointee and mutability
        // are the programmer's claim to make, which is why this needs `unsafe`
        // at the point of *use* rather than here.
        (Type::Ptr { .. }, Type::Ptr { .. }) => true,
        _ => is_scalar(source) && is_scalar(target),
    }
}

/// Splits a `volatile_{read,write}_uN` name into its direction and width.
fn parse_volatile_builtin(name: &str) -> Option<(bool, lk_values::IntKind)> {
    let (is_write, rest) = if let Some(rest) = name.strip_prefix("volatile_read_") {
        (false, rest)
    } else if let Some(rest) = name.strip_prefix("volatile_write_") {
        (true, rest)
    } else {
        return None;
    };
    // Only unsigned widths: a hardware register is a bit pattern, and a signed
    // reading of one is the caller's interpretation, made with a cast.
    let kind = match rest {
        "u8" => lk_values::IntKind::U8,
        "u16" => lk_values::IntKind::U16,
        "u32" => lk_values::IntKind::U32,
        "u64" => lk_values::IntKind::U64,
        _ => return None,
    };
    Some((is_write, kind))
}
