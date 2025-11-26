use core::cmp::Ordering;
use core::fmt::Debug;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

use anyhow::{Result, anyhow};

use crate::{expr::Expr, val::Val};

pub(crate) fn err_op<T: Display, R>(l: &Val, op: T, r: &Val) -> Result<R> {
    Err(anyhow!("Invalid op: {l} {op} {r}"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnaryOp {
    Not,
}

impl UnaryOp {
    pub(crate) fn eval_val(&self, val: &Val) -> Result<Val> {
        match self {
            UnaryOp::Not => match val {
                Val::Bool(b) => Ok(Val::Bool(!b)),
                _ => Err(anyhow!("Invalid operand: !{val}")),
            },
        }
    }
}

impl Display for UnaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnaryOp::Not => write!(f, "!"),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    In,
}

impl BinOp {
    pub(crate) fn is_arith(&self) -> bool {
        matches!(self, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod)
    }

    pub(crate) fn is_cmp(&self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Gt | BinOp::Lt | BinOp::Ge | BinOp::Le | BinOp::In
        )
    }

    fn arith(&self, l: &Val, r: &Val) -> Result<Val> {
        match self {
            BinOp::Add => l + r,
            BinOp::Sub => l - r,
            BinOp::Mul => l * r,
            BinOp::Div => l / r,
            BinOp::Mod => l % r,
            _ => err_op(l, self, r),
        }
    }

    pub(crate) fn cmp(&self, l: &Val, r: &Val) -> Result<bool> {
        match self {
            BinOp::Eq => Ok(l == r),
            BinOp::Ne => Ok(l != r),
            BinOp::In => match (l, r) {
                (Val::Str(l), Val::Str(r)) => Ok(r.as_ref().contains(l.as_ref())),

                // All elements in l must be in r
                (Val::List(l), Val::List(r)) => Ok(Val::list_contains_all(r, l)),
                (_, Val::List(r)) => Ok(Val::list_contains(r, l)),

                // Map key lookup optimization
                (Val::Str(s), Val::Map(m)) => Ok(m.contains_key(s.as_ref())),
                // For non-string keys, convert to string key with fast path when enabled
                (Val::Int(i), Val::Map(m)) => {
                    let mut buf = itoa::Buffer::new();
                    let s = buf.format(*i);
                    Ok(m.contains_key(s))
                }
                (Val::Float(f), Val::Map(m)) => {
                    let mut buf = ryu::Buffer::new();
                    let s = buf.format(*f);
                    Ok(m.contains_key(s))
                }
                (Val::Bool(b), Val::Map(m)) => {
                    // Avoid allocation for boolean conversion
                    if *b {
                        Ok(m.contains_key("true"))
                    } else {
                        Ok(m.contains_key("false"))
                    }
                }
                // Other types return false (Nil or complex structures can't be keys)
                (_, Val::Map(_)) => Ok(false),

                (Val::Float(l), Val::Float(r)) => Ok(l < r),
                (Val::Int(l), Val::Int(r)) => Ok(l < r),
                (Val::Bool(l), Val::Bool(r)) => Ok(l == r),
                (Val::Nil, Val::Nil) => Ok(true),

                _ => err_op(l, self, r),
            },
            _ => {
                // For other comparison operators, we need ordering
                let ord = match l.partial_cmp(r) {
                    Some(ord) => ord,
                    None => return err_op(l, self, r),
                };

                match self {
                    BinOp::Gt => Ok(ord == Ordering::Greater),
                    BinOp::Lt => Ok(ord == Ordering::Less),
                    BinOp::Ge => Ok(ord != Ordering::Less),
                    BinOp::Le => Ok(ord != Ordering::Greater),
                    _ => err_op(l, self, r),
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn eval(&self, l: &Expr, r: &Expr) -> Result<Val> {
        // For comparison operators, we can optimize by only evaluating the left side first
        if self.is_cmp() && matches!(self, BinOp::Eq | BinOp::Ne) {
            let l_val = l.eval()?;

            // Short-circuit for nil comparisons
            match (&l_val, self) {
                (Val::Nil, BinOp::Eq) => {
                    let r_val = r.eval()?;
                    return Ok(Val::Bool(matches!(r_val, Val::Nil)));
                }
                (Val::Nil, BinOp::Ne) => {
                    let r_val = r.eval()?;
                    return Ok(Val::Bool(!matches!(r_val, Val::Nil)));
                }
                _ => {}
            }

            let r_val = r.eval()?;
            return Ok(Val::Bool(self.cmp(&l_val, &r_val)?));
        }

        // For arithmetic operations
        let l_val = l.eval()?;
        let r_val = r.eval()?;

        if self.is_arith() {
            self.arith(&l_val, &r_val)
        } else if self.is_cmp() {
            Ok(Val::Bool(self.cmp(&l_val, &r_val)?))
        } else {
            Err(anyhow!("Invalid eval: {l_val} {self:?} {r_val}"))
        }
    }

    pub(crate) fn eval_vals(&self, l_val: &Val, r_val: &Val) -> Result<Val> {
        if self.is_arith() {
            self.arith(l_val, r_val)
        } else if self.is_cmp() {
            Ok(Val::Bool(self.cmp(l_val, r_val)?))
        } else {
            Err(anyhow!("Invalid eval: {l_val} {self:?} {r_val}"))
        }
    }
}

impl Display for BinOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinOp::Add => write!(f, "+"),
            BinOp::Div => write!(f, "/"),
            BinOp::Mul => write!(f, "*"),
            BinOp::Sub => write!(f, "-"),
            BinOp::Mod => write!(f, "%"),
            BinOp::Eq => write!(f, "=="),
            BinOp::Ne => write!(f, "!="),
            BinOp::Gt => write!(f, ">"),
            BinOp::Lt => write!(f, "<"),
            BinOp::Ge => write!(f, ">="),
            BinOp::Le => write!(f, "<="),
            BinOp::In => write!(f, "in"),
        }
    }
}
