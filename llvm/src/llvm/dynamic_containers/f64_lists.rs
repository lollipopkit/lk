use crate::llvm::ir_text::next_tmp;

/// The `DynamicList<f64>` method bodies used to be hand-written LLVM IR here.
/// They now live in `lkrt` as monomorphized typed helpers (`lkrt_list_f64_*`),
/// declared through the native intrinsic registry and linked from `liblkrt.a`.
/// The emitters below call those symbols directly, so no in-module IR is needed.
pub(in crate::llvm) fn native_dynamic_f64_list_helpers() -> &'static str {
    ""
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
        "  call void @lkrt_list_f64_slice(ptr {src_base}, i64 {src_len}, i64 {start}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_f64_take(ptr {src_base}, i64 {src_len}, i64 {count}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_f64_concat(ptr {lhs_base}, i64 {lhs_len}, ptr {rhs_base}, i64 {rhs_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {result} = call i64 @lkrt_list_f64_contains(ptr {src_base}, i64 {src_len}, double {needle})\n"
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
        "  {result} = call i64 @lkrt_list_f64_index_of(ptr {src_base}, i64 {src_len}, double {needle})\n"
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
        "  call void @lkrt_list_f64_reverse(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_f64_unique(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_f64_sort(ptr {src_base}, i64 {src_len}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {result} = call double @lkrt_list_f64_pop(ptr {src_base}, i64 {src_len})\n"
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
        "  call void @lkrt_list_f64_push(ptr {src_base}, i64 {src_len}, double {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_f64_slice_range(ptr {src_base}, i64 {src_len}, i64 {start}, i64 {end}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  call void @lkrt_list_f64_insert(ptr {src_base}, i64 {src_len}, i64 {index}, double {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {removed} = call double @lkrt_list_f64_remove_at(ptr {src_base}, i64 {src_len}, i64 {index}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
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
        "  {old} = call double @lkrt_list_f64_set(ptr {src_base}, i64 {src_len}, i64 {index}, double {value}, ptr {dst_base}, ptr %list{dst_id}.len.slot)\n"
    ));
    Some(old)
}
