use crate::llvm::ir_text::next_tmp;

pub(in crate::llvm) fn native_dynamic_i64_list_helpers() -> &'static str {
    r#"
define private i64 @lk_contains_i64_list(ptr %values, i64 %len, i64 %needle) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %slot = getelementptr i64, ptr %values, i64 %i
  %value = load i64, ptr %slot
  %matched = icmp eq i64 %value, %needle
  br i1 %matched, label %found, label %cont
found:
  ret i64 1
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 0
}

define private i64 @lk_index_of_i64_list(ptr %values, i64 %len, i64 %needle) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %slot = getelementptr i64, ptr %values, i64 %i
  %value = load i64, ptr %slot
  %matched = icmp eq i64 %value, %needle
  br i1 %matched, label %found, label %cont
found:
  ret i64 %i
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 -1
}

define private void @lk_reverse_i64_list(ptr %src_values, i64 %src_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_index_tmp = sub i64 %src_len, 1
  %src_index = sub i64 %src_index_tmp, %i
  %src_slot = getelementptr i64, ptr %src_values, i64 %src_index
  %value = load i64, ptr %src_slot
  %dst_slot = getelementptr i64, ptr %dst_values, i64 %i
  store i64 %value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %src_len, ptr %dst_len
  ret void
}

define private void @lk_sort_i64_list(ptr %src_values, i64 %src_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %copy_loop
copy_loop:
  %copy_i = phi i64 [ 0, %entry ], [ %copy_next, %copy_body ]
  %copy_done = icmp uge i64 %copy_i, %src_len
  br i1 %copy_done, label %outer_loop, label %copy_body
copy_body:
  %copy_src_slot = getelementptr i64, ptr %src_values, i64 %copy_i
  %copy_value = load i64, ptr %copy_src_slot
  %copy_dst_slot = getelementptr i64, ptr %dst_values, i64 %copy_i
  store i64 %copy_value, ptr %copy_dst_slot
  %copy_next = add i64 %copy_i, 1
  br label %copy_loop
outer_loop:
  %i = phi i64 [ 1, %copy_loop ], [ %outer_next, %insert_done ]
  %outer_done = icmp uge i64 %i, %src_len
  br i1 %outer_done, label %finish, label %outer_body
outer_body:
  %key_slot = getelementptr i64, ptr %dst_values, i64 %i
  %key = load i64, ptr %key_slot
  br label %inner_loop
inner_loop:
  %j = phi i64 [ %i, %outer_body ], [ %j_prev, %shift ]
  %has_prev = icmp ugt i64 %j, 0
  br i1 %has_prev, label %check_prev, label %insert_key
check_prev:
  %j_prev = sub i64 %j, 1
  %prev_slot = getelementptr i64, ptr %dst_values, i64 %j_prev
  %prev = load i64, ptr %prev_slot
  %greater = icmp sgt i64 %prev, %key
  br i1 %greater, label %shift, label %insert_key
shift:
  %dst_slot = getelementptr i64, ptr %dst_values, i64 %j
  store i64 %prev, ptr %dst_slot
  br label %inner_loop
insert_key:
  %insert_slot = getelementptr i64, ptr %dst_values, i64 %j
  store i64 %key, ptr %insert_slot
  br label %insert_done
insert_done:
  %outer_next = add i64 %i, 1
  br label %outer_loop
finish:
  store i64 %src_len, ptr %dst_len
  ret void
}

define private i64 @lk_pop_i64_list(ptr %values, i64 %len) {
entry:
  %empty = icmp eq i64 %len, 0
  br i1 %empty, label %missing, label %found
found:
  %index = sub i64 %len, 1
  %slot = getelementptr i64, ptr %values, i64 %index
  %value = load i64, ptr %slot
  ret i64 %value
missing:
  ret i64 0
}

