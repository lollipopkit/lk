use super::FunctionBuilder;
use crate::{expr::Expr, val::Val, vm::Op};

impl FunctionBuilder {
    pub(crate) fn emit_list_from_exprs_into(&mut self, dst: u16, items: &[Box<Expr>]) {
        let count = items.len() as u16;
        let base = self.n_regs;
        for _ in 0..items.len() {
            let _ = self.alloc();
        }
        for (index, expr) in items.iter().enumerate() {
            self.emit_expr_into(base + index as u16, expr);
        }
        self.emit(Op::BuildList { dst, base, len: count });
    }

    pub(crate) fn emit_list_set_i(&mut self, list_reg: u16, index_expr: &Expr, value_expr: &Expr) -> Option<u16> {
        let Expr::Val(Val::Int(index)) = index_expr else {
            return None;
        };
        let Ok(index) = i16::try_from(*index) else {
            return None;
        };
        let value_reg = if let Expr::Var(arg_name) = value_expr {
            self.lookup(arg_name).unwrap_or_else(|| self.expr(value_expr))
        } else {
            self.expr(value_expr)
        };
        let dst = self.alloc();
        self.emit(Op::ListSetI {
            dst,
            list: list_reg,
            index,
            val: value_reg,
        });
        Some(dst)
    }
}
