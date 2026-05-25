use super::{
    const_display::llvm_string_constant,
    ir_text::next_tmp,
    straightline_value::{NativeStraightlineValue, NativeTextPart},
};

pub(super) fn emit_dynamic_string_int_map_allocas(ir: &mut String, name: &str) {
    ir.push_str(&format!("  %{name}.len.slot = alloca i64\n"));
    ir.push_str(&format!("  %{name}.prefix.slots = alloca [4096 x ptr]\n"));
    ir.push_str(&format!("  %{name}.number.slots = alloca [4096 x i64]\n"));
    ir.push_str(&format!("  %{name}.value.slots = alloca [4096 x i64]\n"));
}

pub(super) fn emit_dynamic_int_list_allocas(ir: &mut String, name: &str) {
    ir.push_str(&format!("  %{name}.len.slot = alloca i64\n"));
    ir.push_str(&format!("  %{name}.value.slots = alloca [4096 x i64]\n"));
    ir.push_str(&format!("  %{name}.text.len.slot = alloca i64\n"));
}

pub(super) fn native_dynamic_container_helpers() -> &'static str {
    r#"
define private i64 @lk_lookup_string_int_map(ptr %prefixes, ptr %numbers, ptr %values, i64 %len, ptr %prefix, i64 %number, ptr %out) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %prefix_slot = getelementptr ptr, ptr %prefixes, i64 %i
  %stored_prefix = load ptr, ptr %prefix_slot
  %prefix_cmp = call i32 @strcmp(ptr %stored_prefix, ptr %prefix)
  %prefix_eq = icmp eq i32 %prefix_cmp, 0
  %number_slot = getelementptr i64, ptr %numbers, i64 %i
  %stored_number = load i64, ptr %number_slot
  %number_eq = icmp eq i64 %stored_number, %number
  %matched = and i1 %prefix_eq, %number_eq
  br i1 %matched, label %found, label %cont
found:
  %value_slot = getelementptr i64, ptr %values, i64 %i
  %value = load i64, ptr %value_slot
  store i64 %value, ptr %out
  ret i64 1
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 0
}

define private i64 @lk_set_string_int_map(ptr %prefixes, ptr %numbers, ptr %values, i64 %len, ptr %prefix, i64 %number, i64 %value) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %append, label %check
check:
  %prefix_slot = getelementptr ptr, ptr %prefixes, i64 %i
  %stored_prefix = load ptr, ptr %prefix_slot
  %prefix_cmp = call i32 @strcmp(ptr %stored_prefix, ptr %prefix)
  %prefix_eq = icmp eq i32 %prefix_cmp, 0
  %number_slot = getelementptr i64, ptr %numbers, i64 %i
  %stored_number = load i64, ptr %number_slot
  %number_eq = icmp eq i64 %stored_number, %number
  %matched = and i1 %prefix_eq, %number_eq
  br i1 %matched, label %update, label %cont
update:
  %update_value_slot = getelementptr i64, ptr %values, i64 %i
  store i64 %value, ptr %update_value_slot
  ret i64 %len
cont:
  %next = add i64 %i, 1
  br label %loop
append:
  %append_prefix_slot = getelementptr ptr, ptr %prefixes, i64 %len
  %append_number_slot = getelementptr i64, ptr %numbers, i64 %len
  %append_value_slot = getelementptr i64, ptr %values, i64 %len
  store ptr %prefix, ptr %append_prefix_slot
  store i64 %number, ptr %append_number_slot
  store i64 %value, ptr %append_value_slot
  %next_len = add i64 %len, 1
  ret i64 %next_len
}

define private i64 @lk_i64_decimal_len(i64 %value) {
entry:
  %is_zero = icmp eq i64 %value, 0
  br i1 %is_zero, label %zero, label %nonzero
zero:
  ret i64 1
nonzero:
  %is_negative = icmp slt i64 %value, 0
  %initial_len = select i1 %is_negative, i64 1, i64 0
  br i1 %is_negative, label %neg_loop, label %pos_loop
pos_loop:
  %pos_value = phi i64 [ %value, %nonzero ], [ %pos_next, %pos_continue ]
  %pos_len = phi i64 [ %initial_len, %nonzero ], [ %pos_len_next, %pos_continue ]
  %pos_len_next = add i64 %pos_len, 1
  %pos_done = icmp slt i64 %pos_value, 10
  br i1 %pos_done, label %pos_ret, label %pos_continue
pos_continue:
  %pos_next = sdiv i64 %pos_value, 10
  br label %pos_loop
pos_ret:
  ret i64 %pos_len_next
neg_loop:
  %neg_value = phi i64 [ %value, %nonzero ], [ %neg_next, %neg_continue ]
  %neg_len = phi i64 [ %initial_len, %nonzero ], [ %neg_len_next, %neg_continue ]
  %neg_len_next = add i64 %neg_len, 1
  %neg_done = icmp sgt i64 %neg_value, -10
  br i1 %neg_done, label %neg_ret, label %neg_continue
neg_continue:
  %neg_next = sdiv i64 %neg_value, 10
  br label %neg_loop
neg_ret:
  ret i64 %neg_len_next
}
"#
}

