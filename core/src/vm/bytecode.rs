use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use crate::expr::Pattern;
use crate::val::{ClosureCapture, Val};
use crate::vm::analysis::FunctionAnalysis;
use crate::vm::bc32::Bc32Decoded;
use crate::vm::context::VmContext;

/// Compact bytecode representation and constant pool.
///
/// This module defines the core data structures for the LK VM's bytecode
/// compiler and interpreter:
///
/// - **`Function`**: A compiled function with constant pool, instruction list,
///   register count, closures, and patterns. Can optionally carry a 32-bit
///   packed encoding (`code32`) for BC32 fast-path execution.
///
/// - **`Op`**: The instruction set (~70 variants). Many ops use RK
///   (register/constant) addressing: bit 15 set → constant pool index.
///
/// - **`ClosureProto`**: Captured closure metadata including parameter
///   lists, default value thunks, and compile-on-demand bytecode.
///
/// ## RK Encoding
/// The 16-bit operand space is split:
/// - Bit 15 = `RK_CONST_BIT`: 1 → constant pool, 0 → register
/// - Bits 0-14 = index (register number or constant pool index)
///
/// Fused opcodes (`CmpLtImmJmp`, `AddIntImmJmp`, etc.) combine two
/// instructions into one to reduce dispatch overhead in hot loops.
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
    pub params: Arc<Vec<String>>,
    // Positional parameter type annotations aligned with `params`.
    pub param_types: Arc<Vec<Option<crate::val::Type>>>,
    // Named parameter declarations for named-arg binding
    pub named_params: Arc<Vec<crate::stmt::NamedParamDecl>>,
    // Optional default value thunks for each named parameter (aligned with `named_params`)
    pub default_funcs: Arc<Vec<Option<Function>>>,
    // Optional precompiled nested function (used by VM/LKB). When None, the
    // bytecode compiler will materialize it from `body` on demand.
    pub func: Option<Arc<Function>>,
    // AST body retained for tooling (formatters, doc generators) now that the
    // legacy interpreter has been retired.
    pub body: Arc<crate::stmt::Stmt>,
    /// Captured bindings for this closure prototype.
    pub captures: Arc<Vec<CaptureSpec>>,
    /// Capture names derived from `captures`, shared by every closure instance.
    pub capture_names: Arc<[String]>,
    /// Shared compiled-code cell for all closure instances created from this prototype.
    pub code: Arc<OnceCell<Arc<Function>>>,
    /// Shared empty environment for non-recursive closure instances.
    pub empty_env: Arc<VmContext>,
    /// Shared empty upvalue list for closure instances.
    pub empty_upvalues: Arc<Vec<Val>>,
    /// Shared empty capture set for zero-capture closure instances.
    pub empty_captures: Arc<ClosureCapture>,
    /// Shared closure instance for non-recursive zero-capture closures.
    pub empty_closure: Arc<OnceCell<Val>>,
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

