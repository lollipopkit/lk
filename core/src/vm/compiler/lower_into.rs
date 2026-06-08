use std::collections::HashSet;

use anyhow::{Result, bail};

use crate::{
    expr::Expr,
    operator::BinOp,
    val::{LiteralVal, ShortStr},
    vm::analysis::PerfValueKind,
};

use super::{Compiler, ConstHeapValue, Instr, Opcode, checked_u8, support::ast_literal_kind};

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
                let static_flavor = super::support::numeric_flavor(lhs, op, rhs);
                if static_flavor == super::support::NumericFlavor::Int
                    && let Some(immediate) = super::support::commuted_int_immediate_operand(op, lhs)
                {
                    let rhs = self.lower_readonly_operand(rhs)?;
                    if self.function.performance.value_kind(rhs) == PerfValueKind::Int {
                        self.emit_int_immediate_to_register(dst, op, rhs, immediate)?;
                        return Ok(true);
                    }
                }
                let lhs = self.lower_readonly_operand(lhs)?;
                if let Some(immediate) = super::support::int_immediate_operand(op, rhs)
                    && self.function.performance.value_kind(lhs) == PerfValueKind::Int
                    && static_flavor == super::support::NumericFlavor::Int
                {
                    self.emit_int_immediate_to_register(dst, op, lhs, immediate)?;
                    return Ok(true);
                }
                let rhs = self.lower_readonly_operand(rhs)?;
                let flavor = super::facts::numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs)
                    .unwrap_or(static_flavor);
                self.emit_bin_op_to_register_with_flavor(dst, op, lhs, rhs, flavor)?;
                Ok(true)
            }
            Expr::CallExpr(callee, args)
                if self.is_external_module_call(callee, args, "math", "floor", 1)
                    && math_floor_arg_is_int_like(&args[0], &self.locals, &self.function.performance) =>
            {
                if self.try_lower_int_midpoint_to_register(dst, &args[0])? {
                    return Ok(true);
                }
                self.try_lower_expr_to_register(dst, &args[0])
            }
            Expr::CallExpr(callee, args) if self.is_external_module_call(callee, args, "map", "get", 2) => {
                self.lower_map_get_function_call_to_register(dst, args)?;
                Ok(true)
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
                && !self.native_names.contains_key(name))
            && matches!(field.as_ref(), Expr::Literal(value) if value.as_str() == Some(method))
    }
}

fn math_floor_arg_is_int_like(
    expr: &Expr,
    locals: &std::collections::HashMap<String, u16>,
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
                crate::operator::BinOp::Add
                    | crate::operator::BinOp::Sub
                    | crate::operator::BinOp::Mul
                    | crate::operator::BinOp::Div
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
    locals: &std::collections::HashMap<String, u16>,
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
