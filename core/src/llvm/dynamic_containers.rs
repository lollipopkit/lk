mod f64_lists;
mod i64_lists;
mod i64_maps;
mod ptr_lists;
mod string_maps;

pub(super) use f64_lists::{
    emit_dynamic_f64_list_concat, emit_dynamic_f64_list_contains, emit_dynamic_f64_list_index_of,
    emit_dynamic_f64_list_insert, emit_dynamic_f64_list_pop, emit_dynamic_f64_list_push_new,
    emit_dynamic_f64_list_remove_at, emit_dynamic_f64_list_reverse, emit_dynamic_f64_list_set_new,
    emit_dynamic_f64_list_slice, emit_dynamic_f64_list_slice_range, emit_dynamic_f64_list_sort,
    emit_dynamic_f64_list_take, emit_dynamic_f64_list_unique, native_dynamic_f64_list_helpers,
};
pub(super) use i64_lists::{
    emit_dynamic_i64_list_contains, emit_dynamic_i64_list_index_of, emit_dynamic_i64_list_insert,
    emit_dynamic_i64_list_pop, emit_dynamic_i64_list_push_new, emit_dynamic_i64_list_remove_at,
    emit_dynamic_i64_list_reverse, emit_dynamic_i64_list_set_new, emit_dynamic_i64_list_slice_range,
    emit_dynamic_i64_list_sort, native_dynamic_i64_list_helpers,
};
pub(super) use i64_maps::{
    emit_dynamic_i64_f64_map_delete_key, emit_dynamic_i64_f64_map_get, emit_dynamic_i64_f64_map_get_key,
    emit_dynamic_i64_f64_map_iter_value, emit_dynamic_i64_f64_map_set, emit_dynamic_i64_f64_map_values,
    emit_dynamic_i64_int_map_delete_key, emit_dynamic_i64_int_map_get, emit_dynamic_i64_int_map_get_key,
    emit_dynamic_i64_int_map_iter_key, emit_dynamic_i64_int_map_iter_value, emit_dynamic_i64_int_map_set,
    emit_dynamic_i64_int_map_values, emit_dynamic_i64_map_has_key, emit_dynamic_i64_map_keys,
    emit_dynamic_i64_ptr_map_delete_key, emit_dynamic_i64_ptr_map_get, emit_dynamic_i64_ptr_map_get_key,
    emit_dynamic_i64_ptr_map_iter_value, emit_dynamic_i64_ptr_map_set, emit_dynamic_i64_ptr_map_values,
    native_dynamic_i64_map_helpers,
};
pub(super) use ptr_lists::{
    emit_dynamic_ptr_list_contains, emit_dynamic_ptr_list_index_of, emit_dynamic_ptr_list_insert,
    emit_dynamic_ptr_list_pop, emit_dynamic_ptr_list_push_new, emit_dynamic_ptr_list_remove_at,
    emit_dynamic_ptr_list_reverse, emit_dynamic_ptr_list_set_new, emit_dynamic_ptr_list_slice_range,
    emit_dynamic_ptr_list_sort, native_dynamic_ptr_list_helpers,
};
pub(super) use string_maps::{
    emit_dynamic_string_f64_map_delete, emit_dynamic_string_int_map_delete, emit_dynamic_string_map_has,
    emit_dynamic_string_ptr_map_delete, emit_dynamic_string_ptr_map_get, emit_dynamic_string_ptr_map_iter_value,
    emit_dynamic_string_ptr_map_set, emit_dynamic_string_ptr_map_values,
};

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
    ir.push_str(&format!("  %{name}.f64.slots = alloca [4096 x double]\n"));
    ir.push_str(&format!("  %{name}.ptr.slots = alloca [4096 x ptr]\n"));
}

