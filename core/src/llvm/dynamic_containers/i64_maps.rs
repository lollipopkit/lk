pub(in crate::llvm) fn native_dynamic_i64_map_helpers() -> &'static str {
    r#"
define private i64 @lk_lookup_i64_int_map(ptr %keys, ptr %values, i64 %len, i64 %key, ptr %out) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %missing, label %check
check:
  %key_slot = getelementptr i64, ptr %keys, i64 %i
  %stored_key = load i64, ptr %key_slot
  %matched = icmp eq i64 %stored_key, %key
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

define private i64 @lk_set_i64_int_map(ptr %keys, ptr %values, i64 %len, i64 %key, i64 %value) {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %next, %cont ]
  %done = icmp uge i64 %i, %len
  br i1 %done, label %append, label %check
check:
  %key_slot = getelementptr i64, ptr %keys, i64 %i
  %stored_key = load i64, ptr %key_slot
  %matched = icmp eq i64 %stored_key, %key
  br i1 %matched, label %update, label %cont
update:
  %update_value_slot = getelementptr i64, ptr %values, i64 %i
  store i64 %value, ptr %update_value_slot
  ret i64 %len
cont:
  %next = add i64 %i, 1
  br label %loop
append:
  %append_key_slot = getelementptr i64, ptr %keys, i64 %len
  %append_value_slot = getelementptr i64, ptr %values, i64 %len
  store i64 %key, ptr %append_key_slot
  store i64 %value, ptr %append_value_slot
  %next_len = add i64 %len, 1
  ret i64 %next_len
}
"#
}
