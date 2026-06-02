use crate::{
    llvm::{
        scalar::{
            block_helpers::local_static_i64_before, contains::local_static_object_before, facts::NativeScalarKind,
        },
        straightline_value::{NativeStraightlineValue, native_static_index},
    },
    vm::Instr32,
};

pub(super) fn arg_list_get_index_value(
    static_values: &[Option<NativeStraightlineValue>],
    kinds: &[Option<NativeScalarKind>],
    code: &[Instr32],
    int_consts: &[i64],
    pc: usize,
    instr: Instr32,
    target: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::ArgList { elements } = target.clone() else {
        return None;
    };
    if kinds.get(instr.c() as usize).copied().flatten() != Some(NativeScalarKind::I64) {
        return None;
    }
    if let Some(key) = static_values
        .get(instr.c() as usize)
        .and_then(Clone::clone)
        .or_else(|| {
            (!register_written_by_enclosing_loop(code, pc, instr.c()))
                .then(|| local_static_i64_before(code, int_consts, pc, instr.c()))
                .flatten()
        })
        && let Some(value) = native_static_index(target, key, String::new())
    {
        return Some(value);
    }
    Some(NativeStraightlineValue::DynamicArgListElement {
        elements,
        index: format!("%r{}.slot", instr.c()),
    })
}

pub(super) fn collected_arg_list_push(value: Option<NativeStraightlineValue>) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::DynamicArgListElement { elements, .. } = value? else {
        return None;
    };
    Some(NativeStraightlineValue::ArgList { elements })
}

pub(super) fn single_callable_arg_list(value: Option<NativeStraightlineValue>) -> Option<NativeStraightlineValue> {
    let value @ (NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }) = value? else {
        return None;
    };
    Some(NativeStraightlineValue::ArgList { elements: vec![value] })
}

pub(super) fn object_arg_list_from_registers(
    static_values: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    pc: usize,
    start: usize,
    end: usize,
) -> Option<NativeStraightlineValue> {
    let elements = (start..end)
        .map(|reg| {
            let reg = u8::try_from(reg).ok()?;
            static_values
                .get(reg as usize)
                .cloned()
                .flatten()
                .or_else(|| local_static_object_before(static_values, code, int_consts, pc, reg))
        })
        .collect::<Option<Vec<_>>>()?;
    elements
        .iter()
        .all(|value| matches!(value, NativeStraightlineValue::Object { .. }))
        .then_some(NativeStraightlineValue::ArgList { elements })
}

pub(super) fn arg_list_set_index_value(
    target: NativeStraightlineValue,
    static_values: &[Option<NativeStraightlineValue>],
    int_consts: &[i64],
    code: &[Instr32],
    pc: usize,
    key_reg: u8,
    value_reg: u8,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::ArgList { mut elements } = target else {
        return None;
    };
    let key = static_values
        .get(key_reg as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_i64_before(code, int_consts, pc, key_reg))?;
    let NativeStraightlineValue::I64(index) = key else {
        return None;
    };
    let value = static_values.get(value_reg as usize).and_then(Clone::clone)?;
    *elements.get_mut(index.parse::<usize>().ok()?)? = value;
    Some(NativeStraightlineValue::ArgList { elements })
}

fn register_written_by_enclosing_loop(code: &[Instr32], pc_limit: usize, reg: u8) -> bool {
    code.iter()
        .copied()
        .enumerate()
        .skip(pc_limit.saturating_add(1))
        .filter(|(_, instr)| instr.opcode() == crate::vm::Opcode32::Jmp)
        .any(|(jump_pc, instr)| {
            let target = jump_pc as i64 + 1 + instr.sj_arg() as i64;
            target >= 0 && (target as usize) <= pc_limit && (target as usize..jump_pc).any(|pc| code[pc].a() == reg)
        })
}
