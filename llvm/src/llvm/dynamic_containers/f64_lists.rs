use crate::llvm::ir_text::next_tmp;

pub(in crate::llvm) fn native_dynamic_f64_list_helpers() -> &'static str {
    r#"
define private void @lk_slice_f64_list(ptr %src_values, i64 %src_len, i64 %start, ptr %dst_values, ptr %dst_len) {
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
  %src_slot = getelementptr double, ptr %src_values, i64 %src_i
  %value = load double, ptr %src_slot
  %dst_slot = getelementptr double, ptr %dst_values, i64 %dst_i
  store double %value, ptr %dst_slot
  %src_next = add i64 %src_i, 1
  %dst_next = add i64 %dst_i, 1
  br label %loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret void
}

define private void @lk_take_f64_list(ptr %src_values, i64 %src_len, i64 %count, ptr %dst_values, ptr %dst_len) {
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
  %src_slot = getelementptr double, ptr %src_values, i64 %i
  %value = load double, ptr %src_slot
  %dst_slot = getelementptr double, ptr %dst_values, i64 %i
  store double %value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %i, ptr %dst_len
  ret void
}

define private void @lk_concat_f64_list(ptr %lhs_values, i64 %lhs_len, ptr %rhs_values, i64 %rhs_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %lhs_loop
lhs_loop:
  %lhs_i = phi i64 [ 0, %entry ], [ %lhs_next, %lhs_copy ]
  %lhs_done = icmp uge i64 %lhs_i, %lhs_len
  br i1 %lhs_done, label %rhs_loop, label %lhs_copy
lhs_copy:
  %lhs_src_slot = getelementptr double, ptr %lhs_values, i64 %lhs_i
  %lhs_value = load double, ptr %lhs_src_slot
  %lhs_dst_slot = getelementptr double, ptr %dst_values, i64 %lhs_i
  store double %lhs_value, ptr %lhs_dst_slot
  %lhs_next = add i64 %lhs_i, 1
  br label %lhs_loop
rhs_loop:
  %rhs_i = phi i64 [ 0, %lhs_loop ], [ %rhs_next, %rhs_copy ]
  %rhs_done = icmp uge i64 %rhs_i, %rhs_len
  br i1 %rhs_done, label %finish, label %rhs_copy
rhs_copy:
  %rhs_src_slot = getelementptr double, ptr %rhs_values, i64 %rhs_i
  %rhs_value = load double, ptr %rhs_src_slot
  %dst_i = add i64 %lhs_len, %rhs_i
  %rhs_dst_slot = getelementptr double, ptr %dst_values, i64 %dst_i
  store double %rhs_value, ptr %rhs_dst_slot
  %rhs_next = add i64 %rhs_i, 1
  br label %rhs_loop
finish:
  %total = add i64 %lhs_len, %rhs_len
  store i64 %total, ptr %dst_len
  ret void
}

define private i64 @lk_contains_f64_list(ptr %values, i64 %len, double %needle) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %slot = getelementptr double, ptr %values, i64 %i
  %value = load double, ptr %slot
  %matched = fcmp oeq double %value, %needle
  br i1 %matched, label %found, label %cont
found:
  ret i64 1
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 0
}

define private i64 @lk_index_of_f64_list(ptr %values, i64 %len, double %needle) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %slot = getelementptr double, ptr %values, i64 %i
  %value = load double, ptr %slot
  %matched = fcmp oeq double %value, %needle
  br i1 %matched, label %found, label %cont
found:
  ret i64 %i
cont:
  %next = add i64 %i, 1
  br label %loop
missing:
  ret i64 -1
}

define private void @lk_unique_f64_list(ptr %src_values, i64 %src_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %outer_loop
outer_loop:
  %src_i = phi i64 [ 0, %entry ], [ %src_next, %outer_next ]
  %dst_i = phi i64 [ 0, %entry ], [ %dst_next_value, %outer_next ]
  %outer_done = icmp uge i64 %src_i, %src_len
  br i1 %outer_done, label %finish, label %check_seen
check_seen:
  %src_slot = getelementptr double, ptr %src_values, i64 %src_i
  %value = load double, ptr %src_slot
  br label %inner_loop
inner_loop:
  %seen_i = phi i64 [ 0, %check_seen ], [ %seen_next, %inner_next ]
  %inner_done = icmp uge i64 %seen_i, %dst_i
  br i1 %inner_done, label %append, label %compare
compare:
  %seen_slot = getelementptr double, ptr %dst_values, i64 %seen_i
  %seen_value = load double, ptr %seen_slot
  %matched = fcmp oeq double %seen_value, %value
  br i1 %matched, label %skip, label %inner_next
inner_next:
  %seen_next = add i64 %seen_i, 1
  br label %inner_loop
append:
  %dst_slot = getelementptr double, ptr %dst_values, i64 %dst_i
  store double %value, ptr %dst_slot
  %dst_appended = add i64 %dst_i, 1
  br label %outer_next
skip:
  br label %outer_next
outer_next:
  %dst_next_value = phi i64 [ %dst_appended, %append ], [ %dst_i, %skip ]
  %src_next = add i64 %src_i, 1
  br label %outer_loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret void
}

