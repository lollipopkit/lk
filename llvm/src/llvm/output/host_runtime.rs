use crate::llvm::{output::emit_local_or_global_string_ptr, straightline_value::NativeStraightlineValue};

pub(super) fn emit_native_bytes_to_string_utf8(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [NativeStraightlineValue::I64(bytes)] = args else {
        return None;
    };
    let out = format!("%bytes_to_string_utf8_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {out} = call ptr @lkrt_bytes_to_string_utf8(i64 {bytes})\n"));
    Some(NativeStraightlineValue::StringPtr(out))
}

pub(super) fn emit_native_env_get(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [key] = args else {
        return None;
    };
    let key = static_string_ptr_arg(body, key)?;
    let id = *ssa_index;
    *ssa_index += 1;
    let out_slot = format!("%env_get_out_{id}");
    let status = format!("%env_get_status_{id}");
    let value = format!("%env_get_value_{id}");
    let status_ok = format!("%env_get_status_ok_{id}");
    let value_present = format!("%env_get_value_present_{id}");
    let present = format!("%env_get_present_{id}");
    let present_i64 = format!("%env_get_present_i64_{id}");
    body.push_str(&format!("  {out_slot} = alloca ptr\n"));
    body.push_str(&format!(
        "  {status} = call i64 @lkrt_env_get(ptr {key}, ptr {out_slot})\n"
    ));
    body.push_str(&format!("  {value} = load ptr, ptr {out_slot}\n"));
    body.push_str(&format!("  {status_ok} = icmp eq i64 {status}, 0\n"));
    body.push_str(&format!("  {value_present} = icmp ne ptr {value}, null\n"));
    body.push_str(&format!("  {present} = and i1 {status_ok}, {value_present}\n"));
    body.push_str(&format!("  {present_i64} = zext i1 {present} to i64\n"));
    Some(NativeStraightlineValue::MaybeStrPtr {
        value,
        present: present_i64,
    })
}

pub(super) fn emit_native_env_get_or(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [key, default] = args else {
        return None;
    };
    let key = static_string_ptr_arg(body, key)?;
    let default = static_string_ptr_arg(body, default)?;
    let out = format!("%env_get_or_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!(
        "  {out} = call ptr @lkrt_env_get_or(ptr {key}, ptr {default})\n"
    ));
    Some(NativeStraightlineValue::StringPtr(out))
}

pub(super) fn emit_native_unary_string_i64_call(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
    name: &str,
    symbol: &str,
    bool_result: bool,
) -> Option<NativeStraightlineValue> {
    let [arg] = args else {
        return None;
    };
    let arg = static_string_ptr_arg(body, arg)?;
    let out = format!("%{name}_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {out} = call i64 {symbol}(ptr {arg})\n"));
    if bool_result {
        Some(NativeStraightlineValue::Bool(out))
    } else {
        Some(NativeStraightlineValue::I64(out))
    }
}

pub(super) fn emit_native_unary_string_ptr_call(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
    name: &str,
    symbol: &str,
) -> Option<NativeStraightlineValue> {
    let [arg] = args else {
        return None;
    };
    let arg = static_string_ptr_arg(body, arg)?;
    let out = format!("%{name}_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {out} = call ptr {symbol}(ptr {arg})\n"));
    Some(NativeStraightlineValue::StringPtr(out))
}

pub(super) fn emit_native_zero_arg_string_ptr_call(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
    name: &str,
    symbol: &str,
) -> Option<NativeStraightlineValue> {
    if !args.is_empty() {
        return None;
    }
    let out = format!("%{name}_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {out} = call ptr {symbol}()\n"));
    Some(NativeStraightlineValue::StringPtr(out))
}

pub(super) fn emit_native_fs_write(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [path, data] = args else {
        return None;
    };
    let path = static_string_ptr_arg(body, path)?;
    let out = format!("%fs_write_{}", *ssa_index);
    *ssa_index += 1;
    match data {
        NativeStraightlineValue::StringPtr(_) | NativeStraightlineValue::String { .. } => {
            let data = static_string_ptr_arg(body, data)?;
            body.push_str(&format!(
                "  {out} = call i64 @lkrt_fs_write_str(ptr {path}, ptr {data})\n"
            ));
        }
        NativeStraightlineValue::I64(bytes) => {
            body.push_str(&format!(
                "  {out} = call i64 @lkrt_fs_write_bytes(ptr {path}, i64 {bytes})\n"
            ));
        }
        _ => return None,
    }
    Some(NativeStraightlineValue::Bool(out))
}

fn static_string_ptr_arg(body: &mut String, value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::StringPtr(ptr) => Some(ptr.clone()),
        NativeStraightlineValue::String { symbol, value, .. } => emit_local_or_global_string_ptr(body, symbol, value),
        _ => None,
    }
}
