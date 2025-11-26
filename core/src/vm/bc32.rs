//! 32-bit packed bytecode encoding scaffold.
//!
//! This module provides a compact encoding for a common subset of ops with a
//! simple encoder/decoder. It is intended as a building block for future i-cache
//! density work, not as a full replacement yet.

use super::bytecode::{
    ClosureProto, Function, NamedParamLayoutEntry, Op, PatternPlan, rk_index, rk_is_const, rk_make_const,
};
use crate::val::Val;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::info;

/// Packed function using 32-bit instructions for a subset of ops.
#[derive(Debug, Clone)]
pub struct Bc32Function {
    pub consts: Vec<Val>,
    pub code32: Vec<u32>,
    pub decoded: Option<Arc<Bc32Decoded>>,
    pub n_regs: u16,
    pub protos: Vec<ClosureProto>,
    pub param_regs: Vec<u16>,
    pub named_param_regs: Vec<u16>,
    pub named_param_layout: Vec<NamedParamLayoutEntry>,
    pub pattern_plans: Vec<PatternPlan>,
}

#[derive(Debug, Clone)]
pub struct Bc32Decoded {
    pub instrs: Vec<Bc32DecodedInstr>,
    pub word_to_instr: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Bc32DecodedInstr {
    pub op: Op,
    pub next_pc: usize,
}

impl Bc32Decoded {
    pub fn from_words(code32: &[u32]) -> Option<Self> {
        let mut instrs = Vec::with_capacity(code32.len());
        let mut word_to_instr = vec![u32::MAX; code32.len()];
        let mut pc = 0usize;

        while pc < code32.len() {
            let word = code32[pc];
            let tag = tag_of(word);

            if tag == TAG_REG_EXT {
                pc += 1;
                continue;
            }

            if tag == TAG_EXT {
                return None;
            }

            let mut next = pc + 1;
            let reg_ext_word = if next < code32.len() && tag_of(code32[next]) == TAG_REG_EXT {
                let ext = code32[next];
                next += 1;
                Some(ext)
            } else {
                None
            };
            let (hi_a, hi_b, hi_c) = unpack_reg_ext(reg_ext_word);

            let op = match tag {
                x if x == TAG_FOR_RANGE_PREP => {
                    let idx = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let limit = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let step = combine_reg(hi_c, (word & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let flags = ((w2 >> 16) & 0xFF) as u8;
                    let inclusive = (flags & 1) != 0;
                    let explicit = (flags & 2) != 0;
                    Op::ForRangePrep {
                        idx,
                        limit,
                        step,
                        inclusive,
                        explicit,
                    }
                }
                x if x == TAG_FOR_RANGE_LOOP => {
                    let idx = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let limit = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let step = combine_reg(hi_c, (word & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let flags = ((w2 >> 16) & 0xFF) as u8;
                    let ofs_raw = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    let inclusive = (flags & 1) != 0;
                    Op::ForRangeLoop {
                        idx,
                        limit,
                        step,
                        inclusive,
                        ofs: ofs_raw,
                    }
                }
                x if x == TAG_FOR_RANGE_STEP => {
                    let idx = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let step = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let back_ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::ForRangeStep { idx, step, back_ofs }
                }
                x if x == TAG_JMP_FALSE_SET_X => {
                    let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let dst = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::JmpFalseSet { r, dst, ofs }
                }
                x if x == TAG_JMP_TRUE_SET_X => {
                    let r = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let dst = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::JmpTrueSet { r, dst, ofs }
                }
                x if x == TAG_NULLISH_PICK_X => {
                    let left = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let dst = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                    Op::NullishPick { l: left, dst, ofs }
                }
                x if x == TAG_CALL_X => {
                    let f = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let base = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let retc = (word & 0xFF) as u8;
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let argc = ((w2 >> 16) & 0xFF) as u8;
                    Op::Call { f, base, argc, retc }
                }
                x if x == TAG_CALL_NAMED_X => {
                    let f = combine_reg(hi_a, ((word >> 16) & 0xFF) as u16);
                    let base_pos = combine_reg(hi_b, ((word >> 8) & 0xFF) as u16);
                    let base_named = combine_reg(hi_c, (word & 0xFF) as u16);
                    let w2 = match code32.get(next) {
                        Some(val) => *val,
                        None => return None,
                    };
                    next += 1;
                    let posc = ((w2 >> 16) & 0xFF) as u8;
                    let namedc = ((w2 >> 8) & 0xFF) as u8;
                    let retc = (w2 & 0xFF) as u8;
                    Op::CallNamed {
                        f,
                        base_pos,
                        posc,
                        base_named,
                        namedc,
                        retc,
                    }
                }
                _ => match decode_tag_byte(tag) {
                    DecodedTag::Regular { tag: base, flags } => {
                        decode_word_with_hi(base, flags, word, (hi_a, hi_b, hi_c))
                    }
                    _ => Op::Jmp(0),
                },
            };

            let instr_idx = instrs.len() as u32;
            if pc < word_to_instr.len() {
                word_to_instr[pc] = instr_idx;
            }
            instrs.push(Bc32DecodedInstr { op, next_pc: next });
            pc = next;
        }

        Some(Self { instrs, word_to_instr })
    }
}

const TRACE_TARGET: &str = "lkr::vm::bc32";

#[derive(Default)]
struct Bc32Metrics {
    attempts: u64,
    packed: u64,
    total_ops: u64,
    total_words: u64,
    fallback_by_reason: HashMap<&'static str, u64>,
    fallback_by_opcode: HashMap<&'static str, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bc32ReasonEntry {
    pub reason: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bc32OpcodeEntry {
    pub opcode: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bc32MetricsSnapshot {
    pub attempts: u64,
    pub packed: u64,
    pub total_ops: u64,
    pub total_words: u64,
    pub fallback_reasons: Vec<Bc32ReasonEntry>,
    pub fallback_opcodes: Vec<Bc32OpcodeEntry>,
}

static METRICS: OnceLock<Mutex<Bc32Metrics>> = OnceLock::new();

fn metrics() -> &'static Mutex<Bc32Metrics> {
    METRICS.get_or_init(|| Mutex::new(Bc32Metrics::default()))
}

fn record_attempt(ops: usize) {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    guard.attempts += 1;
    guard.total_ops += ops as u64;
}

fn record_success(words: usize) {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    guard.packed += 1;
    guard.total_words += words as u64;
}

fn record_failure(reason: &'static str, opcode: &'static str) {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    *guard.fallback_by_reason.entry(reason).or_default() += 1;
    *guard.fallback_by_opcode.entry(opcode).or_default() += 1;
}

pub fn bc32_metrics_snapshot() -> Bc32MetricsSnapshot {
    let guard = metrics().lock().expect("bc32 metrics poisoned");
    let mut fallback_reasons: Vec<Bc32ReasonEntry> = guard
        .fallback_by_reason
        .iter()
        .map(|(reason, count)| Bc32ReasonEntry {
            reason: (*reason).to_string(),
            count: *count,
        })
        .collect();
    fallback_reasons.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.cmp(&b.reason)));

    let mut fallback_opcodes: Vec<Bc32OpcodeEntry> = guard
        .fallback_by_opcode
        .iter()
        .map(|(opcode, count)| Bc32OpcodeEntry {
            opcode: (*opcode).to_string(),
            count: *count,
        })
        .collect();
    fallback_opcodes.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.opcode.cmp(&b.opcode)));

    Bc32MetricsSnapshot {
        attempts: guard.attempts,
        packed: guard.packed,
        total_ops: guard.total_ops,
        total_words: guard.total_words,
        fallback_reasons,
        fallback_opcodes,
    }
}

pub fn bc32_metrics_reset() {
    let mut guard = metrics().lock().expect("bc32 metrics poisoned");
    *guard = Bc32Metrics::default();
}

#[derive(Debug)]
enum Bc32Reject {
    UnsupportedOpcode {
        opcode: &'static str,
        detail: &'static str,
    },
    OperandOutOfRange {
        opcode: &'static str,
        operand: &'static str,
    },
    BranchTargetOutOfBounds {
        opcode: &'static str,
    },
    EncodingInvariant {
        opcode: &'static str,
        detail: &'static str,
    },
}

impl Bc32Reject {
    fn reason_key(&self) -> &'static str {
        match self {
            Bc32Reject::UnsupportedOpcode { .. } => "unsupported_opcode",
            Bc32Reject::OperandOutOfRange { .. } => "operand_out_of_range",
            Bc32Reject::BranchTargetOutOfBounds { .. } => "branch_target_out_of_bounds",
            Bc32Reject::EncodingInvariant { .. } => "encoding_invariant_violation",
        }
    }