define private void @lk_reverse_f64_list(ptr %src_values, i64 %src_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_index_tmp = sub i64 %src_len, 1
  %src_index = sub i64 %src_index_tmp, %i
  %src_slot = getelementptr double, ptr %src_values, i64 %src_index
  %value = load double, ptr %src_slot
  %dst_slot = getelementptr double, ptr %dst_values, i64 %i
  store double %value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %src_len, ptr %dst_len
  ret void
}

define private void @lk_sort_f64_list(ptr %src_values, i64 %src_len, ptr %dst_values, ptr %dst_len) {
entry:
  br label %copy_loop
copy_loop:
  %copy_i = phi i64 [ 0, %entry ], [ %copy_next, %copy ]
  %copy_done = icmp uge i64 %copy_i, %src_len
  br i1 %copy_done, label %outer_loop, label %copy
copy:
  %copy_src_slot = getelementptr double, ptr %src_values, i64 %copy_i
  %copy_value = load double, ptr %copy_src_slot
  %copy_dst_slot = getelementptr double, ptr %dst_values, i64 %copy_i
  store double %copy_value, ptr %copy_dst_slot
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
  %i_slot = getelementptr double, ptr %dst_values, i64 %i
  %j_slot = getelementptr double, ptr %dst_values, i64 %j
  %i_value = load double, ptr %i_slot
  %j_value = load double, ptr %j_slot
  %swap = fcmp ogt double %i_value, %j_value
  br i1 %swap, label %swap_values, label %inner_next
swap_values:
  store double %j_value, ptr %i_slot
  store double %i_value, ptr %j_slot
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

define private double @lk_pop_f64_list(ptr %values, i64 %len) {
entry:
  %empty = icmp eq i64 %len, 0
  br i1 %empty, label %missing, label %found
found:
  %index = sub i64 %len, 1
  %slot = getelementptr double, ptr %values, i64 %index
  %value = load double, ptr %slot
  ret double %value
missing:
  ret double 0.0
}

define private void @lk_slice_range_f64_list(ptr %src_values, i64 %src_len, i64 %start, i64 %end, ptr %dst_values, ptr %dst_len) {
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
  %src_slot = getelementptr double, ptr %src_values, i64 %src_i
  %value = load double, ptr %src_slot
  %dst_slot = getelementptr double, ptr %dst_values, i64 %dst_i
  store double %value, ptr %dst_slot
  %src_next = add i64 %src_i, 1
  %dst_next = add i64 %dst_i, 1
  br label %loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret void
}

define private void @lk_push_f64_list(ptr %src_values, i64 %src_len, double %value, ptr %dst_values, ptr %dst_len) {
entry:
  br label %copy_loop
copy_loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %append, label %copy
copy:
  %src_slot = getelementptr double, ptr %src_values, i64 %i
  %src_value = load double, ptr %src_slot
  %dst_slot = getelementptr double, ptr %dst_values, i64 %i
  store double %src_value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %copy_loop
append:
  %append_slot = getelementptr double, ptr %dst_values, i64 %src_len
  store double %value, ptr %append_slot
  %next_len = add i64 %src_len, 1
  store i64 %next_len, ptr %dst_len
  ret void
}

define private void @lk_insert_f64_list(ptr %src_values, i64 %src_len, i64 %index, double %value, ptr %dst_values, ptr %dst_len) {
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
  %before_src_slot = getelementptr double, ptr %src_values, i64 %i
  %before_value = load double, ptr %before_src_slot
  %before_dst_slot = getelementptr double, ptr %dst_values, i64 %i
  store double %before_value, ptr %before_dst_slot
  br label %cont
copy_after:
  %after_src_slot = getelementptr double, ptr %src_values, i64 %i
  %after_value = load double, ptr %after_src_slot
  %after_dst_i = add i64 %i, 1
  %after_dst_slot = getelementptr double, ptr %dst_values, i64 %after_dst_i
  store double %after_value, ptr %after_dst_slot
  br label %cont
cont:
  %next = add i64 %i, 1
  br label %loop
insert:
  %insert_slot = getelementptr double, ptr %dst_values, i64 %index_clamped
  store double %value, ptr %insert_slot
  %next_len = add i64 %src_len, 1
  store i64 %next_len, ptr %dst_len
  ret void
}

define private double @lk_remove_at_f64_list(ptr %src_values, i64 %src_len, i64 %index, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %dst_i = phi i64 [ 0, %entry ], [ %dst_next, %cont ]
  %old = phi double [ 0.0, %entry ], [ %old_next, %cont ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %check
check:
  %is_removed = icmp eq i64 %i, %index
  br i1 %is_removed, label %removed, label %copy
removed:
  %removed_slot = getelementptr double, ptr %src_values, i64 %i
  %removed_value = load double, ptr %removed_slot
  br label %cont
copy:
  %src_slot = getelementptr double, ptr %src_values, i64 %i
  %value = load double, ptr %src_slot
  %dst_slot = getelementptr double, ptr %dst_values, i64 %dst_i
  store double %value, ptr %dst_slot
  %dst_copy_next = add i64 %dst_i, 1
  br label %cont
cont:
  %old_next = phi double [ %removed_value, %removed ], [ %old, %copy ]
  %dst_next = phi i64 [ %dst_i, %removed ], [ %dst_copy_next, %copy ]
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %dst_i, ptr %dst_len
  ret double %old
}

define private double @lk_set_f64_list(ptr %src_values, i64 %src_len, i64 %index, double %value, ptr %dst_values, ptr %dst_len) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %copy ]
  %old = phi double [ 0.0, %entry ], [ %next_old, %copy ]
  %done = icmp uge i64 %i, %src_len
  br i1 %done, label %finish, label %copy
copy:
  %src_slot = getelementptr double, ptr %src_values, i64 %i
  %src_value = load double, ptr %src_slot
  %matched = icmp eq i64 %i, %index
  %dst_value = select i1 %matched, double %value, double %src_value
  %next_old = select i1 %matched, double %src_value, double %old
  %dst_slot = getelementptr double, ptr %dst_values, i64 %i
  store double %dst_value, ptr %dst_slot
  %next = add i64 %i, 1
  br label %loop
finish:
  store i64 %src_len, ptr %dst_len
  ret double %old
}
"#
}

