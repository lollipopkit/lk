use crate::vm::{Function32Data, Instr32, Opcode32};

pub(super) fn callee_is_native_assert(callee: &Function32Data) -> bool {
    if callee.param_count != 1 || callee.capture_count != 0 {
        return false;
    }
    let Ok(code) = callee
        .code
        .iter()
        .copied()
        .map(Instr32::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    if !matches!(code.first().copied().map(Instr32::opcode), Some(Opcode32::Not))
        || !matches!(code.get(1).copied().map(Instr32::opcode), Some(Opcode32::Test))
    {
        return false;
    }
    code.iter()
        .copied()
        .any(|instr| matches!(instr.opcode(), Opcode32::Call))
}

pub(super) fn callee_contains_call(callee: &Function32Data) -> bool {
    callee
        .code
        .iter()
        .copied()
        .filter_map(|raw| Instr32::try_from_raw(raw).ok())
        .any(|instr| matches!(instr.opcode(), Opcode32::Call | Opcode32::CallNamed))
}
