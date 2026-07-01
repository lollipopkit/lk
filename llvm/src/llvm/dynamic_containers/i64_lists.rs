use crate::llvm::ir_text::next_tmp;

/// The `DynamicList<i64>` method bodies used to be hand-written LLVM IR here.
/// They now live in `lkrt` as monomorphized typed helpers (`lkrt_list_i64_*`),
/// declared through the native intrinsic registry and linked from `liblkrt.a`.
/// The emitters below call those symbols directly, so no in-module IR is needed.
pub(in crate::llvm) fn native_dynamic_i64_list_helpers() -> &'static str {
    ""
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
        "  {result} = call i64 @lkrt_list_i64_contains(ptr {src_base}, i64 {src_len}, i64 {needle})\n"
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
        "  {result} = call i64 @lkrt_list_i64_index_of(ptr {src_base}, i64 {src_len}, i64 {needle})\n"
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
        "  call void @lkrt_list_i64_reverse(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_i64_sort(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {result} = call i64 @lkrt_list_i64_pop(ptr {src_base}, i64 {src_len})\n"
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
        "  call void @lkrt_list_i64_push(ptr {src_base}, i64 {src_len}, i64 {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_i64_slice_range(ptr {src_base}, i64 {src_len}, i64 {start}, i64 {end}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_i64_insert(ptr {src_base}, i64 {src_len}, i64 {index}, i64 {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {removed} = call i64 @lkrt_list_i64_remove_at(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {old} = call i64 @lkrt_list_i64_set(ptr {src_base}, i64 {src_len}, i64 {index}, i64 {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    ir.push_str(&format!("  store i64 0, ptr %list{dst_id}.text.len.slot\n"));
    Some(old)
}
