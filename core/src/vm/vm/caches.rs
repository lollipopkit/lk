use std::sync::Arc;

use crate::val::{ClosureCapture, RustFastFunction, RustFastFunctionNamed, RustFunction, RustFunctionNamed, Val};
use crate::vm::bytecode::{Function, IntCmpKind, Op, rk_index, rk_is_const};
use crate::vm::vm::frame::FrameInfo;
use crate::vm::vm::quickening::QuickeningSite;
use crate::vm::{CaptureSpec, RegionPlan};

mod packed;
pub(super) use packed::*;

#[cfg(test)]
mod tiny_call_tests;

// ────────────── Inline Cache Architecture ──────────────
//
// LK uses polymorphic inline caches (ICs) at each instruction site to
// accelerate property access, indexing, global lookups, and function calls.
//
//  AccessIc (ObjectStr): 4-entry LRU cache keyed by (base_ptr, key).
//  IndexIc (List/Str): 4-entry LRU cache keyed by (base_ptr, index).
//  GlobalEntry: Single-entry per site, with generation tracking for invalidation.
//  CallIc: ClosurePositional (closure_ptr+argc), Rust, RustFastNamed, RustNamed.
//  ForRangeState: Holds (current, limit, step, inclusive) — bare i64s.
//  PackedHotEntry: Caches decoded packed instruction for hot BC32 paths.

// Small polymorphic inline caches (4-way) for property/index access per instruction site.
// This reduces churn at megamorphic sites while staying allocation-free.
#[derive(Clone)]
pub(super) struct ObjectStrEntry {
    pub(super) obj_ptr: usize,
    pub(super) key: String,
    pub(super) value: Val,
}

#[derive(Clone)]
pub(super) enum AccessIc {
    ObjectStr([Option<ObjectStrEntry>; 4]),
}

/// Per-op inline cache entries reused across VM executions (to avoid reallocation).
#[derive(Clone)]
pub(super) struct ListEntry {
    pub(super) base_ptr: usize,
    pub(super) idx: i64,
    pub(super) value: Val,
}

#[derive(Clone)]
pub(super) struct StrEntry {
    pub(super) base_ptr: usize,
    pub(super) idx: i64,
    pub(super) value: Val,
}

#[derive(Clone)]
pub(super) enum IndexIc {
    List([Option<ListEntry>; 4]),
    Str([Option<StrEntry>; 4]),
}

#[derive(Clone)]
pub(super) struct GlobalEntry(
    pub(super) usize, /*name_ptr*/
    pub(super) Val,
    pub(super) u64, /*generation*/
);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct CallReturnLayout {
    pub(super) base: u16,
    pub(super) retc: u8,
}

impl CallReturnLayout {
    #[inline]
    pub(super) const fn new(base: u16, retc: u8) -> Self {
        Self { base, retc }
    }

    #[inline]
    pub(super) fn matches(self, base: u16, retc: u8) -> bool {
        self.base == base && self.retc == retc
    }
}

#[allow(clippy::large_enum_variant)]
pub(super) enum CallIc {
    Rust(RustFunction, u8 /*argc*/, CallReturnLayout),
    RustFast(RustFastFunction, u8 /*argc*/, CallReturnLayout),
    RustFastNamed(RustFastFunctionNamed, u8 /*argc*/, CallReturnLayout),
    RustNamed(RustFunctionNamed, u8 /*argc*/, CallReturnLayout),
    ClosurePositional {
        closure_ptr: usize,
        fun_ptr: *const Function,
        argc: u8,
        ret: CallReturnLayout,
        tiny: Option<TinyCallPlan>,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        cache: ClosureFastCache,
        frame_info: FrameInfo,
    },
    ClosureNamed {
        closure_ptr: usize,
        named_len: u8,
        ret: CallReturnLayout,
        plan: Arc<NamedCallPlan>,
    },
}

impl Clone for CallIc {
    fn clone(&self) -> Self {
        match self {
            CallIc::Rust(f, argc, ret) => CallIc::Rust(*f, *argc, *ret),
            CallIc::RustFast(f, argc, ret) => CallIc::RustFast(*f, *argc, *ret),
            CallIc::RustFastNamed(f, argc, ret) => CallIc::RustFastNamed(*f, *argc, *ret),
            CallIc::RustNamed(f, argc, ret) => CallIc::RustNamed(*f, *argc, *ret),
            CallIc::ClosurePositional {
                closure_ptr,
                fun_ptr,
                argc,
                ret,
                tiny,
                captures,
                capture_specs,
                cache,
                frame_info,
            } => CallIc::ClosurePositional {
                closure_ptr: *closure_ptr,
                fun_ptr: *fun_ptr,
                argc: *argc,
                ret: *ret,
                tiny: tiny.clone(),
                captures: captures.clone(),
                capture_specs: capture_specs.clone(),
                cache: cache.clone(),
                frame_info: frame_info.clone(),
            },
            CallIc::ClosureNamed {
                closure_ptr,
                named_len,
                ret,
                plan,
            } => CallIc::ClosureNamed {
                closure_ptr: *closure_ptr,
                named_len: *named_len,
                ret: *ret,
                plan: Arc::clone(plan),
            },
        }
    }
}

#[derive(Clone)]
pub(super) enum TinyCallPlan {
    Return(TinyOperand),
    Add(TinyOperand, TinyOperand),
    AddMod {
        lhs: TinyOperand,
        rhs: TinyOperand,
        modulo: TinyOperand,
    },
    EuclidGcd {
        lhs: usize,
        rhs: usize,
    },
    BinarySearchImplicit {
        target: usize,
        len: usize,
        scale: i64,
    },
    IsPrimeTrialDivision {
        input: usize,
    },
    IntExpr(TinyIntProgram),
    Expr(TinyExpr),
}

#[derive(Clone)]
pub(super) enum TinyOperand {
    Param(usize),
    Capture(usize),
    Const(Val),
}

#[derive(Clone)]
pub(super) enum TinyExpr {
    Operand(TinyOperand),
    Add(Box<TinyExpr>, Box<TinyExpr>),
    Sub(Box<TinyExpr>, Box<TinyExpr>),
    Mul(Box<TinyExpr>, Box<TinyExpr>),
    Mod(Box<TinyExpr>, Box<TinyExpr>),
}

