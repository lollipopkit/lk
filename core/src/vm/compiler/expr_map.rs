use super::FunctionBuilder;
use crate::{expr::Expr, val::Val, vm::Op};

pub(super) fn expr_result_is_temporary(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_) => false,
        Expr::Paren(inner) => expr_result_is_temporary(inner),
        _ => true,
    }
}

impl FunctionBuilder {
    pub(crate) fn emit_map_from_named_args(&mut self, named_args: &[(String, Box<Expr>)]) -> u16 {
        let dst = self.alloc();
        self.emit_map_from_named_args_into(dst, named_args);
        dst
    }

    pub(crate) fn emit_map_from_named_args_into(&mut self, dst: u16, named_args: &[(String, Box<Expr>)]) {
        let count = named_args.len() as u16;
        let base = self.n_regs;
        for _ in 0..(named_args.len() * 2) {
            let _ = self.alloc();
        }
        for (index, (name, expr)) in named_args.iter().enumerate() {
            let key_reg = base + (2 * index) as u16;
            let key_idx = self.k(Val::from_str(name.as_str()));
            self.emit(Op::LoadK(key_reg, key_idx));
            self.emit_expr_into(key_reg + 1, expr);
        }
        self.emit(Op::BuildMap { dst, base, len: count });
        if named_args.is_empty() {
            self.record_empty_map_value_type(dst);
        } else {
            let value_fact = self.homogeneous_expr_value_fact(named_args.iter().map(|(_, expr)| expr.as_ref()));
            self.record_map_value_type(dst, value_fact);
        }
    }

    pub(crate) fn emit_map_access(&mut self, map_reg: u16, key_expr: &Expr) -> u16 {
        let dst = self.alloc();
        if let Some(key_idx) = self.map_literal_key_const(key_expr) {
            self.emit(Op::MapGetInterned(dst, map_reg, key_idx));
        } else if self.reg_known_map(map_reg) {
            let key_reg = self.expr(key_expr);
            self.emit(Op::MapGetDynamic(dst, map_reg, key_reg));
        } else {
            let key_reg = self.expr(key_expr);
            self.emit(Op::Access(dst, map_reg, key_reg));
        }
        self.mark_map_lookup_result(dst, map_reg);
        dst
    }

    pub(crate) fn emit_map_has(&mut self, map_reg: u16, key_expr: &Expr) -> u16 {
        let dst = self.alloc();
        if let Some(key_idx) = self.map_literal_key_const(key_expr) {
            self.emit(Op::MapHasK(dst, map_reg, key_idx));
        } else {
            let key_reg = self.expr(key_expr);
            self.emit(Op::MapHas(dst, map_reg, key_reg));
        }
        dst
    }

    pub(crate) fn emit_map_set(&mut self, map_reg: u16, key_expr: &Expr, value_expr: &Expr) {
        if let Some(key_idx) = self.map_literal_key_const(key_expr) {
            let val_reg = if let Expr::Var(arg_name) = value_expr {
                self.lookup(arg_name).unwrap_or_else(|| self.expr(value_expr))
            } else {
                self.expr(value_expr)
            };
            if val_reg != map_reg && expr_result_is_temporary(value_expr) {
                self.emit(Op::MapSetInternedMove(map_reg, key_idx, val_reg));
            } else {
                self.emit(Op::MapSetInterned(map_reg, key_idx, val_reg));
            }
            return;
        }

        let key_reg = if let Expr::Var(arg_name) = key_expr {
            self.lookup(arg_name).unwrap_or_else(|| self.expr(key_expr))
        } else {
            self.expr(key_expr)
        };
        let val_reg = if let Expr::Var(arg_name) = value_expr {
            self.lookup(arg_name).unwrap_or_else(|| self.expr(value_expr))
        } else {
            self.expr(value_expr)
        };
        if key_reg != map_reg
            && val_reg != map_reg
            && key_reg != val_reg
            && expr_result_is_temporary(key_expr)
            && expr_result_is_temporary(value_expr)
        {
            self.emit(Op::MapSetMove {
                map: map_reg,
                key: key_reg,
                val: val_reg,
            });
        } else {
            self.emit(Op::MapSet {
                map: map_reg,
                key: key_reg,
                val: val_reg,
            });
        }
    }

    fn map_literal_key_const(&mut self, key_expr: &Expr) -> Option<u16> {
        let Expr::Val(key) = key_expr else {
            return None;
        };
        key.as_str().map(|_| self.k(key.clone()))
    }
}