pub fn capture_names_from_specs(captures: &[CaptureSpec]) -> Arc<[String]> {
    captures
        .iter()
        .map(|capture| match capture {
            CaptureSpec::Register { name, .. } | CaptureSpec::Const { name, .. } | CaptureSpec::Global { name } => {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .into()
}

pub fn closure_code_cell(func: Option<&Arc<Function>>) -> Arc<OnceCell<Arc<Function>>> {
    let cell = Arc::new(OnceCell::new());
    if let Some(func) = func {
        let _ = cell.set(Arc::clone(func));
    }
    cell
}

pub fn closure_empty_env() -> Arc<VmContext> {
    Arc::new(VmContext::new_without_core_vm_builtins())
}

pub fn closure_empty_upvalues() -> Arc<Vec<Val>> {
    Arc::new(Vec::new())
}

pub fn closure_empty_captures() -> Arc<ClosureCapture> {
    ClosureCapture::empty()
}

pub fn closure_empty_closure_cell() -> Arc<OnceCell<Val>> {
    Arc::new(OnceCell::new())
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IntCmpKind {
    Eq = 0,
    Ne = 1,
    Lt = 2,
    Le = 3,
    Gt = 4,
    Ge = 5,
}

impl IntCmpKind {
    #[inline]
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Eq,
            1 => Self::Ne,
            2 => Self::Lt,
            3 => Self::Le,
            4 => Self::Gt,
            5 => Self::Ge,
            _ => return None,
        })
    }

    #[inline]
    pub fn eval(self, lhs: i64, rhs: i64) -> bool {
        match self {
            Self::Eq => lhs == rhs,
            Self::Ne => lhs != rhs,
            Self::Lt => lhs < rhs,
            Self::Le => lhs <= rhs,
            Self::Gt => lhs > rhs,
            Self::Ge => lhs >= rhs,
        }
    }
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
    StrConcatKnownCap(u16 /*dst*/, u16 /*a*/, u16 /*b*/),
    StrConcatToStr(u16 /*dst*/, u16 /*lhs*/, u16 /*src*/),
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
    CmpI {
        dst: u16,
        a: u16,
        b: u16,
        kind: IntCmpKind,
    },
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
    ListIndexI(u16 /*dst*/, u16 /*base*/, i16 /*index*/),
    ListSetI {
        dst: u16,
        list: u16,
        index: i16,
        val: u16,
    },
    StrIndexI(u16 /*dst*/, u16 /*base*/, i16 /*index*/),
    // Length and index helpers
    Len {
        dst: u16,
        src: u16,
    },
    ListLen {
        dst: u16,
        src: u16,
    },
    MapLen {
        dst: u16,
        src: u16,
    },
    StrLen {
        dst: u16,
        src: u16,
    },
    // Floor: convert float to int (truncates toward negative infinity)
    Floor {
        dst: u16,
        src: u16,
    },
    // String starts_with with constant prefix
    StartsWithK(u16 /*dst*/, u16 /*src*/, u16 /*kidx*/),
    // String contains with constant needle
    ContainsK(u16 /*dst*/, u16 /*src*/, u16 /*kidx*/),
    // Map key membership test
    MapHas(u16 /*dst*/, u16 /*map*/, u16 /*key*/),
    // Map get with constant interned/string key
    MapGetInterned(u16 /*dst*/, u16 /*map*/, u16 /*kidx*/),
    // Map get with a dynamic string key, bypassing generic access dispatch
    MapGetDynamic(u16 /*dst*/, u16 /*map*/, u16 /*key*/),
    // Map set with constant interned/string key
    MapSetInterned(u16 /*map*/, u16 /*kidx*/, u16 /*val*/),
    // Map key membership test with constant string key
    MapHasK(u16 /*dst*/, u16 /*map*/, u16 /*kidx*/),
    // Fold list values into an accumulator with Add semantics.
    // Semantically equivalent to: for v in list { acc += v }
    ListFoldAdd {
        acc: u16,
        list: u16,
    },
    // Fold map values into an accumulator with Add semantics.
    // Semantically equivalent to: for v in map.values() { acc += v }
    MapValuesFoldAdd {
        acc: u16,
        map: u16,
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
    // Append value to list in-place (Arc::make_mut when possible)
    ListPush {
        list: u16, // list register (must be List type) — mutated in-place via Arc::make_mut
        val: u16,  // value register to append
    },
    // Set a key-value pair in a map register in-place via Arc::make_mut
    MapSet {
        map: u16,
        key: u16,
        val: u16,
    },
    // Set a key-value pair and consume temporary key/value registers.
    MapSetMove {
        map: u16,
        key: u16,
        val: u16,
    },
    MakeClosure {
        dst: u16,
        proto: u16,
    },
    Jmp(i16 /*ofs*/),
    JmpFalse(u16 /*r*/, i16 /*ofs*/),
    BoolBranch(u16 /*r*/, i16 /*ofs*/),
    // Fused: compare r < imm, if false jump by ofs. Saves one dispatch for while loops.
    CmpLtImmJmp {
        r: u16,
        imm: i16,
        ofs: i16,
    },
    // Fused: if r is nil or false, jump by ofs. Saves ToBool + JmpFalse.
    JmpNilOrFalseJmp {
        r: u16,
        ofs: i16,
    },
    // Fused: r += imm, then jump by ofs. Common loop tail pattern.
    AddIntImmJmp {
        r: u16,
        imm: i16,
        ofs: i16,
    },
    // Fused: add `imm * iteration_count(range(idx, limit, step))` to target.
    // Used for ignored range loops whose body is only `target += imm`.
    AddRangeCountImm {
        target: u16,
        idx: u16,
        limit: u16,
        step: u16,
        inclusive: bool,
        explicit: bool,
        imm: i16,
    },
    // Fused: compare src <= imm, if false jump by ofs. Like CmpLtImmJmp but for <=.
    CmpLeImmJmp {
        r: u16,
        imm: i16,
        ofs: i16,
    },
    // Fused: compare src != imm, if false (i.e., src == imm) jump by ofs.
    // Common for while(x != N) loop exit checks.
    CmpNeImmJmp {
        r: u16,
        imm: i16,
        ofs: i16,
    },
    Call {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    // Exact positional call fast path. Rejects named/default fallback semantics.
    CallExact {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    // Exact positional closure call fast path. Non-closure callees are not accepted by this typed op.
    CallClosureExact {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    // Positional native call fast path. Non-native callees fall back to generic Call semantics.
    CallNativeFast {
        f: u16,
        base: u16,
        argc: u8,
        retc: u8,
    },
    // Zero-argument method dispatch fast path. Preserves the generic method
    // helper semantics but skips helper global lookup and empty argument list construction.
    CallMethod0 {
        dst: u16,
        receiver: u16,
        method: u16,
    },
    // Zero-argument method dispatch with receiver loaded from a global/module binding.
    CallGlobalMethod0 {
        dst: u16,
        receiver: u16,
        method: u16,
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
    // Explicit named-argument fallback call. Result is written to base_pos.
    CallNamedFallback {
        f: u16,
        base_pos: u16,
        posc: u8,
        base_named: u16,
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
        write_idx: bool,
        ofs: i16, // jump to end when guard fails
    },
    RangeLoopI {
        idx: u16,
        limit: u16,
        step: u16,
        inclusive: bool,
        write_idx: bool,
        ofs: i16,
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

impl Op {
    pub fn bc32_typed_gate_name(&self) -> Option<&'static str> {
        match self {
            Op::AddInt(..) => Some("AddInt"),
            Op::StrConcatKnownCap(..) => Some("StrConcatKnownCap"),
            Op::StrConcatToStr(..) => Some("StrConcatToStr"),
            Op::AddFloat(..) => Some("AddFloat"),
            Op::AddIntImm(..) => Some("AddIntImm"),
            Op::SubInt(..) => Some("SubInt"),
            Op::SubFloat(..) => Some("SubFloat"),
            Op::MulInt(..) => Some("MulInt"),
            Op::MulFloat(..) => Some("MulFloat"),
            Op::DivFloat(..) => Some("DivFloat"),
            Op::ModInt(..) => Some("ModInt"),
            Op::ModFloat(..) => Some("ModFloat"),
            Op::CmpEqImm(..) => Some("CmpEqImm"),
            Op::CmpNeImm(..) => Some("CmpNeImm"),
            Op::CmpLtImm(..) => Some("CmpLtImm"),
            Op::CmpLeImm(..) => Some("CmpLeImm"),
            Op::CmpGtImm(..) => Some("CmpGtImm"),
            Op::CmpGeImm(..) => Some("CmpGeImm"),
            Op::CmpI { .. } => Some("CmpI"),
            Op::BoolBranch(..) => Some("BoolBranch"),
            Op::AccessK(..) => Some("AccessK"),
            Op::IndexK(..) => Some("IndexK"),
            Op::ListIndexI(..) => Some("ListIndexI"),
            Op::ListSetI { .. } => Some("ListSetI"),
            Op::StrIndexI(..) => Some("StrIndexI"),
            Op::ListLen { .. } => Some("ListLen"),
            Op::MapLen { .. } => Some("MapLen"),
            Op::StrLen { .. } => Some("StrLen"),
            Op::MapGetInterned(..) => Some("MapGetInterned"),
            Op::MapGetDynamic(..) => Some("MapGetDynamic"),
            Op::MapSetInterned(..) => Some("MapSetInterned"),
            Op::Floor { .. } => Some("Floor"),
            Op::StartsWithK(..) => Some("StartsWithK"),
            Op::ContainsK(..) => Some("ContainsK"),
            Op::MapHasK(..) => Some("MapHasK"),
            Op::ListPush { .. } => Some("ListPush"),
            Op::MapSet { .. } => Some("MapSet"),
            Op::MapSetMove { .. } => Some("MapSetMove"),
            Op::CmpLtImmJmp { .. } => Some("CmpLtImmJmp"),
            Op::CmpLeImmJmp { .. } => Some("CmpLeImmJmp"),
            Op::AddIntImmJmp { .. } => Some("AddIntImmJmp"),
            Op::CallExact { .. } => Some("CallExact"),
            Op::CallClosureExact { .. } => Some("CallClosureExact"),
            Op::CallNativeFast { .. } => Some("CallNativeFast"),
            Op::CallMethod0 { .. } => Some("CallMethod0"),
            Op::CallGlobalMethod0 { .. } => Some("CallGlobalMethod0"),
            Op::CallNamedFallback { .. } => Some("CallNamedFallback"),
            Op::ForRangePrep { .. } => Some("ForRangePrep"),
            Op::RangeLoopI { .. } => Some("RangeLoopI"),
            Op::ForRangeStep { .. } => Some("ForRangeStep"),
            Op::LoadK(..)
            | Op::Move(..)
            | Op::Not(..)
            | Op::ToStr(..)
            | Op::ToBool(..)
            | Op::JmpIfNil(..)
            | Op::JmpIfNotNil(..)
            | Op::NullishPick { .. }
            | Op::JmpFalseSet { .. }
            | Op::JmpTrueSet { .. }
            | Op::Add(..)
            | Op::Sub(..)
            | Op::Mul(..)
            | Op::Div(..)
            | Op::Mod(..)
            | Op::CmpEq(..)
            | Op::CmpNe(..)
            | Op::CmpLt(..)
            | Op::CmpLe(..)
            | Op::CmpGt(..)
            | Op::CmpGe(..)
            | Op::In(..)
            | Op::LoadLocal(..)
            | Op::StoreLocal(..)
            | Op::LoadGlobal(..)
            | Op::DefineGlobal(..)
            | Op::LoadCapture { .. }
            | Op::Access(..)
            | Op::Len { .. }
            | Op::MapHas(..)
            | Op::ListFoldAdd { .. }
            | Op::MapValuesFoldAdd { .. }
            | Op::Index { .. }
            | Op::PatternMatch { .. }
            | Op::PatternMatchOrFail { .. }
            | Op::Raise { .. }
            | Op::ToIter { .. }
            | Op::BuildList { .. }
            | Op::BuildMap { .. }
            | Op::ListSlice { .. }
            | Op::MakeClosure { .. }
            | Op::Jmp(..)
            | Op::JmpFalse(..)
            | Op::JmpNilOrFalseJmp { .. }
            | Op::AddRangeCountImm { .. }
            | Op::CmpNeImmJmp { .. }
            | Op::Call { .. }
            | Op::ForRangeLoop { .. }
            | Op::CallNamed { .. }
            | Op::Ret { .. }
            | Op::Break(..)
            | Op::Continue(..) => None,
        }
    }
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
            Op::StrConcatKnownCap(d, a, b) => write!(f, "StrConcatKnownCap r{}, r{}, r{}", d, a, b),
            Op::StrConcatToStr(d, lhs, src) => write!(f, "StrConcatToStr r{}, r{}, r{}", d, lhs, src),
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
            Op::CmpI { dst, a, b, kind } => write!(f, "CmpI.{:?} r{}, r{}, r{}", kind, dst, a, b),
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
            Op::ListIndexI(d, b, i) => write!(f, "ListIndexI r{}, r{}, {}", d, b, i),
            Op::ListSetI { dst, list, index, val } => {
                write!(f, "ListSetI r{}, r{}, {}, r{}", dst, list, index, val)
            }
            Op::StrIndexI(d, b, i) => write!(f, "StrIndexI r{}, r{}, {}", d, b, i),
            Op::Len { dst, src } => write!(f, "Len r{}, r{}", dst, src),
            Op::ListLen { dst, src } => write!(f, "ListLen r{}, r{}", dst, src),
            Op::MapLen { dst, src } => write!(f, "MapLen r{}, r{}", dst, src),
            Op::StrLen { dst, src } => write!(f, "StrLen r{}, r{}", dst, src),
            Op::Floor { dst, src } => write!(f, "Floor r{}, r{}", dst, src),
            Op::StartsWithK(dst, src, kidx) => write!(f, "StartsWithK r{}, r{}, k{}", dst, src, kidx),
            Op::ContainsK(dst, src, kidx) => write!(f, "ContainsK r{}, r{}, k{}", dst, src, kidx),
            Op::MapHas(dst, map, key) => write!(f, "MapHas r{}, r{}, r{}", dst, map, key),
            Op::MapGetInterned(dst, map, kidx) => write!(f, "MapGetInterned r{}, r{}, k{}", dst, map, kidx),
            Op::MapGetDynamic(dst, map, key) => write!(f, "MapGetDynamic r{}, r{}, r{}", dst, map, key),
            Op::MapSetInterned(map, kidx, val) => write!(f, "MapSetInterned r{}, k{}, r{}", map, kidx, val),
            Op::MapHasK(dst, map, kidx) => write!(f, "MapHasK r{}, r{}, k{}", dst, map, kidx),
            Op::ListFoldAdd { acc, list } => write!(f, "ListFoldAdd r{}, r{}", acc, list),
            Op::MapValuesFoldAdd { acc, map } => write!(f, "MapValuesFoldAdd r{}, r{}", acc, map),
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
            Op::ListPush { list, val } => write!(f, "ListPush r{}, r{}", list, val),
            Op::MapSet { map, key, val } => write!(f, "MapSet r{}, r{}, r{}", map, key, val),
            Op::MapSetMove { map, key, val } => write!(f, "MapSetMove r{}, r{}, r{}", map, key, val),
            Op::MakeClosure { dst, proto } => write!(f, "MakeClosure r{}, p{}", dst, proto),
            Op::Jmp(ofs) => write!(f, "Jmp {}", ofs),
            Op::JmpFalse(r, ofs) => write!(f, "JmpFalse r{}, {}", r, ofs),
            Op::BoolBranch(r, ofs) => write!(f, "BoolBranch r{}, {}", r, ofs),
            Op::CmpLtImmJmp { r, imm, ofs } => write!(f, "CmpLtImmJmp r{}, {}, {}", r, imm, ofs),
            Op::JmpNilOrFalseJmp { r, ofs } => write!(f, "JmpNilOrFalseJmp r{}, {}", r, ofs),
            Op::AddIntImmJmp { r, imm, ofs } => write!(f, "AddIntImmJmp r{}, {}, {}", r, imm, ofs),
            Op::AddRangeCountImm {
                target,
                idx,
                limit,
                step,
                inclusive,
                explicit,
                imm,
            } => write!(
                f,
                "AddRangeCountImm target=r{}, idx=r{}, limit=r{}, step=r{}, inclusive={}, explicit={}, imm={}",
                target, idx, limit, step, inclusive, explicit, imm
            ),
            Op::CmpLeImmJmp { r, imm, ofs } => write!(f, "CmpLeImmJmp r{}, {}, {}", r, imm, ofs),
            Op::CmpNeImmJmp { r, imm, ofs } => write!(f, "CmpNeImmJmp r{}, {}, {}", r, imm, ofs),
            Op::Call {
                f: rf,
                base,
                argc,
                retc,
            } => write!(f, "Call r{}, base={}, argc={}, retc={}", rf, base, argc, retc),
            Op::CallExact {
                f: rf,
                base,
                argc,
                retc,
            } => write!(f, "CallExact r{}, base={}, argc={}, retc={}", rf, base, argc, retc),
            Op::CallClosureExact {
                f: rf,
                base,
                argc,
                retc,
            } => write!(
                f,
                "CallClosureExact r{}, base={}, argc={}, retc={}",
                rf, base, argc, retc
            ),
            Op::CallNativeFast {
                f: rf,
                base,
                argc,
                retc,
            } => write!(f, "CallNativeFast r{}, base={}, argc={}, retc={}", rf, base, argc, retc),
            Op::CallMethod0 { dst, receiver, method } => {
                write!(f, "CallMethod0 r{}, receiver={}, method=k{}", dst, receiver, method)
            }
            Op::CallGlobalMethod0 { dst, receiver, method } => write!(
                f,
                "CallGlobalMethod0 r{}, receiver=k{}, method=k{}",
                dst, receiver, method
            ),
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
            Op::CallNamedFallback {
                f: rf,
                base_pos,
                posc,
                base_named,
                namedc,
                retc,
            } => write!(
                f,
                "CallNamedFallback r{}, base_pos={}, posc={}, base_named={}, namedc={}, retc={}",
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
                write_idx,
                ofs,
            } => write!(
                f,
                "ForRangeLoop idx=r{}, limit=r{}, step=r{}, inclusive={}, write_idx={}, ofs={}",
                idx, limit, step, inclusive, write_idx, ofs
            ),
            Op::RangeLoopI {
                idx,
                limit,
                step,
                inclusive,
                write_idx,
                ofs,
            } => write!(
                f,
                "RangeLoopI idx=r{}, limit=r{}, step=r{}, inclusive={}, write_idx={}, ofs={}",
                idx, limit, step, inclusive, write_idx, ofs
            ),
            Op::ForRangeStep { idx, step, back_ofs } => {
                write!(f, "ForRangeStep idx=r{}, step=r{}, back_ofs={}", idx, step, back_ofs)
            }
        }
    }
}
