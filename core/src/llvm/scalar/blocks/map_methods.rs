use crate::{
    llvm::{
        dynamic_containers::{
            emit_dynamic_i64_int_map_set, emit_dynamic_i64_int_map_values, emit_dynamic_string_f64_map_set,
            emit_dynamic_string_f64_map_values, emit_dynamic_string_int_map_set, emit_dynamic_string_int_map_values,
            emit_dynamic_string_map_keys,
        },
        straightline_value::{
            NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
            native_static_set_index,
        },
    },
    vm::Instr32,
};

pub(super) fn emit_dynamic_map_set_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    if builtin != NativeBuiltin::MapSet {
        return None;
    }
    if let [
        NativeStraightlineValue::DynamicMap {
            id,
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64 | NativeMapValueKind::F64,
        },
        key,
        value,
    ] = args
    {
        let map = NativeStraightlineValue::Map {
            symbol: format!("@lk_map_set_{id}"),
            value: "{}".to_string(),
            entries: Vec::new(),
        };
        if let Some(value) = native_static_set_index(map, key.clone(), value.clone()) {
            static_regs[instr.a() as usize] = Some(value);
            return Some(());
        }
    }
    let [
        NativeStraightlineValue::DynamicMap {
            id,
            key,
            value: map_value,
        },
        map_key,
        value,
    ] = args
    else {
        return None;
    };
    let value_reg = instr.b().checked_add(3)?;
    let key_reg = instr.b().checked_add(2)?;
    let (next_key, next_value) = match (key, map_key, map_value, value) {
        (
            NativeMapKeyKind::Str,
            map_key @ (NativeStraightlineValue::String { .. } | NativeStraightlineValue::StringPtr(_)),
            NativeMapValueKind::I64,
            NativeStraightlineValue::I64(_),
        ) => {
            emit_dynamic_string_int_map_set(ir, extra_globals, *id, value_reg, map_key.clone(), tmp_index)?;
            (NativeMapKeyKind::Str, NativeMapValueKind::I64)
        }
        (
            NativeMapKeyKind::Str,
            map_key @ (NativeStraightlineValue::String { .. } | NativeStraightlineValue::StringPtr(_)),
            NativeMapValueKind::I64 | NativeMapValueKind::F64,
            NativeStraightlineValue::F64(_),
        ) => {
            emit_dynamic_string_f64_map_set(ir, extra_globals, *id, value_reg, map_key.clone(), tmp_index)?;
            (NativeMapKeyKind::Str, NativeMapValueKind::F64)
        }
        (
            NativeMapKeyKind::Str | NativeMapKeyKind::I64,
            NativeStraightlineValue::I64(_),
            NativeMapValueKind::I64,
            NativeStraightlineValue::I64(_),
        ) => {
            emit_dynamic_i64_int_map_set(ir, *id, value_reg, key_reg, tmp_index)?;
            (NativeMapKeyKind::I64, NativeMapValueKind::I64)
        }
        _ => return None,
    };
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicMap {
        id: *id,
        key: next_key,
        value: next_value,
    });
    Some(())
}

pub(super) fn emit_dynamic_map_values_call(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeBuiltin::MapModuleMethod("values") = builtin else {
        return None;
    };
    let [NativeStraightlineValue::DynamicMap { id, key, value }] = args else {
        return None;
    };
    let element = match value {
        NativeMapValueKind::I64 => {
            match key {
                NativeMapKeyKind::Str => emit_dynamic_string_int_map_values(ir, *id, pc, tmp_index)?,
                NativeMapKeyKind::I64 => emit_dynamic_i64_int_map_values(ir, *id, pc, tmp_index)?,
            }
            NativeListElementKind::I64
        }
        NativeMapValueKind::F64 => {
            if *key != NativeMapKeyKind::Str {
                return None;
            }
            emit_dynamic_string_f64_map_values(ir, *id, pc, tmp_index)?;
            NativeListElementKind::F64
        }
    };
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList { id: pc, element });
    Some(())
}

pub(super) fn emit_dynamic_map_keys_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeBuiltin::MapModuleMethod("keys") = builtin else {
        return None;
    };
    let [
        NativeStraightlineValue::DynamicMap {
            id,
            key: NativeMapKeyKind::Str,
            ..
        },
    ] = args
    else {
        return None;
    };
    emit_dynamic_string_map_keys(ir, extra_globals, *id, pc, tmp_index)?;
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList {
        id: pc,
        element: NativeListElementKind::StrPtr,
    });
    Some(())
}
