use anyhow::{Result, bail};

use crate::{
    expr::Expr,
    val::{LiteralVal, ShortStr},
    vm::analysis::PerfValueKind,
};

use super::{Compiler32, ConstHeapValue32, Instr32, Opcode32, checked_u8, support::ast_literal_kind};

impl Compiler32 {
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

    pub(super) fn try_lower_expr_to_register(&mut self, dst: u16, expr: &Expr) -> Result<bool> {
        match expr {
            Expr::Paren(inner) => self.try_lower_expr_to_register(dst, inner),
            Expr::Literal(value) => {
                self.emit_literal_to_register(dst, value)?;
                Ok(true)
            }
            Expr::Bin(lhs, op, rhs) => {
                let static_flavor = super::support::numeric_flavor(lhs, op, rhs);
                let lhs = self.lower_readonly_operand(lhs)?;
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
        match value {
            LiteralVal::Nil => {
                self.emit(Instr32::abc(Opcode32::LoadNil, checked_u8("dst", dst)?, 0, 0));
                self.set_register_kind(dst, PerfValueKind::Nil);
            }
            LiteralVal::Bool(value) => {
                self.emit(Instr32::abc(
                    Opcode32::LoadBool,
                    checked_u8("dst", dst)?,
                    u8::from(*value),
                    0,
                ));
                self.set_register_kind(dst, PerfValueKind::Bool);
            }
            LiteralVal::Int(value) => {
                let k = self.push_int(*value)?;
                self.emit(Instr32::abx(Opcode32::LoadInt, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Int);
            }
            LiteralVal::Float(value) => {
                let k = self.push_float(*value)?;
                self.emit(Instr32::abx(Opcode32::LoadFloat, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Float);
            }
            value if value.as_str().is_some() => {
                let value = value.as_str().expect("checked string");
                if ShortStr::new(value).is_some() {
                    let k = self.push_string(value)?;
                    self.emit(Instr32::abx(Opcode32::LoadString, checked_u8("dst", dst)?, k));
                } else {
                    let k = self.push_heap_value(ConstHeapValue32::LongString(value.into()))?;
                    self.emit(Instr32::abx(Opcode32::LoadHeapConst, checked_u8("dst", dst)?, k));
                }
                self.set_register_kind(dst, PerfValueKind::String);
            }
            other => bail!(
                "Compiler32 cannot materialize AST literal value yet: {}",
                ast_literal_kind(other)
            ),
        }
        Ok(())
    }
}
