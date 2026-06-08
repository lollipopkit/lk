use core::cmp::Ordering;
use core::fmt::Debug;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

use crate::val::LiteralVal;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnaryOp {
    Not,
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

    pub(crate) fn cmp_literals(&self, l: &LiteralVal, r: &LiteralVal) -> Option<bool> {
        match self {
            BinOp::Eq => Some(l == r),
            BinOp::Ne => Some(l != r),
            BinOp::In => match (l, r) {
                (l, r) if l.as_str().is_some() && r.as_str().is_some() => {
                    Some(r.as_str().unwrap().contains(l.as_str().unwrap()))
                }
                _ => None,
            },
            _ => {
                let ord = cmp_literal_ordering(l, r)?;

                match self {
                    BinOp::Gt => Some(ord == Ordering::Greater),
                    BinOp::Lt => Some(ord == Ordering::Less),
                    BinOp::Ge => Some(ord != Ordering::Less),
                    BinOp::Le => Some(ord != Ordering::Greater),
                    _ => None,
                }
            }
        }
    }
}

fn cmp_literal_ordering(l: &LiteralVal, r: &LiteralVal) -> Option<Ordering> {
    match (l, r) {
        (LiteralVal::Int(a), LiteralVal::Int(b)) => a.partial_cmp(b),
        (LiteralVal::Float(a), LiteralVal::Float(b)) => a.partial_cmp(b),
        (LiteralVal::Int(a), LiteralVal::Float(b)) => (*a as f64).partial_cmp(b),
        (LiteralVal::Float(a), LiteralVal::Int(b)) => a.partial_cmp(&(*b as f64)),
        _ => match (l.as_str(), r.as_str()) {
            (Some(a), Some(b)) => a.partial_cmp(b),
            _ => None,
        },
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