    fn opcode(&self) -> &'static str {
        match self {
            Bc32Reject::UnsupportedOpcode { opcode, .. }
            | Bc32Reject::OperandOutOfRange { opcode, .. }
            | Bc32Reject::BranchTargetOutOfBounds { opcode }
            | Bc32Reject::EncodingInvariant { opcode, .. } => opcode,
        }
    }

    fn detail(&self) -> &'static str {
        match self {
            Bc32Reject::UnsupportedOpcode { detail, .. } | Bc32Reject::EncodingInvariant { detail, .. } => detail,
            Bc32Reject::OperandOutOfRange { operand, .. } => operand,
            Bc32Reject::BranchTargetOutOfBounds { .. } => "",
        }
    }
}

struct PackIssue {
    reason: Bc32Reject,
    op_index: Option<usize>,
}

impl PackIssue {
    fn new(reason: Bc32Reject, op_index: usize) -> Self {
        Self {
            reason,
            op_index: Some(op_index),
        }
    }
}

fn ensure_u8(opcode: &'static str, operand: &'static str, value: u16) -> Result<(), Bc32Reject> {
    if value < 256 {
        Ok(())
    } else {
        Err(Bc32Reject::OperandOutOfRange { opcode, operand })
    }
}

fn ensure_regs_u8(opcode: &'static str, dst: u16, arg1: u16, arg2: u16) -> Result<(), Bc32Reject> {
    ensure_u8(opcode, "dst", dst)?;
    ensure_u8(opcode, "arg1", arg1)?;
    ensure_u8(opcode, "arg2", arg2)?;
    Ok(())
}

fn ensure_rk_u8(opcode: &'static str, operand: &'static str, value: u16) -> Result<(), Bc32Reject> {
    ensure_u8(opcode, operand, rk_index(value))
}

fn ensure_i8_range(opcode: &'static str, operand: &'static str, value: i32) -> Result<(), Bc32Reject> {
    if (-128..=127).contains(&value) {
        Ok(())
    } else {
        Err(Bc32Reject::EncodingInvariant {
            opcode,
            detail: operand,
        })
    }
}

impl Bc32Function {
    /// Decode back to the standard Function format for execution.
    pub fn decode(&self) -> Function {
        // Multi-word aware decode to reconstruct enum Ops, including extended forms.
        let mut code = Vec::with_capacity(self.code32.len());
        let mut pc = 0usize;
        while pc < self.code32.len() {
            let w = self.code32[pc];
            match decode_tag_byte(tag_of(w)) {
                DecodedTag::RegExt => {
                    pc += 1;
                    continue;
                }
                DecodedTag::Ext => break,
                DecodedTag::Regular { tag, flags } => {
                    let mut next = pc + 1;
                    let reg_ext_word = if next < self.code32.len() && tag_of(self.code32[next]) == TAG_REG_EXT {
                        let ext = Some(self.code32[next]);
                        next += 1;
                        ext
                    } else {
                        None
                    };
                    let (hi_a, hi_b, hi_c) = unpack_reg_ext(reg_ext_word);
                    match tag {
                        Tag::ForRangePrep => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let a = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let b = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let c = combine_reg(hi_c, (w & 0xFF) as u16);
                            let w2 = self.code32[next];
                            next += 1;
                            let flag_word = ((w2 >> 16) & 0xFF) as u8;
                            let inclusive = (flag_word & 1) != 0;
                            let explicit = (flag_word & 2) != 0;
                            code.push(Op::ForRangePrep {
                                idx: a,
                                limit: b,
                                step: c,
                                inclusive,
                                explicit,
                            });
                            pc = next;
                        }
                        Tag::ForRangeLoop => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let a = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let b = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let c = combine_reg(hi_c, (w & 0xFF) as u16);
                            let w2 = self.code32[next];
                            next += 1;
                            let inclusive = (((w2 >> 16) & 0xFF) as u8 & 1) != 0;
                            let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                            code.push(Op::ForRangeLoop {
                                idx: a,
                                limit: b,
                                step: c,
                                inclusive,
                                ofs,
                            });
                            pc = next;
                        }
                        Tag::ForRangeStep => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let a = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let b = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let w2 = self.code32[next];
                            next += 1;
                            let back_ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                            code.push(Op::ForRangeStep {
                                idx: a,
                                step: b,
                                back_ofs,
                            });
                            pc = next;
                        }
                        Tag::JmpFalseSetX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let r = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let dst = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let w2 = self.code32[next];
                            next += 1;
                            let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                            code.push(Op::JmpFalseSet { r, dst, ofs });
                            pc = next;
                        }
                        Tag::JmpTrueSetX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let r = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let dst = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let w2 = self.code32[next];
                            next += 1;
                            let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                            code.push(Op::JmpTrueSet { r, dst, ofs });
                            pc = next;
                        }
                        Tag::NullishPickX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let l = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let dst = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let w2 = self.code32[next];
                            next += 1;
                            let ofs = (((((w2 >> 8) & 0xFF) as u16) << 8) | ((w2 & 0xFF) as u16)) as i16;
                            code.push(Op::NullishPick { l, dst, ofs });
                            pc = next;
                        }
                        Tag::CallX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let f_reg = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let base = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let retc = (w & 0xFF) as u8;
                            let w2 = self.code32[next];
                            next += 1;
                            let argc = ((w2 >> 16) & 0xFF) as u8;
                            code.push(Op::Call {
                                f: f_reg,
                                base,
                                argc,
                                retc,
                            });
                            pc = next;
                        }
                        Tag::CallNamedX => {
                            if next >= self.code32.len() {
                                break;
                            }
                            let f_reg = combine_reg(hi_a, ((w >> 16) & 0xFF) as u16);
                            let base_pos = combine_reg(hi_b, ((w >> 8) & 0xFF) as u16);
                            let base_named = combine_reg(hi_c, (w & 0xFF) as u16);
                            let w2 = self.code32[next];
                            next += 1;
                            let posc = ((w2 >> 16) & 0xFF) as u8;
                            let namedc = ((w2 >> 8) & 0xFF) as u8;
                            let retc = (w2 & 0xFF) as u8;
                            code.push(Op::CallNamed {
                                f: f_reg,
                                base_pos,
                                posc,
                                base_named,
                                namedc,
                                retc,
                            });
                            pc = next;
                        }
                        _ => {
                            let op = decode_word_with_hi(tag, flags, w, (hi_a, hi_b, hi_c));
                            code.push(op);
                            pc = next;
                        }
                    }
                }
            }
        }
        Function {
            consts: self.consts.clone(),
            code,
            n_regs: self.n_regs,
            protos: self.protos.clone(),
            param_regs: self.param_regs.clone(),
            named_param_regs: self.named_param_regs.clone(),
            named_param_layout: self.named_param_layout.clone(),
            pattern_plans: self.pattern_plans.clone(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }
}

