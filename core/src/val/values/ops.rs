use std::ops::{Add, Div, Mul, Rem, Sub};

use anyhow::Result;

use crate::op::{BinOp, err_op};
use crate::util::fast_map::{fast_hash_map_with_capacity, fast_hash_set_with_capacity};

use super::Val;

#[inline(always)]
fn copy_collection_op_value(value: &Val, collect_metrics: bool) -> Val {
    if collect_metrics {
        crate::vm::copy_container_value_for_register_with_metrics(value, true)
    } else {
        value.clone()
    }
}

impl Val {
    #[inline]
    pub(crate) fn add_with_metrics(&self, other: &Self, collect_metrics: bool) -> Result<Val> {
        match (self, other) {
            (Val::Map(l), Val::Map(r)) => {
                let mut merged = fast_hash_map_with_capacity(l.len() + r.len());
                for (k, v) in l.iter() {
                    merged.insert(k.clone(), copy_collection_op_value(v, collect_metrics));
                }
                for (k, v) in r.iter() {
                    merged.insert(k.clone(), copy_collection_op_value(v, collect_metrics));
                }
                Ok(merged.into())
            }
            (Val::List(l), Val::List(r)) => Ok(Val::List(Val::concat_lists_with_metrics(
                l.as_ref(),
                r.as_ref(),
                collect_metrics,
            ))),
            (Val::List(l), r) => Ok(Val::List(Val::append_to_list_with_metrics(
                l.as_ref(),
                r,
                collect_metrics,
            ))),
            _ => self + other,
        }
    }

    #[inline]
    pub(crate) fn sub_with_metrics(&self, other: &Self, collect_metrics: bool) -> Result<Val> {
        match (self, other) {
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
                                _ => out.push(copy_collection_op_value(v, collect_metrics)),
                            }
                        }
                        return Ok(out.into());
                    }
                    if r.iter().all(|v| matches!(v, Val::ShortStr(_) | Val::Str(_))) {
                        let mut set: std::collections::HashSet<&str> =
                            std::collections::HashSet::with_capacity(r.len());
                        for v in r.iter() {
                            if let Some(s) = v.as_str() {
                                set.insert(s);
                            }
                        }
                        let mut out = Vec::with_capacity(l.len());
                        for v in l.iter() {
                            match v.as_str() {
                                Some(s) if set.contains(s) => {}
                                _ => out.push(copy_collection_op_value(v, collect_metrics)),
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
                                _ => out.push(copy_collection_op_value(v, collect_metrics)),
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
                    result.push(copy_collection_op_value(left_val, collect_metrics));
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
                    result.push(copy_collection_op_value(val, collect_metrics));
                }
                Ok(result.into())
            }
            (Val::Map(l), Val::Map(r)) => {
                let mut result = fast_hash_map_with_capacity(l.len());
                for (k, v) in l.iter() {
                    if !r.contains_key(k) {
                        result.insert(k.clone(), copy_collection_op_value(v, collect_metrics));
                    }
                }
                Ok(result.into())
            }
            (Val::Map(l), r) => {
                if let Some(k) = r.as_str() {
                    let mut result = fast_hash_map_with_capacity(l.len());
                    for (existing_k, v) in l.iter() {
                        if existing_k.as_str() != k {
                            result.insert(existing_k.clone(), copy_collection_op_value(v, collect_metrics));
                        }
                    }
                    return Ok(result.into());
                }
                err_op(self, BinOp::Sub, other)
            }
            _ => self - other,
        }
    }
}

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
            (Val::ShortStr(a), Val::ShortStr(b)) => Ok(Val::concat_strings(a.as_str(), b.as_str())),
            (Val::ShortStr(a), Val::Str(b)) => Ok(Val::concat_strings(a.as_str(), b.as_str())),
            (Val::Str(a), Val::ShortStr(b)) => Ok(Val::concat_strings(a.as_str(), b.as_str())),
            (Val::Str(a), Val::Str(b)) => Ok(Val::concat_strings(a.as_str(), b.as_str())),
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
            (Val::Map(_), Val::Map(_)) => self.add_with_metrics(other, crate::vm::vm_runtime_metrics_enabled()),
            (Val::List(l), Val::List(r)) => {
                let _ = (l, r);
                self.add_with_metrics(other, crate::vm::vm_runtime_metrics_enabled())
            }
            (Val::List(l), r) => {
                let _ = (l, r);
                self.add_with_metrics(other, crate::vm::vm_runtime_metrics_enabled())
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
            (Val::List(_), Val::List(_)) | (Val::List(_), _) | (Val::Map(_), Val::Map(_)) | (Val::Map(_), _) => {
                self.sub_with_metrics(other, crate::vm::vm_runtime_metrics_enabled())
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