define private void @lk_slice_range_i64_list(ptr %src_values, i64 %src_len, i64 %start, i64 %end, ptr %dst_values, ptr %dst_len) {
entry:
  %start_neg = icmp slt i64 %start, 0
  %start_nonneg = select i1 %start_neg, i64 0, i64 %start
  %start_over = icmp sgt i64 %start_nonneg, %src_len
  %start_clamped = select i1 %start_over, i64 %src_len, i64 %start_nonneg
  %end_neg = icmp slt i64 %end, 0
  %end_nonneg = select i1 %end_neg, i64 0, i64 %end
  %end_over = icmp sgt i64 %end_nonneg, %src_len
  %end_len_clamped = select i1 %end_over, i64 %src_len, i64 %end_nonneg
  %end_before_start = icmp slt i64 %end_len_clamped, %start_clamped
  %end_clamped = select i1 %end_before_start, i64 %start_clamped, i64 %end_len_clamped
  br label %loop
loop:
  %src_i = phi i64 [ %start_clamped, %entry ], [ %src_next, %copy ]
  %dst_i = phi i64 [ 0, %entry ], [ %dst_next, %copy ]
  %done = icmp uge i64 %src_i, %end_clamped
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

define private void @lk_push_i64_list(ptr %src_values, i64 %src_len, i64 %value, ptr %dst_values, ptr %dst_len) {
entry:
  br label %copy_loop
copy_loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %append, label %copy
copy:
  %src_slot = getelementptr i64, ptr %src_values, i64 %i
  %src_value = load i64, ptr %src_slot
  %dst_slot = getelementptr i64, ptr %dst_values, i64 %i
  store i64 %src_value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %copy_loop
append:
  %append_slot = getelementptr i64, ptr %dst_values, i64 %src_len
  store i64 %value, ptr %append_slot
  %next_len = add i64 %src_len, 1
  store i64 %next_len, ptr %dst_len
  ret void
}

define private void @lk_insert_i64_list(ptr %src_values, i64 %src_len, i64 %index, i64 %value, ptr %dst_values, ptr %dst_len) {
entry:
  %index_neg = icmp slt i64 %index, 0
  %index_nonneg = select i1 %index_neg, i64 0, i64 %index
  %index_over = icmp sgt i64 %index_nonneg, %src_len
  %index_clamped = select i1 %index_over, i64 %src_len, i64 %index_nonneg
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %insert, label %check
check:
  %before_insert = icmp ult i64 %i, %index_clamped
  br i1 %before_insert, label %copy_before, label %copy_after
copy_before:
  %before_src_slot = getelementptr i64, ptr %src_values, i64 %i
  %before_value = load i64, ptr %before_src_slot
  %before_dst_slot = getelementptr i64, ptr %dst_values, i64 %i
  store i64 %before_value, ptr %before_dst_slot
  br label %cont
copy_after:
  %after_src_slot = getelementptr i64, ptr %src_values, i64 %i
  %after_value = load i64, ptr %after_src_slot
  %after_dst_i = add i64 %i, 1
  %after_dst_slot = getelementptr i64, ptr %dst_values, i64 %after_dst_i
  store i64 %after_value, ptr %after_dst_slot
  br label %cont
cont:
  %next = add i64 %i, 1
  br label %loop
insert:
  %insert_slot = getelementptr i64, ptr %dst_values, i64 %index_clamped
  store i64 %value, ptr %insert_slot
  %next_len = add i64 %src_len, 1
  store i64 %next_len, ptr %dst_len
  ret void
}

define private i64 @lk_remove_at_i64_list(ptr %src_values, i64 %src_len, i64 %index, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %dst_i = phi i64 [ 0, %entry ], [ %dst_next, %cont ]
  %old = phi i64 [ 0, %entry ], [ %old_next, %cont ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %check
check:
  %is_removed = icmp eq i64 %i, %index
  br i1 %is_removed, label %removed, label %copy
removed:
  %removed_slot = getelementptr i64, ptr %src_values, i64 %i
  %removed_value = load i64, ptr %removed_slot
  br label %cont
copy:
  %src_slot = getelementptr i64, ptr %src_values, i64 %i
  %value = load i64, ptr %src_slot
  %dst_slot = getelementptr i64, ptr %dst_values, i64 %dst_i
  store i64 %value, ptr %dst_slot
  %dst_copy_next = add i64 %dst_i, 1
  br label %cont
cont:
  %old_next = phi i64 [ %removed_value, %removed ], [ %old, %copy ]
  %dst_next = phi i64 [ %dst_i, %removed ], [ %dst_copy_next, %copy ]
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret i64 %old
}

define private i64 @lk_set_i64_list(ptr %src_values, i64 %src_len, i64 %index, i64 %value, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %old = phi i64 [ 0, %entry ], [ %next_old, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_slot = getelementptr i64, ptr %src_values, i64 %i
  %src_value = load i64, ptr %src_slot
  %matched = icmp eq i64 %i, %index
  %dst_value = select i1 %matched, i64 %value, i64 %src_value
  %next_old = select i1 %matched, i64 %src_value, i64 %old
  %dst_slot = getelementptr i64, ptr %dst_values, i64 %i
  store i64 %dst_value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %src_len, ptr %dst_len
  ret i64 %old
}
"#
}

pub(in crate::llvm) fn emit_dynamic_i64_list_contains(
    ir: &mut String,
    src_id: usize,
    dst_reg: u8,
    needle: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let result = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call i64 @lk_contains_i64_list(ptr {src_base}, i64 {src_len}, i64 {needle})\n"
    ));
    ir.push_str(&format!("  store i64 {result}, ptr %r{dst_reg}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_list_index_of(
    ir: &mut String,
    src_id: usize,
    dst_reg: u8,
    needle: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let result = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call i64 @lk_index_of_i64_list(ptr {src_base}, i64 {src_len}, i64 {needle})\n"
    ));
    ir.push_str(&format!("  store i64 {result}, ptr %r{dst_reg}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_list_reverse(
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
        "  call void @lk_reverse_i64_list(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_list_sort(
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
        "  call void @lk_sort_i64_list(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_list_pop(
    ir: &mut String,
    src_id: usize,
    dst_reg: u8,
    tmp_index: &mut usize,
) -> Option<String> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let result = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call i64 @lk_pop_i64_list(ptr {src_base}, i64 {src_len})\n"
    ));
    ir.push_str(&format!("  store i64 {result}, ptr %r{dst_reg}.slot\n"));
    Some(result)
}