#[derive(Clone)]
pub(super) struct TinyIntProgram {
    ops: Vec<TinyIntOp>,
    max_stack: usize,
}

#[derive(Clone)]
enum TinyIntOp {
    Param(usize),
    Capture(usize),
    Const(i64),
    Add,
    Sub,
    Mul,
    Mod,
}

impl TinyCallPlan {
    pub(super) fn analyze(fun: &Function) -> Option<Self> {
        if let Some(plan) = Self::analyze_is_prime_trial_division(fun) {
            return Some(plan);
        }
        if let Some(plan) = Self::analyze_binary_search_implicit(fun) {
            return Some(plan);
        }
        let ret_pos = fun.code.iter().position(|op| matches!(op, Op::Ret { retc: 1, .. }))?;
        if !Self::only_default_nil_tail_after_return(fun, ret_pos) {
            return None;
        }
        let Op::Ret { base, retc: 1 } = fun.code.get(ret_pos)? else {
            return None;
        };
        let base = *base;
        if let Some(plan) = Self::analyze_euclid_gcd_loop(fun, ret_pos, base) {
            return Some(plan);
        }
        let mut reg_operands: Vec<Option<TinyOperand>> = vec![None; fun.n_regs as usize];
        let mut local_operands: Vec<Option<TinyOperand>> = vec![None; fun.n_regs as usize];
        for op in &fun.code[..ret_pos] {
            match op {
                Op::LoadK(dst, kidx) => {
                    let value = fun.consts.get(*kidx as usize)?.clone();
                    *reg_operands.get_mut(*dst as usize)? = Some(TinyOperand::Const(value));
                }
                Op::LoadCapture { dst, idx } => {
                    *reg_operands.get_mut(*dst as usize)? = Some(TinyOperand::Capture(*idx as usize));
                }
                Op::Move(dst, src) => {
                    let operand = Self::operand_for_reg(*src, fun, &reg_operands)?;
                    *reg_operands.get_mut(*dst as usize)? = Some(operand);
                }
                Op::LoadLocal(dst, idx) => {
                    let operand = local_operands.get(*idx as usize)?.clone()?;
                    *reg_operands.get_mut(*dst as usize)? = Some(operand);
                }
                Op::StoreLocal(idx, src) => {
                    let operand = Self::operand_for_reg(*src, fun, &reg_operands)?;
                    *local_operands.get_mut(*idx as usize)? = Some(operand);
                }
                Op::Add(_, _, _)
                | Op::AddInt(_, _, _)
                | Op::AddFloat(_, _, _)
                | Op::AddIntImm(_, _, _)
                | Op::Sub(_, _, _)
                | Op::SubInt(_, _, _)
                | Op::SubFloat(_, _, _)
                | Op::Mul(_, _, _)
                | Op::MulInt(_, _, _)
                | Op::MulFloat(_, _, _)
                | Op::Mod(_, _, _)
                | Op::ModInt(_, _, _)
                | Op::ModFloat(_, _, _) => {}
                _ => return None,
            }
        }
        let return_operand = || Self::operand_for_reg(base, fun, &reg_operands);
        let defining_op = fun.code[..ret_pos].iter().rev().find(|op| Self::defines_reg(op, base));
        match defining_op {
            Some(Op::Mod(dst, lhs, modulo)) if *dst == base => {
                Self::add_mod_plan_for_rk(*lhs, *modulo, fun, &reg_operands)
                    .or_else(|| Self::analyze_expr_plan(fun, ret_pos, base))
            }
            Some(Op::ModInt(dst, lhs, modulo)) | Some(Op::ModFloat(dst, lhs, modulo)) if *dst == base => {
                Self::add_mod_plan_for_reg(*lhs, *modulo, fun, &reg_operands)
                    .or_else(|| Self::analyze_expr_plan(fun, ret_pos, base))
            }
            Some(Op::Add(dst, a, b)) if *dst == base => Some(Self::Add(
                Self::operand_for_rk(*a, fun, &reg_operands)?,
                Self::operand_for_rk(*b, fun, &reg_operands)?,
            )),
            Some(Op::AddInt(dst, a, b)) | Some(Op::AddFloat(dst, a, b)) if *dst == base => Some(Self::Add(
                Self::operand_for_reg(*a, fun, &reg_operands)?,
                Self::operand_for_reg(*b, fun, &reg_operands)?,
            )),
            Some(Op::AddIntImm(dst, src, imm)) if *dst == base => Some(Self::Add(
                Self::operand_for_reg(*src, fun, &reg_operands)?,
                TinyOperand::Const(Val::Int(*imm as i64)),
            )),
            Some(Op::Move(dst, src)) if *dst == base => {
                Some(Self::Return(Self::operand_for_reg(*src, fun, &reg_operands)?))
            }
            Some(Op::LoadCapture { dst, idx }) if *dst == base => {
                Some(Self::Return(TinyOperand::Capture(*idx as usize)))
            }
            Some(Op::LoadK(dst, _)) if *dst == base => Some(Self::Return(return_operand()?)),
            None => Some(Self::Return(return_operand()?)),
            _ => Self::analyze_expr_plan(fun, ret_pos, base),
        }
    }

    fn only_default_nil_tail_after_return(fun: &Function, ret_pos: usize) -> bool {
        match fun.code.get(ret_pos + 1..) {
            Some([]) => true,
            Some([Op::LoadK(_, kidx), Op::Ret { retc: 1, .. }]) => {
                matches!(fun.consts.get(*kidx as usize), Some(Val::Nil))
            }
            _ => false,
        }
    }

