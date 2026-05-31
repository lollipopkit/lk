use crate::llvm::straightline_value::{
    NativeListElementKind, NativeStraightlineValue, native_runtime_string_key_kind,
    native_straightline_heap_const_value,
};
use crate::vm::{ConstHeapValue32Data, Instr32, Opcode32};

use super::{control_flow_static_boundaries, local_static_i64_before};

pub(super) fn static_direct_call_args(
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    static_regs: &[Option<NativeStraightlineValue>],
    callee: u8,
    count: u8,
) -> Option<(Vec<NativeStraightlineValue>, bool)> {
    let start = callee as usize + 1;
    let end = start.checked_add(count as usize)?;
    let mut args = Vec::with_capacity(count as usize);
    let mut recovered_heap_const = false;
    for reg in start..end {
        let static_value = static_regs.get(reg).cloned().flatten();
        let value = if matches!(
            static_value,
            Some(NativeStraightlineValue::DynamicList {
                element: NativeListElementKind::I64,
                ..
            })
        ) {
            if let Some(value) = local_static_heap_const_before(code, heap_values, pc, reg as u8) {
                recovered_heap_const = true;
                value
            } else {
                static_value?
            }
        } else {
            static_value
                .or_else(|| local_static_i64_before(code, int_consts, pc, reg as u8))
                .or_else(|| local_static_string_before(code, strings, pc, reg as u8))?
        };
        args.push(value);
    }
    Some((args, recovered_heap_const))
}

fn local_static_string_before(
    code: &[Instr32],
    strings: &[String],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    for prev_pc in (pc.saturating_sub(32)..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        return match prev.opcode() {
            Opcode32::LoadString => {
                let value = strings.get(prev.bx() as usize)?;
                Some(NativeStraightlineValue::String {
                    symbol: String::new(),
                    value: value.clone(),
                    len: value.chars().count(),
                    key_kind: native_runtime_string_key_kind(value),
                })
            }
            Opcode32::Move if prev.b() != reg => local_static_string_before(code, strings, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}

fn local_static_heap_const_before(
    code: &[Instr32],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    reg: u8,
) -> Option<NativeStraightlineValue> {
    let boundaries = control_flow_static_boundaries(code);
    let start = pc.saturating_sub(32);
    for prev_pc in (start..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != reg {
            continue;
        }
        if boundaries
            .iter()
            .copied()
            .skip(prev_pc + 1)
            .take(pc.saturating_sub(prev_pc + 1))
            .any(|boundary| boundary)
        {
            return None;
        }
        return match prev.opcode() {
            Opcode32::LoadHeapConst => {
                let value = heap_values.get(prev.bx() as usize)?;
                native_straightline_heap_const_value(0, prev.bx(), value)
            }
            Opcode32::Move if prev.b() != reg => local_static_heap_const_before(code, heap_values, prev_pc, prev.b()),
            _ => None,
        };
    }
    None
}