pub(super) fn emit_dynamic_int_list_allocas(ir: &mut String, name: &str) {
    ir.push_str(&format!("  %{name}.len.slot = alloca i64\n"));
    ir.push_str(&format!("  %{name}.value.slots = alloca [4096 x i64]\n"));
    ir.push_str(&format!("  %{name}.f64.slots = alloca [4096 x double]\n"));
    ir.push_str(&format!("  %{name}.ptr.slots = alloca [4096 x ptr]\n"));
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

define private i64 @lk_lookup_string_f64_map(ptr %prefixes, ptr %numbers, ptr %values, i64 %len, ptr %prefix, i64 %number, ptr %out) {
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
  %value_slot = getelementptr double, ptr %values, i64 %i
  %value = load double, ptr %value_slot
  store double %value, ptr %out
  ret i64 1
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 0
}

define private i64 @lk_set_string_f64_map(ptr %prefixes, ptr %numbers, ptr %values, i64 %len, ptr %prefix, i64 %number, double %value) {
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
  %update_value_slot = getelementptr double, ptr %values, i64 %i
  store double %value, ptr %update_value_slot
  ret i64 %len
cont:
  %next = add i64 %i, 1
  br label %loop
append:
  %append_prefix_slot = getelementptr ptr, ptr %prefixes, i64 %len
  %append_number_slot = getelementptr i64, ptr %numbers, i64 %len
  %append_value_slot = getelementptr double, ptr %values, i64 %len
  store ptr %prefix, ptr %append_prefix_slot
  store i64 %number, ptr %append_number_slot
  store double %value, ptr %append_value_slot
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

define private void @lk_slice_i64_list(ptr %src_values, i64 %src_len, i64 %start, ptr %dst_values, ptr %dst_len) {
entry:
  %start_neg = icmp slt i64 %start, 0
  %start_nonneg = select i1 %start_neg, i64 0, i64 %start
  %start_over = icmp sgt i64 %start_nonneg, %src_len
  %start_clamped = select i1 %start_over, i64 %src_len, i64 %start_nonneg
  br label %loop
loop:
  %src_i = phi i64 [ %start_clamped, %entry ], [ %src_next, %copy ]
  %dst_i = phi i64 [ 0, %entry ], [ %dst_next, %copy ]
  %done = icmp uge i64 %src_i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_slot = getelementptr i64, ptr %src_values, i64 %src_i
  %value = load i64, ptr %src_slot
  %dst_slot = getelementptr i64, ptr %dst_values, i64 %dst_i
  store i64 %value, ptr %dst_slot
  %src_next = add i64 %src_i, 1
  %dst_next = add i64 %dst_i, 1
  br label %loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret void
}

define private void @lk_take_i64_list(ptr %src_values, i64 %src_len, i64 %count, ptr %dst_values, ptr %dst_len) {
entry:
  %count_neg = icmp slt i64 %count, 0
  %count_nonneg = select i1 %count_neg, i64 0, i64 %count
  %count_over = icmp sgt i64 %count_nonneg, %src_len
  %count_clamped = select i1 %count_over, i64 %src_len, i64 %count_nonneg
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %count_clamped
  br i1 %done, label %finish, label %copy
copy:
  %src_slot = getelementptr i64, ptr %src_values, i64 %i
  %value = load i64, ptr %src_slot
  %dst_slot = getelementptr i64, ptr %dst_values, i64 %i
  store i64 %value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %i, ptr %dst_len
  ret void
}

define private void @lk_concat_i64_list(ptr %lhs_values, i64 %lhs_len, ptr %rhs_values, i64 %rhs_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %lhs_loop
lhs_loop:
  %lhs_i = phi i64 [ 0, %entry ], [ %lhs_next, %lhs_copy ]
  %lhs_done = icmp uge i64 %lhs_i, %lhs_len
  br i1 %lhs_done, label %rhs_loop, label %lhs_copy
lhs_copy:
  %lhs_src_slot = getelementptr i64, ptr %lhs_values, i64 %lhs_i
  %lhs_value = load i64, ptr %lhs_src_slot
  %lhs_dst_slot = getelementptr i64, ptr %dst_values, i64 %lhs_i
  store i64 %lhs_value, ptr %lhs_dst_slot
  %lhs_next = add i64 %lhs_i, 1
  br label %lhs_loop
rhs_loop:
  %rhs_i = phi i64 [ 0, %lhs_loop ], [ %rhs_next, %rhs_copy ]
  %rhs_done = icmp uge i64 %rhs_i, %rhs_len
  br i1 %rhs_done, label %finish, label %rhs_copy
rhs_copy:
  %rhs_src_slot = getelementptr i64, ptr %rhs_values, i64 %rhs_i
  %rhs_value = load i64, ptr %rhs_src_slot
  %dst_i = add i64 %lhs_len, %rhs_i
  %rhs_dst_slot = getelementptr i64, ptr %dst_values, i64 %dst_i
  store i64 %rhs_value, ptr %rhs_dst_slot
  %rhs_next = add i64 %rhs_i, 1
  br label %rhs_loop
finish:
  %total = add i64 %lhs_len, %rhs_len
  store i64 %total, ptr %dst_len
  ret void
}

define private void @lk_slice_ptr_list(ptr %src_values, i64 %src_len, i64 %start, ptr %dst_values, ptr %dst_len) {
entry:
  %start_neg = icmp slt i64 %start, 0
  %start_nonneg = select i1 %start_neg, i64 0, i64 %start
  %start_over = icmp sgt i64 %start_nonneg, %src_len
  %start_clamped = select i1 %start_over, i64 %src_len, i64 %start_nonneg
  br label %loop
loop:
  %src_i = phi i64 [ %start_clamped, %entry ], [ %src_next, %copy ]
  %dst_i = phi i64 [ 0, %entry ], [ %dst_next, %copy ]
  %done = icmp uge i64 %src_i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_slot = getelementptr ptr, ptr %src_values, i64 %src_i
  %value = load ptr, ptr %src_slot
  %dst_slot = getelementptr ptr, ptr %dst_values, i64 %dst_i
  store ptr %value, ptr %dst_slot
  %src_next = add i64 %src_i, 1
  %dst_next = add i64 %dst_i, 1
  br label %loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret void
}

define private void @lk_take_ptr_list(ptr %src_values, i64 %src_len, i64 %count, ptr %dst_values, ptr %dst_len) {
entry:
  %count_neg = icmp slt i64 %count, 0
  %count_nonneg = select i1 %count_neg, i64 0, i64 %count
  %count_over = icmp sgt i64 %count_nonneg, %src_len
  %count_clamped = select i1 %count_over, i64 %src_len, i64 %count_nonneg
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %count_clamped
  br i1 %done, label %finish, label %copy
copy:
  %src_slot = getelementptr ptr, ptr %src_values, i64 %i
  %value = load ptr, ptr %src_slot
  %dst_slot = getelementptr ptr, ptr %dst_values, i64 %i
  store ptr %value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %i, ptr %dst_len
  ret void
}

define private void @lk_concat_ptr_list(ptr %lhs_values, i64 %lhs_len, ptr %rhs_values, i64 %rhs_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %lhs_loop
lhs_loop:
  %lhs_i = phi i64 [ 0, %entry ], [ %lhs_next, %lhs_copy ]
  %lhs_done = icmp uge i64 %lhs_i, %lhs_len
  br i1 %lhs_done, label %rhs_loop, label %lhs_copy
lhs_copy:
  %lhs_src_slot = getelementptr ptr, ptr %lhs_values, i64 %lhs_i
  %lhs_value = load ptr, ptr %lhs_src_slot
  %lhs_dst_slot = getelementptr ptr, ptr %dst_values, i64 %lhs_i
  store ptr %lhs_value, ptr %lhs_dst_slot
  %lhs_next = add i64 %lhs_i, 1
  br label %lhs_loop
rhs_loop:
  %rhs_i = phi i64 [ 0, %lhs_loop ], [ %rhs_next, %rhs_copy ]
  %rhs_done = icmp uge i64 %rhs_i, %rhs_len
  br i1 %rhs_done, label %finish, label %rhs_copy
rhs_copy:
  %rhs_src_slot = getelementptr ptr, ptr %rhs_values, i64 %rhs_i
  %rhs_value = load ptr, ptr %rhs_src_slot
  %dst_i = add i64 %lhs_len, %rhs_i
  %rhs_dst_slot = getelementptr ptr, ptr %dst_values, i64 %dst_i
  store ptr %rhs_value, ptr %rhs_dst_slot
  %rhs_next = add i64 %rhs_i, 1
  br label %rhs_loop
finish:
  %total = add i64 %lhs_len, %rhs_len
  store i64 %total, ptr %dst_len
  ret void
}

define private i64 @lk_eq_i64_list(ptr %lhs_values, i64 %lhs_len, ptr %rhs_values, i64 %rhs_len) {
entry:
  %len_eq = icmp eq i64 %lhs_len, %rhs_len
  br i1 %len_eq, label %loop, label %not_equal
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %lhs_len
  br i1 %done, label %equal, label %check
check:
  %lhs_slot = getelementptr i64, ptr %lhs_values, i64 %i
  %rhs_slot = getelementptr i64, ptr %rhs_values, i64 %i
  %lhs = load i64, ptr %lhs_slot
  %rhs = load i64, ptr %rhs_slot
  %same = icmp eq i64 %lhs, %rhs
  br i1 %same, label %cont, label %not_equal
cont:
  %next = add i64 %i, 1
  br label %loop
equal:
  ret i64 1
not_equal:
  ret i64 0
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

pub(super) fn emit_dynamic_string_f64_map_set(
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
    ir.push_str(&format!("  {value} = load double, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x double], ptr %map{id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {next_len} = call i64 @lk_set_string_f64_map(ptr {prefix_base}, ptr {number_base}, ptr {value_base}, i64 {len}, ptr {prefix}, i64 {number}, double {value})\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %map{id}.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_string_f64_map_get(
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
    ir.push_str(&format!("  store double 0.0, ptr %r{dst}.slot\n"));
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
        "  {value_base} = getelementptr [4096 x double], ptr %map{id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {found} = call i64 @lk_lookup_string_f64_map(ptr {prefix_base}, ptr {number_base}, ptr {value_base}, i64 {len}, ptr {prefix}, i64 {number}, ptr %r{dst}.slot)\n"));
    ir.push_str(&format!("  store i64 {found}, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_string_int_map_iter_key(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    dst: u8,
    index_reg: u8,
    tmp_index: &mut usize,
) -> Option<String> {
    let index = next_tmp(tmp_index);
    let key_slot = next_tmp(tmp_index);
    let key = next_tmp(tmp_index);
    let number_slot = next_tmp(tmp_index);
    let number = next_tmp(tmp_index);
    let has_number = next_tmp(tmp_index);
    let formatted = next_tmp(tmp_index);
    let fmt = format!("@lk_map_iter_key_fmt_{id}_{dst}");
    let zero_label = format!("lk.map{id}.key{dst}.zero.{}", *tmp_index);
    let format_label = format!("lk.map{id}.key{dst}.format.{}", *tmp_index);
    let done_label = format!("lk.map{id}.key{dst}.done.{}", *tmp_index);
    extra_globals.push_str(&llvm_string_constant(&fmt, "%s%ld"));
    ir.push_str(&format!("  {index} = load i64, ptr %r{index_reg}.slot\n"));
    ir.push_str(&format!(
        "  {key_slot} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {key} = load ptr, ptr {key_slot}\n"));
    ir.push_str(&format!(
        "  {number_slot} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {number} = load i64, ptr {number_slot}\n"));
    ir.push_str(&format!("  {has_number} = icmp ne i64 {number}, 0\n"));
    ir.push_str(&format!(
        "  br i1 {has_number}, label %{format_label}, label %{zero_label}\n"
    ));
    ir.push_str(&format!("{format_label}:\n"));
    ir.push_str(&format!(
        "  {formatted} = getelementptr [4096 x i8], ptr %r{dst}.text.buf, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call i32 (ptr, i64, ptr, ...) @snprintf(ptr {formatted}, i64 4096, ptr {fmt}, ptr {key}, i64 {number})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{zero_label}:\n"));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{done_label}:\n"));
    let selected = next_tmp(tmp_index);
    ir.push_str(&format!(
        "  {selected} = phi ptr [ {formatted}, %{format_label} ], [ {key}, %{zero_label} ]\n"
    ));
    ir.push_str(&format!("  store ptr {selected}, ptr %r{dst}.slot\n"));
    Some(selected)
}

pub(super) fn emit_dynamic_string_int_map_iter_value(
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
        "  {value_slot} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {value} = load i64, ptr {value_slot}\n"));
    ir.push_str(&format!("  store i64 {value}, ptr %r{dst}.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_string_f64_map_iter_value(
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
        "  {value_slot} = getelementptr [4096 x double], ptr %map{id}.f64.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {value} = load double, ptr {value_slot}\n"));
    ir.push_str(&format!("  store double {value}, ptr %r{dst}.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_string_int_map_values(
    ir: &mut String,
    map_id: usize,
    list_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let map_base = next_tmp(tmp_index);
    let list_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{map_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {map_base} = getelementptr [4096 x i64], ptr %map{map_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {list_base} = getelementptr [4096 x i64], ptr %list{list_id}.value.slots, i64 0, i64 0\n"
    ));
    emit_dynamic_value_copy_loop(ir, map_base, list_base, len.clone(), "i64", list_id, tmp_index);
    Some(())
}

pub(super) fn emit_dynamic_string_f64_map_values(
    ir: &mut String,
    map_id: usize,
    list_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let map_base = next_tmp(tmp_index);
    let list_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{map_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {map_base} = getelementptr [4096 x double], ptr %map{map_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {list_base} = getelementptr [4096 x double], ptr %list{list_id}.f64.slots, i64 0, i64 0\n"
    ));
    emit_dynamic_value_copy_loop(ir, map_base, list_base, len.clone(), "double", list_id, tmp_index);
    Some(())
}

pub(super) fn emit_dynamic_string_map_keys(
    ir: &mut String,
    extra_globals: &mut String,
    map_id: usize,
    list_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let temp = next_tmp(tmp_index);
    let fmt = format!("@lk_map_keys_fmt_{map_id}_{list_id}");
    let label = format!("lk.copy.map.keys.{}", *tmp_index);
    *tmp_index += 1;
    extra_globals.push_str(&llvm_string_constant(&fmt, "%s%ld"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{map_id}.len.slot\n"));
    ir.push_str(&format!("  store i64 {len}, ptr %list{list_id}.len.slot\n"));
    ir.push_str(&format!("  {temp} = alloca [4096 x i8]\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.loop:\n"));
    ir.push_str(&format!(
        "  %{label}.i = phi i64 [ 0, %bb{list_id} ], [ %{label}.next, %{label}.store ]\n"
    ));
    ir.push_str(&format!("  %{label}.done = icmp uge i64 %{label}.i, {len}\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.done, label %{label}.finish, label %{label}.item\n"
    ));
    ir.push_str(&format!("{label}.item:\n"));
    ir.push_str(&format!(
        "  %{label}.prefix.slot = getelementptr [4096 x ptr], ptr %map{map_id}.prefix.slots, i64 0, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  %{label}.prefix = load ptr, ptr %{label}.prefix.slot\n"));
    ir.push_str(&format!(
        "  %{label}.number.slot = getelementptr [4096 x i64], ptr %map{map_id}.number.slots, i64 0, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  %{label}.number = load i64, ptr %{label}.number.slot\n"));
    ir.push_str(&format!("  %{label}.has.number = icmp ne i64 %{label}.number, 0\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.has.number, label %{label}.format, label %{label}.prefix.only\n"
    ));
    ir.push_str(&format!("{label}.format:\n"));
    ir.push_str(&format!(
        "  %{label}.buf = getelementptr [4096 x i8], ptr {temp}, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call i32 (ptr, i64, ptr, ...) @snprintf(ptr %{label}.buf, i64 4096, ptr {fmt}, ptr %{label}.prefix, i64 %{label}.number)\n"
    ));
    ir.push_str(&format!("  %{label}.copy = call ptr @strdup(ptr %{label}.buf)\n"));
    ir.push_str(&format!("  br label %{label}.store\n"));
    ir.push_str(&format!("{label}.prefix.only:\n"));
    ir.push_str(&format!("  br label %{label}.store\n"));
    ir.push_str(&format!("{label}.store:\n"));
    ir.push_str(&format!(
        "  %{label}.key = phi ptr [ %{label}.copy, %{label}.format ], [ %{label}.prefix, %{label}.prefix.only ]\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst = getelementptr [4096 x ptr], ptr %list{list_id}.ptr.slots, i64 0, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  store ptr %{label}.key, ptr %{label}.dst\n"));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.finish:\n"));
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

pub(super) fn emit_dynamic_f64_list_push(
    ir: &mut String,
    id: usize,
    value_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let value = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let value_slot = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load double, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {value_slot} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 {len}\n"
    ));
    ir.push_str(&format!("  store double {value}, ptr {value_slot}\n"));
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

pub(super) fn emit_dynamic_text_list_push_len(ir: &mut String, id: usize, item_len: &str, tmp_index: &mut usize) {
    let len = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    let text_len = next_tmp(tmp_index);
    let next_text_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  {next_len} = add i64 {len}, 1\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  {text_len} = load i64, ptr %list{id}.text.len.slot\n"));
    ir.push_str(&format!("  {next_text_len} = add i64 {text_len}, {item_len}\n"));
    ir.push_str(&format!("  store i64 {next_text_len}, ptr %list{id}.text.len.slot\n"));
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

pub(super) fn emit_dynamic_f64_list_get(
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
        "  {value_slot} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {value} = load double, ptr {value_slot}\n"));
    ir.push_str(&format!("  store double {value}, ptr %r{dst}.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_ptr_list_get(
    ir: &mut String,
    id: usize,
    dst: u8,
    index_reg: u8,
    tmp_index: &mut usize,
) -> Option<String> {
    let index = next_tmp(tmp_index);
    let value_slot = next_tmp(tmp_index);
    let value = next_tmp(tmp_index);
    ir.push_str(&format!("  {index} = load i64, ptr %r{index_reg}.slot\n"));
    ir.push_str(&format!(
        "  {value_slot} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {value} = load ptr, ptr {value_slot}\n"));
    ir.push_str(&format!("  store ptr {value}, ptr %r{dst}.slot\n"));
    Some(value)
}

pub(super) fn emit_dynamic_ptr_list_push(
    ir: &mut String,
    id: usize,
    value_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let value = next_tmp(tmp_index);
    ir.push_str(&format!("  {value} = load ptr, ptr %r{value_reg}.slot\n"));
    emit_dynamic_ptr_list_push_value(ir, id, &value, tmp_index)
}

pub(super) fn emit_dynamic_ptr_list_push_value(
    ir: &mut String,
    id: usize,
    value: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let copy = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let value_slot = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {copy} = call ptr @strdup(ptr {value})\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {value_slot} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 {len}\n"
    ));
    ir.push_str(&format!("  store ptr {copy}, ptr {value_slot}\n"));
    ir.push_str(&format!("  {next_len} = add i64 {len}, 1\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %list{id}.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_pair_list_push(
    ir: &mut String,
    id: usize,
    first: &NativeStraightlineValue,
    second: &NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let first = native_value_expr(first)?;
    let second = native_value_expr(second)?;
    if !native_pair_values_have_distinct_storage(&first, &second) {
        return None;
    }
    let len = next_tmp(tmp_index);
    let first_slot = next_tmp(tmp_index);
    let second_slot = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    match first {
        NativeValueExpr::I64(value) => {
            ir.push_str(&format!(
                "  {first_slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {len}\n"
            ));
            ir.push_str(&format!("  store i64 {value}, ptr {first_slot}\n"));
        }
        NativeValueExpr::F64(value) => {
            ir.push_str(&format!(
                "  {first_slot} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 {len}\n"
            ));
            ir.push_str(&format!("  store double {value}, ptr {first_slot}\n"));
        }
        NativeValueExpr::Ptr(value) => {
            let first_copy = next_tmp(tmp_index);
            ir.push_str(&format!("  {first_copy} = call ptr @strdup(ptr {value})\n"));
            ir.push_str(&format!(
                "  {first_slot} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 {len}\n"
            ));
            ir.push_str(&format!("  store ptr {first_copy}, ptr {first_slot}\n"));
        }
    }
    match second {
        NativeValueExpr::I64(value) => {
            ir.push_str(&format!(
                "  {second_slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {len}\n"
            ));
            ir.push_str(&format!("  store i64 {value}, ptr {second_slot}\n"));
        }
        NativeValueExpr::F64(value) => {
            ir.push_str(&format!(
                "  {second_slot} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 {len}\n"
            ));
            ir.push_str(&format!("  store double {value}, ptr {second_slot}\n"));
        }
        NativeValueExpr::Ptr(_) => return None,
    }
    ir.push_str(&format!("  {next_len} = add i64 {len}, 1\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %list{id}.len.slot\n"));
    Some(())
}

fn native_pair_values_have_distinct_storage(first: &NativeValueExpr, second: &NativeValueExpr) -> bool {
    !matches!(
        (first, second),
        (NativeValueExpr::I64(_), NativeValueExpr::I64(_))
            | (NativeValueExpr::F64(_), NativeValueExpr::F64(_))
            | (NativeValueExpr::Ptr(_), NativeValueExpr::Ptr(_))
    )
}

fn emit_dynamic_value_copy_loop(
    ir: &mut String,
    src_base: String,
    dst_base: String,
    len: String,
    element_ty: &str,
    list_id: usize,
    tmp_index: &mut usize,
) {
    let entry_label = format!("lk.copy.values.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  store i64 {len}, ptr %list{list_id}.len.slot\n"));
    ir.push_str(&format!("  br label %{entry_label}.loop\n"));
    ir.push_str(&format!("{entry_label}.loop:\n"));
    ir.push_str(&format!(
        "  %{entry_label}.i = phi i64 [ 0, %bb{list_id} ], [ %{entry_label}.next, %{entry_label}.copy ]\n"
    ));
    ir.push_str(&format!(
        "  %{entry_label}.done = icmp uge i64 %{entry_label}.i, {len}\n"
    ));
    ir.push_str(&format!(
        "  br i1 %{entry_label}.done, label %{entry_label}.finish, label %{entry_label}.copy\n"
    ));
    ir.push_str(&format!("{entry_label}.copy:\n"));
    ir.push_str(&format!(
        "  %{entry_label}.src = getelementptr {element_ty}, ptr {src_base}, i64 %{entry_label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{entry_label}.value = load {element_ty}, ptr %{entry_label}.src\n"
    ));
    ir.push_str(&format!(
        "  %{entry_label}.dst = getelementptr {element_ty}, ptr {dst_base}, i64 %{entry_label}.i\n"
    ));
    ir.push_str(&format!(
        "  store {element_ty} %{entry_label}.value, ptr %{entry_label}.dst\n"
    ));
    ir.push_str(&format!("  %{entry_label}.next = add i64 %{entry_label}.i, 1\n"));
    ir.push_str(&format!("  br label %{entry_label}.loop\n"));
    ir.push_str(&format!("{entry_label}.finish:\n"));
}

pub(super) fn emit_dynamic_ptr_list_slice(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    start_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let start = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!("  {start} = load i64, ptr %r{start_reg}.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_slice_ptr_list(ptr {src_base}, i64 {src_len}, i64 {start}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(super) fn emit_dynamic_ptr_list_take(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    count_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let count = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!("  {count} = load i64, ptr %r{count_reg}.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_take_ptr_list(ptr {src_base}, i64 {src_len}, i64 {count}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(super) fn emit_dynamic_ptr_list_concat(
    ir: &mut String,
    lhs_id: usize,
    rhs_id: usize,
    dst_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let lhs_len = next_tmp(tmp_index);
    let rhs_len = next_tmp(tmp_index);
    let lhs_base = next_tmp(tmp_index);
    let rhs_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_len} = load i64, ptr %list{lhs_id}.len.slot\n"));
    ir.push_str(&format!("  {rhs_len} = load i64, ptr %list{rhs_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {lhs_base} = getelementptr [4096 x ptr], ptr %list{lhs_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {rhs_base} = getelementptr [4096 x ptr], ptr %list{rhs_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_concat_ptr_list(ptr {lhs_base}, i64 {lhs_len}, ptr {rhs_base}, i64 {rhs_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(super) fn emit_dynamic_ptr_list_copy(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_slice_ptr_list(ptr {src_base}, i64 {src_len}, i64 0, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(super) fn emit_dynamic_int_list_set(
    ir: &mut String,
    id: usize,
    index_reg: u8,
    value_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let index = next_tmp(tmp_index);
    let value = next_tmp(tmp_index);
    let value_slot = next_tmp(tmp_index);
    ir.push_str(&format!("  {index} = load i64, ptr %r{index_reg}.slot\n"));
    ir.push_str(&format!("  {value} = load i64, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!(
        "  {value_slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  store i64 {value}, ptr {value_slot}\n"));
    Some(())
}

pub(super) fn emit_dynamic_int_list_slice(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    start_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let start = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!("  {start} = load i64, ptr %r{start_reg}.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_slice_i64_list(ptr {src_base}, i64 {src_len}, i64 {start}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_int_list_take(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    count_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let count = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!("  {count} = load i64, ptr %r{count_reg}.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_take_i64_list(ptr {src_base}, i64 {src_len}, i64 {count}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_int_list_concat(
    ir: &mut String,
    lhs_id: usize,
    rhs_id: usize,
    dst_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let lhs_len = next_tmp(tmp_index);
    let rhs_len = next_tmp(tmp_index);
    let lhs_base = next_tmp(tmp_index);
    let rhs_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_len} = load i64, ptr %list{lhs_id}.len.slot\n"));
    ir.push_str(&format!("  {rhs_len} = load i64, ptr %list{rhs_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {lhs_base} = getelementptr [4096 x i64], ptr %list{lhs_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {rhs_base} = getelementptr [4096 x i64], ptr %list{rhs_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_concat_i64_list(ptr {lhs_base}, i64 {lhs_len}, ptr {rhs_base}, i64 {rhs_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_int_list_copy(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_slice_i64_list(ptr {src_base}, i64 {src_len}, i64 0, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(super) fn emit_dynamic_int_list_equality(
    ir: &mut String,
    lhs_id: usize,
    rhs_id: usize,
    dst: u8,
    not_equal: bool,
    tmp_index: &mut usize,
) -> Option<()> {
    let lhs_len = next_tmp(tmp_index);
    let rhs_len = next_tmp(tmp_index);
    let lhs_base = next_tmp(tmp_index);
    let rhs_base = next_tmp(tmp_index);
    let equal = next_tmp(tmp_index);
    let value = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_len} = load i64, ptr %list{lhs_id}.len.slot\n"));
    ir.push_str(&format!("  {rhs_len} = load i64, ptr %list{rhs_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {lhs_base} = getelementptr [4096 x i64], ptr %list{lhs_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {rhs_base} = getelementptr [4096 x i64], ptr %list{rhs_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {equal} = call i64 @lk_eq_i64_list(ptr {lhs_base}, i64 {lhs_len}, ptr {rhs_base}, i64 {rhs_len})\n"
    ));
    if not_equal {
        ir.push_str(&format!("  {value} = xor i64 {equal}, 1\n"));
    } else {
        ir.push_str(&format!("  {value} = add i64 {equal}, 0\n"));
    }
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
            NativeTextPart::StrPtr(_) => len += 1,
            _ => return None,
        }
    }
    Some(len.to_string())
}

enum NativeValueExpr {
    I64(String),
    F64(String),
    Ptr(String),
}

fn native_value_expr(value: &NativeStraightlineValue) -> Option<NativeValueExpr> {
    match value {
        NativeStraightlineValue::I64(value) | NativeStraightlineValue::Bool(value) => {
            Some(NativeValueExpr::I64(value.clone()))
        }
        NativeStraightlineValue::F64(value) => Some(NativeValueExpr::F64(value.clone())),
        NativeStraightlineValue::String { symbol, .. } | NativeStraightlineValue::StringPtr(symbol) => {
            Some(NativeValueExpr::Ptr(symbol.clone()))
        }
        _ => None,
    }
}