pub(in crate::llvm) fn emit_dynamic_i64_list_push_new(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    value: &str,
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
        "  call void @lk_push_i64_list(ptr {src_base}, i64 {src_len}, i64 {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_list_slice_range(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    start: &str,
    end: Option<&str>,
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
    let end = end.unwrap_or(src_len.as_str());
    ir.push_str(&format!(
        "  call void @lk_slice_range_i64_list(ptr {src_base}, i64 {src_len}, i64 {start}, i64 {end}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_list_insert(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    index: &str,
    value: &str,
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
        "  call void @lk_insert_i64_list(ptr {src_base}, i64 {src_len}, i64 {index}, i64 {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_list_remove_at(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    index: &str,
    tmp_index: &mut usize,
) -> Option<String> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    let removed = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {removed} = call i64 @lk_remove_at_i64_list(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(removed)
}

pub(in crate::llvm) fn emit_dynamic_i64_list_set_new(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    index: &str,
    value: &str,
    tmp_index: &mut usize,
) -> Option<String> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let dst_base = next_tmp(tmp_index);
    let old = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x i64], ptr %list{src_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x i64], ptr %list{dst_id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {old} = call i64 @lk_set_i64_list(ptr {src_base}, i64 {src_len}, i64 {index}, i64 {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(old)
}
