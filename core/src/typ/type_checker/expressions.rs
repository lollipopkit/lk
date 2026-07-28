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
    /// The block's *type* is its last statement's, when that statement is an
    /// expression — which is not a new rule but the type catching up with one.
    /// The executor already evaluates an `unsafe` block to exactly that value,
    /// trailing semicolon included: `unsafe { 7; }` is 7.
    ///
    /// Typing it `Any` instead had a cost that shows up wherever this construct
    /// is actually used. Every device read in a driver is one:
    ///
    /// ```lk
    /// let value = unsafe { volatile_read_u32(address as *mut u32) };
    /// return value as Int;
    /// ```
    ///
    /// The binding and the cast are both laundering — there to turn `Any` back
    /// into the `Int` the read always produced. A cast written to satisfy the
    /// checker rather than to state something is a cast that will one day be
    /// wrong and say nothing, which is the opposite of what `unsafe` is for.
    ///
    /// A last statement that is *not* an expression leaves the block `Any`, as
    /// before. Those shapes (`unsafe { let x = …; }`) have no value the
    /// executor promises, and inventing one here would be a claim rather than a
    /// description.
    fn check_unsafe_body(&mut self, inner: &Expr) -> Result<Type> {
        let Expr::Block(statements) = inner else {
            return self.check_expr(inner);
        };
        let Some((last, leading)) = statements.split_last() else {
            return Ok(Type::Any);
        };
        for stmt in leading {
            stmt.type_check(self)?;
        }
        if let crate::stmt::Stmt::Expr(expr) = last.as_ref() {
            return self.check_expr(expr);
        }
        last.type_check(self)?;
        Ok(Type::Any)
    }

    /// The `cpu_*` intrinsics: barriers, interrupt masking, wait-for-interrupt,
    /// and the system-control instructions (descriptor tables, CR2/CR3, the
    /// TLB).
    ///
    /// These need `unsafe` for a different reason than pointers do — a barrier
    /// cannot corrupt memory. Masking interrupts or parking the core changes
    /// the machine's state in a way the rest of the program's correctness may
    /// depend on, and getting the nesting wrong deadlocks rather than crashes.
    /// Marking it makes the region auditable.
    ///
    /// The system-control half earns the same keyword far more directly: a
    /// malformed descriptor table is not a fault the kernel gets to report,
    /// because the CPU faults trying to report it and the machine resets.
    fn check_cpu_builtin(&mut self, name: &str, args: &[Box<Expr>]) -> Result<Option<Type>> {
        let (arity, result) = match name {
            "cpu_barrier" | "cpu_compiler_barrier" | "cpu_wait_for_interrupt" => (0, Type::Nil),
            "cpu_irq_save" | "cpu_timestamp" | "cpu_read_cr2" | "cpu_read_cr3" => (0, Type::Int),
            "cpu_irq_restore"
            | "cpu_load_task_register"
            | "cpu_write_cr3"
            | "cpu_invalidate_page"
            | "cpu_raise_interrupt" => (1, Type::Nil),
            "cpu_load_idt" | "cpu_load_gdt" | "cpu_reload_segments" => (2, Type::Nil),
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
        // Every operand of every one of these is a machine word — a port, a
        // selector, a physical address, a saved flag. Checked in one loop
        // rather than per intrinsic: the arm that gets forgotten is the one
        // whose argument is never visited by the checker at all, and the
        // lowering then rejects it as a type mismatch with no source location.
        for (index, arg) in args.iter().enumerate() {
            let actual = self.check_expr(arg)?;
            if !self.is_assignable(&actual, &Type::Int) {
                // `cpu_irq_restore` says where the value should have come
                // from; the nesting discipline is the thing being got wrong
                // when this fires, and naming the type is no help.
                if name == "cpu_irq_restore" {
                    return Err(anyhow!(
                        "cpu_irq_restore expects the value returned by cpu_irq_save, got {}",
                        actual.display()
                    ));
                }
                return Err(anyhow!(
                    "{name} expects Int for argument {}, got {}",
                    index + 1,
                    actual.display()
                ));
            }
        }
        Ok(Some(result))
    }

    /// `port_in_uN(port)` / `port_out_uN(port, value)` — x86 port I/O.
    ///
    /// A separate address space from memory, reached by the `in`/`out`
    /// instructions rather than by a load or a store, which is why it cannot
    /// reuse the volatile intrinsics: there is no pointer to take. The port
    /// number is 16-bit; the value's width is in the name for the same reason
    /// it is for `volatile_*`.
    ///
    /// x86 only. Other architectures memory-map their devices and have no such
    /// instructions, so a program that uses these is inherently x86 code — the
    /// runtime raises there rather than pretending.
    fn check_port_builtin(&mut self, name: &str, args: &[Box<Expr>]) -> Result<Option<Type>> {
        let Some((is_write, kind)) = parse_port_builtin(name) else {
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
                "{name} requires an `unsafe` block: the compiler cannot check what device answers \
                 at that port, or what writing to it does"
            ));
        }
        let port_ty = self.check_expr(&args[0])?;
        if !self.is_assignable(&port_ty, &Type::Int)
            && !self.is_assignable(&port_ty, &Type::MachineInt(lk_values::IntKind::U16))
        {
            return Err(anyhow!(
                "{name} expects a port number as its first argument, got {}",
                port_ty.display()
            ));
        }
        let value_ty = Type::MachineInt(kind);
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

    /// `volatile_read_uN(ptr)` / `volatile_write_uN(ptr, value)`.
    ///
    /// Returns `None` for any other name, so ordinary calls fall through.
    ///
    /// These are intrinsics rather than syntax on purpose. `*p` would need the
    /// *compiler* to know the pointee's width to emit the right load, and the
    /// compiler has no access to the type checker — the width lives in the name
    /// instead. It also makes the volatile-ness explicit, which `*p` never is
    /// in any language.
    /// `symbol_address("name")` and `call_address_2(addr, a, b)` — the two
    /// halves of a driver table.
    ///
    /// They had no entry here at all, which meant a call to either produced
    /// `Any` and neither its arity nor its arguments were checked. `Any`
    /// spreads: subtracting two addresses to measure a stride gave something
    /// with no `as Int` out of it, and the error named the cast rather than the
    /// missing type. Both of those cost real time in this repository.
    ///
    /// The name has to be a *literal*, and saying so here is the point. A
    /// relocation is a name resolved at link time; there is nothing to look one
    /// up in at run time, so a variable name cannot work — and without this it
    /// type-checked, ran under the VM (which refuses), and failed to lower
    /// natively with a message about an unsupported opcode.
    fn check_address_builtin(&mut self, name: &str, args: &[Box<Expr>]) -> Result<Option<Type>> {
        let arity = match name {
            "symbol_address" => 1,
            "call_address_2" => 3,
            _ => return Ok(None),
        };
        if args.len() != arity {
            return Err(anyhow!("{name} expects {arity} argument(s), got {}", args.len()));
        }
        if !self.in_unsafe() {
            return Err(anyhow!(
                "{name} requires an `unsafe` block: a code address is a number, and nothing here \
                 can check that the one you have is code"
            ));
        }
        if name == "symbol_address" {
            if !matches!(
                args[0].as_ref(),
                Expr::Literal(crate::val::LiteralVal::String(_) | crate::val::LiteralVal::ShortStr(_))
            ) {
                return Err(anyhow!(
                    "symbol_address needs a literal name: it becomes a relocation, which is a name \
                     resolved when the image is linked, and there is nothing to look one up in at \
                     run time"
                ));
            }
        } else {
            for (index, arg) in args.iter().enumerate() {
                let actual = self.check_expr(arg)?;
                if !self.is_assignable(&actual, &Type::Int) {
                    return Err(anyhow!(
                        "call_address_2 expects Int for argument {}, got {}",
                        index + 1,
                        actual.display()
                    ));
                }
            }
        }
        Ok(Some(Type::Int))
    }

    fn check_volatile_builtin(&mut self, name: &str, args: &[Box<Expr>]) -> Result<Option<Type>> {
        if let Some(result) = self.check_cpu_builtin(name, args)? {
            return Ok(Some(result));
        }
        if let Some(result) = self.check_port_builtin(name, args)? {
            return Ok(Some(result));
        }
        if let Some(result) = self.check_address_builtin(name, args)? {
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
                // A shift keeps the width of what is being shifted.
                //
                // The parser desugars `a << b` into `__lk_shl(a, b)` before
                // anything knows a type, so without this the result of shifting
                // a `u32` is an ordinary `Int` and the next thing done with it
                // is a width mistake.
                //
                // This is the *last* piece of the unsigned-`u64` work rather than
                // the first, and the order mattered: on its own it makes
                // `let top = one << 63; top < one;` type-check, and until the
                // compiler rewrote that comparison to its unsigned form the
                // answer was `true`. A rule that turns a compile error into a
                // wrong answer is worse than the error.
                if let Some(result) = self.check_shift_builtin(func, args)? {
                    return Ok(result);
                }
                // For Call with string name, create a variable expression for the function
                let func_expr = Expr::Var(func.clone());
                self.check_function_call(&func_expr, args)
            }
            Expr::CallExpr(func_expr, args) => {
                // Source-level calls parse to `CallExpr`; `Call` is only built
                // by internal desugars.
                if let Expr::Var(name) = func_expr.as_ref() {
                    if let Some(result) = self.check_volatile_builtin(name, args)? {
                        return Ok(result);
                    }
                    // Both shapes, because name resolution rewrites a plain
                    // call: `__lk_shl(a, b)` is a `Call` in the parser's output
                    // and a `CallExpr(Var(…))` by the time this sees it.
                    // Matching only the first is why the first version of this
                    // looked correct and changed nothing — the same trap the
                    // compiler's width inference fell into, in the same words.
                    if let Some(result) = self.check_shift_builtin(name, args)? {
                        return Ok(result);
                    }
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
                // Like a named function's body: a closure runs when it is
                // called, which is after the top level has finished, so it may
                // read a binding declared below it.
                let pending = self.suspend_pending_top_level();
                let ret_type = self.check_expr(body);
                self.restore_pending_top_level(pending);
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

        // A top-level binding declared further down the file. The top level
        // runs in order, so this read gets nil — and the error that eventually
        // surfaces is about nil, not about order.
        if self.is_pending_top_level(name) {
            return Err(Self::type_err(
                &format!("`{name}` is used before it is defined; move its definition above this statement"),
                None,
                None,
                Some(Expr::Var(name.to_string())),
            ));
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
                self.check_ordering_operands(left_expr, &left_type, right_expr, &right_type)?;
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

    /// An integer literal's value, seeing through parentheses.
    fn int_literal_operand(expr: &Expr) -> Option<i128> {
        match expr {
            Expr::Literal(crate::val::LiteralVal::Int(value)) => Some(i128::from(*value)),
            Expr::Paren(inner) => Self::int_literal_operand(inner),
            _ => None,
        }
    }

    /// The result of a machine-int operation whose other operand was a literal.
    ///
    /// The range is checked here rather than left to wrap, because having one is
    /// the whole point of asking for a fixed width — `port + 300` on a `u8` is a
    /// mistake worth being told about where it was written.
    fn machine_literal_result(kind: lk_values::IntKind, literal: i128, expr: &Expr) -> Result<Type> {
        if !kind.accepts_literal(literal) {
            return Err(Self::type_err(
                "literal is out of range for the machine integer it is used with",
                Some(Type::MachineInt(kind)),
                None,
                Some(expr.clone()),
            ));
        }
        Ok(Type::MachineInt(kind))
    }

    /// `a << b` / `a >> b`, whose result is `a`'s type when that is a machine
    /// integer.
    ///
    /// The shift *amount* is deliberately not constrained to the same width —
    /// `flags << 3` is what people write, and requiring `3 as u32` there is the
    /// ceremony that gets fixed widths abandoned.
    fn check_shift_builtin(&mut self, func: &str, args: &[Box<Expr>]) -> Result<Option<Type>> {
        if !matches!(func, "__lk_shl" | "__lk_shr" | "__lk_shr_u") || args.len() != 2 {
            return Ok(None);
        }
        let left = self.check_expr(&args[0])?;
        let _ = self.check_expr(&args[1])?;
        let resolved = self.resolve_aliases(&left);
        Ok(match resolved {
            Type::MachineInt(_) => Some(resolved),
            _ => None,
        })
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
        // An integer *literal* takes the machine width of the other side.
        //
        // `let x: u8 = 5` already works — a literal is retyped rather than
        // rejected, because requiring `5 as u8` there would make a fixed width
        // unusable. `reg + 1` is the same need with more force: `reg + (1 as u32)`
        // at every increment is what gets fixed widths abandoned in favour of
        // `Int`, which is the opposite of what asking for a width was for.
        //
        // This half alone is a *miscompile*, and it was one for a round: the
        // checker says `u8` while the compiler goes on materialising the literal
        // as an ordinary `Int` and doing 64-bit arithmetic, so `255u8 + 1`
        // answers 256 with the type still claiming `u8`. The other half is
        // `adopt_machine_width_for_literal` in the compiler, which normalises the
        // literal to that width before the operation, so the wrap that follows
        // has two proven operands to agree about.
        //
        // Only a literal. A *variable* of another numeric type is still a width
        // mistake — that is the rule this preserves — and a literal out of range
        // says so, measured against the width it was used with.
        if let Type::MachineInt(kind) = &resolved_left
            && !matches!(resolved_right, Type::MachineInt(_))
            && let Some(literal) = Self::int_literal_operand(right_expr)
        {
            return Self::machine_literal_result(*kind, literal, right_expr);
        }
        if let Type::MachineInt(kind) = &resolved_right
            && !matches!(resolved_left, Type::MachineInt(_))
            && let Some(literal) = Self::int_literal_operand(left_expr)
        {
            return Self::machine_literal_result(*kind, literal, left_expr);
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

    /// `<`, `<=`, `>`, `>=` — including machine integers, which the numeric
    /// hierarchy deliberately does not classify.
    ///
    /// Ordering is where the hierarchy's own rule (promote to the wider class)
    /// is wrong for a fixed-width type: there is nothing to promote to, and
    /// promoting to `Int` would give the comparison 64-bit semantics. So the
    /// machine-int cases are answered here — same width compares, mixed widths
    /// are the same error mixed-width arithmetic gives — and everything else
    /// goes to the hierarchy unchanged. Without this, `u32 < u32` was rejected
    /// as "not numeric", which is the one thing it obviously is.
    fn check_ordering_operands(
        &mut self,
        left_expr: &Expr,
        left_ty: &Type,
        right_expr: &Expr,
        right_ty: &Type,
    ) -> Result<()> {
        let resolved_left = self.resolve_aliases(left_ty);
        let resolved_right = self.resolve_aliases(right_ty);
        match (&resolved_left, &resolved_right) {
            (Type::MachineInt(left_kind), Type::MachineInt(right_kind)) => {
                if left_kind != right_kind {
                    return Err(Self::type_err(
                        "machine integer operands must have the same type",
                        Some(Type::MachineInt(*left_kind)),
                        Some(Type::MachineInt(*right_kind)),
                        Some(right_expr.clone()),
                    ));
                }
                Ok(())
            }
            // A literal takes the width of what it is compared against, the same
            // as in arithmetic: `reg > 0` and `count < 8` are what driver code
            // is made of, and `reg > (0 as u32)` is the ceremony that gets fixed
            // widths abandoned. The range is still checked against that width.
            (Type::MachineInt(kind), _) if Self::int_literal_operand(right_expr).is_some() => {
                let literal = Self::int_literal_operand(right_expr).expect("checked");
                Self::machine_literal_result(*kind, literal, right_expr).map(|_| ())
            }
            (_, Type::MachineInt(kind)) if Self::int_literal_operand(left_expr).is_some() => {
                let literal = Self::int_literal_operand(left_expr).expect("checked");
                Self::machine_literal_result(*kind, literal, left_expr).map(|_| ())
            }
            (Type::MachineInt(kind), other) => Err(Self::type_err(
                "machine integers do not mix with other numeric types; cast explicitly",
                Some(Type::MachineInt(*kind)),
                Some(other.clone()),
                Some(right_expr.clone()),
            )),
            (other, Type::MachineInt(kind)) => Err(Self::type_err(
                "machine integers do not mix with other numeric types; cast explicitly",
                Some(Type::MachineInt(*kind)),
                Some(other.clone()),
                Some(left_expr.clone()),
            )),
            _ => {
                self.ensure_numeric_operand(left_ty, left_expr, "左侧")?;
                self.ensure_numeric_operand(right_ty, right_expr, "右侧")?;
                Ok(())
            }
        }
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
        if let Expr::Var(namespace) = expr
            && let Some(member) = stdlib::segment_name(field)
            && let Some(member_type) = self.imported_member_type(namespace, member)
        {
            return Ok(member_type);
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

/// Splits a `port_{in,out}_uN` name into its direction and width.
///
/// Only 8/16/32 bits: `in`/`out` have no 64-bit form on x86.
fn parse_port_builtin(name: &str) -> Option<(bool, lk_values::IntKind)> {
    let (is_write, rest) = match name.strip_prefix("port_in_") {
        Some(rest) => (false, rest),
        None => (true, name.strip_prefix("port_out_")?),
    };
    let kind = match rest {
        "u8" => lk_values::IntKind::U8,
        "u16" => lk_values::IntKind::U16,
        "u32" => lk_values::IntKind::U32,
        _ => return None,
    };
    Some((is_write, kind))
}

/// Splits a `volatile_{read,write}_uN` name into its direction and width.
/// The machine width a builtin's *name* declares, for the ones whose result is
/// a machine int.
///
/// Exposed for the bytecode compiler, which needs the same answer to know when
/// arithmetic on the result has to wrap. Derived from the name by the same
/// parsers the checks above use, rather than a second table — a second table is
/// one entry away from a value that wraps in the type system and not in the
/// program.
pub fn builtin_machine_result(name: &str) -> Option<crate::val::IntKind> {
    if let Some((is_write, kind)) = parse_volatile_builtin(name) {
        return (!is_write).then_some(kind);
    }
    if let Some((is_write, kind)) = parse_port_builtin(name) {
        return (!is_write).then_some(kind);
    }
    None
}

fn parse_volatile_builtin(name: &str) -> Option<(bool, lk_values::IntKind)> {
    let (is_write, rest) = match name.strip_prefix("volatile_read_") {
        Some(rest) => (false, rest),
        None => (true, name.strip_prefix("volatile_write_")?),
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