// Common 8-bit tags for encodable ops. Layout: [tag:8 | a:8 | b:8 | c:8]
const RAW_TAG_EXT: u8 = 0xFF;
const RAW_TAG_REG_EXT: u8 = 0xFE;
const TAG_FLAG_MASK: u8 = 0x03;
const TAG_FLAG_SHIFT: u8 = 2;
const RK_FLAG_B: u8 = 0x01;
const RK_FLAG_C: u8 = 0x02;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tag {
    Move = 0,
    LoadK,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    AddIntImm,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    CmpEqImm,
    CmpNeImm,
    CmpLtImm,
    CmpLeImm,
    CmpGtImm,
    CmpGeImm,
    Jmp,
    JmpFalse,
    ToBool,
    Not,
    Len,
    Index,
    ToStr,
    JmpIfNil,
    JmpIfNotNil,
    NullishPick,
    Ret,
    LoadGlobal,
    DefineGlobal,
    Access,
    AccessK,
    IndexK,
    LoadLocal,
    StoreLocal,
    Call,
    LoadCapture,
    JmpFalseSet,
    JmpTrueSet,
    ListSlice,
    JmpFalseSetX,
    JmpTrueSetX,
    NullishPickX,
    ForRangePrep,
    ForRangeLoop,
    ForRangeStep,
    Break,
    Continue,
    CallX,
    PatternMatch,
    PatternMatchOrFail,
    PatternMatchOrFailConst,
    BuildList,
    BuildMap,
    MakeClosure,
    CallNamedX,
}

impl Tag {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Tag::Move,
            1 => Tag::LoadK,
            2 => Tag::Add,
            3 => Tag::Sub,
            4 => Tag::Mul,
            5 => Tag::Div,
            6 => Tag::Mod,
            7 => Tag::AddIntImm,
            8 => Tag::Eq,
            9 => Tag::Ne,
            10 => Tag::Lt,
            11 => Tag::Le,
            12 => Tag::Gt,
            13 => Tag::Ge,
            14 => Tag::CmpEqImm,
            15 => Tag::CmpNeImm,
            16 => Tag::CmpLtImm,
            17 => Tag::CmpLeImm,
            18 => Tag::CmpGtImm,
            19 => Tag::CmpGeImm,
            20 => Tag::Jmp,
            21 => Tag::JmpFalse,
            22 => Tag::ToBool,
            23 => Tag::Not,
            24 => Tag::Len,
            25 => Tag::Index,
            26 => Tag::ToStr,
            27 => Tag::JmpIfNil,
            28 => Tag::JmpIfNotNil,
            29 => Tag::NullishPick,
            30 => Tag::Ret,
            31 => Tag::LoadGlobal,
            32 => Tag::DefineGlobal,
            33 => Tag::Access,
            34 => Tag::AccessK,
            35 => Tag::IndexK,
            36 => Tag::LoadLocal,
            37 => Tag::StoreLocal,
            38 => Tag::Call,
            39 => Tag::LoadCapture,
            40 => Tag::JmpFalseSet,
            41 => Tag::JmpTrueSet,
            42 => Tag::ListSlice,
            43 => Tag::JmpFalseSetX,
            44 => Tag::JmpTrueSetX,
            45 => Tag::NullishPickX,
            46 => Tag::ForRangePrep,
            47 => Tag::ForRangeLoop,
            48 => Tag::ForRangeStep,
            49 => Tag::Break,
            50 => Tag::Continue,
            51 => Tag::CallX,
            52 => Tag::PatternMatch,
            53 => Tag::PatternMatchOrFail,
            54 => Tag::PatternMatchOrFailConst,
            55 => Tag::BuildList,
            56 => Tag::BuildMap,
            57 => Tag::MakeClosure,
            58 => Tag::CallNamedX,
            _ => return None,
        })
    }
}

#[inline]
const fn encode_tag_raw(tag: Tag) -> u8 {
    (tag as u8) << TAG_FLAG_SHIFT
}

#[inline]
const fn encode_tag_with_flags(tag: Tag, flags: u8) -> u8 {
    encode_tag_raw(tag) | (flags & TAG_FLAG_MASK)
}

pub(crate) enum DecodedTag {
    Regular { tag: Tag, flags: u8 },
    RegExt,
    Ext,
}

#[inline]
pub(crate) fn decode_tag_byte(byte: u8) -> DecodedTag {
    if byte == RAW_TAG_REG_EXT {
        DecodedTag::RegExt
    } else if byte == RAW_TAG_EXT {
        DecodedTag::Ext
    } else {
        let base = byte >> TAG_FLAG_SHIFT;
        let flags = byte & TAG_FLAG_MASK;
        if let Some(tag) = Tag::from_u8(base) {
            DecodedTag::Regular { tag, flags }
        } else {
            DecodedTag::Ext
        }
    }
}