    #[inline]
    pub(super) fn try_eval(&self, args: &[Val], captures: Option<&ClosureCapture>) -> Option<Val> {
        match self {
            Self::Return(operand) => operand.resolve(args, captures).cloned(),
            Self::Add(lhs, rhs) => {
                let lhs = lhs.resolve(args, captures)?;
                let rhs = rhs.resolve(args, captures)?;
                Self::eval_add(lhs, rhs)
            }
            Self::AddMod { lhs, rhs, modulo } => {
                let lhs = lhs.resolve(args, captures)?;
                let rhs = rhs.resolve(args, captures)?;
                let modulo = modulo.resolve(args, captures)?;
                let sum = Self::eval_add(lhs, rhs)?;
                Self::eval_mod(&sum, modulo)
            }
            Self::EuclidGcd { lhs, rhs } => {
                let &Val::Int(mut a) = args.get(*lhs)? else {
                    return None;
                };
                let &Val::Int(mut b) = args.get(*rhs)? else {
                    return None;
                };
                while b != 0 {
                    let rem = a % b;
                    a = b;
                    b = rem;
                }
                Some(Val::Int(a))
            }
            Self::BinarySearchImplicit { target, len, scale } => {
                let &Val::Int(target) = args.get(*target)? else {
                    return None;
                };
                let &Val::Int(len) = args.get(*len)? else {
                    return None;
                };
                Self::eval_binary_search_implicit(target, len, *scale).map(Val::Int)
            }
            Self::IsPrimeTrialDivision { input } => {
                let &Val::Int(value) = args.get(*input)? else {
                    return None;
                };
                Some(Val::Bool(Self::eval_is_prime_trial_division(value)?))
            }
            Self::IntExpr(program) => program.eval(args, captures).map(Val::Int),
            Self::Expr(expr) => expr
                .eval_int(args, captures)
                .map(Val::Int)
                .or_else(|| expr.eval(args, captures)),
        }
    }

    fn analyze_is_prime_trial_division(fun: &Function) -> Option<Self> {
        Self::analyze_is_prime_trial_division_fused(fun)
            .or_else(|| Self::analyze_is_prime_trial_division_bool_branch(fun))
    }

    fn analyze_is_prime_trial_division_fused(fun: &Function) -> Option<Self> {
        if fun.param_regs.len() != 1 {
            return None;
        }
        let input_param = *fun.param_regs.first()?;
        let [
            Op::CmpLtImmJmp {
                r: lt_reg,
                imm: 2,
                ofs: 3,
            },
            Op::LoadK(false_early_reg, false_early_const),
            Op::Ret {
                base: false_early_base,
                retc: 1,
            },
            Op::CmpEqImmJmp {
                r: eq_two_reg,
                imm: 2,
                ofs: 3,
            },
            Op::LoadK(true_two_reg, true_two_const),
            Op::Ret {
                base: true_two_base,
                retc: 1,
            },
            Op::Mod(even_rem_reg, even_lhs, even_rhs) | Op::ModInt(even_rem_reg, even_lhs, even_rhs),
            Op::CmpEqImmJmp {
                r: even_cmp_reg,
                imm: 0,
                ofs: 3,
            },
            Op::LoadK(false_even_reg, false_even_const),
            Op::Ret {
                base: false_even_base,
                retc: 1,
            },
            Op::LoadK(divisor_reg, divisor_const),
            Op::MulInt(square_reg, square_lhs, square_rhs),
            Op::CmpIntJmp {
                kind: IntCmpKind::Le,
                a: square_cmp_lhs,
                b: square_cmp_rhs,
                ofs: 6,
            },
            Op::Mod(loop_rem_reg, loop_rem_lhs, loop_rem_rhs) | Op::ModInt(loop_rem_reg, loop_rem_lhs, loop_rem_rhs),
            Op::CmpEqImmJmp {
                r: loop_cmp_reg,
                imm: 0,
                ofs: 3,
            },
            Op::LoadK(false_loop_reg, false_loop_const),
            Op::Ret {
                base: false_loop_base,
                retc: 1,
            },
            Op::AddIntImmJmp {
                r: divisor_update_reg,
                imm: 2,
                ofs: -6,
            },
            Op::LoadK(true_reg, true_const),
            Op::Ret {
                base: true_base,
                retc: 1,
            },
            Op::LoadK(_, nil_const),
            Op::Ret { retc: 1, .. },
        ] = fun.code.as_slice()
        else {
            return None;
        };
        if *lt_reg != input_param
            || *false_early_base != *false_early_reg
            || *eq_two_reg != input_param
            || *true_two_base != *true_two_reg
            || *even_lhs != input_param
            || Self::int_const_for_rk(*even_rhs, fun)? != 2
            || *even_cmp_reg != *even_rem_reg
            || *false_even_base != *false_even_reg
            || fun.consts.get(*divisor_const as usize) != Some(&Val::Int(3))
            || *square_lhs != *divisor_reg
            || *square_rhs != *divisor_reg
            || *square_cmp_lhs != *square_reg
            || *square_cmp_rhs != input_param
            || *loop_rem_lhs != input_param
            || *loop_rem_rhs != *divisor_reg
            || *loop_cmp_reg != *loop_rem_reg
            || *false_loop_base != *false_loop_reg
            || *divisor_update_reg != *divisor_reg
            || *true_base != *true_reg
            || fun.consts.get(*false_early_const as usize) != Some(&Val::Bool(false))
            || fun.consts.get(*false_even_const as usize) != Some(&Val::Bool(false))
            || fun.consts.get(*false_loop_const as usize) != Some(&Val::Bool(false))
            || fun.consts.get(*true_two_const as usize) != Some(&Val::Bool(true))
            || fun.consts.get(*true_const as usize) != Some(&Val::Bool(true))
            || !matches!(fun.consts.get(*nil_const as usize), Some(Val::Nil))
        {
            return None;
        }
        Some(Self::IsPrimeTrialDivision { input: 0 })
    }