pub(in crate::llvm) fn emit_dynamic_f64_list_slice(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_slice_f64_list(ptr {src_base}, i64 {src_len}, i64 {start}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_take(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_take_f64_list(ptr {src_base}, i64 {src_len}, i64 {count}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_concat(
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
        "  {lhs_base} = getelementptr [4096 x double], ptr %list{lhs_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {rhs_base} = getelementptr [4096 x double], ptr %list{rhs_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_concat_f64_list(ptr {lhs_base}, i64 {lhs_len}, ptr {rhs_base}, i64 {rhs_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_contains(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call i64 @lk_contains_f64_list(ptr {src_base}, i64 {src_len}, double {needle})\n"
    ));
    ir.push_str(&format!("  store i64 {result}, ptr %r{dst_reg}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_index_of(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call i64 @lk_index_of_f64_list(ptr {src_base}, i64 {src_len}, double {needle})\n"
    ));
    ir.push_str(&format!("  store i64 {result}, ptr %r{dst_reg}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_reverse(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_reverse_f64_list(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_unique(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_unique_f64_list(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_sort(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_sort_f64_list(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_pop(
    ir: &mut String,
    src_id: usize,
    dst_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let src_len = next_tmp(tmp_index);
    let src_base = next_tmp(tmp_index);
    let result = next_tmp(tmp_index);
    ir.push_str(&format!("  {src_len} = load i64, ptr %list{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {result} = call double @lk_pop_f64_list(ptr {src_base}, i64 {src_len})\n"
    ));
    ir.push_str(&format!("  store double {result}, ptr %r{dst_reg}.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_push_new(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_push_f64_list(ptr {src_base}, i64 {src_len}, double {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_slice_range(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    let end = end.unwrap_or(src_len.as_str());
    ir.push_str(&format!(
        "  call void @lk_slice_range_f64_list(ptr {src_base}, i64 {src_len}, i64 {start}, i64 {end}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_insert(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  call void @lk_insert_f64_list(ptr {src_base}, i64 {src_len}, i64 {index}, double {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_f64_list_remove_at(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {removed} = call double @lk_remove_at_f64_list(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(removed)
}

pub(in crate::llvm) fn emit_dynamic_f64_list_set_new(
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
        "  {src_base} = getelementptr [4096 x double], ptr %list{src_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_base} = getelementptr [4096 x double], ptr %list{dst_id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {old} = call double @lk_set_f64_list(ptr {src_base}, i64 {src_len}, i64 {index}, double {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(old)
}
