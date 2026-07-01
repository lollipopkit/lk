use crate::llvm::{
    const_display::llvm_string_constant,
    ir_text::next_tmp,
    straightline_value::{NativeStraightlineValue, NativeTextPart},
};

pub(in crate::llvm) fn emit_dynamic_string_map_has(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    _pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let len = next_tmp(tmp_index);
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let found = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {found} = call i64 @lkrt_map_str_contains(ptr {prefix_base}, ptr {number_base}, i64 {len}, ptr {prefix}, i64 {number})\n"
    ));
    ir.push_str(&format!("  store i64 {found}, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_int_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_string_map_delete(
        ir,
        extra_globals,
        src_id,
        dst_id,
        dst,
        key,
        pc,
        "i64",
        "value",
        "0",
        tmp_index,
    )
}

pub(in crate::llvm) fn emit_dynamic_string_f64_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_string_map_delete(
        ir,
        extra_globals,
        src_id,
        dst_id,
        dst,
        key,
        pc,
        "double",
        "f64",
        "0.0",
        tmp_index,
    )
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_set(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    _pc: usize,
    value_reg: u8,
    key: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let value = next_tmp(tmp_index);
    let value_copy = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load ptr, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {value_copy} = call ptr @strdup(ptr {value})\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {next_len} = call i64 @lkrt_map_str_ptr_set(ptr {prefix_base}, ptr {number_base}, ptr {value_base}, i64 {len}, ptr {prefix}, i64 {number}, ptr {value_copy})\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %map{id}.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_get(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    _pc: usize,
    dst: u8,
    key: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let len = next_tmp(tmp_index);
    let found = next_tmp(tmp_index);
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  store ptr @lk_nil_text, ptr %r{dst}.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {found} = call i64 @lkrt_map_str_ptr_lookup(ptr {prefix_base}, ptr {number_base}, ptr {value_base}, i64 {len}, ptr {prefix}, i64 {number}, ptr %r{dst}.slot)\n"));
    ir.push_str(&format!("  store i64 {found}, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_iter_value(
    ir: &mut String,
    id: usize,
    dst: u8,
    index_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let index = next_tmp(tmp_index);
    let value_slot = next_tmp(tmp_index);
    let value = next_tmp(tmp_index);
    ir.push_str(&format!("  {index} = load i64, ptr %r{index_reg}.slot\n"));
    ir.push_str(&format!(
        "  {value_slot} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {value} = load ptr, ptr {value_slot}\n"));
    ir.push_str(&format!("  store ptr {value}, ptr %r{dst}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_values(
    ir: &mut String,
    map_id: usize,
    list_id: usize,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let map_base = next_tmp(tmp_index);
    let list_base = next_tmp(tmp_index);
    let label = format!("lk.copy.string.ptr.map.values.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {len} = load i64, ptr %map{map_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {map_base} = getelementptr [4096 x ptr], ptr %map{map_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {list_base} = getelementptr [4096 x ptr], ptr %list{list_id}.ptr.slots, i64 0, i64 0\n"
    ));
    emit_ptr_copy_loop(ir, &label, &map_base, &list_base, &len, list_id, pc);
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_string_map_delete(
        ir,
        extra_globals,
        src_id,
        dst_id,
        dst,
        key,
        pc,
        "ptr",
        "ptr",
        "@lk_nil_text",
        tmp_index,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_dynamic_string_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    _pc: usize,
    value_ty: &str,
    value_slot_name: &str,
    missing_value: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let helper = match value_ty {
        "i64" => "lkrt_map_str_int_delete",
        "double" => "lkrt_map_str_f64_delete",
        "ptr" => "lkrt_map_str_ptr_delete",
        _ => return None,
    };
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, src_id, tmp_index)?;
    let len = next_tmp(tmp_index);
    let src_prefix_base = next_tmp(tmp_index);
    let src_number_base = next_tmp(tmp_index);
    let src_value_base = next_tmp(tmp_index);
    let dst_prefix_base = next_tmp(tmp_index);
    let dst_number_base = next_tmp(tmp_index);
    let dst_value_base = next_tmp(tmp_index);
    let new_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_prefix_base} = getelementptr [4096 x ptr], ptr %map{src_id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {src_number_base} = getelementptr [4096 x i64], ptr %map{src_id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {src_value_base} = getelementptr [4096 x {value_ty}], ptr %map{src_id}.{value_slot_name}.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_prefix_base} = getelementptr [4096 x ptr], ptr %map{dst_id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_number_base} = getelementptr [4096 x i64], ptr %map{dst_id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_value_base} = getelementptr [4096 x {value_ty}], ptr %map{dst_id}.{value_slot_name}.slots, i64 0, i64 0\n"
    ));
    // Pre-seed the "missing" result; the helper only overwrites it on a hit.
    ir.push_str(&format!("  store {value_ty} {missing_value}, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!(
        "  {new_len} = call i64 @{helper}(ptr {src_prefix_base}, ptr {src_number_base}, ptr {src_value_base}, i64 {len}, ptr {dst_prefix_base}, ptr {dst_number_base}, ptr {dst_value_base}, ptr {prefix}, i64 {number}, ptr %r{dst}.slot, ptr %r{dst}.present.slot)\n"
    ));
    ir.push_str(&format!("  store i64 {new_len}, ptr %map{dst_id}.len.slot\n"));
    Some(())
}

fn emit_ptr_copy_loop(
    ir: &mut String,
    label: &str,
    src_base: &str,
    dst_base: &str,
    len: &str,
    list_id: usize,
    pc: usize,
) {
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.loop:\n"));
    ir.push_str(&format!(
        "  %{label}.i = phi i64 [ 0, %bb{pc} ], [ %{label}.next, %{label}.copy ]\n"
    ));
    ir.push_str(&format!("  %{label}.done = icmp uge i64 %{label}.i, {len}\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.done, label %{label}.finish, label %{label}.copy\n"
    ));
    ir.push_str(&format!("{label}.copy:\n"));
    ir.push_str(&format!(
        "  %{label}.src.slot = getelementptr ptr, ptr {src_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.slot = getelementptr ptr, ptr {dst_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  %{label}.value = load ptr, ptr %{label}.src.slot\n"));
    ir.push_str(&format!("  store ptr %{label}.value, ptr %{label}.dst.slot\n"));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.finish:\n"));
    ir.push_str(&format!("  store i64 {len}, ptr %list{list_id}.len.slot\n"));
}

fn dynamic_string_int_key_parts(
    extra_globals: &mut String,
    key: NativeStraightlineValue,
    map_id: usize,
    tmp_index: &mut usize,
) -> Option<(String, String)> {
    if let NativeStraightlineValue::String { symbol, value, .. } = key {
        if !value.is_ascii() {
            return None;
        }
        if let Some((prefix, number)) = split_static_string_int_key(&value) {
            let prefix_symbol = format!("@lk_map{map_id}_key_prefix_{}", *tmp_index);
            *tmp_index += 1;
            extra_globals.push_str(&llvm_string_constant(&prefix_symbol, prefix));
            return Some((prefix_symbol, number.to_string()));
        }
        if !symbol.is_empty() {
            return Some((symbol, "0".to_string()));
        }
        let symbol = format!("@lk_map{map_id}_key_{}", *tmp_index);
        *tmp_index += 1;
        extra_globals.push_str(&llvm_string_constant(&symbol, &value));
        return Some((symbol, "0".to_string()));
    }
    if let NativeStraightlineValue::StringPtr(value) = key {
        return Some((value, "0".to_string()));
    }
    let NativeStraightlineValue::Text(parts) = key else {
        return None;
    };
    let Some((NativeTextPart::I64(number), prefix_parts)) = parts.split_last() else {
        return None;
    };
    if prefix_parts.is_empty() {
        return None;
    }
    let mut prefix = String::new();
    for part in prefix_parts {
        let NativeTextPart::String { value, .. } = part else {
            return None;
        };
        prefix.push_str(value);
    }
    for part in prefix_parts {
        if let NativeTextPart::String { symbol, value } = part
            && *value == prefix
            && !symbol.is_empty()
        {
            return Some((symbol.clone(), number.clone()));
        }
    }
    let symbol = format!("@lk_map{map_id}_key_prefix_{}", *tmp_index);
    *tmp_index += 1;
    extra_globals.push_str(&llvm_string_constant(&symbol, &prefix));
    Some((symbol, number.clone()))
}

fn split_static_string_int_key(value: &str) -> Option<(&str, i64)> {
    let split = value
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    if split == 0 || split == value.len() {
        return None;
    }
    let (prefix, suffix) = value.split_at(split);
    let number = suffix.parse().ok()?;
    Some((prefix, number))
}