    fn analyze_is_prime_trial_division_bool_branch(fun: &Function) -> Option<Self> {
        if fun.param_regs.len() != 1 {
            return None;
        }
        let input_param = *fun.param_regs.first()?;
        let [
            Op::CmpLtImmJmp {
                r: lt_reg,
                imm: 2,
                ofs: 3,
            },
            Op::LoadK(false_early_reg, false_early_const),
            Op::Ret {
                base: false_early_base,
                retc: 1,
            },
            Op::CmpEqImmJmp {
                r: eq_two_reg,
                imm: 2,
                ofs: 3,
            },
            Op::LoadK(true_two_reg, true_two_const),
            Op::Ret {
                base: true_two_base,
                retc: 1,
            },
            Op::Mod(even_rem_reg, even_lhs, even_rhs) | Op::ModInt(even_rem_reg, even_lhs, even_rhs),
            Op::CmpEqImmJmp {
                r: even_cmp_reg,
                imm: 0,
                ofs: 3,
            },
            Op::LoadK(false_even_reg, false_even_const),
            Op::Ret {
                base: false_even_base,
                retc: 1,
            },
            Op::LoadK(divisor_reg, divisor_const),
            Op::MulInt(square_reg, square_lhs, square_rhs),
            Op::CmpLe(loop_bool_reg, square_cmp_lhs, square_cmp_rhs),
            Op::BoolBranch(loop_branch_reg, 6),
            Op::Mod(loop_rem_reg, loop_rem_lhs, loop_rem_rhs) | Op::ModInt(loop_rem_reg, loop_rem_lhs, loop_rem_rhs),
            Op::CmpEqImmJmp {
                r: loop_cmp_reg,
                imm: 0,
                ofs: 3,
            },
            Op::LoadK(false_loop_reg, false_loop_const),
            Op::Ret {
                base: false_loop_base,
                retc: 1,
            },
            Op::AddIntImmJmp {
                r: divisor_update_reg,
                imm: 2,
                ofs: -7,
            },
            Op::LoadK(true_reg, true_const),
            Op::Ret {
                base: true_base,
                retc: 1,
            },
            Op::LoadK(_, nil_const),
            Op::Ret { retc: 1, .. },
        ] = fun.code.as_slice()
        else {
            return None;
        };
        if *lt_reg != input_param
            || *false_early_base != *false_early_reg
            || *eq_two_reg != input_param
            || *true_two_base != *true_two_reg
            || *even_lhs != input_param
            || Self::int_const_for_rk(*even_rhs, fun)? != 2
            || *even_cmp_reg != *even_rem_reg
            || *false_even_base != *false_even_reg
            || fun.consts.get(*divisor_const as usize) != Some(&Val::Int(3))
            || *square_lhs != *divisor_reg
            || *square_rhs != *divisor_reg
            || *square_cmp_lhs != *square_reg
            || *square_cmp_rhs != input_param
            || *loop_branch_reg != *loop_bool_reg
            || *loop_rem_lhs != input_param
            || *loop_rem_rhs != *divisor_reg
            || *loop_cmp_reg != *loop_rem_reg
            || *false_loop_base != *false_loop_reg
            || *divisor_update_reg != *divisor_reg
            || *true_base != *true_reg
            || fun.consts.get(*false_early_const as usize) != Some(&Val::Bool(false))
            || fun.consts.get(*false_even_const as usize) != Some(&Val::Bool(false))
            || fun.consts.get(*false_loop_const as usize) != Some(&Val::Bool(false))
            || fun.consts.get(*true_two_const as usize) != Some(&Val::Bool(true))
            || fun.consts.get(*true_const as usize) != Some(&Val::Bool(true))
            || !matches!(fun.consts.get(*nil_const as usize), Some(Val::Nil))
        {
            return None;
        }
        Some(Self::IsPrimeTrialDivision { input: 0 })
    }

    fn analyze_binary_search_implicit(fun: &Function) -> Option<Self> {
        Self::analyze_binary_search_implicit_fused(fun)
            .or_else(|| Self::analyze_binary_search_implicit_bool_branch(fun))
    }

    fn analyze_binary_search_implicit_fused(fun: &Function) -> Option<Self> {
        if fun.param_regs.len() != 2 {
            return None;
        }
        let target_param = *fun.param_regs.first()?;
        let len_param = *fun.param_regs.get(1)?;
        let [
            Op::LoadK(lo_reg, zero_const),
            Op::AddIntImm(hi_reg, len_reg, -1),
            Op::CmpIntJmp {
                kind: IntCmpKind::Le,
                a: loop_lhs,
                b: loop_rhs,
                ofs: 11,
            },
            Op::AddInt(sum_reg, sum_lhs, sum_rhs),
            Op::FloorDivImm {
                dst: mid_reg,
                src: mid_src,
                imm: 2,
            },
            Op::MulInt(value_reg, value_lhs, scale_rk),
            Op::CmpIntJmp {
                kind: IntCmpKind::Eq,
                a: eq_lhs,
                b: eq_rhs,
                ofs: 2,
            },
            Op::Ret {
                base: found_base,
                retc: 1,
            },
            Op::CmpIntJmp {
                kind: IntCmpKind::Lt,
                a: lt_lhs,
                b: lt_rhs,
                ofs: 3,
            },
            Op::AddIntImm(next_lo, next_lo_src, 1),
            Op::Jmp(2),
            Op::AddIntImm(next_hi, next_hi_src, -1),
            Op::Jmp(-10),
            Op::LoadK(fail_reg, fail_const),
            Op::Ret {
                base: fail_base,
                retc: 1,
            },
            Op::LoadK(_, nil_const),
            Op::Ret { retc: 1, .. },
        ] = fun.code.as_slice()
        else {
            return None;
        };
        let scale = Self::int_const_for_rk(*scale_rk, fun)?;
        if fun.consts.get(*zero_const as usize) != Some(&Val::Int(0))
            || fun.consts.get(*fail_const as usize) != Some(&Val::Int(-1))
            || !matches!(fun.consts.get(*nil_const as usize), Some(Val::Nil))
            || scale <= 0
            || *len_reg != len_param
            || *loop_lhs != *lo_reg
            || *loop_rhs != *hi_reg
            || *sum_reg == *lo_reg
            || *sum_lhs != *lo_reg
            || *sum_rhs != *hi_reg
            || *mid_src != *sum_reg
            || *value_lhs != *mid_reg
            || *eq_lhs != *value_reg
            || *eq_rhs != target_param
            || *found_base != *mid_reg
            || *lt_lhs != *value_reg
            || *lt_rhs != target_param
            || *next_lo != *lo_reg
            || *next_lo_src != *mid_reg
            || *next_hi != *hi_reg
            || *next_hi_src != *mid_reg
            || *fail_base != *fail_reg
        {
            return None;
        }
        Some(Self::BinarySearchImplicit {
            target: 0,
            len: 1,
            scale,
        })
    }

