use crate::{
    expr::Expr,
    op::BinOp,
    val::Val,
    vm::{
        Op,
        bytecode::{rk_as_const, rk_index, rk_is_const, rk_make_const},
    },
};

use super::{ArithFlavor, FunctionBuilder};

impl FunctionBuilder {
    fn try_const_operand(&mut self, expr: &Expr) -> Option<u16> {
        match expr {
            Expr::Val(v) => Some(self.k(v.clone())),
            Expr::Var(name) => {
                if self.const_names.contains(name)
                    && let Some(val) = self.lookup_const(name)
                {
                    let val = val.clone();
                    Some(self.k(val))
                } else {
                    None
                }
            }
            Expr::Paren(inner) => self.try_const_operand(inner),
            _ => {
                let val = self.try_eval_const_expr(expr)?;
                Some(self.k(val))
            }
        }
    }

    pub(super) fn try_small_int_const(&mut self, expr: &Expr) -> Option<i16> {
        fn fits_i16(value: i64) -> Option<i16> {
            if (i16::MIN as i64..=i16::MAX as i64).contains(&value) {
                Some(value as i16)
            } else {
                None
            }
        }

        match expr {
            Expr::Val(Val::Int(v)) => fits_i16(*v),
            Expr::Var(name) if self.const_names.contains(name) => self.lookup_const(name).and_then(|val| match val {
                Val::Int(v) => fits_i16(*v),
                _ => None,
            }),
            Expr::Var(_) => None,
            Expr::Paren(inner) => self.try_small_int_const(inner),
            _ => match self.try_eval_const_expr(expr)? {
                Val::Int(v) => fits_i16(v),
                _ => None,
            },
        }
    }

    pub(super) fn expr_operand(&mut self, expr: &Expr) -> u16 {
        if let Expr::Var(name) = expr {
            if let Some(idx) = self.lookup(name) {
                idx
            } else if let Some(kidx) = self.try_const_operand(expr) {
                rk_make_const(kidx)
            } else {
                self.expr(expr)
            }
        } else if let Some(kidx) = self.try_const_operand(expr) {
            rk_make_const(kidx)
        } else {
            self.expr(expr)
        }
    }

    pub(super) fn operand_to_reg(&mut self, operand: u16) -> u16 {
        if rk_is_const(operand) {
            let dst = self.alloc();
            let kidx = rk_as_const(operand);
            self.emit(Op::LoadK(dst, kidx));
            dst
        } else {
            operand
        }
    }

    pub(super) fn try_emit_binop_imm(
        &mut self,
        dst: u16,
        left_operand: u16,
        imm: i16,
        op: &BinOp,
        flavor: ArithFlavor,
    ) -> bool {
        if rk_is_const(left_operand) {
            return false;
        }
        let left_reg = rk_index(left_operand);

        let mut emit_add_int_imm = |value: i16| {
            self.emit(Op::AddIntImm(dst, left_reg, value));
            true
        };

        match op {
            BinOp::Add => {
                if flavor != ArithFlavor::Float && (-128..=127).contains(&imm) {
                    return emit_add_int_imm(imm);
                }
            }
            BinOp::Sub => {
                if flavor != ArithFlavor::Float
                    && let Some(neg) = imm.checked_neg()
                    && (-128..=127).contains(&neg)
                {
                    return emit_add_int_imm(neg);
                }
            }
            BinOp::Eq => {
                self.emit(Op::CmpEqImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Ne => {
                self.emit(Op::CmpNeImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Lt => {
                if (-128..=127).contains(&imm) {
                    self.emit(Op::CmpLtImm(dst, left_reg, imm));
                } else {
                    let k = self.k(Val::Int(imm as i64));
                    let tmp = self.alloc();
                    self.emit(Op::LoadK(tmp, k));
                    self.emit(Op::CmpLt(dst, left_reg, tmp));
                }
                return true;
            }
            BinOp::Le => {
                if (-128..=127).contains(&imm) {
                    self.emit(Op::CmpLeImm(dst, left_reg, imm));
                } else {
                    let k = self.k(Val::Int(imm as i64));
                    let tmp = self.alloc();
                    self.emit(Op::LoadK(tmp, k));
                    self.emit(Op::CmpLe(dst, left_reg, tmp));
                }
                return true;
            }
            BinOp::Gt => {
                self.emit(Op::CmpGtImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Ge => {
                self.emit(Op::CmpGeImm(dst, left_reg, imm));
                return true;
            }
            _ => {}
        }

        false
    }
}
