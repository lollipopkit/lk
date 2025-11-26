use std::ops::{Add, Div, Mul, Rem, Sub};

use anyhow::Result;

use crate::op::{BinOp, err_op};
use crate::util::fast_map::{fast_hash_map_with_capacity, fast_hash_set_with_capacity};

use super::Val;

impl Add for &Val {
    type Output = Result<Val>;

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
            (Val::Str(a), Val::Str(b)) => {
                if a.is_empty() {
                    return Ok(Val::Str(b.clone()));
                }
                if b.is_empty() {
                    return Ok(Val::Str(a.clone()));
                }
                Ok(Val::concat_strings(a.as_ref(), b.as_ref()))
            }
            (Val::Str(a), Val::Int(b)) => {
                let mut buf = itoa::Buffer::new();
                let b_str = buf.format(*b);
                Ok(Val::concat_strings(a.as_ref(), b_str))
            }
            (Val::Str(a), Val::Float(b)) => {
                let mut buf = ryu::Buffer::new();
                let b_str = buf.format(*b);
                Ok(Val::concat_strings(a.as_ref(), b_str))
            }
            (Val::Int(a), Val::Str(b)) => {
                let mut buf = itoa::Buffer::new();
                let a_str = buf.format(*a);
                Ok(Val::concat_strings(a_str, b.as_ref()))
            }
            (Val::Float(a), Val::Str(b)) => {
                let mut buf = ryu::Buffer::new();
                let a_str = buf.format(*a);
                Ok(Val::concat_strings(a_str, b.as_ref()))
            }
            (Val::Map(l), Val::Map(r)) => {
                let mut merged = fast_hash_map_with_capacity(l.len() + r.len());
                for (k, v) in l.iter() {
                    merged.insert(k.clone(), v.clone());
                }
                for (k, v) in r.iter() {
                    merged.insert(k.clone(), v.clone());
                }
                Ok(merged.into())
            }
            (Val::List(l), Val::List(r)) => {
                let merged = Val::concat_lists(l.as_ref(), r.as_ref());
                Ok(Val::List(merged))
            }
            (Val::List(l), r) => {
                let merged = Val::append_to_list(l.as_ref(), r);
                Ok(Val::List(merged))
            }
            _ => err_op(self, BinOp::Add, other),
        }
    }
}

impl Sub for &Val {
    type Output = Result<Val>;

    #[inline]
    fn sub(self, other: Self) -> Self::Output {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => Ok((a - b).into()),
            (Val::Float(a), Val::Float(b)) => Ok((a - b).into()),
            (Val::Float(a), Val::Int(b)) => Ok((a - *b as f64).into()),
            (Val::Int(a), Val::Float(b)) => Ok((*a as f64 - b).into()),
            (Val::List(l), Val::List(r)) => {
                if r.len() > 32 {
                    if r.iter().all(|v| matches!(v, Val::Int(_))) {
                        let mut set = fast_hash_set_with_capacity(r.len());
                        for v in r.iter() {
                            if let Val::Int(x) = v {
                                set.insert(*x);
                            }
                        }
                        let mut out = Vec::with_capacity(l.len());
                        for v in l.iter() {
                            match v {
                                Val::Int(x) if set.contains(x) => {}
                                _ => out.push(v.clone()),
                            }
                        }
                        return Ok(out.into());
                    }
                    if r.iter().all(|v| matches!(v, Val::Str(_))) {
                        let mut set = fast_hash_set_with_capacity(r.len());
                        for v in r.iter() {
                            if let Val::Str(s) = v {
                                set.insert(s.clone());
                            }
                        }
                        let mut out = Vec::with_capacity(l.len());
                        for v in l.iter() {
                            match v {
                                Val::Str(s) if set.contains(s) => {}
                                _ => out.push(v.clone()),
                            }
                        }
                        return Ok(out.into());
                    }
                    if r.iter().all(|v| matches!(v, Val::Bool(_))) {
                        let mut set = fast_hash_set_with_capacity(2);
                        for v in r.iter() {
                            if let Val::Bool(b) = v {
                                set.insert(*b);
                            }
                        }
                        let mut out = Vec::with_capacity(l.len());
                        for v in l.iter() {
                            match v {
                                Val::Bool(b) if set.contains(b) => {}
                                _ => out.push(v.clone()),
                            }
                        }
                        return Ok(out.into());
                    }
                }

                let mut result = Vec::with_capacity(l.len());
                'outer: for left_val in l.iter() {
                    for right_val in r.iter() {
                        if left_val == right_val {
                            continue 'outer;
                        }
                    }
                    result.push(left_val.clone());
                }
                Ok(result.into())
            }
            (Val::List(l), r) => {
                let mut result = Vec::with_capacity(l.len());
                let mut found = false;
                for val in l.iter() {
                    if !found && val == r {
                        found = true;
                        continue;
                    }
                    result.push(val.clone());
                }
                Ok(result.into())
            }
            (Val::Map(l), Val::Map(r)) => {
                let mut result = fast_hash_map_with_capacity(l.len());
                for (k, v) in l.iter() {
                    if !r.contains_key(k) {
                        result.insert(k.clone(), v.clone());
                    }
                }
                Ok(result.into())
            }
            (Val::Map(l), r) => {
                if let Val::Str(k) = r {
                    let mut result = fast_hash_map_with_capacity(l.len());
                    for (existing_k, v) in l.iter() {
                        if existing_k.as_ref() != k.as_ref() {
                            result.insert(existing_k.clone(), v.clone());
                        }
                    }
                    return Ok(result.into());
                }
                err_op(self, BinOp::Sub, other)
            }
            _ => err_op(self, BinOp::Sub, other),
        }
    }
}

impl Mul for &Val {
    type Output = Result<Val>;

    #[inline]
    fn mul(self, other: Self) -> Self::Output {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => Ok((a * b).into()),
            (Val::Float(a), Val::Float(b)) => Ok((a * b).into()),
            (Val::Float(a), Val::Int(b)) => Ok((a * *b as f64).into()),
            (Val::Int(a), Val::Float(b)) => Ok((*a as f64 * b).into()),
            _ => err_op(self, BinOp::Mul, other),
        }
    }
}

impl Div for &Val {
    type Output = Result<Val>;

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
    type Output = Result<Val>;

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
