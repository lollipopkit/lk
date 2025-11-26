use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use crate::expr::Pattern;
use crate::val::Val;
use crate::vm::analysis::FunctionAnalysis;
use crate::vm::bc32::Bc32Decoded;

/// Compact bytecode representation and constant pool.
/// This is a minimal scaffold to unblock incremental VM work.
#[derive(Debug, Clone)]
pub struct Function {
    pub consts: Vec<Val>,
    pub code: Vec<Op>,
    pub n_regs: u16,
    pub protos: Vec<ClosureProto>,
    // Register indices for parameters in the order declared by the closure/function.
    // Empty for expression/statement wrappers that are not functions.
    pub param_regs: Vec<u16>,
    // Register indices aligned with ClosureProto::named_params for named-argument binding.
    pub named_param_regs: Vec<u16>,
    pub named_param_layout: Vec<NamedParamLayoutEntry>,
    pub pattern_plans: Vec<PatternPlan>,
    pub code32: Option<Vec<u32>>, // Optional packed encoding for direct execution
    pub bc32_decoded: Option<Arc<Bc32Decoded>>,
    pub analysis: Option<FunctionAnalysis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedParamLayoutEntry {
    pub name_const_idx: u16,
    pub dest_reg: u16,
    pub default_index: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternBinding {
    pub name: String,
    pub reg: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternPlan {
    pub pattern: Pattern,
    pub bindings: Vec<PatternBinding>,
}

pub const RK_CONST_BIT: u16 = 1 << 15;
pub const RK_INDEX_MASK: u16 = RK_CONST_BIT - 1;

#[inline]
pub fn rk_is_const(rk: u16) -> bool {
    (rk & RK_CONST_BIT) != 0
}

#[inline]
pub fn rk_index(rk: u16) -> u16 {
    rk & RK_INDEX_MASK
}

#[inline]
pub fn rk_as_const(rk: u16) -> u16 {
    debug_assert!(rk_is_const(rk));
    rk & RK_INDEX_MASK
}

#[inline]
pub fn rk_as_reg(rk: u16) -> u16 {
    debug_assert!(!rk_is_const(rk));
    rk & RK_INDEX_MASK
}

#[inline]
pub const fn rk_make_const(kidx: u16) -> u16 {
    kidx | RK_CONST_BIT
}

#[derive(Debug, Clone)]
pub struct ClosureProto {
    /// Optional function name used to self-bind recursive closures.
    pub self_name: Option<String>,
    // Parameter names for arity check and binding
    pub params: Vec<String>,
    // Named parameter declarations for named-arg binding
    pub named_params: Vec<crate::stmt::NamedParamDecl>,
    // Optional default value thunks for each named parameter (aligned with `named_params`)
    pub default_funcs: Vec<Option<Function>>,
    // Optional precompiled nested function (used by VM/LKRB). When None, the
    // bytecode compiler will materialize it from `body` on demand.
    pub func: Option<Box<Function>>,
    // AST body retained for tooling (formatters, doc generators) now that the
    // legacy interpreter has been retired.
    pub body: crate::stmt::Stmt,
    /// Captured bindings for this closure prototype.
    pub captures: Vec<CaptureSpec>,
}

#[derive(Debug, Clone)]
pub enum CaptureSpec {
    /// Capture a local register from the enclosing function.
    Register { name: String, src: u16 },
    /// Capture a compile-time constant by constant-pool index.
    Const { name: String, kidx: u16 },
    /// Capture a global binding by name (looked up when the closure is created).
    Global { name: String },
}

#[derive(Copy, Clone)]
pub enum Op {
    LoadK(u16 /*dst*/, u16 /*kidx*/),
    Move(u16 /*dst*/, u16 /*src*/),
    // Boolean/logic
    Not(u16 /*dst*/, u16 /*src*/),
    // Convert any value to string via Display semantics
    ToStr(u16 /*dst*/, u16 /*src*/),
    // Convert any value to boolean truthiness (only Nil/false are falsey)
    ToBool(u16 /*dst*/, u16 /*src*/),
    // Branch helpers for nil checks
    JmpIfNil(u16 /*r*/, i16 /*ofs*/),
    JmpIfNotNil(u16 /*r*/, i16 /*ofs*/),
    // Nullish coalescing fused branch: if l != nil { dst = l; jmp ofs } else fallthrough
    NullishPick {
        l: u16,
        dst: u16,
        ofs: i16,
    },
    // Boolean short-circuit helpers that also set a boolean result register
    // If r is falsey: set dst=false and jump by ofs; else fallthrough
    JmpFalseSet {
        r: u16,
        dst: u16,
        ofs: i16,
    },
    // If r is truthy: set dst=true and jump by ofs; else fallthrough
    JmpTrueSet {
        r: u16,
        dst: u16,
        ofs: i16,
    },
    // Arithmetic
    Add(u16 /*dst*/, u16 /*a*/, u16 /*b*/),
    Sub(u16, u16, u16),
    Mul(u16, u16, u16),
    Div(u16, u16, u16),
    Mod(u16, u16, u16),
    AddInt(u16, u16, u16),
    AddFloat(u16, u16, u16),
    AddIntImm(u16, u16, i16),
    SubInt(u16, u16, u16),
    SubFloat(u16, u16, u16),
    MulInt(u16, u16, u16),
    MulFloat(u16, u16, u16),
    DivFloat(u16, u16, u16),
    ModInt(u16, u16, u16),
    ModFloat(u16, u16, u16),
    // Comparisons -> Bool
    CmpEq(u16 /*dst*/, u16 /*a*/, u16 /*b*/),
    CmpNe(u16, u16, u16),
    CmpLt(u16, u16, u16),
    CmpLe(u16, u16, u16),
    CmpGt(u16, u16, u16),
    CmpGe(u16, u16, u16),
    CmpEqImm(u16, u16, i16),
    CmpNeImm(u16, u16, i16),
    CmpLtImm(u16, u16, i16),
    CmpLeImm(u16, u16, i16),
    CmpGtImm(u16, u16, i16),
    CmpGeImm(u16, u16, i16),
    // Membership test: dst = (a in b)
    In(u16 /*dst*/, u16 /*a*/, u16 /*b*/),
    // Locals
    LoadLocal(u16 /*dst*/, u16 /*idx*/),
    StoreLocal(u16 /*idx*/, u16 /*src*/),
    // Globals
    LoadGlobal(u16 /*dst*/, u16 /*name_kidx*/),
    DefineGlobal(u16 /*name_kidx*/, u16 /*src*/),
    LoadCapture {
        dst: u16,
        idx: u16,
    },
    // Access and constructors
    Access(u16 /*dst*/, u16 /*base*/, u16 /*field*/),
    // Access with constant string field (avoids allocating/register for field expr)
    AccessK(u16 /*dst*/, u16 /*base*/, u16 /*kidx*/),
    // Index with constant integer (avoids temp registers)
    IndexK(u16 /*dst*/, u16 /*base*/, u16 /*kidx*/),
    // Length and index helpers
    Len {
        dst: u16,
        src: u16,
    },
    Index {
        dst: u16,
        base: u16,
        idx: u16,
    },
    PatternMatch {
        dst: u16,
        src: u16,
        plan: u16,
    },
    PatternMatchOrFail {
        src: u16,
        plan: u16,
        err_kidx: u16,
        is_const: bool,
    },
    Raise {
        err_kidx: u16,
    },
    // Normalize a value into an iterable for for-in loops.
    // - List, Str: passthrough
    // - Map: materialize a stable, sorted list of [key, value] pairs once
    ToIter {
        dst: u16,
        src: u16,
    },
    BuildList {
        dst: u16,
        base: u16,
        len: u16,
    },
    BuildMap {
        dst: u16,
        base: u16,
        len: u16,
    }, // base..base+2*len-1 as k,v pairs
    // List slicing helpers
    ListSlice {
        dst: u16,   // destination register for result list
        src: u16,   // source list register
        start: u16, // start index (inclusive) in register (must be Int)
    },
    MakeClosure {
        dst: u16,
        proto: u16,
    },
    Jmp(i16 /*ofs*/),
    JmpFalse(u16 /*r*/, i16 /*ofs*/),
    Call {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    // Call with named arguments. Result is written to base_pos.
    CallNamed {
        f: u16,
        base_pos: u16,
        posc: u8,
        base_named: u16, // pairs at [base_named + 2*i] = name(Str), [base_named + 2*i + 1] = value
        namedc: u8,
        retc: u8,
    },
    Ret {
        base: u16,
        retc: u8,
    },
    // Numeric for-range (specialized fast path):
    // Usage pattern compiled as:
    //   ForRangePrep { idx, limit, step, inclusive, explicit }
    //   ForRangeLoop { idx, limit, step, inclusive, ofs: end } // jump to end when done
    //   ... body ... (optional: move idx into loop variable before body)
    //   ForRangeStep { idx, step, back_ofs: loop } // idx += step; jump back to loop
    ForRangePrep {
        idx: u16,
        limit: u16,
        step: u16,       // register holding step (+1 or -1)
        inclusive: bool, // ..= vs ..
        explicit: bool,  // if true, keep provided step as-is
    },
    ForRangeLoop {
        idx: u16,
        limit: u16,
        step: u16,
        inclusive: bool,
        ofs: i16, // jump to end when guard fails
    },
    ForRangeStep {
        idx: u16,
        step: u16,
        back_ofs: i16, // jump back to loop
    },
    // Control flow for loops
    Break(i16 /*ofs*/),    // break to loop end by jumping ofs
    Continue(i16 /*ofs*/), // continue to loop head by jumping ofs
}

impl fmt::Debug for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Op::LoadK(d, k) => write!(f, "LoadK r{}, k{}", d, k),
            Op::Move(d, s) => write!(f, "Move r{}, r{}", d, s),
            Op::Not(d, s) => write!(f, "Not r{}, r{}", d, s),
            Op::ToStr(d, s) => write!(f, "ToStr r{}, r{}", d, s),
            Op::ToBool(d, s) => write!(f, "ToBool r{}, r{}", d, s),
            Op::JmpIfNil(r, ofs) => write!(f, "JmpIfNil r{}, {}", r, ofs),
            Op::JmpIfNotNil(r, ofs) => write!(f, "JmpIfNotNil r{}, {}", r, ofs),
            Op::NullishPick { l, dst, ofs } => write!(f, "NullishPick l=r{}, dst=r{}, {}", l, dst, ofs),
            Op::JmpFalseSet { r, dst, ofs } => write!(f, "JmpFalseSet r{}, dst=r{}, {}", r, dst, ofs),
            Op::JmpTrueSet { r, dst, ofs } => write!(f, "JmpTrueSet r{}, dst=r{}, {}", r, dst, ofs),
            Op::Add(d, a, b) => write!(f, "Add r{}, r{}, r{}", d, a, b),
            Op::Sub(d, a, b) => write!(f, "Sub r{}, r{}, r{}", d, a, b),
            Op::Mul(d, a, b) => write!(f, "Mul r{}, r{}, r{}", d, a, b),
            Op::Div(d, a, b) => write!(f, "Div r{}, r{}, r{}", d, a, b),
            Op::Mod(d, a, b) => write!(f, "Mod r{}, r{}, r{}", d, a, b),
            Op::AddInt(d, a, b) => write!(f, "AddInt r{}, r{}, r{}", d, a, b),
            Op::AddFloat(d, a, b) => write!(f, "AddFloat r{}, r{}, r{}", d, a, b),
            Op::AddIntImm(d, a, imm) => write!(f, "AddIntImm r{}, r{}, {}", d, a, imm),
            Op::SubInt(d, a, b) => write!(f, "SubInt r{}, r{}, r{}", d, a, b),
            Op::SubFloat(d, a, b) => write!(f, "SubFloat r{}, r{}, r{}", d, a, b),
            Op::MulInt(d, a, b) => write!(f, "MulInt r{}, r{}, r{}", d, a, b),
            Op::MulFloat(d, a, b) => write!(f, "MulFloat r{}, r{}, r{}", d, a, b),
            Op::DivFloat(d, a, b) => write!(f, "DivFloat r{}, r{}, r{}", d, a, b),
            Op::ModInt(d, a, b) => write!(f, "ModInt r{}, r{}, r{}", d, a, b),
            Op::ModFloat(d, a, b) => write!(f, "ModFloat r{}, r{}, r{}", d, a, b),
            Op::CmpEq(d, a, b) => write!(f, "CmpEq r{}, r{}, r{}", d, a, b),
            Op::CmpNe(d, a, b) => write!(f, "CmpNe r{}, r{}, r{}", d, a, b),
            Op::CmpLt(d, a, b) => write!(f, "CmpLt r{}, r{}, r{}", d, a, b),
            Op::CmpLe(d, a, b) => write!(f, "CmpLe r{}, r{}, r{}", d, a, b),
            Op::CmpGt(d, a, b) => write!(f, "CmpGt r{}, r{}, r{}", d, a, b),
            Op::CmpGe(d, a, b) => write!(f, "CmpGe r{}, r{}, r{}", d, a, b),
            Op::CmpEqImm(d, a, imm) => write!(f, "CmpEqImm r{}, r{}, {}", d, a, imm),
            Op::CmpNeImm(d, a, imm) => write!(f, "CmpNeImm r{}, r{}, {}", d, a, imm),
            Op::CmpLtImm(d, a, imm) => write!(f, "CmpLtImm r{}, r{}, {}", d, a, imm),
            Op::CmpLeImm(d, a, imm) => write!(f, "CmpLeImm r{}, r{}, {}", d, a, imm),
            Op::CmpGtImm(d, a, imm) => write!(f, "CmpGtImm r{}, r{}, {}", d, a, imm),
            Op::CmpGeImm(d, a, imm) => write!(f, "CmpGeImm r{}, r{}, {}", d, a, imm),
            Op::In(d, a, b) => write!(f, "In r{}, r{}, r{}", d, a, b),
            Op::LoadLocal(d, i) => write!(f, "LoadLocal r{}, [{}]", d, i),
            Op::StoreLocal(i, s) => write!(f, "StoreLocal [{}], r{}", i, s),
            Op::LoadGlobal(d, k) => write!(f, "LoadGlobal r{}, k{}", d, k),
            Op::DefineGlobal(k, s) => write!(f, "DefineGlobal k{}, r{}", k, s),
            Op::LoadCapture { dst, idx } => write!(f, "LoadCapture r{}, c{}", dst, idx),
            Op::Access(d, b, fld) => write!(f, "Access r{}, r{}, r{}", d, b, fld),
            Op::AccessK(d, b, k) => write!(f, "AccessK r{}, r{}, k{}", d, b, k),
            Op::IndexK(d, b, k) => write!(f, "IndexK r{}, r{}, k{}", d, b, k),
            Op::Len { dst, src } => write!(f, "Len r{}, r{}", dst, src),
            Op::Index { dst, base, idx } => write!(f, "Index r{}, r{}, r{}", dst, base, idx),
            Op::PatternMatch { dst, src, plan } => write!(f, "PatternMatch r{}, r{}, plan{}", dst, src, plan),
            Op::PatternMatchOrFail {
                src,
                plan,
                err_kidx,
                is_const,
            } => write!(
                f,
                "PatternMatchOrFail r{}, plan{}, k{}, const={}",
                src, plan, err_kidx, is_const
            ),
            Op::Raise { err_kidx } => write!(f, "Raise k{}", err_kidx),
            Op::ToIter { dst, src } => write!(f, "ToIter r{}, r{}", dst, src),
            Op::BuildList { dst, base, len } => {
                write!(f, "BuildList r{}, base={}, len={}", dst, base, len)
            }
            Op::BuildMap { dst, base, len } => {
                write!(f, "BuildMap r{}, base={}, len={}", dst, base, len)
            }
            Op::ListSlice { dst, src, start } => {
                write!(f, "ListSlice r{}, r{}, r{}", dst, src, start)
            }
            Op::MakeClosure { dst, proto } => write!(f, "MakeClosure r{}, p{}", dst, proto),
            Op::Jmp(ofs) => write!(f, "Jmp {}", ofs),
            Op::JmpFalse(r, ofs) => write!(f, "JmpFalse r{}, {}", r, ofs),
            Op::Call {
                f: rf,
                base,
                argc,
                retc,
            } => write!(f, "Call r{}, base={}, argc={}, retc={}", rf, base, argc, retc),
            Op::CallNamed {
                f: rf,
                base_pos,
                posc,
                base_named,
                namedc,
                retc,
            } => write!(
                f,
                "CallNamed r{}, base_pos={}, posc={}, base_named={}, namedc={}, retc={}",
                rf, base_pos, posc, base_named, namedc, retc
            ),
            Op::Ret { base, retc } => write!(f, "Ret base={}, retc={}", base, retc),
            Op::Break(ofs) => write!(f, "Break {}", ofs),
            Op::Continue(ofs) => write!(f, "Continue {}", ofs),
            Op::ForRangePrep {
                idx,
                limit,
                step,
                inclusive,
                explicit,
            } => write!(
                f,
                "ForRangePrep idx=r{}, limit=r{}, step=r{}, inclusive={}, explicit={}",
                idx, limit, step, inclusive, explicit
            ),
            Op::ForRangeLoop {
                idx,
                limit,
                step,
                inclusive,
                ofs,
            } => write!(
                f,
                "ForRangeLoop idx=r{}, limit=r{}, step=r{}, inclusive={}, ofs={}",
                idx, limit, step, inclusive, ofs
            ),
            Op::ForRangeStep { idx, step, back_ofs } => {
                write!(f, "ForRangeStep idx=r{}, step=r{}, back_ofs={}", idx, step, back_ofs)
            }
        }
    }
}
