use crate::llvm::ir_text::next_tmp;

/// The `DynamicList<str>` method bodies used to be hand-written LLVM IR here
/// (using `strcmp`/`strlen`/`strdup`). They now live in `lkrt` as monomorphized
/// typed helpers (`lkrt_list_str_*`), declared through the native intrinsic
/// registry and linked from `liblkrt.a`. The emitters below call those symbols
/// directly, so no in-module IR is needed.
pub(in crate::llvm) fn native_dynamic_ptr_list_helpers() -> &'static str {
    ""
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
        "  {text_len} = call i64 @lkrt_list_str_text_len(ptr {base}, i64 {list_len})\n"
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
        "  {result} = call i64 @lkrt_list_str_contains(ptr {src_base}, i64 {src_len}, ptr {needle})\n"
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
        "  {result} = call i64 @lkrt_list_str_index_of(ptr {src_base}, i64 {src_len}, ptr {needle})\n"
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
        "  call void @lkrt_list_str_reverse(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_str_sort(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {result} = call ptr @lkrt_list_str_pop(ptr {src_base}, i64 {src_len})\n"
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
        "  call void @lkrt_list_str_push(ptr {src_base}, i64 {src_len}, ptr {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_str_slice_range(ptr {src_base}, i64 {src_len}, i64 {start}, i64 {end}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_str_insert(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {removed} = call ptr @lkrt_list_str_remove_at(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {old} = call ptr @lkrt_list_str_set(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(old)
}
