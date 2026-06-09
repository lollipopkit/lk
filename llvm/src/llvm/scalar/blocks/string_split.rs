use crate::{
    llvm::{
        ir_text::{emit_branch_to_next, next_tmp},
        scalar::{
            block_helpers::{local_static_container_before, three_regs_in_bounds},
            contains::local_static_string_before,
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{NativeStraightlineValue, NativeTextPart, native_static_string_split},
    },
    vm::{ConstHeapValueData, Instr},
};

pub(super) fn emit_string_split_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    let target = static_regs
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()))
        .or_else(|| {
            (facts.register_kind_before(pc, instr.b()) == Some(NativeScalarKind::StrPtr)).then(|| {
                let value = next_tmp(tmp_index);
                ir.push_str(&format!("  {value} = load ptr, ptr %r{}.slot\n", instr.b()));
                NativeStraightlineValue::StringPtr(value)
            })
        });
    let Some(target) = target else { return false };
    let Some(delimiter) = static_regs
        .get(instr.c() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_string_before(code, strings, pc, instr.c()))
    else {
        return false;
    };
    if let Some(value) = native_static_string_split(target.clone(), delimiter.clone(), String::new()) {
        static_regs[instr.a() as usize] = Some(value);
        emit_branch_to_next(ir, pc, code.len());
        return true;
    }
    let NativeStraightlineValue::String { value: delimiter, .. } = delimiter else {
        return false;
    };
    let text = match target {
        NativeStraightlineValue::Text(text) => text,
        NativeStraightlineValue::StringPtr(value) => vec![NativeTextPart::StrPtr(value)],
        _ => return false,
    };
    if !delimiter.is_ascii() {
        return false;
    }
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicSplitText { text, delimiter });
    emit_branch_to_next(ir, pc, code.len());
    true
}