    fn analyze_binary_search_implicit_bool_branch(fun: &Function) -> Option<Self> {
        if fun.param_regs.len() != 2 {
            return None;
        }
        let target_param = *fun.param_regs.first()?;
        let len_param = *fun.param_regs.get(1)?;
        let [
            Op::LoadK(lo_reg, zero_const),
            Op::AddIntImm(hi_reg, len_reg, -1),
            Op::CmpIntJmp {
                kind: IntCmpKind::Le,
                a: loop_lhs,
                b: loop_rhs,
                ofs: 13,
            },
            Op::AddInt(sum_reg, sum_lhs, sum_rhs),
            Op::FloorDivImm {
                dst: mid_reg,
                src: mid_src,
                imm: 2,
            },
            Op::MulInt(value_reg, value_lhs, scale_rk),
            Op::CmpEq(eq_bool, eq_lhs, eq_rhs),
            Op::BoolBranch(eq_branch, 2),
            Op::Ret {
                base: found_base,
                retc: 1,
            },
            Op::CmpLt(lt_bool, lt_lhs, lt_rhs),
            Op::BoolBranch(lt_branch, 3),
            Op::AddIntImm(next_lo, next_lo_src, 1),
            Op::Jmp(2),
            Op::AddIntImm(next_hi, next_hi_src, -1),
            Op::Jmp(-12),
            Op::LoadK(fail_reg, fail_const),
            Op::Ret {
                base: fail_base,
                retc: 1,
            },
            Op::LoadK(_, nil_const),
            Op::Ret { retc: 1, .. },
        ] = fun.code.as_slice()
        else {
            return None;
        };
        let scale = Self::int_const_for_rk(*scale_rk, fun)?;
        if fun.consts.get(*zero_const as usize) != Some(&Val::Int(0))
            || fun.consts.get(*fail_const as usize) != Some(&Val::Int(-1))
            || !matches!(fun.consts.get(*nil_const as usize), Some(Val::Nil))
            || scale <= 0
            || *len_reg != len_param
            || *loop_lhs != *lo_reg
            || *loop_rhs != *hi_reg
            || *sum_lhs != *lo_reg
            || *sum_rhs != *hi_reg
            || *mid_src != *sum_reg
            || *value_lhs != *mid_reg
            || *eq_lhs != *value_reg
            || *eq_rhs != target_param
            || *eq_branch != *eq_bool
            || *found_base != *mid_reg
            || *lt_lhs != *value_reg
            || *lt_rhs != target_param
            || *lt_branch != *lt_bool
            || *next_lo != *lo_reg
            || *next_lo_src != *mid_reg
            || *next_hi != *hi_reg
            || *next_hi_src != *mid_reg
            || *fail_base != *fail_reg
        {
            return None;
        }
        Some(Self::BinarySearchImplicit {
            target: 0,
            len: 1,
            scale,
        })
    }

    fn analyze_euclid_gcd_loop(fun: &Function, ret_pos: usize, ret_base: u16) -> Option<Self> {
        if fun.param_regs.len() != 2 || ret_pos != 7 {
            return None;
        }
        let lhs_param = *fun.param_regs.first()?;
        let rhs_param = *fun.param_regs.get(1)?;
        let [
            Op::LoadLocal(lhs_work, lhs_local),
            Op::LoadLocal(rhs_work, rhs_local),
            Op::CmpNeImmJmp {
                r: cmp_reg,
                imm: 0,
                ofs: cmp_ofs,
            },
            Op::Mod(rem_reg, mod_lhs, mod_rhs) | Op::ModInt(rem_reg, mod_lhs, mod_rhs),
            Op::Move(move_lhs_dst, move_lhs_src),
            Op::Move(move_rhs_dst, move_rhs_src),
            Op::Jmp(loop_ofs),
        ] = fun.code.get(..7)?
        else {
            return None;
        };
        if *lhs_local == lhs_param
            && *rhs_local == rhs_param
            && *cmp_reg == *rhs_work
            && *cmp_ofs == 5
            && *mod_lhs == *lhs_work
            && *mod_rhs == *rhs_work
            && *move_lhs_dst == *lhs_work
            && *move_lhs_src == *rhs_work
            && *move_rhs_dst == *rhs_work
            && *move_rhs_src == *rem_reg
            && *loop_ofs == -4
            && ret_base == *lhs_work
        {
            Some(Self::EuclidGcd { lhs: 0, rhs: 1 })
        } else {
            None
        }
    }

    fn int_const_for_rk(rk: u16, fun: &Function) -> Option<i64> {
        if !rk_is_const(rk) {
            return None;
        }
        match fun.consts.get(rk_index(rk) as usize)? {
            Val::Int(value) => Some(*value),
            _ => None,
        }
    }

