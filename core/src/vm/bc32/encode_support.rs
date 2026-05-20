use super::Op;

#[derive(Debug)]
pub(super) enum Bc32Reject {
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
    pub(super) fn reason_key(&self) -> &'static str {
        match self {
            Bc32Reject::UnsupportedOpcode { .. } => "unsupported_opcode",
            Bc32Reject::OperandOutOfRange { .. } => "operand_out_of_range",
            Bc32Reject::BranchTargetOutOfBounds { .. } => "branch_target_out_of_bounds",
            Bc32Reject::EncodingInvariant { .. } => "encoding_invariant_violation",
        }
    }

    pub(super) fn opcode(&self) -> &'static str {
        match self {
            Bc32Reject::UnsupportedOpcode { opcode, .. }
            | Bc32Reject::OperandOutOfRange { opcode, .. }
            | Bc32Reject::BranchTargetOutOfBounds { opcode }
            | Bc32Reject::EncodingInvariant { opcode, .. } => opcode,
        }
    }

    pub(super) fn detail(&self) -> &'static str {
        match self {
            Bc32Reject::UnsupportedOpcode { detail, .. } | Bc32Reject::EncodingInvariant { detail, .. } => detail,
            Bc32Reject::OperandOutOfRange { operand, .. } => operand,
            Bc32Reject::BranchTargetOutOfBounds { .. } => "",
        }
    }
}

pub(super) struct PackIssue {
    pub(super) reason: Bc32Reject,
    pub(super) op_index: Option<usize>,
}

impl PackIssue {
    pub(super) fn new(reason: Bc32Reject, op_index: usize) -> Self {
        Self {
            reason,
            op_index: Some(op_index),
        }
    }
}

pub(super) fn ensure_u8(opcode: &'static str, operand: &'static str, value: u16) -> Result<(), Bc32Reject> {
    if value < 256 {
        Ok(())
    } else {
        Err(Bc32Reject::OperandOutOfRange { opcode, operand })
    }
}

pub(super) fn ensure_regs_u8(opcode: &'static str, dst: u16, arg1: u16, arg2: u16) -> Result<(), Bc32Reject> {
    ensure_u8(opcode, "dst", dst)?;
    ensure_u8(opcode, "arg1", arg1)?;
    ensure_u8(opcode, "arg2", arg2)?;
    Ok(())
}

