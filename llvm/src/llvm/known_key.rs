use crate::vm::FunctionData;

use super::{
    const_display::native_string_const_value,
    straightline_value::{NativeStraightlineValue, NativeStringKeyKind},
};

pub(super) fn native_known_string_key(
    function: &FunctionData,
    pc: usize,
    symbol: impl Into<String>,
) -> Option<NativeStraightlineValue> {
    let key_index = function.performance.known_key(pc)?.const_key?;
    let value = function.consts.strings.get(key_index as usize)?;
    Some(NativeStraightlineValue::String {
        symbol: symbol.into(),
        value: native_string_const_value(value)?,
        len: value.len(),
        key_kind: NativeStringKeyKind::Short,
    })
}
