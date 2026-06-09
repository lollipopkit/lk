use crate::llvm::{
    const_display::llvm_string_constant,
    straightline_value::{NativeStraightlineValue, native_runtime_string_key_kind},
};

pub(in crate::llvm) fn store_native_inline_scalar_value(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    call_pc: usize,
    dst: u8,
    value: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %call{call_pc}.r{dst}.present.slot\n"));
        }
        NativeStraightlineValue::MaybeI64 { value, present } => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::F64(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store double {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeF64 { value, present } => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store double {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::Bool(value) => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeBool { value, present } => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::Nil => {
            static_regs[dst as usize] = None;
            ir.push_str(&format!("  store i64 0, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            let symbol = if symbol.is_empty() {
                let symbol = format!("@lk_call_inline_str_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                symbol
            } else {
                emit_string_global_once(extra_globals, &symbol, &value);
                symbol
            };
            static_regs[dst as usize] = Some(native_static_string(&value, symbol.clone()));
            ir.push_str(&format!("  store ptr {symbol}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::StringPtr(value) => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::StringPtr(value.clone()));
            ir.push_str(&format!("  store ptr {value}, ptr %call{call_pc}.r{dst}.slot\n"));
        }
        NativeStraightlineValue::MaybeStrPtr { value, present } => {
            static_regs[dst as usize] = Some(NativeStraightlineValue::MaybeStrPtr {
                value: value.clone(),
                present: present.clone(),
            });
            ir.push_str(&format!("  store ptr {value}, ptr %call{call_pc}.r{dst}.slot\n"));
            ir.push_str(&format!(
                "  store i64 {present}, ptr %call{call_pc}.r{dst}.present.slot\n"
            ));
        }
        NativeStraightlineValue::Object { .. } => {
            static_regs[dst as usize] = Some(value);
        }
        _ => return None,
    }
    Some(())
}

fn emit_string_global_once(extra_globals: &mut String, symbol: &str, value: &str) {
    if symbol.starts_with('@') {
        let definition_prefix = format!("{symbol} = ");
        if !extra_globals.contains(&definition_prefix) {
            extra_globals.push_str(&llvm_string_constant(symbol, value));
        }
    }
}

fn native_static_string(value: &str, symbol: String) -> NativeStraightlineValue {
    NativeStraightlineValue::String {
        symbol,
        value: value.to_string(),
        len: value.chars().count(),
        key_kind: native_runtime_string_key_kind(value),
    }
}
