use anyhow::Result;

use crate::op::BinOp;
use crate::val::Val;
use crate::vm::bytecode::{rk_index, rk_is_const};
use crate::vm::{
    record_quickening_build_attempt_known_enabled, record_quickening_build_success_known_enabled,
    record_quickening_deopt_known_enabled, record_quickening_hit_known_enabled, record_quickening_miss_known_enabled,
    write_register_value,
};

const WARMUP_THRESHOLD: u8 = 4;
const BACKOFF_TICKS: u8 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuickenedKind {
    AddInt,
    AddFloat,
    AddStrRhs,
    AddStrLhs,
    SubInt,
    SubFloat,
    MulInt,
    MulFloat,
    ModInt,
    ModFloat,
    CmpEqInt,
    CmpNeInt,
    CmpLtInt,
    CmpLeInt,
    CmpGtInt,
    CmpGeInt,
    IndexListInt,
    IndexStrInt,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct QuickeningSite {
    hits: u8,
    backoff: u8,
    kind: Option<QuickenedKind>,
}

impl QuickeningSite {
    #[inline]
    fn execute_add(
        &mut self,
        regs: &mut [Val],
        consts: &[Val],
        dst: u16,
        lhs: u16,
        rhs: u16,
        collect_metrics: bool,
    ) -> Result<bool> {
        if let Some(
            kind @ (QuickenedKind::AddInt
            | QuickenedKind::AddFloat
            | QuickenedKind::AddStrRhs
            | QuickenedKind::AddStrLhs),
        ) = self.kind
        {
            if let Some(out) = eval_add_kind(kind, regs, consts, lhs, rhs) {
                record_hit(collect_metrics);
                assign_reg(regs, dst as usize, out);
                return Ok(true);
            }
            self.deopt(collect_metrics);
            return Ok(false);
        }

        if self.backoff > 0 {
            self.backoff -= 1;
            record_miss(collect_metrics);
            return Ok(false);
        }

        record_build_attempt(collect_metrics);
        let observed = observe_add_kind(regs, consts, lhs, rhs);
        if let Some(kind) = observed
            && let Some(out) = eval_add_kind(kind, regs, consts, lhs, rhs)
        {
            self.hits = self.hits.saturating_add(1);
            if self.hits >= WARMUP_THRESHOLD {
                self.kind = Some(kind);
                record_build_success(collect_metrics);
            }
            assign_reg(regs, dst as usize, out);
            Ok(true)
        } else {
            self.hits = 0;
            record_miss(collect_metrics);
            Ok(false)
        }
    }

    #[inline]
    fn execute_numeric_binary(
        &mut self,
        int_kind: QuickenedKind,
        float_kind: QuickenedKind,
        regs: &mut [Val],
        consts: &[Val],
        dst: u16,
        lhs: u16,
        rhs: u16,
        collect_metrics: bool,
    ) -> Result<bool> {
        if self.kind == Some(int_kind) || self.kind == Some(float_kind) {
            let kind = self.kind.expect("numeric quickening kind");
            if let Some(value) = eval_numeric_kind(kind, regs, consts, lhs, rhs) {
                record_hit(collect_metrics);
                assign_reg(regs, dst as usize, value);
                return Ok(true);
            }
            self.deopt(collect_metrics);
            return Ok(false);
        }

        if self.backoff > 0 {
            self.backoff -= 1;
            record_miss(collect_metrics);
            return Ok(false);
        }

        record_build_attempt(collect_metrics);
        if let Some(kind) = observe_numeric_kind(regs, consts, lhs, rhs, int_kind, float_kind)
            && let Some(value) = eval_numeric_kind(kind, regs, consts, lhs, rhs)
        {
            self.hits = self.hits.saturating_add(1);
            if self.hits >= WARMUP_THRESHOLD {
                self.kind = Some(kind);
                record_build_success(collect_metrics);
            }
            assign_reg(regs, dst as usize, value);
            Ok(true)
        } else {
            self.hits = 0;
            record_miss(collect_metrics);
            Ok(false)
        }
    }

    #[inline]
    fn execute_int_compare(
        &mut self,
        expected: QuickenedKind,
        regs: &mut [Val],
        consts: &[Val],
        dst: u16,
        lhs: u16,
        rhs: u16,
        collect_metrics: bool,
    ) -> Result<bool> {
        if self.kind == Some(expected) {
            if let (Val::Int(a), Val::Int(b)) = (rk_read(regs, consts, lhs), rk_read(regs, consts, rhs)) {
                record_hit(collect_metrics);
                assign_reg(regs, dst as usize, Val::Bool(expected.eval_int_cmp(a, b)));
                return Ok(true);
            }
            self.deopt(collect_metrics);
            return Ok(false);
        }

        if self.backoff > 0 {
            self.backoff -= 1;
            record_miss(collect_metrics);
            return Ok(false);
        }

        record_build_attempt(collect_metrics);
        if let (Val::Int(a), Val::Int(b)) = (rk_read(regs, consts, lhs), rk_read(regs, consts, rhs)) {
            self.hits = self.hits.saturating_add(1);
            if self.hits >= WARMUP_THRESHOLD {
                self.kind = Some(expected);
                record_build_success(collect_metrics);
            }
            assign_reg(regs, dst as usize, Val::Bool(expected.eval_int_cmp(a, b)));
            Ok(true)
        } else {
            self.hits = 0;
            record_miss(collect_metrics);
            Ok(false)
        }
    }

    #[inline]
    fn deopt(&mut self, collect_metrics: bool) {
        self.kind = None;
        self.hits = 0;
        self.backoff = BACKOFF_TICKS;
        record_deopt(collect_metrics);
    }
}

#[inline]
fn record_hit(collect_metrics: bool) {
    if collect_metrics {
        record_quickening_hit_known_enabled();
    }
}

#[inline]
fn record_miss(collect_metrics: bool) {
    if collect_metrics {
        record_quickening_miss_known_enabled();
    }
}

#[inline]
fn record_build_attempt(collect_metrics: bool) {
    if collect_metrics {
        record_quickening_build_attempt_known_enabled();
    }
}

#[inline]
fn record_build_success(collect_metrics: bool) {
    if collect_metrics {
        record_quickening_build_success_known_enabled();
    }
}

#[inline]
fn record_deopt(collect_metrics: bool) {
    if collect_metrics {
        record_quickening_deopt_known_enabled();
    }
}

impl QuickenedKind {
    #[inline]
    fn eval_int(self, lhs: &i64, rhs: &i64) -> i64 {
        match self {
            QuickenedKind::AddInt => lhs + rhs,
            QuickenedKind::SubInt => lhs - rhs,
            QuickenedKind::MulInt => lhs * rhs,
            QuickenedKind::ModInt => lhs % rhs,
            QuickenedKind::AddFloat | QuickenedKind::SubFloat | QuickenedKind::MulFloat | QuickenedKind::ModFloat => {
                unreachable!("float quickening kind used as int arithmetic")
            }
            QuickenedKind::CmpEqInt
            | QuickenedKind::AddStrRhs
            | QuickenedKind::AddStrLhs
            | QuickenedKind::CmpNeInt
            | QuickenedKind::CmpLtInt
            | QuickenedKind::CmpLeInt
            | QuickenedKind::CmpGtInt
            | QuickenedKind::CmpGeInt => unreachable!("comparison quickening kind used as arithmetic"),
            QuickenedKind::IndexListInt | QuickenedKind::IndexStrInt => {
                unreachable!("index quickening kind used as arithmetic")
            }
        }
    }

    #[inline]
    fn eval_float(self, lhs: f64, rhs: f64) -> f64 {
        match self {
            QuickenedKind::AddFloat => lhs + rhs,
            QuickenedKind::SubFloat => lhs - rhs,
            QuickenedKind::MulFloat => lhs * rhs,
            QuickenedKind::ModFloat => lhs % rhs,
            _ => unreachable!("non-float quickening kind used as float arithmetic"),
        }
    }

    #[inline]
    fn eval_int_cmp(self, lhs: &i64, rhs: &i64) -> bool {
        match self {
            QuickenedKind::CmpEqInt => lhs == rhs,
            QuickenedKind::CmpNeInt => lhs != rhs,
            QuickenedKind::CmpLtInt => lhs < rhs,
            QuickenedKind::CmpLeInt => lhs <= rhs,
            QuickenedKind::CmpGtInt => lhs > rhs,
            QuickenedKind::CmpGeInt => lhs >= rhs,
            QuickenedKind::AddInt
            | QuickenedKind::AddFloat
            | QuickenedKind::SubInt
            | QuickenedKind::SubFloat
            | QuickenedKind::MulInt
            | QuickenedKind::MulFloat
            | QuickenedKind::ModInt
            | QuickenedKind::ModFloat => unreachable!("arithmetic quickening kind used as comparison"),
            QuickenedKind::AddStrRhs | QuickenedKind::AddStrLhs => {
                unreachable!("string quickening kind used as comparison")
            }
            QuickenedKind::IndexListInt | QuickenedKind::IndexStrInt => {
                unreachable!("index quickening kind used as comparison")
            }
        }
    }
}

#[inline]
fn observe_add_kind(regs: &[Val], consts: &[Val], lhs: u16, rhs: u16) -> Option<QuickenedKind> {
    let lhs_value = rk_read(regs, consts, lhs);
    let rhs_value = rk_read(regs, consts, rhs);
    if matches!((lhs_value, rhs_value), (Val::Int(_), Val::Int(_))) {
        Some(QuickenedKind::AddInt)
    } else if numeric_float_pair(lhs_value, rhs_value).is_some() {
        Some(QuickenedKind::AddFloat)
    } else if lhs_value.as_str().is_some() && Val::concat_str_add_rhs(lhs_value.as_str().unwrap(), rhs_value).is_some()
    {
        Some(QuickenedKind::AddStrRhs)
    } else if rhs_value.as_str().is_some() && Val::concat_add_lhs_str(lhs_value, rhs_value.as_str().unwrap()).is_some()
    {
        Some(QuickenedKind::AddStrLhs)
    } else {
        None
    }
}

#[inline]
fn observe_numeric_kind(
    regs: &[Val],
    consts: &[Val],
    lhs: u16,
    rhs: u16,
    int_kind: QuickenedKind,
    float_kind: QuickenedKind,
) -> Option<QuickenedKind> {
    let lhs_value = rk_read(regs, consts, lhs);
    let rhs_value = rk_read(regs, consts, rhs);
    if matches!((lhs_value, rhs_value), (Val::Int(_), Val::Int(_))) {
        Some(int_kind)
    } else if numeric_float_pair(lhs_value, rhs_value).is_some() {
        Some(float_kind)
    } else {
        None
    }
}

#[inline]
fn eval_add_kind(kind: QuickenedKind, regs: &[Val], consts: &[Val], lhs: u16, rhs: u16) -> Option<Val> {
    let lhs_value = rk_read(regs, consts, lhs);
    let rhs_value = rk_read(regs, consts, rhs);
    match kind {
        QuickenedKind::AddInt => match (lhs_value, rhs_value) {
            (Val::Int(left), Val::Int(right)) => Some(Val::Int(left + right)),
            _ => None,
        },
        QuickenedKind::AddFloat => {
            numeric_float_pair(lhs_value, rhs_value).map(|(left, right)| Val::Float(kind.eval_float(left, right)))
        }
        QuickenedKind::AddStrRhs => lhs_value
            .as_str()
            .and_then(|lhs_str| Val::concat_str_add_rhs(lhs_str, rhs_value)),
        QuickenedKind::AddStrLhs => rhs_value
            .as_str()
            .and_then(|rhs_str| Val::concat_add_lhs_str(lhs_value, rhs_str)),
        _ => None,
    }
}

#[inline]
fn eval_numeric_kind(kind: QuickenedKind, regs: &[Val], consts: &[Val], lhs: u16, rhs: u16) -> Option<Val> {
    let lhs_value = rk_read(regs, consts, lhs);
    let rhs_value = rk_read(regs, consts, rhs);
    match kind {
        QuickenedKind::AddInt | QuickenedKind::SubInt | QuickenedKind::MulInt | QuickenedKind::ModInt => {
            match (lhs_value, rhs_value) {
                (Val::Int(left), Val::Int(right)) => Some(Val::Int(kind.eval_int(left, right))),
                _ => None,
            }
        }
        QuickenedKind::AddFloat | QuickenedKind::SubFloat | QuickenedKind::MulFloat | QuickenedKind::ModFloat => {
            numeric_float_pair(lhs_value, rhs_value).map(|(left, right)| Val::Float(kind.eval_float(left, right)))
        }
        _ => None,
    }
}

#[inline]
fn numeric_float_pair(lhs: &Val, rhs: &Val) -> Option<(f64, f64)> {
    match (lhs, rhs) {
        (Val::Float(left), Val::Float(right)) => Some((*left, *right)),
        (Val::Float(left), Val::Int(right)) => Some((*left, *right as f64)),
        (Val::Int(left), Val::Float(right)) => Some((*left as f64, *right)),
        _ => None,
    }
}

#[inline]
fn execute_numeric_binary_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    int_kind: QuickenedKind,
    float_kind: QuickenedKind,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    if sites.len() <= pc {
        sites.resize(pc + 1, QuickeningSite::default());
    }
    sites[pc].execute_numeric_binary(int_kind, float_kind, regs, consts, dst, lhs, rhs, collect_metrics)
}

