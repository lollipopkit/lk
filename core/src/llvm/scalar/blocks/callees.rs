use crate::vm::{FunctionData, Instr, Opcode};

pub(super) fn callee_is_native_assert(callee: &FunctionData) -> bool {
    if callee.param_count != 1 || callee.capture_count != 0 {
        return false;
    }
    let Ok(code) = callee
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    if !matches!(code.first().copied().map(Instr::opcode), Some(Opcode::Not))
        || !matches!(
            code.get(1).copied().map(Instr::opcode),
            Some(Opcode::Test | Opcode::BrFalse | Opcode::BrTrue)
        )
    {
        return false;
    }
    code.iter().copied().any(|instr| matches!(instr.opcode(), Opcode::Call))
}

pub(super) fn callee_contains_call(callee: &FunctionData) -> bool {
    callee
        .code
        .iter()
        .copied()
        .filter_map(|raw| Instr::try_from_raw(raw).ok())
        .any(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallNamed))
}
