use crate::compat::collections::HashSet;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use anyhow::{Result, bail};

use crate::{
    expr::Expr,
    operator::BinOp,
    val::{LiteralVal, ShortStr},
    vm::analysis::PerfValueKind,
};

use super::{
    Compiler, ConstHeapValue, Instr, Opcode, call::map_get_method_call_args, checked_u8, support::ast_literal_kind,
};

impl Compiler {
    pub(super) fn lower_readonly_operand(&mut self, expr: &Expr) -> Result<u16> {
        match expr {
            Expr::Paren(inner) => self.lower_readonly_operand(inner),
            Expr::Var(name) => {
                if let Some(local) = self.locals.get(name).copied()
                    && !self.cell_locals.contains(name)
                {
                    return Ok(local);
                }
                self.lower_expr(expr)
            }
            _ => self.lower_expr(expr),
        }
    }

    pub(super) fn lower_loop_snapshot_operand(&mut self, expr: &Expr, mutated_names: &HashSet<String>) -> Result<u16> {
        if super::support::simple_local_expr_name(expr).is_some_and(|name| !mutated_names.contains(name)) {
            self.lower_readonly_operand(expr)
        } else {
            self.lower_expr(expr)
        }
    }

    pub(super) fn try_lower_expr_to_register(&mut self, dst: u16, expr: &Expr) -> Result<bool> {
        match expr {
            Expr::Paren(inner) => self.try_lower_expr_to_register(dst, inner),
            Expr::Var(name) => {
                // For simple local variable references (not cell locals),
                // emit a direct Move from the source register to dst,
                // avoiding an intermediate register allocation.
                if let Some(src) = self.locals.get(name).copied()
                    && !self.cell_locals.contains(name)
                {
                    let move_source = !self.is_current_local_slot(src);
                    self.emit_move_with_policy(dst, src, "assign var", move_source)?;
                    return Ok(true);
                }
                Ok(false)
            }
            Expr::Literal(value) => {
                self.emit_literal_to_register(dst, value)?;
                Ok(true)
            }
            Expr::Bin(lhs, op, rhs) => {
                // The same concat rendering `lower_bin` does — a string sum
                // reaches this path when it is lowered into a register.
                if let Some((lhs, rhs)) = self.rendered_concat_operands(lhs, op, rhs) {
                    let rewritten = Expr::Bin(Box::new(lhs), op.clone(), Box::new(rhs));
                    return self.try_lower_expr_to_register(dst, &rewritten);
                }
                // `u64` compares and divides unsigned — the same rewrite
                // `lower_bin` does, because a comparison that feeds a value (as
                // in `println(a < b)`) arrives here instead. Two lowering paths
                // for one shape is why the first version of this fixed division
                // and left the comparison signed.
                if self.lower_unsigned_bin_into(dst, lhs, op, rhs)?.is_some() {
                    return Ok(true);
                }
                let static_flavor = super::support::numeric_flavor(lhs, op, rhs);
                // Whether each side is written as an integer literal, before the
                // names are shadowed by the registers they lower into. A literal
                // beside a machine integer takes that width — see below.
                let lhs_is_literal = super::support::is_int_literal(lhs);
                let rhs_is_literal = super::support::is_int_literal(rhs);
                // The commuted attempt keeps the register it lowered — see the
                // same shape in `lower_bin_op`: falling through to lower `rhs`
                // again ran the expression twice, so `1 + f(x)` called `f` twice.
                // This copy had the identical defect.
                let mut commuted_rhs = None;
                if static_flavor == super::support::NumericFlavor::Int
                    && let Some(immediate) = super::support::commuted_int_immediate_operand(op, lhs)
                {
                    let rhs = self.lower_readonly_operand(rhs)?;
                    // Not for a machine integer: the immediate form skips the
                    // width normalisation below, so `1 + reg` would answer at 64
                    // bits while the type says otherwise.
                    if self.function.performance.value_kind(rhs) == PerfValueKind::Int
                        && !self.machine_regs.contains_key(&rhs)
                    {
                        self.emit_int_immediate_to_register(dst, op, rhs, immediate)?;
                        return Ok(true);
                    }
                    commuted_rhs = Some(rhs);
                }
                let lhs = self.lower_readonly_operand(lhs)?;
                if commuted_rhs.is_none()
                    && let Some(immediate) = super::support::int_immediate_operand(op, rhs)
                    && self.function.performance.value_kind(lhs) == PerfValueKind::Int
                    && static_flavor == super::support::NumericFlavor::Int
                    && !self.machine_regs.contains_key(&lhs)
                {
                    self.emit_int_immediate_to_register(dst, op, lhs, immediate)?;
                    return Ok(true);
                }
                let rhs = match commuted_rhs {
                    Some(reg) => reg,
                    None => self.lower_readonly_operand(rhs)?,
                };
                // A literal beside a machine integer takes its width.
                //
                // `reg + 1` is what driver code is made of, and the type checker
                // now accepts it. What makes that *correct* is here: the literal
                // is normalised to the same width first, so the wrap that
                // follows the operation has two proven operands to agree about.
                // Without it the checker would say `u8` while the arithmetic ran
                // at 64 bits — `255u8 + 1` answering 256, which is the shape
                // this whole path exists to prevent.
                self.adopt_machine_width_for_literal(lhs, rhs, lhs_is_literal, rhs_is_literal)?;
                let flavor = super::facts::numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs)
                    .unwrap_or(static_flavor);
                self.emit_bin_op_to_register_with_flavor(dst, op, lhs, rhs, flavor)?;
                Ok(true)
            }
            // `math.floor(x)` where `x` is already an integer is the identity,
            // so the call can go. That is only true when it *is* one: `/`
            // yields a `Float`, so `math.floor(subtotal / 10)` must keep its
            // call. Eliding it there answered `11.4` for `math.floor(114 / 10)`
            // — a wrong number from an optimisation, which the bench corpus
            // caught as a checksum mismatch against Lua.
            Expr::CallExpr(callee, args) if self.is_external_module_call(callee, args, "math", "floor", 1) => {
                // The midpoint fusion computes `(lo + hi) / 2` floored in one
                // opcode, so it is exact and stays.
                if self.try_lower_int_midpoint_to_register(dst, &args[0])? {
                    return Ok(true);
                }
                if math_floor_arg_is_int_like(&args[0], &self.locals, &self.function.performance) {
                    return self.try_lower_expr_to_register(dst, &args[0]);
                }
                // `math.floor(a / b)` over two integers *is* integer division,
                // and since `/` yields a `Float` it is the only way to write
                // one. Fused so the idiom costs one instruction instead of a
                // float divide plus a native call.
                if self.try_lower_int_floor_div_to_register(dst, &args[0])? {
                    return Ok(true);
                }
                Ok(false)
            }
            Expr::CallExpr(callee, args) => {
                if self.is_external_module_call(callee, args, "map", "get", 2) {
                    self.lower_map_get_function_call_to_register(dst, args)?;
                    return Ok(true);
                }
                if let Some((target, key)) = map_get_method_call_args(callee, args)
                    && !self.is_external_module_target(target, "map")
                {
                    self.lower_map_get_method_call_to_register(dst, target, key)?;
                    return Ok(true);
                }
                Ok(false)
            }
            Expr::Access(target, key) => {
                self.lower_access_to_register(dst, target, key)?;
                Ok(true)
            }
            Expr::TemplateString(parts) => {
                self.lower_template_string_to_register(dst, parts)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn try_lower_int_midpoint_to_register(&mut self, dst: u16, expr: &Expr) -> Result<bool> {
        let Some((lhs_expr, rhs_expr)) = int_midpoint_terms(expr, &self.locals, &self.function.performance) else {
            return Ok(false);
        };
        let lhs = self.lower_readonly_operand(lhs_expr)?;
        let rhs = self.lower_readonly_operand(rhs_expr)?;
        if self.function.performance.value_kind(lhs) != PerfValueKind::Int
            || self.function.performance.value_kind(rhs) != PerfValueKind::Int
        {
            return Ok(false);
        }
        self.emit(Instr::abc(
            Opcode::MidInt,
            checked_u8("midpoint dst", dst)?,
            checked_u8("midpoint lhs", lhs)?,
            checked_u8("midpoint rhs", rhs)?,
        ));
        self.set_register_kind(dst, PerfValueKind::Int);
        Ok(true)
    }

    /// `math.floor(a / b)` over two proven `Int`s → one `FloorDivInt`.
    pub(super) fn try_lower_int_floor_div_to_register(&mut self, dst: u16, expr: &Expr) -> Result<bool> {
        let Expr::Bin(numerator, BinOp::Div, divisor) = strip_parens(expr) else {
            return Ok(false);
        };
        // No proven-`Int` requirement. The opcode answers for any numeric pair
        // — two `Int`s take the integer path, anything else divides as `f64`
        // and floors — which is exactly what the `math.floor` call it replaces
        // did. Demanding a proof only meant the fusion missed the calls that
        // needed it most: `math.floor(subtotal / 10)` where `subtotal` came
        // out of a map, which no analysis here can type.
        let lhs = self.lower_readonly_operand(numerator)?;
        let rhs = self.lower_readonly_operand(divisor)?;
        self.emit(Instr::abc(
            Opcode::FloorDivInt,
            checked_u8("floor div dst", dst)?,
            checked_u8("floor div lhs", lhs)?,
            checked_u8("floor div rhs", rhs)?,
        ));
        self.set_register_kind(dst, PerfValueKind::Int);
        Ok(true)
    }

    pub(super) fn lower_expr_to_register(&mut self, dst: u16, expr: &Expr, context: &str) -> Result<()> {
        if self.try_lower_expr_to_register(dst, expr)? {
            return Ok(());
        }
        let src = self.lower_readonly_operand(expr)?;
        let move_source = !self.is_current_local_slot(src);
        self.emit_move_with_policy(dst, src, context, move_source)
    }

    pub(super) fn emit_literal_to_register(&mut self, dst: u16, value: &LiteralVal) -> Result<()> {
        if let Some(src) = self.cached_loop_literal(value) {
            self.emit_move_with_policy(dst, src, "loop cached literal", false)?;
            return Ok(());
        }
        match value {
            LiteralVal::Nil => {
                self.emit(Instr::abc(Opcode::LoadNil, checked_u8("dst", dst)?, 0, 0));
                self.set_register_kind(dst, PerfValueKind::Nil);
            }
            LiteralVal::Bool(value) => {
                self.emit(Instr::abc(
                    Opcode::LoadBool,
                    checked_u8("dst", dst)?,
                    u8::from(*value),
                    0,
                ));
                self.set_register_kind(dst, PerfValueKind::Bool);
            }
            LiteralVal::Int(value) => {
                let k = self.push_int(*value)?;
                self.emit(Instr::abx(Opcode::LoadInt, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Int);
            }
            LiteralVal::Float(value) => {
                let k = self.push_float(*value)?;
                self.emit(Instr::abx(Opcode::LoadFloat, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Float);
            }
            value if value.as_str().is_some() => {
                let value = value.as_str().expect("checked string");
                if ShortStr::new(value).is_some() {
                    let k = self.push_string(value)?;
                    self.emit(Instr::abx(Opcode::LoadString, checked_u8("dst", dst)?, k));
                } else {
                    let k = self.push_heap_value(ConstHeapValue::LongString(value.into()))?;
                    self.emit(Instr::abx(Opcode::LoadHeapConst, checked_u8("dst", dst)?, k));
                }
                self.set_register_kind(dst, PerfValueKind::String);
            }
            other => bail!(
                "Compiler cannot materialize AST literal value yet: {}",
                ast_literal_kind(other)
            ),
        }
        Ok(())
    }
}

impl Compiler {
    fn is_external_module_call(
        &self,
        callee: &Expr,
        args: &[Box<Expr>],
        module: &str,
        method: &str,
        arity: usize,
    ) -> bool {
        if args.len() != arity {
            return false;
        }
        let Expr::Access(target, field) = callee else {
            return false;
        };
        matches!(target.as_ref(), Expr::Var(name)
            if name == module
                && self.global_names.contains_key(name)
                && !self.locals.contains_key(name)
                && !self.function_names.contains_key(name)
                && !self.native_names.contains_key(name)
                && !self.user_let_globals.contains(name))
            && matches!(field.as_ref(), Expr::Literal(value) if value.as_str() == Some(method))
    }

    fn is_external_module_target(&self, target: &Expr, module: &str) -> bool {
        matches!(target, Expr::Var(name)
            if name == module
                && self.global_names.contains_key(name)
                && !self.locals.contains_key(name)
                && !self.function_names.contains_key(name)
                && !self.native_names.contains_key(name)
                && !self.user_let_globals.contains(name))
    }
}

fn math_floor_arg_is_int_like(
    expr: &Expr,
    locals: &crate::compat::collections::HashMap<String, u16>,
    facts: &crate::vm::analysis::PerformanceFacts,
) -> bool {
    match expr {
        Expr::Paren(inner) => math_floor_arg_is_int_like(inner, locals, facts),
        Expr::Literal(LiteralVal::Int(_)) => true,
        Expr::Var(name) => locals
            .get(name)
            .copied()
            .is_some_and(|reg| facts.value_kind(reg) == PerfValueKind::Int),
        Expr::Bin(lhs, op, rhs)
            if matches!(
                op,
                // No `Div`: a quotient is a `Float`, so an expression
                // containing one is not "already an integer".
                crate::operator::BinOp::Add
                    | crate::operator::BinOp::Sub
                    | crate::operator::BinOp::Mul
                    | crate::operator::BinOp::Mod
            ) && super::support::numeric_flavor(lhs, op, rhs) == super::support::NumericFlavor::Int =>
        {
            math_floor_arg_is_int_like(lhs, locals, facts) && math_floor_arg_is_int_like(rhs, locals, facts)
        }
        _ => false,
    }
}

fn int_midpoint_terms<'a>(
    expr: &'a Expr,
    locals: &crate::compat::collections::HashMap<String, u16>,
    facts: &crate::vm::analysis::PerformanceFacts,
) -> Option<(&'a Expr, &'a Expr)> {
    let Expr::Bin(numerator, BinOp::Div, divisor) = strip_parens(expr) else {
        return None;
    };
    if !matches!(strip_parens(divisor), Expr::Literal(LiteralVal::Int(2))) {
        return None;
    }
    let Expr::Bin(lhs, BinOp::Add, rhs) = strip_parens(numerator) else {
        return None;
    };
    (math_floor_arg_is_int_like(lhs, locals, facts) && math_floor_arg_is_int_like(rhs, locals, facts))
        .then_some((strip_parens(lhs), strip_parens(rhs)))
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip_parens(inner),
        other => other,
    }
}