pub(super) fn ensure_i8_range(opcode: &'static str, operand: &'static str, value: i32) -> Result<(), Bc32Reject> {
    if (-128..=127).contains(&value) {
        Ok(())
    } else {
        Err(Bc32Reject::EncodingInvariant {
            opcode,
            detail: operand,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct EncodedOp {
    pub(super) word: u32,
    extra: [u32; 3],
    extra_len: u8,
}

impl EncodedOp {
    pub(super) fn new(word: u32, extra: Option<u32>) -> Self {
        let mut op = Self {
            word,
            extra: [0; 3],
            extra_len: 0,
        };
        if let Some(extra) = extra {
            op.extra[0] = extra;
            op.extra_len = 1;
        }
        op
    }

    pub(super) fn with_extra(word: u32, extra: [u32; 2]) -> Self {
        Self {
            word,
            extra: [extra[0], extra[1], 0],
            extra_len: 2,
        }
    }

    pub(super) fn with_extra3(word: u32, extra: [u32; 3]) -> Self {
        Self {
            word,
            extra,
            extra_len: 3,
        }
    }

    pub(super) fn len(&self) -> usize {
        1 + self.extra_len as usize
    }

    pub(super) fn emit(self, out: &mut Vec<u32>) {
        out.push(self.word);
        for index in 0..self.extra_len as usize {
            out.push(self.extra[index]);
        }
    }
}

pub(super) fn opcode_name(op: &Op) -> &'static str {
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
        Op::StrConcatKnownCap(..) => "StrConcatKnownCap",
        Op::StrConcatToStr(..) => "StrConcatToStr",
        Op::AddIntImm(..) => "AddIntImm",
        Op::Sub(..) => "Sub",
        Op::SubInt(..) => "SubInt",
        Op::SubFloat(..) => "SubFloat",
        Op::Mul(..) => "Mul",
        Op::MulInt(..) => "MulInt",
        Op::MulFloat(..) => "MulFloat",
        Op::Div(..) => "Div",
        Op::DivFloat(..) => "DivFloat",
        Op::FloorDivImm { .. } => "FloorDivImm",
        Op::Mod(..) => "Mod",
        Op::ModInt(..) => "ModInt",
        Op::ModFloat(..) => "ModFloat",
        Op::CmpEq(..) => "CmpEq",
        Op::CmpNe(..) => "CmpNe",
        Op::CmpLt(..) => "CmpLt",
        Op::CmpLe(..) => "CmpLe",
        Op::CmpGt(..) => "CmpGt",
        Op::CmpGe(..) => "CmpGe",
        Op::CmpI { .. } => "CmpI",
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
        Op::ListIndexI(..) => "ListIndexI",
        Op::ListSetI { .. } => "ListSetI",
        Op::StrIndexI(..) => "StrIndexI",
        Op::Len { .. } => "Len",
        Op::ListLen { .. } => "ListLen",
        Op::MapLen { .. } => "MapLen",
        Op::StrLen { .. } => "StrLen",
        Op::Floor { .. } => "Floor",
        Op::StartsWithK(..) => "StartsWithK",
        Op::ContainsK(..) => "ContainsK",
        Op::MapHas(..) => "MapHas",
        Op::MapGetInterned(..) => "MapGetInterned",
        Op::MapGetDynamic(..) => "MapGetDynamic",
        Op::MapSetInterned(..) => "MapSetInterned",
        Op::MapSetInternedMove(..) => "MapSetInternedMove",
        Op::MapHasK(..) => "MapHasK",
        Op::ListFoldAdd { .. } => "ListFoldAdd",
        Op::MapValuesFoldAdd { .. } => "MapValuesFoldAdd",
        Op::Index { .. } => "Index",
        Op::ToIter { .. } => "ToIter",
        Op::BuildList { .. } => "BuildList",
        Op::BuildMap { .. } => "BuildMap",
        Op::ListSlice { .. } => "ListSlice",
        Op::ListPush { .. } => "ListPush",
        Op::ListPushMove { .. } => "ListPushMove",
        Op::MapSet { .. } => "MapSet",
        Op::MapSetMove { .. } => "MapSetMove",
        Op::MakeClosure { .. } => "MakeClosure",
        Op::Jmp(..) => "Jmp",
        Op::JmpFalse(..) => "JmpFalse",
        Op::BoolBranch(..) => "BoolBranch",
        Op::Call { .. } => "Call",
        Op::CallExact { .. } => "CallExact",
        Op::CallClosureExact { .. } => "CallClosureExact",
        Op::CallNativeFast { .. } => "CallNativeFast",
        Op::CallMethod0 { .. } => "CallMethod0",
        Op::CallGlobalMethod0 { .. } => "CallGlobalMethod0",
        Op::CallNamed { .. } => "CallNamed",
        Op::CallNamedFallback { .. } => "CallNamedFallback",
        Op::Ret { .. } => "Ret",
        Op::ForRangePrep { .. } => "ForRangePrep",
        Op::ForRangeLoop { .. } => "ForRangeLoop",
        Op::RangeLoopI { .. } => "RangeLoopI",
        Op::ForRangeStep { .. } => "ForRangeStep",
        Op::Break(..) => "Break",
        Op::Continue(..) => "Continue",
        Op::PatternMatch { .. } => "PatternMatch",
        Op::PatternMatchOrFail { .. } => "PatternMatchOrFail",
        Op::Raise { .. } => "Raise",
        Op::JmpNilOrFalseJmp { .. } => "JmpNilOrFalseJmp",
        Op::AddRangeCountImm { .. } => "AddRangeCountImm",
        Op::CmpEqImmJmp { .. } => "CmpEqImmJmp",
        Op::CmpNeImmJmp { .. } => "CmpNeImmJmp",
        Op::CmpGtImmJmp { .. } => "CmpGtImmJmp",
        Op::CmpGeImmJmp { .. } => "CmpGeImmJmp",
        Op::CmpLtImmJmp { .. } => "CmpLtImmJmp",
        Op::CmpLeImmJmp { .. } => "CmpLeImmJmp",
        Op::AddIntImmJmp { .. } => "AddIntImmJmp",
        Op::CmpIntJmp { .. } => "CmpIntJmp",
    }
}