pub(super) fn emit_dynamic_string_int_map_set(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    value_reg: u8,
    key: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let value = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load i64, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {next_len} = call i64 @lk_set_string_int_map(ptr {prefix_base}, ptr {number_base}, ptr {value_base}, i64 {len}, ptr {prefix}, i64 {number}, i64 {value})\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %map{id}.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_string_int_map_get(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let len = next_tmp(tmp_index);
    let found = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.slot\n"));
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {found} = call i64 @lk_lookup_string_int_map(ptr {prefix_base}, ptr {number_base}, ptr {value_base}, i64 {len}, ptr {prefix}, i64 {number}, ptr %r{dst}.slot)\n"));
    ir.push_str(&format!("  store i64 {found}, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_int_list_push(
    ir: &mut String,
    id: usize,
    value_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let value = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let value_slot = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load i64, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {value_slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {len}\n"
    ));
    ir.push_str(&format!("  store i64 {value}, ptr {value_slot}\n"));
    ir.push_str(&format!("  {next_len} = add i64 {len}, 1\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %list{id}.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_text_list_push(
    ir: &mut String,
    id: usize,
    value: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let NativeStraightlineValue::Text(parts) = value else {
        return None;
    };
    let len = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    let text_len = next_tmp(tmp_index);
    let next_text_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  {next_len} = add i64 {len}, 1\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  {text_len} = load i64, ptr %list{id}.text.len.slot\n"));
    let part_len = emit_dynamic_text_len_value(ir, &parts, tmp_index)?;
    ir.push_str(&format!("  {next_text_len} = add i64 {text_len}, {part_len}\n"));
    ir.push_str(&format!("  store i64 {next_text_len}, ptr %list{id}.text.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_int_list_get(
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
        "  {value_slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {value} = load i64, ptr {value_slot}\n"));
    ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_text_len(
    ir: &mut String,
    dst: u8,
    parts: &[NativeTextPart],
    tmp_index: &mut usize,
) -> Option<()> {
    let total = emit_dynamic_text_len_value(ir, parts, tmp_index)?;
    ir.push_str(&format!("  store i64 {total}, ptr %r{dst}.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_joined_text_len(
    ir: &mut String,
    dst: u8,
    id: usize,
    delimiter_len: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let list_len = next_tmp(tmp_index);
    let text_len = next_tmp(tmp_index);
    let has_delimiters = next_tmp(tmp_index);
    let delimiter_base = next_tmp(tmp_index);
    let delimiter_count = next_tmp(tmp_index);
    let delimiter_total = next_tmp(tmp_index);
    let total = next_tmp(tmp_index);
    ir.push_str(&format!("  {list_len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  {text_len} = load i64, ptr %list{id}.text.len.slot\n"));
    ir.push_str(&format!("  {has_delimiters} = icmp sgt i64 {list_len}, 1\n"));
    ir.push_str(&format!(
        "  {delimiter_base} = select i1 {has_delimiters}, i64 {list_len}, i64 1\n"
    ));
    ir.push_str(&format!("  {delimiter_count} = sub i64 {delimiter_base}, 1\n"));
    ir.push_str(&format!(
        "  {delimiter_total} = mul i64 {delimiter_count}, {delimiter_len}\n"
    ));
    ir.push_str(&format!("  {total} = add i64 {text_len}, {delimiter_total}\n"));
    ir.push_str(&format!("  store i64 {total}, ptr %r{dst}.slot\n"));
    Some(())
}

fn emit_dynamic_text_len_value(ir: &mut String, parts: &[NativeTextPart], tmp_index: &mut usize) -> Option<String> {
    let mut total = static_text_len_prefix(parts)?;
    for part in parts {
        if let NativeTextPart::I64(value) = part {
            let len = next_tmp(tmp_index);
            let next_total = next_tmp(tmp_index);
            ir.push_str(&format!("  {len} = call i64 @lk_i64_decimal_len(i64 {value})\n"));
            ir.push_str(&format!("  {next_total} = add i64 {total}, {len}\n"));
            total = next_total;
        }
    }
    Some(total)
}

fn dynamic_string_int_key_parts(
    extra_globals: &mut String,
    key: NativeStraightlineValue,
    map_id: usize,
    tmp_index: &mut usize,
) -> Option<(String, String)> {
    if let NativeStraightlineValue::String { symbol, value, .. } = key {
        if !value.is_ascii() || symbol.is_empty() {
            return None;
        }
        return Some((symbol, "0".to_string()));
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
        {
            return Some((symbol.clone(), number.clone()));
        }
    }
    let symbol = format!("@lk_map{map_id}_key_prefix_{}", *tmp_index);
    *tmp_index += 1;
    extra_globals.push_str(&llvm_string_constant(&symbol, &prefix));
    Some((symbol, number.clone()))
}

fn static_text_len_prefix(parts: &[NativeTextPart]) -> Option<String> {
    let mut len = 0usize;
    for part in parts {
        match part {
            NativeTextPart::String { value, .. } => {
                if !value.is_ascii() {
                    return None;
                }
                len += value.len();
            }
            NativeTextPart::I64(_) => {}
            _ => return None,
        }
    }
    Some(len.to_string())
}
