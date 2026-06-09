use crate::vm::{ConstHeapValueData, FunctionData, Instr, Opcode};

pub(in crate::llvm) fn function_returns_pushed_list(function: &FunctionData) -> bool {
    let Ok(code) = function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    let mut list_regs = vec![false; function.register_count as usize];
    for instr in code {
        let a = instr.a() as usize;
        match instr.opcode() {
            Opcode::LoadHeapConst => {
                let is_empty_list = matches!(
                    function.consts.heap_values.get(instr.bx() as usize),
                    Some(ConstHeapValueData::List(values)) if values.is_empty()
                );
                if let Some(slot) = list_regs.get_mut(a) {
                    *slot = is_empty_list;
                }
            }
            Opcode::NewList | Opcode::ListPush => {
                if let Some(slot) = list_regs.get_mut(a) {
                    *slot = true;
                }
            }
            Opcode::Move => {
                let src = list_regs.get(instr.b() as usize).copied().unwrap_or(false);
                if let Some(slot) = list_regs.get_mut(a) {
                    *slot = src;
                }
            }
            opcode if opcode.is_return() && instr.return_count() == 1 => {
                if list_regs.get(a).copied().unwrap_or(false) {
                    return true;
                }
            }
            opcode if opcode.is_return() => {}
            _ => {
                if let Some(slot) = list_regs.get_mut(a) {
                    *slot = false;
                }
            }
        }
    }
    false
}