#[inline]
pub(in crate::vm::vm) fn execute_add_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    if sites.len() <= pc {
        sites.resize(pc + 1, QuickeningSite::default());
    }
    sites[pc].execute_add(regs, consts, dst, lhs, rhs, collect_metrics)
}

#[inline]
pub(in crate::vm::vm) fn execute_sub_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_numeric_binary_site(
        sites,
        pc,
        QuickenedKind::SubInt,
        QuickenedKind::SubFloat,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_mul_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_numeric_binary_site(
        sites,
        pc,
        QuickenedKind::MulInt,
        QuickenedKind::MulFloat,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_mod_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_numeric_binary_site(
        sites,
        pc,
        QuickenedKind::ModInt,
        QuickenedKind::ModFloat,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
fn execute_int_compare_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    kind: QuickenedKind,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    if sites.len() <= pc {
        sites.resize(pc + 1, QuickeningSite::default());
    }
    sites[pc].execute_int_compare(kind, regs, consts, dst, lhs, rhs, collect_metrics)
}

#[inline]
pub(in crate::vm::vm) fn execute_cmp_lt_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_int_compare_site(
        sites,
        pc,
        QuickenedKind::CmpLtInt,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_cmp_eq_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_int_compare_site(
        sites,
        pc,
        QuickenedKind::CmpEqInt,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_cmp_ne_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_int_compare_site(
        sites,
        pc,
        QuickenedKind::CmpNeInt,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_cmp_le_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_int_compare_site(
        sites,
        pc,
        QuickenedKind::CmpLeInt,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_cmp_gt_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_int_compare_site(
        sites,
        pc,
        QuickenedKind::CmpGtInt,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_cmp_ge_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    lhs: u16,
    rhs: u16,
    collect_metrics: bool,
) -> Result<bool> {
    execute_int_compare_site(
        sites,
        pc,
        QuickenedKind::CmpGeInt,
        regs,
        consts,
        dst,
        lhs,
        rhs,
        collect_metrics,
    )
}

#[inline]
pub(in crate::vm::vm) fn execute_index_site(
    sites: &mut Vec<QuickeningSite>,
    pc: usize,
    regs: &mut [Val],
    dst: u16,
    base: u16,
    index: u16,
    collect_metrics: bool,
) -> Result<bool> {
    if sites.len() <= pc {
        sites.resize(pc + 1, QuickeningSite::default());
    }
    let site = &mut sites[pc];
    if let Some(kind @ (QuickenedKind::IndexListInt | QuickenedKind::IndexStrInt)) = site.kind {
        if let Some(out) = eval_index_kind(kind, regs, base, index) {
            record_hit(collect_metrics);
            assign_reg(regs, dst as usize, out);
            return Ok(true);
        }
        site.deopt(collect_metrics);
        return Ok(false);
    }

    if site.backoff > 0 {
        site.backoff -= 1;
        record_miss(collect_metrics);
        return Ok(false);
    }

    record_build_attempt(collect_metrics);
    let observed = observe_index_kind(regs, base, index);
    if let Some(kind) = observed
        && let Some(out) = eval_index_kind(kind, regs, base, index)
    {
        site.hits = site.hits.saturating_add(1);
        if site.hits >= WARMUP_THRESHOLD {
            site.kind = Some(kind);
            record_build_success(collect_metrics);
        }
        assign_reg(regs, dst as usize, out);
        Ok(true)
    } else {
        site.hits = 0;
        record_miss(collect_metrics);
        Ok(false)
    }
}

#[inline]
fn observe_index_kind(regs: &[Val], base: u16, index: u16) -> Option<QuickenedKind> {
    match (&regs[base as usize], &regs[index as usize]) {
        (Val::List(_), Val::Int(_)) => Some(QuickenedKind::IndexListInt),
        (value, Val::Int(_)) if value.as_str().is_some() => Some(QuickenedKind::IndexStrInt),
        _ => None,
    }
}

#[inline]
fn eval_index_kind(kind: QuickenedKind, regs: &[Val], base: u16, index: u16) -> Option<Val> {
    match (kind, &regs[base as usize], &regs[index as usize]) {
        (QuickenedKind::IndexListInt, Val::List(values), Val::Int(index)) => Some(index_list(values, *index)),
        (QuickenedKind::IndexStrInt, value, Val::Int(index)) if value.as_str().is_some() => {
            Some(index_str(value.as_str().unwrap(), *index))
        }
        _ => None,
    }
}

#[inline]
fn index_list(values: &[Val], index: i64) -> Val {
    let index = if index < 0 {
        match values.len().checked_sub(index.unsigned_abs() as usize) {
            Some(index) => index,
            None => return Val::Nil,
        }
    } else {
        index as usize
    };
    values.get(index).cloned().unwrap_or(Val::Nil)
}

#[inline]
fn index_str(value: &str, index: i64) -> Val {
    let len = if value.is_ascii() {
        value.len()
    } else {
        value.chars().count()
    };
    let index = if index < 0 {
        match len.checked_sub(index.unsigned_abs() as usize) {
            Some(index) => index,
            None => return Val::Nil,
        }
    } else {
        index as usize
    };
    if value.is_ascii() {
        let bytes = value.as_bytes();
        if index < bytes.len() {
            Val::ascii_char_value(bytes[index])
        } else {
            Val::Nil
        }
    } else {
        value
            .chars()
            .nth(index as usize)
            .map(|character| Val::from_str(&character.to_string()))
            .unwrap_or(Val::Nil)
    }
}

#[inline]
pub(in crate::vm::vm) fn fallback_add(regs: &mut [Val], consts: &[Val], dst: u16, lhs: u16, rhs: u16) -> Result<()> {
    let out = BinOp::Add.eval_vals(rk_read(regs, consts, lhs), rk_read(regs, consts, rhs))?;
    assign_reg(regs, dst as usize, out);
    Ok(())
}

#[inline]
fn rk_read<'a>(regs: &'a [Val], consts: &'a [Val], rk: u16) -> &'a Val {
    if rk_is_const(rk) {
        &consts[rk_index(rk) as usize]
    } else {
        &regs[rk_index(rk) as usize]
    }
}

#[inline]
fn assign_reg(regs: &mut [Val], idx: usize, value: Val) {
    write_register_value(regs, idx, value);
}

#[cfg(test)]
mod tests;