#[inline]
fn pack(tag: Tag, flags: u8, a: u8, b: u8, c: u8) -> u32 {
    ((encode_tag_with_flags(tag, flags) as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8) | (c as u32)
}

#[inline]
fn pack_reg_ext_bits(a: u16, b: u16, c: u16) -> Option<u32> {
    let hi_a = (a >> 8) as u8;
    let hi_b = (b >> 8) as u8;
    let hi_c = (c >> 8) as u8;
    if hi_a == 0 && hi_b == 0 && hi_c == 0 {
        None
    } else {
        Some(((RAW_TAG_REG_EXT as u32) << 24) | ((hi_a as u32) << 16) | ((hi_b as u32) << 8) | (hi_c as u32))
    }
}

#[inline]
fn pack_ext_word(a: u8, b: u8, c: u8) -> u32 {
    ((RAW_TAG_EXT as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8) | (c as u32)
}

#[inline]
pub(crate) fn unpack_reg_ext(word: Option<u32>) -> (u16, u16, u16) {
    if let Some(ext) = word {
        let hi_a = ((ext >> 16) & 0xFF) as u16;
        let hi_b = ((ext >> 8) & 0xFF) as u16;
        let hi_c = (ext & 0xFF) as u16;
        (hi_a, hi_b, hi_c)
    } else {
        (0, 0, 0)
    }
}

#[inline]
pub(crate) fn combine_reg(hi: u16, lo: u16) -> u16 {
    (hi << 8) | (lo & 0xFF)
}

#[inline]
fn combine_rk(hi: u16, lo: u16, is_const: bool) -> u16 {
    let value = combine_reg(hi, lo);
    if is_const { rk_make_const(value) } else { value }
}

#[inline]
fn encode_i16(x: i16) -> (u8, u8) {
    (((x as u16) >> 8) as u8, (x as u8))
}

#[derive(Clone, Copy)]
struct EncodedOp {
    word: u32,
    reg_ext: Option<u32>,
}

impl EncodedOp {
    fn new(word: u32, reg_ext: Option<u32>) -> Self {
        Self { word, reg_ext }
    }

    fn len(&self) -> usize {
        if self.reg_ext.is_some() { 2 } else { 1 }
    }

    fn emit(self, out: &mut Vec<u32>) {
        out.push(self.word);
        if let Some(ext) = self.reg_ext {
            out.push(ext);
        }
    }
}

#[allow(unreachable_patterns)]
fn opcode_name(op: &Op) -> &'static str {
    match op {
        Op::LoadK(..) => "LoadK",
        Op::Move(..) => "Move",
        Op::Not(..) => "Not",
        Op::ToStr(..) => "ToStr",
        Op::ToBool(..) => "ToBool",
        Op::JmpIfNil(..) => "JmpIfNil",
        Op::JmpIfNotNil(..) => "JmpIfNotNil",
        Op::NullishPick { .. } => "NullishPick",
        Op::JmpFalseSet { .. } => "JmpFalseSet",
        Op::JmpTrueSet { .. } => "JmpTrueSet",
        Op::Add(..) => "Add",
        Op::AddInt(..) => "AddInt",
        Op::AddFloat(..) => "AddFloat",
        Op::AddIntImm(..) => "AddIntImm",
        Op::Sub(..) => "Sub",
        Op::SubInt(..) => "SubInt",
        Op::SubFloat(..) => "SubFloat",
        Op::Mul(..) => "Mul",
        Op::MulInt(..) => "MulInt",
        Op::MulFloat(..) => "MulFloat",
        Op::Div(..) => "Div",
        Op::DivFloat(..) => "DivFloat",
        Op::Mod(..) => "Mod",
        Op::ModInt(..) => "ModInt",
        Op::ModFloat(..) => "ModFloat",
        Op::CmpEq(..) => "CmpEq",
        Op::CmpNe(..) => "CmpNe",
        Op::CmpLt(..) => "CmpLt",
        Op::CmpLe(..) => "CmpLe",
        Op::CmpGt(..) => "CmpGt",
        Op::CmpGe(..) => "CmpGe",
        Op::CmpEqImm(..) => "CmpEqImm",
        Op::CmpNeImm(..) => "CmpNeImm",
        Op::CmpLtImm(..) => "CmpLtImm",
        Op::CmpLeImm(..) => "CmpLeImm",
        Op::CmpGtImm(..) => "CmpGtImm",
        Op::CmpGeImm(..) => "CmpGeImm",
        Op::In(..) => "In",
        Op::LoadLocal(..) => "LoadLocal",
        Op::StoreLocal(..) => "StoreLocal",
        Op::LoadGlobal(..) => "LoadGlobal",
        Op::DefineGlobal(..) => "DefineGlobal",
        Op::LoadCapture { .. } => "LoadCapture",
        Op::Access(..) => "Access",
        Op::AccessK(..) => "AccessK",
        Op::IndexK(..) => "IndexK",
        Op::Len { .. } => "Len",
        Op::Index { .. } => "Index",
        Op::ToIter { .. } => "ToIter",
        Op::BuildList { .. } => "BuildList",
        Op::BuildMap { .. } => "BuildMap",
        Op::ListSlice { .. } => "ListSlice",
        Op::MakeClosure { .. } => "MakeClosure",
        Op::Jmp(..) => "Jmp",
        Op::JmpFalse(..) => "JmpFalse",
        Op::Call { .. } => "Call",
        Op::CallNamed { .. } => "CallNamed",
        Op::Ret { .. } => "Ret",
        Op::ForRangePrep { .. } => "ForRangePrep",
        Op::ForRangeLoop { .. } => "ForRangeLoop",
        Op::ForRangeStep { .. } => "ForRangeStep",
        Op::Break(..) => "Break",
        Op::Continue(..) => "Continue",
        _ => "Unknown",
    }
}

fn encode_op(op: &Op) -> Result<EncodedOp, Bc32Reject> {
    match *op {
        Op::Move(d, s) => {
            ensure_regs_u8("Move", d, s, 0)?;
            let word = pack(Tag::Move, 0, d as u8, s as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::LoadK(d, k) => {
            ensure_regs_u8("LoadK", d, 0, 0)?;
            ensure_u8("LoadK", "const", k)?;
            let word = pack(Tag::LoadK, 0, d as u8, k as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::Add(d, a, b) | Op::AddInt(d, a, b) | Op::AddFloat(d, a, b) => {
            ensure_u8("Add", "dst", d)?;
            ensure_rk_u8("Add", "lhs", a)?;
            ensure_rk_u8("Add", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Add, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::AddIntImm(d, a, imm) => {
            ensure_regs_u8("AddIntImm", d, a, 0)?;
            ensure_i8_range("AddIntImm", "imm", imm as i32)?;
            let word = pack(Tag::AddIntImm, 0, d as u8, a as u8, (imm as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::Sub(d, a, b) | Op::SubInt(d, a, b) | Op::SubFloat(d, a, b) => {
            ensure_u8("Sub", "dst", d)?;
            ensure_rk_u8("Sub", "lhs", a)?;
            ensure_rk_u8("Sub", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Sub, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Mul(d, a, b) | Op::MulInt(d, a, b) | Op::MulFloat(d, a, b) => {
            ensure_u8("Mul", "dst", d)?;
            ensure_rk_u8("Mul", "lhs", a)?;
            ensure_rk_u8("Mul", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Mul, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Div(d, a, b) | Op::DivFloat(d, a, b) => {
            ensure_u8("Div", "dst", d)?;
            ensure_rk_u8("Div", "lhs", a)?;
            ensure_rk_u8("Div", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Div, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::Mod(d, a, b) | Op::ModInt(d, a, b) | Op::ModFloat(d, a, b) => {
            ensure_u8("Mod", "dst", d)?;
            ensure_rk_u8("Mod", "lhs", a)?;
            ensure_rk_u8("Mod", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Mod, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpEq(d, a, b) => {
            ensure_u8("CmpEq", "dst", d)?;
            ensure_rk_u8("CmpEq", "lhs", a)?;
            ensure_rk_u8("CmpEq", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Eq, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpNe(d, a, b) => {
            ensure_u8("CmpNe", "dst", d)?;
            ensure_rk_u8("CmpNe", "lhs", a)?;
            ensure_rk_u8("CmpNe", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Ne, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpLt(d, a, b) => {
            ensure_u8("CmpLt", "dst", d)?;
            ensure_rk_u8("CmpLt", "lhs", a)?;
            ensure_rk_u8("CmpLt", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Lt, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpLe(d, a, b) => {
            ensure_u8("CmpLe", "dst", d)?;
            ensure_rk_u8("CmpLe", "lhs", a)?;
            ensure_rk_u8("CmpLe", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Le, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpGt(d, a, b) => {
            ensure_u8("CmpGt", "dst", d)?;
            ensure_rk_u8("CmpGt", "lhs", a)?;
            ensure_rk_u8("CmpGt", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Gt, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpGe(d, a, b) => {
            ensure_u8("CmpGe", "dst", d)?;
            ensure_rk_u8("CmpGe", "lhs", a)?;
            ensure_rk_u8("CmpGe", "rhs", b)?;
            let flags = (if rk_is_const(a) { RK_FLAG_B } else { 0 }) | (if rk_is_const(b) { RK_FLAG_C } else { 0 });
            let word = pack(Tag::Ge, flags, d as u8, rk_index(a) as u8, rk_index(b) as u8);
            let reg_ext = pack_reg_ext_bits(d, rk_index(a), rk_index(b));
            Ok(EncodedOp::new(word, reg_ext))
        }
        Op::CmpEqImm(d, a, imm) => {
            ensure_regs_u8("CmpEqImm", d, a, 0)?;
            ensure_i8_range("CmpEqImm", "imm", imm as i32)?;
            let word = pack(Tag::CmpEqImm, 0, d as u8, a as u8, (imm as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::CmpNeImm(d, a, imm) => {
            ensure_regs_u8("CmpNeImm", d, a, 0)?;
            ensure_i8_range("CmpNeImm", "imm", imm as i32)?;
            let word = pack(Tag::CmpNeImm, 0, d as u8, a as u8, (imm as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::CmpLtImm(d, a, imm) => {
            ensure_regs_u8("CmpLtImm", d, a, 0)?;
            ensure_i8_range("CmpLtImm", "imm", imm as i32)?;
            let word = pack(Tag::CmpLtImm, 0, d as u8, a as u8, (imm as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::CmpLeImm(d, a, imm) => {
            ensure_regs_u8("CmpLeImm", d, a, 0)?;
            ensure_i8_range("CmpLeImm", "imm", imm as i32)?;
            let word = pack(Tag::CmpLeImm, 0, d as u8, a as u8, (imm as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::CmpGtImm(d, a, imm) => {
            ensure_regs_u8("CmpGtImm", d, a, 0)?;
            ensure_i8_range("CmpGtImm", "imm", imm as i32)?;
            let word = pack(Tag::CmpGtImm, 0, d as u8, a as u8, (imm as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::CmpGeImm(d, a, imm) => {
            ensure_regs_u8("CmpGeImm", d, a, 0)?;
            ensure_i8_range("CmpGeImm", "imm", imm as i32)?;
            let word = pack(Tag::CmpGeImm, 0, d as u8, a as u8, (imm as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::Jmp(ofs) => Ok(EncodedOp::new(
            ((encode_tag_with_flags(Tag::Jmp, 0) as u32) << 24) | (ofs as i32 as u32 & 0x00FF_FFFF),
            None,
        )),
        Op::JmpFalse(r, ofs) => {
            ensure_regs_u8("JmpFalse", r, 0, 0)?;
            let (hi, lo) = encode_i16(ofs);
            let word = pack(Tag::JmpFalse, 0, r as u8, hi, lo);
            Ok(EncodedOp::new(word, None))
        }
        Op::ToBool(d, s) => {
            ensure_regs_u8("ToBool", d, s, 0)?;
            let word = pack(Tag::ToBool, 0, d as u8, s as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::ToStr(d, s) => {
            ensure_regs_u8("ToStr", d, s, 0)?;
            let word = pack(Tag::ToStr, 0, d as u8, s as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::Not(d, s) => {
            ensure_regs_u8("Not", d, s, 0)?;
            let word = pack(Tag::Not, 0, d as u8, s as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::Len { dst, src } => {
            ensure_regs_u8("Len", dst, src, 0)?;
            let word = pack(Tag::Len, 0, dst as u8, src as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::Index { dst, base, idx } => {
            ensure_regs_u8("Index", dst, base, idx)?;
            let word = pack(Tag::Index, 0, dst as u8, base as u8, idx as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::JmpIfNil(r, ofs) => {
            ensure_regs_u8("JmpIfNil", r, 0, 0)?;
            let (hi, lo) = encode_i16(ofs);
            let word = pack(Tag::JmpIfNil, 0, r as u8, hi, lo);
            Ok(EncodedOp::new(word, None))
        }
        Op::JmpIfNotNil(r, ofs) => {
            ensure_regs_u8("JmpIfNotNil", r, 0, 0)?;
            let (hi, lo) = encode_i16(ofs);
            let word = pack(Tag::JmpIfNotNil, 0, r as u8, hi, lo);
            Ok(EncodedOp::new(word, None))
        }
        Op::NullishPick { l, dst, ofs } => {
            ensure_regs_u8("NullishPick", l, dst, 0)?;
            ensure_i8_range("NullishPick", "ofs", ofs as i32)?;
            let word = pack(Tag::NullishPick, 0, l as u8, dst as u8, (ofs as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::Ret { base, retc } => {
            ensure_regs_u8("Ret", base, 0, 0)?;
            let word = pack(Tag::Ret, 0, base as u8, retc, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::LoadGlobal(dst, k) => {
            ensure_regs_u8("LoadGlobal", dst, 0, 0)?;
            ensure_u8("LoadGlobal", "const", k)?;
            let word = pack(Tag::LoadGlobal, 0, dst as u8, k as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::DefineGlobal(k, src) => {
            ensure_regs_u8("DefineGlobal", src, 0, 0)?;
            ensure_u8("DefineGlobal", "name", k)?;
            let word = pack(Tag::DefineGlobal, 0, k as u8, src as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::Access(d, b, f) => {
            ensure_regs_u8("Access", d, b, f)?;
            let word = pack(Tag::Access, 0, d as u8, b as u8, f as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::AccessK(d, b, k) => {
            ensure_regs_u8("AccessK", d, b, 0)?;
            ensure_u8("AccessK", "key", k)?;
            let word = pack(Tag::AccessK, 0, d as u8, b as u8, k as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::IndexK(d, b, k) => {
            ensure_regs_u8("IndexK", d, b, 0)?;
            ensure_u8("IndexK", "key", k)?;
            let word = pack(Tag::IndexK, 0, d as u8, b as u8, k as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::LoadLocal(d, i) => {
            ensure_regs_u8("LoadLocal", d, i, 0)?;
            let word = pack(Tag::LoadLocal, 0, d as u8, i as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::StoreLocal(i, s) => {
            ensure_regs_u8("StoreLocal", i, s, 0)?;
            let word = pack(Tag::StoreLocal, 0, i as u8, s as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::Call { f, base, argc, retc } => {
            if retc != 1 {
                return Err(Bc32Reject::UnsupportedOpcode {
                    opcode: "Call",
                    detail: "retc!=1",
                });
            }
            ensure_regs_u8("Call", f, base, 0)?;
            let word = pack(Tag::Call, 0, f as u8, base as u8, argc);
            Ok(EncodedOp::new(word, None))
        }
        Op::LoadCapture { dst, idx } => {
            ensure_regs_u8("LoadCapture", dst, 0, 0)?;
            ensure_u8("LoadCapture", "idx", idx)?;
            let word = pack(Tag::LoadCapture, 0, dst as u8, idx as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::JmpFalseSet { r, dst, ofs } => {
            ensure_i8_range("JmpFalseSet", "ofs", ofs as i32)?;
            ensure_regs_u8("JmpFalseSet", r, dst, 0)?;
            let word = pack(Tag::JmpFalseSet, 0, r as u8, dst as u8, (ofs as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::JmpTrueSet { r, dst, ofs } => {
            ensure_i8_range("JmpTrueSet", "ofs", ofs as i32)?;
            ensure_regs_u8("JmpTrueSet", r, dst, 0)?;
            let word = pack(Tag::JmpTrueSet, 0, r as u8, dst as u8, (ofs as i8) as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::ListSlice { dst, src, start } => {
            ensure_regs_u8("ListSlice", dst, src, start)?;
            let word = pack(Tag::ListSlice, 0, dst as u8, src as u8, start as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::BuildList { dst, base, len } => {
            ensure_regs_u8("BuildList", dst, base, len)?;
            let word = pack(Tag::BuildList, 0, dst as u8, base as u8, len as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::BuildMap { dst, base, len } => {
            ensure_regs_u8("BuildMap", dst, base, len)?;
            let word = pack(Tag::BuildMap, 0, dst as u8, base as u8, len as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::MakeClosure { dst, proto } => {
            ensure_regs_u8("MakeClosure", dst, proto, 0)?;
            let word = pack(Tag::MakeClosure, 0, dst as u8, proto as u8, 0);
            Ok(EncodedOp::new(word, None))
        }
        Op::Break(ofs) => Ok(EncodedOp::new(
            ((encode_tag_with_flags(Tag::Break, 0) as u32) << 24) | (ofs as i32 as u32 & 0x00FF_FFFF),
            None,
        )),
        Op::Continue(ofs) => Ok(EncodedOp::new(
            ((encode_tag_with_flags(Tag::Continue, 0) as u32) << 24) | (ofs as i32 as u32 & 0x00FF_FFFF),
            None,
        )),
        Op::PatternMatch { dst, src, plan } => {
            ensure_regs_u8("PatternMatch", dst, src, plan)?;
            let word = pack(Tag::PatternMatch, 0, dst as u8, src as u8, plan as u8);
            Ok(EncodedOp::new(word, None))
        }
        Op::PatternMatchOrFail {
            src,
            plan,
            err_kidx,
            is_const,
        } => {
            let tag = if is_const {
                Tag::PatternMatchOrFailConst
            } else {
                Tag::PatternMatchOrFail
            };
            ensure_regs_u8("PatternMatchOrFail", src, plan, err_kidx)?;
            let word = pack(tag, 0, src as u8, plan as u8, err_kidx as u8);
            Ok(EncodedOp::new(word, None))
        }
        _ => Err(Bc32Reject::UnsupportedOpcode {
            opcode: opcode_name(op),
            detail: "not_supported",
        }),
    }
}

#[inline]
fn sign_extend_24(x: u32) -> i32 {
    ((x as i32) << 8) >> 8
}

pub(crate) fn decode_word_with_hi(tag: Tag, flags: u8, w: u32, hi: (u16, u16, u16)) -> Op {
    let lo_a = ((w >> 16) & 0xFF) as u16;
    let lo_b = ((w >> 8) & 0xFF) as u16;
    let lo_c = (w & 0xFF) as u16;
    let (hi_a, hi_b, hi_c) = hi;
    let a = combine_reg(hi_a, lo_a);
    let b_reg = combine_reg(hi_b, lo_b);
    let c_reg = combine_reg(hi_c, lo_c);
    let b_rk = combine_rk(hi_b, lo_b, (flags & RK_FLAG_B) != 0);
    let c_rk = combine_rk(hi_c, lo_c, (flags & RK_FLAG_C) != 0);
    match tag {
        Tag::Move => Op::Move(a, b_reg),
        Tag::LoadK => Op::LoadK(a, b_reg),
        Tag::Add => Op::Add(a, b_rk, c_rk),
        Tag::Sub => Op::Sub(a, b_rk, c_rk),
        Tag::Mul => Op::Mul(a, b_rk, c_rk),
        Tag::Div => Op::Div(a, b_rk, c_rk),
        Tag::Mod => Op::Mod(a, b_rk, c_rk),
        Tag::AddIntImm => Op::AddIntImm(a, b_reg, (lo_c as i8) as i16),
        Tag::Eq => Op::CmpEq(a, b_rk, c_rk),
        Tag::Ne => Op::CmpNe(a, b_rk, c_rk),
        Tag::Lt => Op::CmpLt(a, b_rk, c_rk),
        Tag::Le => Op::CmpLe(a, b_rk, c_rk),
        Tag::Gt => Op::CmpGt(a, b_rk, c_rk),
        Tag::Ge => Op::CmpGe(a, b_rk, c_rk),
        Tag::CmpEqImm => Op::CmpEqImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpNeImm => Op::CmpNeImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpLtImm => Op::CmpLtImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpLeImm => Op::CmpLeImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpGtImm => Op::CmpGtImm(a, b_reg, (lo_c as i8) as i16),
        Tag::CmpGeImm => Op::CmpGeImm(a, b_reg, (lo_c as i8) as i16),
        Tag::Jmp => Op::Jmp(sign_extend_24(w) as i16),
        Tag::JmpFalse => Op::JmpFalse(a, ((b_reg << 8) | c_reg) as i16),
        Tag::ToBool => Op::ToBool(a, b_reg),
        Tag::ToStr => Op::ToStr(a, b_reg),
        Tag::Not => Op::Not(a, b_reg),
        Tag::Len => Op::Len { dst: a, src: b_reg },
        Tag::Index => Op::Index {
            dst: a,
            base: b_reg,
            idx: c_reg,
        },
        Tag::JmpIfNil => Op::JmpIfNil(a, ((b_reg << 8) | c_reg) as i16),
        Tag::JmpIfNotNil => Op::JmpIfNotNil(a, ((b_reg << 8) | c_reg) as i16),
        Tag::NullishPick => Op::NullishPick {
            l: a,
            dst: b_reg,
            ofs: (c_reg as i8) as i16,
        },
        Tag::Ret => Op::Ret {
            base: a,
            retc: b_reg as u8,
        },
        Tag::LoadGlobal => Op::LoadGlobal(a, b_reg),
        Tag::DefineGlobal => Op::DefineGlobal(a, b_reg),
        Tag::Access => Op::Access(a, b_reg, c_reg),
        Tag::AccessK => Op::AccessK(a, b_reg, c_reg),
        Tag::IndexK => Op::IndexK(a, b_reg, c_reg),
        Tag::LoadLocal => Op::LoadLocal(a, b_reg),
        Tag::StoreLocal => Op::StoreLocal(a, b_reg),
        Tag::Call => Op::Call {
            f: a,
            base: b_reg,
            argc: c_reg as u8,
            retc: 1,
        },
        Tag::LoadCapture => Op::LoadCapture { dst: a, idx: b_reg },
        Tag::JmpFalseSet => Op::JmpFalseSet {
            r: a,
            dst: b_reg,
            ofs: (c_reg as i8) as i16,
        },
        Tag::JmpTrueSet => Op::JmpTrueSet {
            r: a,
            dst: b_reg,
            ofs: (c_reg as i8) as i16,
        },
        Tag::ListSlice => Op::ListSlice {
            dst: a,
            src: b_reg,
            start: c_reg,
        },
        Tag::BuildList => Op::BuildList {
            dst: a,
            base: b_reg,
            len: c_reg,
        },
        Tag::BuildMap => Op::BuildMap {
            dst: a,
            base: b_reg,
            len: c_reg,
        },
        Tag::MakeClosure => Op::MakeClosure { dst: a, proto: b_reg },
        Tag::Break => Op::Break(sign_extend_24(w) as i16),
        Tag::Continue => Op::Continue(sign_extend_24(w) as i16),
        Tag::PatternMatch => Op::PatternMatch {
            dst: a,
            src: b_reg,
            plan: c_reg,
        },
        Tag::PatternMatchOrFail => Op::PatternMatchOrFail {
            src: a,
            plan: b_reg,
            err_kidx: c_reg,
            is_const: false,
        },
        Tag::PatternMatchOrFailConst => Op::PatternMatchOrFail {
            src: a,
            plan: b_reg,
            err_kidx: c_reg,
            is_const: true,
        },
        _ => Op::Jmp(0),
    }
}

#[allow(dead_code)]
pub(crate) fn decode_word(w: u32) -> Op {
    match decode_tag_byte(tag_of(w)) {
        DecodedTag::Regular { tag, flags } => decode_word_with_hi(tag, flags, w, (0, 0, 0)),
        DecodedTag::RegExt | DecodedTag::Ext => Op::Jmp(0),
    }
}

impl Bc32Function {
    /// Two-pass packing: computes word indices per Op to remap branch offsets and handle multi-word ops.
    fn try_pack(f: &Function) -> Result<Self, PackIssue> {
        let n = f.code.len();
        if n == 0 {
            return Ok(Self {
                consts: f.consts.clone(),
                code32: vec![],
                decoded: None,
                n_regs: f.n_regs,
                protos: f.protos.clone(),
                param_regs: f.param_regs.clone(),
                named_param_regs: f.named_param_regs.clone(),
                named_param_layout: f.named_param_layout.clone(),
                pattern_plans: f.pattern_plans.clone(),
            });
        }
        // Pass 1a: initial word size guess (ForRange* -> 2, others -> 1 if encodable)
        let mut words_per_op: Vec<usize> = vec![1; n];
        for (i, op) in f.code.iter().enumerate() {
            words_per_op[i] = match op {
                Op::ForRangePrep { idx, limit, step, .. } => {
                    let extra = pack_reg_ext_bits(*idx, *limit, *step).is_some() as usize;
                    2 + extra
                }
                Op::ForRangeLoop { idx, limit, step, .. } => {
                    let extra = pack_reg_ext_bits(*idx, *limit, *step).is_some() as usize;
                    2 + extra
                }
                Op::ForRangeStep { idx, step, .. } => {
                    let extra = pack_reg_ext_bits(*idx, *step, 0).is_some() as usize;
                    2 + extra
                }
                // Optimistically 1 for Jmp*Set/NullishPick; we refine offset below. Include reg-ext if needed.
                Op::JmpFalseSet { .. } => 1,
                Op::JmpTrueSet { .. } => 1,
                Op::NullishPick { .. } => 1,
                Op::Call { f, base, retc, .. } if *retc != 1 => 2 + pack_reg_ext_bits(*f, *base, 0).is_some() as usize,
                Op::CallNamed {
                    f,
                    base_pos,
                    base_named,
                    ..
                } => 2 + pack_reg_ext_bits(*f, *base_pos, *base_named).is_some() as usize,
                _ => encode_op(op)
                    .map(|encoded| encoded.len())
                    .map_err(|err| PackIssue::new(err, i))?,
            };
        }
        // Iteratively refine sizes for Jmp*Set to allow i16 extended forms when needed.
        loop {
            let mut changed = false;
            // Prefix sum to map op index -> word index
            let mut pref: Vec<usize> = vec![0; n + 1];
            for i in 0..n {
                pref[i + 1] = pref[i] + words_per_op[i];
            }
            for (i, op) in f.code.iter().enumerate() {
                match *op {
                    Op::JmpFalseSet { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                        let j = j as usize;
                        let wofs = (pref[j] as isize - pref[i] as isize) as i32;
                        let need_two = !(-128..=127).contains(&wofs);
                        let old = words_per_op[i];
                        let new = if need_two { 2 } else { 1 };
                        if new != old {
                            words_per_op[i] = new;
                            changed = true;
                        }
                    }
                    Op::JmpTrueSet { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                        let j = j as usize;
                        let wofs = (pref[j] as isize - pref[i] as isize) as i32;
                        let need_two = !(-128..=127).contains(&wofs);
                        let old = words_per_op[i];
                        let new = if need_two { 2 } else { 1 };
                        if new != old {
                            words_per_op[i] = new;
                            changed = true;
                        }
                    }
                    Op::NullishPick { ofs, .. } => {
                        let j = (i as isize) + ofs as isize;
                        if j < 0 || j as usize >= n {
                            return Err(PackIssue::new(
                                Bc32Reject::BranchTargetOutOfBounds {
                                    opcode: opcode_name(op),
                                },
                                i,
                            ));
                        }
                        let j = j as usize;
                        let wofs = (pref[j] as isize - pref[i] as isize) as i32;
                        let need_two = !(-128..=127).contains(&wofs);
                        let old = words_per_op[i];
                        let new = if need_two { 2 } else { 1 };
                        if new != old {
                            words_per_op[i] = new;
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }
        // Build op->word index map after convergence
        let mut op_to_word: Vec<usize> = vec![0; n];
        let mut acc = 0usize;
        for (i, w) in words_per_op.iter().enumerate() {
            op_to_word[i] = acc;
            acc += *w;
        }
        let total_words = acc;
        // Pass 2: encode with remapped offsets using final mapping
        let mut out: Vec<u32> = Vec::with_capacity(total_words);
        for (i, op) in f.code.iter().enumerate() {
            match op {
                Op::Jmp(ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    out.push(((encode_tag_with_flags(Tag::Jmp, 0) as u32) << 24) | ((wofs as u32) & 0x00FF_FFFF));
                }
                Op::JmpFalse(r, ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let (hi, lo) = ((wofs >> 8) as u8, (wofs & 0xFF) as u8);
                    out.push(pack(Tag::JmpFalse, 0, (*r & 0xFF) as u8, hi, lo));
                }
                Op::JmpIfNil(r, ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let (hi, lo) = ((wofs >> 8) as u8, (wofs & 0xFF) as u8);
                    out.push(pack(Tag::JmpIfNil, 0, (*r & 0xFF) as u8, hi, lo));
                }
                Op::JmpIfNotNil(r, ofs) => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let (hi, lo) = ((wofs >> 8) as u8, (wofs & 0xFF) as u8);
                    out.push(pack(Tag::JmpIfNotNil, 0, (*r & 0xFF) as u8, hi, lo));
                }
                Op::NullishPick { l, dst, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    if (-128..=127).contains(&wofs) {
                        out.push(pack(
                            Tag::NullishPick,
                            0,
                            (*l & 0xFF) as u8,
                            (*dst & 0xFF) as u8,
                            (wofs as i8) as u8,
                        ));
                    } else {
                        let wofs16 = wofs as i16;
                        out.push(pack(Tag::NullishPickX, 0, (*l & 0xFF) as u8, (*dst & 0xFF) as u8, 0));
                        out.push(pack_ext_word(0, (wofs16 >> 8) as u8, (wofs16 & 0xFF) as u8));
                    }
                }
                Op::JmpFalseSet { r, dst, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    if (-128..=127).contains(&wofs) && words_per_op[i] == 1 {
                        out.push(pack(
                            Tag::JmpFalseSet,
                            0,
                            (*r & 0xFF) as u8,
                            (*dst & 0xFF) as u8,
                            (wofs as i8) as u8,
                        ));
                    } else {
                        let wofs16 = wofs as i16;
                        out.push(pack(Tag::JmpFalseSetX, 0, (*r & 0xFF) as u8, (*dst & 0xFF) as u8, 0));
                        out.push(pack_ext_word(0, (wofs16 >> 8) as u8, (wofs16 & 0xFF) as u8));
                    }
                }
                Op::JmpTrueSet { r, dst, ofs } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i32;
                    if (-128..=127).contains(&wofs) && words_per_op[i] == 1 {
                        out.push(pack(
                            Tag::JmpTrueSet,
                            0,
                            (*r & 0xFF) as u8,
                            (*dst & 0xFF) as u8,
                            (wofs as i8) as u8,
                        ));
                    } else {
                        let wofs16 = wofs as i16;
                        out.push(pack(Tag::JmpTrueSetX, 0, (*r & 0xFF) as u8, (*dst & 0xFF) as u8, 0));
                        out.push(pack_ext_word(0, (wofs16 >> 8) as u8, (wofs16 & 0xFF) as u8));
                    }
                }
                Op::ForRangePrep {
                    idx,
                    limit,
                    step,
                    inclusive,
                    explicit,
                } => {
                    let flags = (if *inclusive { 1 } else { 0 }) | (if *explicit { 2 } else { 0 });
                    out.push(pack(Tag::ForRangePrep, 0, *idx as u8, *limit as u8, *step as u8));
                    if let Some(ext) = pack_reg_ext_bits(*idx, *limit, *step) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(flags as u8, 0, 0));
                }
                Op::ForRangeLoop {
                    idx,
                    limit,
                    step,
                    inclusive,
                    ofs,
                } => {
                    let tgt = ((i as isize) + *ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    let flags = if *inclusive { 1 } else { 0 };
                    out.push(pack(Tag::ForRangeLoop, 0, *idx as u8, *limit as u8, *step as u8));
                    if let Some(ext) = pack_reg_ext_bits(*idx, *limit, *step) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(flags as u8, (wofs >> 8) as u8, (wofs & 0xFF) as u8));
                }
                Op::ForRangeStep { idx, step, back_ofs } => {
                    let tgt = ((i as isize) + *back_ofs as isize) as usize;
                    let wofs = (op_to_word[tgt] as isize - op_to_word[i] as isize) as i16;
                    out.push(pack(Tag::ForRangeStep, 0, *idx as u8, *step as u8, 0));
                    if let Some(ext) = pack_reg_ext_bits(*idx, *step, 0) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(0, (wofs >> 8) as u8, (wofs & 0xFF) as u8));
                }
                Op::Call { f, base, argc, retc } => {
                    if *retc == 1 {
                        out.push(pack(Tag::Call, 0, *f as u8, *base as u8, *argc));
                    } else {
                        out.push(pack(Tag::CallX, 0, *f as u8, *base as u8, *retc));
                        if let Some(ext) = pack_reg_ext_bits(*f, *base, 0) {
                            out.push(ext);
                        }
                        out.push(pack_ext_word(*argc, 0, 0));
                    }
                }
                Op::CallNamed {
                    f,
                    base_pos,
                    posc,
                    base_named,
                    namedc,
                    retc,
                } => {
                    out.push(pack(Tag::CallNamedX, 0, *f as u8, *base_pos as u8, *base_named as u8));
                    if let Some(ext) = pack_reg_ext_bits(*f, *base_pos, *base_named) {
                        out.push(ext);
                    }
                    out.push(pack_ext_word(*posc, *namedc, *retc));
                }
                _ => {
                    let encoded = encode_op(op).map_err(|err| PackIssue::new(err, i))?;
                    encoded.emit(&mut out);
                }
            }
        }
        let decoded = Bc32Decoded::from_words(&out).map(Arc::new);

        Ok(Self {
            consts: f.consts.clone(),
            code32: out,
            decoded,
            n_regs: f.n_regs,
            protos: f.protos.clone(),
            param_regs: f.param_regs.clone(),
            named_param_regs: f.named_param_regs.clone(),
            named_param_layout: f.named_param_layout.clone(),
            pattern_plans: f.pattern_plans.clone(),
        })
    }

    pub fn try_from_function(f: &Function) -> Option<Self> {
        record_attempt(f.code.len());
        match Self::try_pack(f) {
            Ok(packed) => {
                record_success(packed.code32.len());
                Some(packed)
            }
            Err(issue) => {
                let PackIssue { reason, op_index } = issue;
                let reason_key = reason.reason_key();
                let opcode = reason.opcode();
                let detail = reason.detail();
                record_failure(reason_key, opcode);
                let op_index_str = op_index.map(|idx| idx.to_string()).unwrap_or_else(|| "n/a".to_string());
                info!(
                    target: TRACE_TARGET,
                    reason = reason_key,
                    opcode = opcode,
                    detail = detail,
                    op_index = %op_index_str,
                    "bc32 packing fallback"
                );
                None
            }
        }
    }
}

/// Utility: expose tag and constants for VM bc32 fast-path
pub(crate) fn tag_of(w: u32) -> u8 {
    ((w >> 24) & 0xFF) as u8
}
pub(crate) const TAG_FOR_RANGE_PREP: u8 = encode_tag_raw(Tag::ForRangePrep);
pub(crate) const TAG_FOR_RANGE_LOOP: u8 = encode_tag_raw(Tag::ForRangeLoop);
pub(crate) const TAG_FOR_RANGE_STEP: u8 = encode_tag_raw(Tag::ForRangeStep);
pub(crate) const TAG_JMP_FALSE_SET_X: u8 = encode_tag_raw(Tag::JmpFalseSetX);
pub(crate) const TAG_JMP_TRUE_SET_X: u8 = encode_tag_raw(Tag::JmpTrueSetX);
pub(crate) const TAG_NULLISH_PICK_X: u8 = encode_tag_raw(Tag::NullishPickX);
pub(crate) const TAG_CALL_X: u8 = encode_tag_raw(Tag::CallX);
pub(crate) const TAG_CALL_NAMED_X: u8 = encode_tag_raw(Tag::CallNamedX);
pub(crate) const TAG_REG_EXT: u8 = RAW_TAG_REG_EXT;
pub(crate) const TAG_EXT: u8 = RAW_TAG_EXT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expr::Pattern, stmt::Stmt, vm::bytecode::PatternBinding};
    #[test]
    fn test_bc32_roundtrip_simple() {
        let f = Function {
            consts: vec![Val::Int(42)],
            code: vec![
                Op::LoadK(0, 0),
                Op::Move(1, 0),
                Op::ToStr(2, 1),
                Op::ToBool(2, 1),
                Op::Jmp(1),
                Op::JmpFalse(2, -1),
            ],
            n_regs: 3,
            protos: vec![],
            param_regs: vec![],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&f).expect("encodable");
        let f2 = bc.decode();
        assert_eq!(format!("{:?}", f.code), format!("{:?}", f2.code));
    }

    #[test]
    fn test_bc32_call_multi_return() {
        let f = Function {
            consts: vec![],
            code: vec![Op::Call {
                f: 0,
                base: 1,
                argc: 2,
                retc: 2,
            }],
            n_regs: 4,
            protos: vec![],
            param_regs: vec![],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&f).expect("encodable multi-ret call");
        assert_eq!(bc.code32.len(), 2, "CallX should occupy two words");
        let decoded = bc.decode();
        let expected = vec![Op::Call {
            f: 0,
            base: 1,
            argc: 2,
            retc: 2,
        }];
        assert_eq!(format!("{:?}", decoded.code), format!("{:?}", expected));
    }

    #[test]
    fn test_bc32_call_named_out_of_range_uses_extended_path() {
        let f = Function {
            consts: vec![],
            code: vec![Op::CallNamed {
                f: 300,
                base_pos: 301,
                posc: 2,
                base_named: 512,
                namedc: 3,
                retc: 2,
            }],
            n_regs: 600,
            protos: vec![],
            param_regs: vec![],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&f).expect("extended call encodable");
        assert_eq!(bc.code32.len(), 3);
        let decoded = bc.decode();
        assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
    }

    #[test]
    fn test_bc32_pattern_ops() {
        let f = Function {
            consts: vec![Val::Str("fail".into())],
            code: vec![
                Op::PatternMatch {
                    dst: 0,
                    src: 1,
                    plan: 0,
                },
                Op::PatternMatchOrFail {
                    src: 1,
                    plan: 0,
                    err_kidx: 0,
                    is_const: false,
                },
                Op::PatternMatchOrFail {
                    src: 1,
                    plan: 0,
                    err_kidx: 0,
                    is_const: true,
                },
            ],
            n_regs: 4,
            protos: vec![],
            param_regs: vec![],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: vec![PatternPlan {
                pattern: Pattern::Variable("x".into()),
                bindings: vec![PatternBinding {
                    name: "x".into(),
                    reg: 2,
                }],
            }],
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&f).expect("pattern ops encodable");
        assert_eq!(bc.code32.len(), 3);

        let decoded = bc.decode();
        assert_eq!(decoded.pattern_plans.len(), 1);
        assert!(matches!(
            decoded.pattern_plans[0].pattern,
            Pattern::Variable(ref name) if name == "x"
        ));
        assert!(matches!(
            decoded.code[0],
            Op::PatternMatch {
                dst: 0,
                src: 1,
                plan: 0
            }
        ));
        assert!(matches!(
            decoded.code[1],
            Op::PatternMatchOrFail { is_const: false, .. }
        ));
        assert!(matches!(decoded.code[2], Op::PatternMatchOrFail { is_const: true, .. }));
    }

    #[test]
    fn test_bc32_build_list_map() {
        let f = Function {
            consts: vec![],
            code: vec![
                Op::BuildList {
                    dst: 0,
                    base: 1,
                    len: 6,
                },
                Op::BuildMap {
                    dst: 2,
                    base: 8,
                    len: 4,
                },
            ],
            n_regs: 16,
            protos: Vec::new(),
            param_regs: vec![],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        let bc = Bc32Function::try_from_function(&f).expect("build ops encodable");
        assert_eq!(bc.code32.len(), 2);

        let decoded = bc.decode();
        assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
        assert_eq!(decoded.n_regs, 16);
    }

    #[test]
    fn test_bc32_build_list_map_out_of_range_falls_back() {
        let f = Function {
            consts: vec![],
            code: vec![
                Op::BuildList {
                    dst: 3,
                    base: 20,
                    len: 300,
                },
                Op::MakeClosure { dst: 4, proto: 300 },
            ],
            n_regs: 24,
            protos: {
                let proto_template = ClosureProto {
                    self_name: None,
                    params: Vec::new(),
                    named_params: Vec::new(),
                    default_funcs: Vec::new(),
                    func: None,
                    body: Stmt::Block { statements: Vec::new() },
                    captures: Vec::new(),
                };
                vec![proto_template; 301]
            },
            param_regs: vec![],
            named_param_regs: vec![],
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        };
        assert!(Bc32Function::try_from_function(&f).is_none());
    }
}
