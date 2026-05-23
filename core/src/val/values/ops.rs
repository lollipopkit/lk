use std::ops::{Add, Div, Mul, Rem, Sub};

use crate::op::{BinOp, err_op};

use super::Val;

impl Add for &Val {
    type Output = anyhow::Result<Val>;

    /// - Str + Num may leads to unexpected behavior.
    /// - List can + Val, but Val + List is not supported.
    /// - Map can + Map, but Map can't + Val, since the value of the map is not defined.
    #[inline]
    fn add(self, other: Self) -> Self::Output {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => Ok(Val::Int(a + b)),
            (Val::Float(a), Val::Float(b)) => Ok(Val::Float(a + b)),
            (Val::Float(a), Val::Int(b)) => Ok(Val::Float(a + *b as f64)),
            (Val::Int(a), Val::Float(b)) => Ok(Val::Float(*a as f64 + b)),
            (lhs, rhs) if lhs.as_str().is_some() && rhs.as_str().is_some() => Ok(Val::concat_strings(
                lhs.as_str().expect("checked string"),
                rhs.as_str().expect("checked string"),
            )),
            (lhs, Val::Int(b)) if lhs.as_str().is_some() => {
                let mut buf = itoa::Buffer::new();
                Ok(Val::concat_strings(lhs.as_str().unwrap(), buf.format(*b)))
            }
            (lhs, Val::Float(b)) if lhs.as_str().is_some() => {
                let mut buf = ryu::Buffer::new();
                Ok(Val::concat_strings(lhs.as_str().unwrap(), buf.format(*b)))
            }
            (Val::Int(a), rhs) if rhs.as_str().is_some() => {
                let mut buf = itoa::Buffer::new();
                Ok(Val::concat_strings(buf.format(*a), rhs.as_str().unwrap()))
            }
            (Val::Float(a), rhs) if rhs.as_str().is_some() => {
                let mut buf = ryu::Buffer::new();
                Ok(Val::concat_strings(buf.format(*a), rhs.as_str().unwrap()))
            }
            _ => err_op(self, BinOp::Add, other),
        }
    }
}

impl Sub for &Val {
    type Output = anyhow::Result<Val>;

    #[inline]
    fn sub(self, other: Self) -> Self::Output {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => Ok((a - b).into()),
            (Val::Float(a), Val::Float(b)) => Ok((a - b).into()),
            (Val::Float(a), Val::Int(b)) => Ok((a - *b as f64).into()),
            (Val::Int(a), Val::Float(b)) => Ok((*a as f64 - b).into()),
            _ => err_op(self, BinOp::Sub, other),
        }
    }
}

impl Mul for &Val {
    type Output = anyhow::Result<Val>;

    #[inline]
    fn mul(self, other: Self) -> Self::Output {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => Ok((a * b).into()),
            (Val::Float(a), Val::Float(b)) => Ok((a * b).into()),
            (Val::Float(a), Val::Int(b)) => Ok((a * *b as f64).into()),
            (Val::Int(a), Val::Float(b)) => Ok((*a as f64 * b).into()),
            (left, Val::Int(count)) if left.as_str().is_some() => {
                if *count <= 0 {
                    Ok(Val::from_str(""))
                } else {
                    Ok(Val::from_str(&left.as_str().unwrap().repeat(*count as usize)))
                }
            }
            (Val::Int(count), right) if right.as_str().is_some() => {
                if *count <= 0 {
                    Ok(Val::from_str(""))
                } else {
                    Ok(Val::from_str(&right.as_str().unwrap().repeat(*count as usize)))
                }
            }
            _ => err_op(self, BinOp::Mul, other),
        }
    }
}

impl Div for &Val {
    type Output = anyhow::Result<Val>;

    #[inline]
    fn div(self, other: Self) -> Self::Output {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => {
                let res = (*a as f64) / (*b as f64);
                if res.fract() == 0.0 {
                    Ok((res as i64).into())
                } else {
                    Ok(res.into())
                }
            }
            (Val::Float(a), Val::Float(b)) => Ok((a / b).into()),
            (Val::Float(a), Val::Int(b)) => Ok((a / *b as f64).into()),
            (Val::Int(a), Val::Float(b)) => Ok((*a as f64 / b).into()),
            _ => err_op(self, BinOp::Div, other),
        }
    }
}

impl Rem for &Val {
    type Output = anyhow::Result<Val>;

    #[inline]
    fn rem(self, other: Self) -> Self::Output {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => Ok((a % b).into()),
            (Val::Float(a), Val::Float(b)) => Ok((a % b).into()),
            (Val::Float(a), Val::Int(b)) => Ok((a % *b as f64).into()),
            (Val::Int(a), Val::Float(b)) => Ok((*a as f64 % b).into()),
            _ => err_op(self, BinOp::Mod, other),
        }
    }
}
