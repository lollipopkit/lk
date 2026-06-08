use crate::llvm::ir_text::next_tmp;

pub(in crate::llvm) fn native_dynamic_ptr_list_helpers() -> &'static str {
    r#"
define private i64 @lk_contains_ptr_list(ptr %values, i64 %len, ptr %needle) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %slot = getelementptr ptr, ptr %values, i64 %i
  %value = load ptr, ptr %slot
  %cmp = call i32 @strcmp(ptr %value, ptr %needle)
  %matched = icmp eq i32 %cmp, 0
  br i1 %matched, label %found, label %cont
found:
  ret i64 1
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 0
}

define private i64 @lk_index_of_ptr_list(ptr %values, i64 %len, ptr %needle) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %slot = getelementptr ptr, ptr %values, i64 %i
  %value = load ptr, ptr %slot
  %cmp = call i32 @strcmp(ptr %value, ptr %needle)
  %matched = icmp eq i32 %cmp, 0
  br i1 %matched, label %found, label %cont
found:
  ret i64 %i
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 -1
}

define private i64 @lk_ptr_list_text_len(ptr %values, i64 %len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %total = phi i64 [ 0, %entry ], [ %total_next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %finish, label %cont
cont:
  %slot = getelementptr ptr, ptr %values, i64 %i
  %value = load ptr, ptr %slot
  %part_len = call i64 @strlen(ptr %value)
  %total_next = add i64 %total, %part_len
  %next = add i64 %i, 1
  br label %loop
finish:
  ret i64 %total
}

define private void @lk_reverse_ptr_list(ptr %src_values, i64 %src_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_index_tmp = sub i64 %src_len, 1
  %src_index = sub i64 %src_index_tmp, %i
  %src_slot = getelementptr ptr, ptr %src_values, i64 %src_index
  %value = load ptr, ptr %src_slot
  %dst_slot = getelementptr ptr, ptr %dst_values, i64 %i
  store ptr %value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %src_len, ptr %dst_len
  ret void
}

define private void @lk_sort_ptr_list(ptr %src_values, i64 %src_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %copy_loop
copy_loop:
  %copy_i = phi i64 [ 0, %entry ], [ %copy_next, %copy ]
  %copy_done = icmp uge i64 %copy_i, %src_len
  br i1 %copy_done, label %outer_loop, label %copy
copy:
  %copy_src_slot = getelementptr ptr, ptr %src_values, i64 %copy_i
  %copy_value = load ptr, ptr %copy_src_slot
  %copy_dst_slot = getelementptr ptr, ptr %dst_values, i64 %copy_i
  store ptr %copy_value, ptr %copy_dst_slot
  %copy_next = add i64 %copy_i, 1
  br label %copy_loop
outer_loop:
  %i = phi i64 [ 0, %copy_loop ], [ %i_next, %outer_next ]
  %outer_done = icmp uge i64 %i, %src_len
  br i1 %outer_done, label %finish, label %inner_loop
inner_loop:
  %j_start = add i64 %i, 1
  br label %inner_check
inner_check:
  %j = phi i64 [ %j_start, %inner_loop ], [ %j_next, %inner_next ]
  %inner_done = icmp uge i64 %j, %src_len
  br i1 %inner_done, label %outer_next, label %compare
compare:
  %i_slot = getelementptr ptr, ptr %dst_values, i64 %i
  %j_slot = getelementptr ptr, ptr %dst_values, i64 %j
  %i_value = load ptr, ptr %i_slot
  %j_value = load ptr, ptr %j_slot
  %cmp = call i32 @strcmp(ptr %i_value, ptr %j_value)
  %swap = icmp sgt i32 %cmp, 0
  br i1 %swap, label %swap_values, label %inner_next
swap_values:
  store ptr %j_value, ptr %i_slot
  store ptr %i_value, ptr %j_slot
  br label %inner_next
inner_next:
  %j_next = add i64 %j, 1
  br label %inner_check
outer_next:
  %i_next = add i64 %i, 1
  br label %outer_loop
finish:
  store i64 %src_len, ptr %dst_len
  ret void
}

define private ptr @lk_pop_ptr_list(ptr %values, i64 %len) {
entry:
  %empty = icmp eq i64 %len, 0
  br i1 %empty, label %missing, label %found
found:
  %index = sub i64 %len, 1
  %slot = getelementptr ptr, ptr %values, i64 %index
  %value = load ptr, ptr %slot
  ret ptr %value
missing:
  ret ptr @lk_empty_text
}

define private void @lk_slice_range_ptr_list(ptr %src_values, i64 %src_len, i64 %start, i64 %end, ptr %dst_values, ptr %dst_len) {
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

define private void @lk_push_ptr_list(ptr %src_values, i64 %src_len, ptr %value, ptr %dst_values, ptr %dst_len) {
entry:
  br label %copy_loop
copy_loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %append, label %copy
copy:
  %src_slot = getelementptr ptr, ptr %src_values, i64 %i
  %src_value = load ptr, ptr %src_slot
  %dst_slot = getelementptr ptr, ptr %dst_values, i64 %i
  store ptr %src_value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %copy_loop
append:
  %copy_value = call ptr @strdup(ptr %value)
  %append_slot = getelementptr ptr, ptr %dst_values, i64 %src_len
  store ptr %copy_value, ptr %append_slot
  %next_len = add i64 %src_len, 1
  store i64 %next_len, ptr %dst_len
  ret void
}

define private void @lk_insert_ptr_list(ptr %src_values, i64 %src_len, i64 %index, ptr %value, ptr %dst_values, ptr %dst_len) {
entry:
  %index_neg = icmp slt i64 %index, 0
  %index_nonneg = select i1 %index_neg, i64 0, i64 %index
  %index_over = icmp sgt i64 %index_nonneg, %src_len
  %index_clamped = select i1 %index_over, i64 %src_len, i64 %index_nonneg
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %append_check, label %check
check:
  %before_insert = icmp ult i64 %i, %index_clamped
  br i1 %before_insert, label %copy_before, label %copy_after
copy_before:
  %before_src_slot = getelementptr ptr, ptr %src_values, i64 %i
  %before_value = load ptr, ptr %before_src_slot
  %before_dst_slot = getelementptr ptr, ptr %dst_values, i64 %i
  store ptr %before_value, ptr %before_dst_slot
  br label %cont
copy_after:
  %after_src_slot = getelementptr ptr, ptr %src_values, i64 %i
  %after_value = load ptr, ptr %after_src_slot
  %after_dst_i = add i64 %i, 1
  %after_dst_slot = getelementptr ptr, ptr %dst_values, i64 %after_dst_i
  store ptr %after_value, ptr %after_dst_slot
  br label %cont
cont:
  %next = add i64 %i, 1
  br label %loop
append_check:
  %copy_value = call ptr @strdup(ptr %value)
  %insert_slot = getelementptr ptr, ptr %dst_values, i64 %index_clamped
  store ptr %copy_value, ptr %insert_slot
  %next_len = add i64 %src_len, 1
  store i64 %next_len, ptr %dst_len
  ret void
}

define private ptr @lk_remove_at_ptr_list(ptr %src_values, i64 %src_len, i64 %index, ptr %dst_values, ptr %dst_len) {
entry:
  %empty = icmp eq i64 %src_len, 0
  br i1 %empty, label %missing, label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %dst_i = phi i64 [ 0, %entry ], [ %dst_next, %cont ]
  %old = phi ptr [ @lk_empty_text, %entry ], [ %old_next, %cont ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %check
check:
  %is_removed = icmp eq i64 %i, %index
  br i1 %is_removed, label %removed, label %copy
removed:
  %removed_slot = getelementptr ptr, ptr %src_values, i64 %i
  %removed_value = load ptr, ptr %removed_slot
  br label %cont
copy:
  %src_slot = getelementptr ptr, ptr %src_values, i64 %i
  %value = load ptr, ptr %src_slot
  %dst_slot = getelementptr ptr, ptr %dst_values, i64 %dst_i
  store ptr %value, ptr %dst_slot
  %dst_copy_next = add i64 %dst_i, 1
  br label %cont
cont:
  %old_next = phi ptr [ %removed_value, %removed ], [ %old, %copy ]
  %dst_next = phi i64 [ %dst_i, %removed ], [ %dst_copy_next, %copy ]
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret ptr %old
missing:
  store i64 0, ptr %dst_len
  ret ptr @lk_empty_text
}

define private ptr @lk_set_ptr_list(ptr %src_values, i64 %src_len, i64 %index, ptr %value, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %old = phi ptr [ @lk_empty_text, %entry ], [ %next_old, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_slot = getelementptr ptr, ptr %src_values, i64 %i
  %src_value = load ptr, ptr %src_slot
  %matched = icmp eq i64 %i, %index
  %copy_value = call ptr @strdup(ptr %value)
  %dst_value = select i1 %matched, ptr %copy_value, ptr %src_value
  %next_old = select i1 %matched, ptr %src_value, ptr %old
  %dst_slot = getelementptr ptr, ptr %dst_values, i64 %i
  store ptr %dst_value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %src_len, ptr %dst_len
  ret ptr %old
}
"#
}

pub(in crate::llvm) fn emit_dynamic_joined_ptr_text_len(
    ir: &mut String,
    dst: u8,
    id: usize,
    delimiter_len: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let list_len = next_tmp(tmp_index);
    let base = next_tmp(tmp_index);
    let text_len = next_tmp(tmp_index);
    let has_delimiters = next_tmp(tmp_index);
    let delimiter_base = next_tmp(tmp_index);
    let delimiter_count = next_tmp(tmp_index);
    let delimiter_total = next_tmp(tmp_index);
    let total = next_tmp(tmp_index);
    ir.push_str(&format!("  {list_len} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {base} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {text_len} = call i64 @lk_ptr_list_text_len(ptr {base}, i64 {list_len})\n"
    ));
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

pub(in crate::llvm) fn emit_dynamic_ptr_list_contains(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call i64 @lk_contains_ptr_list(ptr {src_base}, i64 {src_len}, ptr {needle})\n"
    ));
    ir.push_str(&format!("  store i64 {result}, ptr %r{dst_reg}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_index_of(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call i64 @lk_index_of_ptr_list(ptr {src_base}, i64 {src_len}, ptr {needle})\n"
    ));
    ir.push_str(&format!("  store i64 {result}, ptr %r{dst_reg}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_reverse(
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
        "  call void @lk_reverse_ptr_list(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_sort(
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
        "  call void @lk_sort_ptr_list(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_pop(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call ptr @lk_pop_ptr_list(ptr {src_base}, i64 {src_len})\n"
    ));
    ir.push_str(&format!("  store ptr {result}, ptr %r{dst_reg}.slot\n"));
    Some(result)
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_push_new(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_push_ptr_list(ptr {src_base}, i64 {src_len}, ptr {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_slice_range(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    let end = end.unwrap_or(src_len.as_str());
    ir.push_str(&format!(
        "  call void @lk_slice_range_ptr_list(ptr {src_base}, i64 {src_len}, i64 {start}, i64 {end}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_insert(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_insert_ptr_list(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_remove_at(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {removed} = call ptr @lk_remove_at_ptr_list(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(removed)
}

pub(in crate::llvm) fn emit_dynamic_ptr_list_set_new(
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
        "  {src_base} = getelementptr [4096 x ptr], ptr %list{src_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x ptr], ptr %list{dst_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {old} = call ptr @lk_set_ptr_list(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(old)
}