    #[inline]
    fn eval_binary_search_implicit(target: i64, len: i64, scale: i64) -> Option<i64> {
        if scale <= 0 {
            return None;
        }
        if len == i64::MIN {
            return None;
        }
        let mut lo = 0i64;
        let mut hi = len - 1;
        if hi > 0 && (hi > i64::MAX / scale || hi > i64::MAX / 2) {
            return None;
        }
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let value = mid * scale;
            if value == target {
                return Some(mid);
            }
            if value < target {
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        Some(-1)
    }

    #[inline]
    fn eval_is_prime_trial_division(value: i64) -> Option<bool> {
        if value < 2 {
            return Some(false);
        }
        if value == 2 {
            return Some(true);
        }
        if value % 2 == 0 {
            return Some(false);
        }
        const MAX_SAFE_SQUARE_INPUT: i64 = 9_223_372_030_926_249_001;
        if value > MAX_SAFE_SQUARE_INPUT {
            return None;
        }
        let mut divisor = 3i64;
        while divisor <= value / divisor {
            if value % divisor == 0 {
                return Some(false);
            }
            divisor = divisor.checked_add(2)?;
        }
        Some(true)
    }

    fn add_mod_plan_for_rk(
        lhs: u16,
        modulo: u16,
        fun: &Function,
        reg_operands: &[Option<TinyOperand>],
    ) -> Option<Self> {
        if rk_is_const(lhs) {
            return None;
        }
        let add_reg = rk_index(lhs);
        let modulo = Self::operand_for_rk(modulo, fun, reg_operands)?;
        Self::add_mod_plan_for_add_reg(add_reg, modulo, fun, reg_operands)
    }

    fn add_mod_plan_for_reg(
        lhs: u16,
        modulo: u16,
        fun: &Function,
        reg_operands: &[Option<TinyOperand>],
    ) -> Option<Self> {
        let modulo = Self::operand_for_reg(modulo, fun, reg_operands)?;
        Self::add_mod_plan_for_add_reg(lhs, modulo, fun, reg_operands)
    }

    fn add_mod_plan_for_add_reg(
        add_reg: u16,
        modulo: TinyOperand,
        fun: &Function,
        reg_operands: &[Option<TinyOperand>],
    ) -> Option<Self> {
        let add_op = fun.code.iter().rev().find(|op| Self::defines_reg(op, add_reg))?;
        match add_op {
            Op::Add(dst, a, b) if *dst == add_reg => Some(Self::AddMod {
                lhs: Self::operand_for_rk(*a, fun, reg_operands)?,
                rhs: Self::operand_for_rk(*b, fun, reg_operands)?,
                modulo,
            }),
            Op::AddInt(dst, a, b) | Op::AddFloat(dst, a, b) if *dst == add_reg => Some(Self::AddMod {
                lhs: Self::operand_for_reg(*a, fun, reg_operands)?,
                rhs: Self::operand_for_reg(*b, fun, reg_operands)?,
                modulo,
            }),
            Op::AddIntImm(dst, src, imm) if *dst == add_reg => Some(Self::AddMod {
                lhs: Self::operand_for_reg(*src, fun, reg_operands)?,
                rhs: TinyOperand::Const(Val::Int(*imm as i64)),
                modulo,
            }),
            _ => None,
        }
    }

    fn operand_for_rk(rk: u16, fun: &Function, reg_operands: &[Option<TinyOperand>]) -> Option<TinyOperand> {
        if rk_is_const(rk) {
            fun.consts.get(rk_index(rk) as usize).cloned().map(TinyOperand::Const)
        } else {
            Self::operand_for_reg(rk_index(rk), fun, reg_operands)
        }
    }

    fn operand_for_reg(reg: u16, fun: &Function, reg_operands: &[Option<TinyOperand>]) -> Option<TinyOperand> {
        if let Some(idx) = fun.param_regs.iter().position(|param_reg| *param_reg == reg) {
            return Some(TinyOperand::Param(idx));
        }
        reg_operands.get(reg as usize)?.clone()
    }

    fn analyze_expr(fun: &Function, ret_pos: usize, base: u16) -> Option<TinyExpr> {
        let mut reg_exprs: Vec<Option<TinyExpr>> = vec![None; fun.n_regs as usize];
        let mut local_exprs: Vec<Option<TinyExpr>> = vec![None; fun.n_regs as usize];
        for op in &fun.code[..ret_pos] {
            match op {
                Op::LoadK(dst, kidx) => {
                    let value = fun.consts.get(*kidx as usize)?.clone();
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Operand(TinyOperand::Const(value)));
                }
                Op::LoadCapture { dst, idx } => {
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Operand(TinyOperand::Capture(*idx as usize)));
                }
                Op::Move(dst, src) => {
                    let expr = Self::expr_for_reg(*src, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(expr);
                }
                Op::LoadLocal(dst, idx) => {
                    let expr = local_exprs.get(*idx as usize)?.clone()?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(expr);
                }
                Op::StoreLocal(idx, src) => {
                    let expr = Self::expr_for_reg(*src, fun, &reg_exprs)?;
                    *local_exprs.get_mut(*idx as usize)? = Some(expr);
                }
                Op::Add(dst, a, b) => {
                    let lhs = Self::expr_for_rk(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_rk(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Add(Box::new(lhs), Box::new(rhs)));
                }
                Op::AddInt(dst, a, b) | Op::AddFloat(dst, a, b) => {
                    let lhs = Self::expr_for_reg(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_reg(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Add(Box::new(lhs), Box::new(rhs)));
                }
                Op::AddIntImm(dst, src, imm) => {
                    let lhs = Self::expr_for_reg(*src, fun, &reg_exprs)?;
                    let rhs = TinyExpr::Operand(TinyOperand::Const(Val::Int(*imm as i64)));
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Add(Box::new(lhs), Box::new(rhs)));
                }
                Op::Sub(dst, a, b) => {
                    let lhs = Self::expr_for_rk(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_rk(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Sub(Box::new(lhs), Box::new(rhs)));
                }
                Op::SubInt(dst, a, b) | Op::SubFloat(dst, a, b) => {
                    let lhs = Self::expr_for_reg(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_reg(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Sub(Box::new(lhs), Box::new(rhs)));
                }
                Op::Mul(dst, a, b) => {
                    let lhs = Self::expr_for_rk(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_rk(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Mul(Box::new(lhs), Box::new(rhs)));
                }
                Op::MulInt(dst, a, b) | Op::MulFloat(dst, a, b) => {
                    let lhs = Self::expr_for_reg(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_reg(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Mul(Box::new(lhs), Box::new(rhs)));
                }
                Op::Mod(dst, a, b) => {
                    let lhs = Self::expr_for_rk(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_rk(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Mod(Box::new(lhs), Box::new(rhs)));
                }
                Op::ModInt(dst, a, b) | Op::ModFloat(dst, a, b) => {
                    let lhs = Self::expr_for_reg(*a, fun, &reg_exprs)?;
                    let rhs = Self::expr_for_reg(*b, fun, &reg_exprs)?;
                    *reg_exprs.get_mut(*dst as usize)? = Some(TinyExpr::Mod(Box::new(lhs), Box::new(rhs)));
                }
                _ => return None,
            }
        }
        Self::expr_for_reg(base, fun, &reg_exprs)
    }

    fn analyze_expr_plan(fun: &Function, ret_pos: usize, base: u16) -> Option<Self> {
        let expr = Self::analyze_expr(fun, ret_pos, base)?;
        TinyIntProgram::compile(&expr)
            .map(Self::IntExpr)
            .or(Some(Self::Expr(expr)))
    }

    fn expr_for_rk(rk: u16, fun: &Function, reg_exprs: &[Option<TinyExpr>]) -> Option<TinyExpr> {
        if rk_is_const(rk) {
            fun.consts
                .get(rk_index(rk) as usize)
                .cloned()
                .map(TinyOperand::Const)
                .map(TinyExpr::Operand)
        } else {
            Self::expr_for_reg(rk_index(rk), fun, reg_exprs)
        }
    }

    fn expr_for_reg(reg: u16, fun: &Function, reg_exprs: &[Option<TinyExpr>]) -> Option<TinyExpr> {
        if let Some(idx) = fun.param_regs.iter().position(|param_reg| *param_reg == reg) {
            return Some(TinyExpr::Operand(TinyOperand::Param(idx)));
        }
        reg_exprs.get(reg as usize)?.clone()
    }

    fn defines_reg(op: &Op, reg: u16) -> bool {
        match op {
            Op::LoadK(dst, _)
            | Op::LoadCapture { dst, .. }
            | Op::Move(dst, _)
            | Op::LoadLocal(dst, _)
            | Op::Add(dst, _, _)
            | Op::AddInt(dst, _, _)
            | Op::AddFloat(dst, _, _)
            | Op::AddIntImm(dst, _, _)
            | Op::Sub(dst, _, _)
            | Op::SubInt(dst, _, _)
            | Op::SubFloat(dst, _, _)
            | Op::Mul(dst, _, _)
            | Op::MulInt(dst, _, _)
            | Op::MulFloat(dst, _, _)
            | Op::Mod(dst, _, _)
            | Op::ModInt(dst, _, _)
            | Op::ModFloat(dst, _, _) => *dst == reg,
            _ => false,
        }
    }

    #[inline]
    fn eval_add(lhs: &Val, rhs: &Val) -> Option<Val> {
        match (lhs, rhs) {
            (Val::Int(a), Val::Int(b)) => Some(Val::Int(a.wrapping_add(*b))),
            (Val::Float(a), Val::Float(b)) => Some(Val::Float(a + b)),
            (Val::Int(a), Val::Float(b)) => Some(Val::Float(*a as f64 + b)),
            (Val::Float(a), Val::Int(b)) => Some(Val::Float(a + *b as f64)),
            (Val::Str(a), Val::Str(b)) => Some(Val::concat_strings(a, b)),
            _ => None,
        }
    }

    #[inline]
    fn eval_mod(lhs: &Val, rhs: &Val) -> Option<Val> {
        match (lhs, rhs) {
            (Val::Int(_), Val::Int(0)) => None,
            (Val::Int(a), Val::Int(b)) => Some(Val::Int(a % b)),
            (Val::Float(_), Val::Float(b)) if *b == 0.0 => None,
            (Val::Float(a), Val::Float(b)) => Some(Val::Float(a % b)),
            _ => None,
        }
    }
}

impl TinyOperand {
    #[inline]
    fn resolve<'a>(&'a self, args: &'a [Val], captures: Option<&'a ClosureCapture>) -> Option<&'a Val> {
        match self {
            Self::Param(idx) => args.get(*idx),
            Self::Capture(idx) => captures?.value_at(*idx),
            Self::Const(value) => Some(value),
        }
    }
}

impl TinyIntProgram {
    const MAX_STACK: usize = 32;

    fn compile(expr: &TinyExpr) -> Option<Self> {
        let mut ops = Vec::new();
        Self::compile_expr(expr, &mut ops)?;
        let max_stack = Self::validate_stack(&ops)?;
        Some(Self { ops, max_stack })
    }

    fn compile_expr(expr: &TinyExpr, ops: &mut Vec<TinyIntOp>) -> Option<()> {
        match expr {
            TinyExpr::Operand(TinyOperand::Param(idx)) => ops.push(TinyIntOp::Param(*idx)),
            TinyExpr::Operand(TinyOperand::Capture(idx)) => ops.push(TinyIntOp::Capture(*idx)),
            TinyExpr::Operand(TinyOperand::Const(Val::Int(value))) => ops.push(TinyIntOp::Const(*value)),
            TinyExpr::Operand(TinyOperand::Const(_)) => return None,
            TinyExpr::Add(lhs, rhs) => {
                Self::compile_expr(lhs, ops)?;
                Self::compile_expr(rhs, ops)?;
                ops.push(TinyIntOp::Add);
            }
            TinyExpr::Sub(lhs, rhs) => {
                Self::compile_expr(lhs, ops)?;
                Self::compile_expr(rhs, ops)?;
                ops.push(TinyIntOp::Sub);
            }
            TinyExpr::Mul(lhs, rhs) => {
                Self::compile_expr(lhs, ops)?;
                Self::compile_expr(rhs, ops)?;
                ops.push(TinyIntOp::Mul);
            }
            TinyExpr::Mod(lhs, rhs) => {
                Self::compile_expr(lhs, ops)?;
                Self::compile_expr(rhs, ops)?;
                ops.push(TinyIntOp::Mod);
            }
        }
        Some(())
    }

    fn validate_stack(ops: &[TinyIntOp]) -> Option<usize> {
        let mut depth = 0usize;
        let mut max_depth = 0usize;
        for op in ops {
            match op {
                TinyIntOp::Param(_) | TinyIntOp::Capture(_) | TinyIntOp::Const(_) => {
                    depth = depth.checked_add(1)?;
                    max_depth = max_depth.max(depth);
                }
                TinyIntOp::Add | TinyIntOp::Sub | TinyIntOp::Mul | TinyIntOp::Mod => {
                    if depth < 2 {
                        return None;
                    }
                    depth -= 1;
                }
            }
        }
        if depth == 1 && max_depth <= Self::MAX_STACK {
            Some(max_depth)
        } else {
            None
        }
    }

    #[inline]
    fn eval(&self, args: &[Val], captures: Option<&ClosureCapture>) -> Option<i64> {
        let mut stack = [0i64; Self::MAX_STACK];
        let mut sp = 0usize;
        debug_assert!(self.max_stack <= Self::MAX_STACK);
        for op in &self.ops {
            match op {
                TinyIntOp::Param(idx) => {
                    let Val::Int(value) = args.get(*idx)? else {
                        return None;
                    };
                    stack[sp] = *value;
                    sp += 1;
                }
                TinyIntOp::Capture(idx) => {
                    let Val::Int(value) = captures?.value_at(*idx)? else {
                        return None;
                    };
                    stack[sp] = *value;
                    sp += 1;
                }
                TinyIntOp::Const(value) => {
                    stack[sp] = *value;
                    sp += 1;
                }
                TinyIntOp::Add => {
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].wrapping_add(stack[sp]);
                }
                TinyIntOp::Sub => {
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].wrapping_sub(stack[sp]);
                }
                TinyIntOp::Mul => {
                    sp -= 1;
                    stack[sp - 1] = stack[sp - 1].wrapping_mul(stack[sp]);
                }
                TinyIntOp::Mod => {
                    sp -= 1;
                    let rhs = stack[sp];
                    if rhs == 0 {
                        return None;
                    }
                    stack[sp - 1] %= rhs;
                }
            }
        }
        (sp == 1).then_some(stack[0])
    }
}

impl TinyExpr {
    #[inline]
    fn eval_int(&self, args: &[Val], captures: Option<&ClosureCapture>) -> Option<i64> {
        match self {
            Self::Operand(operand) => match operand.resolve(args, captures)? {
                Val::Int(value) => Some(*value),
                _ => None,
            },
            Self::Add(lhs, rhs) => Some(
                lhs.eval_int(args, captures)?
                    .wrapping_add(rhs.eval_int(args, captures)?),
            ),
            Self::Sub(lhs, rhs) => Some(
                lhs.eval_int(args, captures)?
                    .wrapping_sub(rhs.eval_int(args, captures)?),
            ),
            Self::Mul(lhs, rhs) => Some(
                lhs.eval_int(args, captures)?
                    .wrapping_mul(rhs.eval_int(args, captures)?),
            ),
            Self::Mod(lhs, rhs) => {
                let rhs = rhs.eval_int(args, captures)?;
                if rhs == 0 {
                    None
                } else {
                    Some(lhs.eval_int(args, captures)? % rhs)
                }
            }
        }
    }

    #[inline]
    fn eval(&self, args: &[Val], captures: Option<&ClosureCapture>) -> Option<Val> {
        match self {
            Self::Operand(operand) => operand.resolve(args, captures).cloned(),
            Self::Add(lhs, rhs) => {
                let lhs = lhs.eval(args, captures)?;
                let rhs = rhs.eval(args, captures)?;
                TinyCallPlan::eval_add(&lhs, &rhs)
            }
            Self::Sub(lhs, rhs) => {
                let lhs = lhs.eval(args, captures)?;
                let rhs = rhs.eval(args, captures)?;
                match (&lhs, &rhs) {
                    (Val::Int(a), Val::Int(b)) => Some(Val::Int(a.wrapping_sub(*b))),
                    (Val::Float(a), Val::Float(b)) => Some(Val::Float(a - b)),
                    (Val::Int(a), Val::Float(b)) => Some(Val::Float(*a as f64 - b)),
                    (Val::Float(a), Val::Int(b)) => Some(Val::Float(a - *b as f64)),
                    _ => None,
                }
            }
            Self::Mul(lhs, rhs) => {
                let lhs = lhs.eval(args, captures)?;
                let rhs = rhs.eval(args, captures)?;
                match (&lhs, &rhs) {
                    (Val::Int(a), Val::Int(b)) => Some(Val::Int(a.wrapping_mul(*b))),
                    (Val::Float(a), Val::Float(b)) => Some(Val::Float(a * b)),
                    (Val::Int(a), Val::Float(b)) => Some(Val::Float(*a as f64 * b)),
                    (Val::Float(a), Val::Int(b)) => Some(Val::Float(a * *b as f64)),
                    _ => None,
                }
            }
            Self::Mod(lhs, rhs) => {
                let lhs = lhs.eval(args, captures)?;
                let rhs = rhs.eval(args, captures)?;
                TinyCallPlan::eval_mod(&lhs, &rhs)
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct ClosureFastCache {
    pub(super) access_ic: Vec<Option<AccessIc>>,
    pub(super) index_ic: Vec<Option<IndexIc>>,
    pub(super) global_ic: Vec<Option<GlobalEntry>>,
    pub(super) call_ic: Vec<Option<CallIc>>,
    pub(super) for_range: Vec<Option<ForRangeState>>,
    pub(super) packed_hot: Vec<Option<PackedHotEntry>>,
    pub(super) packed_hot_key: usize,
    pub(super) quickening: Vec<QuickeningSite>,
    pub(super) prepared_func_key: usize,
    pub(super) prepared_code_len: usize,
    /// Cached region plan — avoids Arc clone per call for closure calls.
    pub(super) region_plan: Option<Arc<RegionPlan>>,
}

impl ClosureFastCache {
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            access_ic: Vec::new(),
            index_ic: Vec::new(),
            global_ic: Vec::new(),
            call_ic: Vec::new(),
            for_range: Vec::new(),
            packed_hot: Vec::new(),
            packed_hot_key: 0,
            quickening: Vec::new(),
            prepared_func_key: 0,
            prepared_code_len: 0,
            region_plan: None,
        }
    }
}

#[derive(Clone)]
pub(super) struct NamedCallPlan {
    pub(super) provided_indices: Arc<[usize]>,
    pub(super) defaults_to_eval: Arc<[usize]>,
    pub(super) optional_nil: Arc<[usize]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ForRangeState {
    pub(super) current: i64,
    pub(super) limit: i64,
    pub(super) step: i64,
    pub(super) inclusive: bool,
    pub(super) positive: bool,
}

impl ForRangeState {
    #[inline(always)]
    pub(super) fn new(current: i64, limit: i64, step: i64, inclusive: bool) -> Self {
        Self {
            current,
            limit,
            step,
            inclusive,
            positive: step > 0,
        }
    }

    #[inline(always)]
    pub(super) fn should_continue(&self) -> bool {
        if self.positive {
            if self.inclusive {
                self.current <= self.limit
            } else {
                self.current < self.limit
            }
        } else if self.inclusive {
            self.current >= self.limit
        } else {
            self.current > self.limit
        }
    }
}

pub(super) struct VmCaches<'a> {
    pub(super) access_ic: &'a mut Vec<Option<AccessIc>>,
    pub(super) index_ic: &'a mut Vec<Option<IndexIc>>,
    pub(super) global_ic: &'a mut Vec<Option<GlobalEntry>>,
    pub(super) call_ic: &'a mut Vec<Option<CallIc>>,
    pub(super) for_range: &'a mut Vec<Option<ForRangeState>>,
    pub(super) packed_hot: &'a mut Vec<Option<PackedHotEntry>>,
    pub(super) quickening: &'a mut Vec<QuickeningSite>,
}
