use std::collections::HashSet;

use anyhow::{Result, bail};

use crate::{
    expr::Expr,
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
                let lhs = self.lower_readonly_operand(lhs)?;
                if let Some(delta) = super::support::int_immediate_delta(op, rhs)
                    && self.function.performance.value_kind(lhs) == PerfValueKind::Int
                    && static_flavor == super::support::NumericFlavor::Int
                {
                    self.emit_add_int_immediate_to_register(dst, lhs, delta)?;
                    return Ok(true);
                }
                let rhs = self.lower_readonly_operand(rhs)?;
                let flavor = super::facts::numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs)
                    .unwrap_or(static_flavor);
                self.emit_bin_op_to_register_with_flavor(dst, op, lhs, rhs, flavor)?;
                Ok(true)
            }
            _ => Ok(false),
        }
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
