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
        if items.is_empty() {
            self.record_empty_list_value_type(dst);
        } else {
            let value_fact = self.homogeneous_expr_value_fact(items.iter().map(|expr| expr.as_ref()));
            self.record_list_value_type(dst, value_fact);
        }
    }

    pub(crate) fn emit_list_access(&mut self, list_reg: u16, index_expr: &Expr) -> u16 {
        let dst = self.alloc();
        if let Expr::Val(Val::Int(index)) = index_expr
            && let Ok(index) = i16::try_from(*index)
        {
            self.emit(Op::ListIndexI(dst, list_reg, index));
            self.mark_list_lookup_result(dst, list_reg);
            return dst;
        }

        let index_reg = if let Expr::Var(arg_name) = index_expr {
            self.lookup(arg_name).unwrap_or_else(|| self.expr(index_expr))
        } else {
            self.expr(index_expr)
        };
        self.emit(Op::Access(dst, list_reg, index_reg));
        if self.int_regs.contains(&index_reg) {
            self.mark_list_lookup_result(dst, list_reg);
        }
        dst
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
