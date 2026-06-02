use crate::{
    llvm::{
        dynamic_containers::{emit_dynamic_joined_text_len, emit_dynamic_text_len},
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::{
            block_helpers::local_static_container_before,
            blocks::const_lists::emit_const_list_element_len,
            contains::{local_static_index_value_before, local_static_map_rest_before},
            facts::NativeScalarFacts,
        },
        straightline_value::{NativeListElementKind, NativeStraightlineValue},
    },
    vm::{ConstHeapValue32Data, Instr32},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_len_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr32],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValue32Data],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    _facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return None;
    }
    let target = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
        .or_else(|| local_static_map_rest_before(code, strings, heap_values, pc, instr.b()))
        .or_else(|| local_static_index_value_before(code, int_consts, strings, heap_values, pc, instr.b()))?;
    match target {
        NativeStraightlineValue::String { value, .. } if value.is_ascii() => {
            let len = value.len();
            ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(len.to_string()));
        }
        NativeStraightlineValue::Text(parts) => emit_dynamic_text_len(ir, instr.a(), &parts, tmp_index)?,
        NativeStraightlineValue::DynamicTextChar => {
            ir.push_str(&format!("  store i64 1, ptr %r{}.slot\n", instr.a()));
        }
        NativeStraightlineValue::List { elements, .. } => {
            let len = elements.len();
            ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(len.to_string()));
        }
        NativeStraightlineValue::ArgList { elements } => {
            let len = elements.len();
            ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(len.to_string()));
        }
        NativeStraightlineValue::Map { entries, .. } => {
            let len = entries.len();
            ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
            static_regs[instr.a() as usize] = Some(NativeStraightlineValue::I64(len.to_string()));
        }
        NativeStraightlineValue::DynamicMapIter { id, .. } => {
            let len = next_tmp(tmp_index);
            ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
            ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
        }
        NativeStraightlineValue::DynamicJoinedText { id, delimiter_len } => {
            emit_dynamic_joined_text_len(ir, instr.a(), id, delimiter_len, tmp_index)?;
        }
        NativeStraightlineValue::DynamicList {
            id,
            element:
                NativeListElementKind::I64
                | NativeListElementKind::F64
                | NativeListElementKind::Text
                | NativeListElementKind::StrPtr,
        } => {
            let len = next_tmp(tmp_index);
            ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
            ir.push_str(&format!("  store i64 {len}, ptr %r{}.slot\n", instr.a()));
        }
        NativeStraightlineValue::DynamicConstListElement { elements, index } => {
            let value = emit_const_list_element_len(ir, &elements, &index, instr.a(), tmp_index)?;
            static_regs[instr.a() as usize] = Some(value);
        }
        _ => return None,
    }
    if !matches!(
        static_regs.get(instr.a() as usize),
        Some(Some(NativeStraightlineValue::I64(_)))
    ) {
        static_regs[instr.a() as usize] = None;
    }
    emit_branch_to_next(ir, pc, code.len());
    Some(())
}
