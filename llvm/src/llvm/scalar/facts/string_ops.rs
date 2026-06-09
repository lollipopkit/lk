use crate::{
    llvm::{
        scalar::block_helpers::local_static_container_before,
        straightline_value::{NativeStraightlineValue, NativeTextPart, native_static_string_split},
    },
    vm::{ConstHeapValueData, Instr},
};

use super::{NativeScalarKind, analysis::static_value_kind, slots::set_static_value};

pub(super) fn propagate_string_split(
    kinds: &mut [Option<NativeScalarKind>],
    static_values: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    heap_values: &[ConstHeapValueData],
    pc: usize,
    instr: Instr,
) -> Option<()> {
    let delimiter = static_values.get(instr.c() as usize).and_then(Clone::clone)?;
    let target = static_values
        .get(instr.b() as usize)
        .and_then(Clone::clone)
        .or_else(|| local_static_container_before(code, heap_values, pc, instr.b()));
    if let Some(target) = target.clone()
        && let Some(value) = native_static_string_split(target, delimiter.clone(), String::new())
    {
        return set_static_value(kinds, static_values, instr.a(), static_value_kind(&value), value).then_some(());
    }
    let NativeStraightlineValue::String { value: delimiter, .. } = delimiter else {
        return None;
    };
    let text = match target {
        Some(NativeStraightlineValue::Text(text)) => text,
        None if kinds.get(instr.b() as usize).copied().flatten() == Some(NativeScalarKind::StrPtr) => {
            vec![NativeTextPart::StrPtr(String::new())]
        }
        _ => return None,
    };
    (delimiter.is_ascii()
        && set_static_value(
            kinds,
            static_values,
            instr.a(),
            None,
            NativeStraightlineValue::DynamicSplitText { text, delimiter },
        ))
    .then_some(())
}
