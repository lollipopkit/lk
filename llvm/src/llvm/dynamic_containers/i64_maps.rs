use crate::llvm::ir_text::next_tmp;

pub(in crate::llvm) fn emit_dynamic_i64_int_map_set(
    ir: &mut String,
    id: usize,
    value_reg: u8,
    key_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let key = next_tmp(tmp_index);
    let value = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let key_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {key} = load i64, ptr %r{key_reg}.slot\n"));
    ir.push_str(&format!("  {value} = load i64, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {key_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {next_len} = call i64 @lkrt_map_i64_int_set(ptr {key_base}, ptr {value_base}, i64 {len}, i64 {key}, i64 {value})\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %map{id}.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_int_map_get(
    ir: &mut String,
    id: usize,
    dst: u8,
    key_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let key = next_tmp(tmp_index);
    ir.push_str(&format!("  {key} = load i64, ptr %r{key_reg}.slot\n"));
    emit_dynamic_i64_int_map_get_key(ir, id, dst, &key, tmp_index)
}

pub(in crate::llvm) fn emit_dynamic_i64_int_map_get_key(
    ir: &mut String,
    id: usize,
    dst: u8,
    key: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let found = next_tmp(tmp_index);
    let key_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.slot\n"));
    ir.push_str(&format!(
        "  {key_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {found} = call i64 @lkrt_map_i64_int_lookup(ptr {key_base}, ptr {value_base}, i64 {len}, i64 {key}, ptr %r{dst}.slot)\n"));
    ir.push_str(&format!("  store i64 {found}, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_f64_map_set(
    ir: &mut String,
    id: usize,
    value_reg: u8,
    key_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let key = next_tmp(tmp_index);
    let value = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let key_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {key} = load i64, ptr %r{key_reg}.slot\n"));
    ir.push_str(&format!("  {value} = load double, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {key_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x double], ptr %map{id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {next_len} = call i64 @lkrt_map_i64_f64_set(ptr {key_base}, ptr {value_base}, i64 {len}, i64 {key}, double {value})\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %map{id}.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_f64_map_get(
    ir: &mut String,
    id: usize,
    dst: u8,
    key_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let key = next_tmp(tmp_index);
    ir.push_str(&format!("  {key} = load i64, ptr %r{key_reg}.slot\n"));
    emit_dynamic_i64_f64_map_get_key(ir, id, dst, &key, tmp_index)
}

pub(in crate::llvm) fn emit_dynamic_i64_f64_map_get_key(
    ir: &mut String,
    id: usize,
    dst: u8,
    key: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let found = next_tmp(tmp_index);
    let key_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  store double 0.0, ptr %r{dst}.slot\n"));
    ir.push_str(&format!(
        "  {key_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x double], ptr %map{id}.f64.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {found} = call i64 @lkrt_map_i64_f64_lookup(ptr {key_base}, ptr {value_base}, i64 {len}, i64 {key}, ptr %r{dst}.slot)\n"));
    ir.push_str(&format!("  store i64 {found}, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_ptr_map_set(
    ir: &mut String,
    id: usize,
    value_reg: u8,
    key_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let key = next_tmp(tmp_index);
    let value = next_tmp(tmp_index);
    let value_copy = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let key_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    let next_len = next_tmp(tmp_index);
    ir.push_str(&format!("  {key} = load i64, ptr %r{key_reg}.slot\n"));
    ir.push_str(&format!("  {value} = load ptr, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {value_copy} = call ptr @strdup(ptr {value})\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {key_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {next_len} = call i64 @lkrt_map_i64_ptr_set(ptr {key_base}, ptr {value_base}, i64 {len}, i64 {key}, ptr {value_copy})\n"));
    ir.push_str(&format!("  store i64 {next_len}, ptr %map{id}.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_ptr_map_get(
    ir: &mut String,
    id: usize,
    dst: u8,
    key_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let key = next_tmp(tmp_index);
    ir.push_str(&format!("  {key} = load i64, ptr %r{key_reg}.slot\n"));
    emit_dynamic_i64_ptr_map_get_key(ir, id, dst, &key, tmp_index)
}

pub(in crate::llvm) fn emit_dynamic_i64_ptr_map_get_key(
    ir: &mut String,
    id: usize,
    dst: u8,
    key: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let found = next_tmp(tmp_index);
    let key_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  store ptr @lk_nil_text, ptr %r{dst}.slot\n"));
    ir.push_str(&format!(
        "  {key_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  {found} = call i64 @lkrt_map_i64_ptr_lookup(ptr {key_base}, ptr {value_base}, i64 {len}, i64 {key}, ptr %r{dst}.slot)\n"));
    ir.push_str(&format!("  store i64 {found}, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_map_has_key(
    ir: &mut String,
    id: usize,
    dst: u8,
    key: &str,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let key_base = next_tmp(tmp_index);
    let label = format!("lk.has.i64.map.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {key_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.loop:\n"));
    ir.push_str(&format!(
        "  %{label}.i = phi i64 [ 0, %bb{pc} ], [ %{label}.next, %{label}.cont ]\n"
    ));
    ir.push_str(&format!("  %{label}.done = icmp uge i64 %{label}.i, {len}\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.done, label %{label}.missing, label %{label}.check\n"
    ));
    ir.push_str(&format!("{label}.check:\n"));
    ir.push_str(&format!(
        "  %{label}.key.slot = getelementptr i64, ptr {key_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  %{label}.stored.key = load i64, ptr %{label}.key.slot\n"));
    ir.push_str(&format!(
        "  %{label}.matched = icmp eq i64 %{label}.stored.key, {key}\n"
    ));
    ir.push_str(&format!(
        "  br i1 %{label}.matched, label %{label}.found, label %{label}.cont\n"
    ));
    ir.push_str(&format!("{label}.found:\n"));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  br label %{label}.finish\n"));
    ir.push_str(&format!("{label}.cont:\n"));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.missing:\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  br label %{label}.finish\n"));
    ir.push_str(&format!("{label}.finish:\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_int_map_delete_key(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: &str,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_i64_map_delete_key(ir, src_id, dst_id, dst, key, pc, "i64", "value", "0", tmp_index)
}

pub(in crate::llvm) fn emit_dynamic_i64_f64_map_delete_key(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: &str,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_i64_map_delete_key(ir, src_id, dst_id, dst, key, pc, "double", "f64", "0.0", tmp_index)
}

pub(in crate::llvm) fn emit_dynamic_i64_ptr_map_delete_key(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: &str,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_i64_map_delete_key(
        ir,
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
fn emit_dynamic_i64_map_delete_key(
    ir: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: &str,
    pc: usize,
    value_ty: &str,
    value_slot_name: &str,
    missing_value: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let src_key_base = next_tmp(tmp_index);
    let src_value_base = next_tmp(tmp_index);
    let dst_key_base = next_tmp(tmp_index);
    let dst_value_base = next_tmp(tmp_index);
    let label = format!("lk.delete.i64.map.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {len} = load i64, ptr %map{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_key_base} = getelementptr [4096 x i64], ptr %map{src_id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {src_value_base} = getelementptr [4096 x {value_ty}], ptr %map{src_id}.{value_slot_name}.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_key_base} = getelementptr [4096 x i64], ptr %map{dst_id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_value_base} = getelementptr [4096 x {value_ty}], ptr %map{dst_id}.{value_slot_name}.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!("  store {value_ty} {missing_value}, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.loop:\n"));
    ir.push_str(&format!(
        "  %{label}.i = phi i64 [ 0, %bb{pc} ], [ %{label}.next, %{label}.cont ]\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.i = phi i64 [ 0, %bb{pc} ], [ %{label}.next.dst, %{label}.cont ]\n"
    ));
    ir.push_str(&format!("  %{label}.done = icmp uge i64 %{label}.i, {len}\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.done, label %{label}.finish, label %{label}.check\n"
    ));
    ir.push_str(&format!("{label}.check:\n"));
    ir.push_str(&format!(
        "  %{label}.key.slot = getelementptr i64, ptr {src_key_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  %{label}.stored.key = load i64, ptr %{label}.key.slot\n"));
    ir.push_str(&format!(
        "  %{label}.matched = icmp eq i64 %{label}.stored.key, {key}\n"
    ));
    ir.push_str(&format!(
        "  br i1 %{label}.matched, label %{label}.remove, label %{label}.copy\n"
    ));
    ir.push_str(&format!("{label}.remove:\n"));
    ir.push_str(&format!(
        "  %{label}.removed.value.slot = getelementptr {value_ty}, ptr {src_value_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.removed.value = load {value_ty}, ptr %{label}.removed.value.slot\n"
    ));
    ir.push_str(&format!(
        "  store {value_ty} %{label}.removed.value, ptr %r{dst}.slot\n"
    ));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  br label %{label}.cont\n"));
    ir.push_str(&format!("{label}.copy:\n"));
    ir.push_str(&format!(
        "  %{label}.src.value.slot = getelementptr {value_ty}, ptr {src_value_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.src.value = load {value_ty}, ptr %{label}.src.value.slot\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.key.slot = getelementptr i64, ptr {dst_key_base}, i64 %{label}.dst.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.value.slot = getelementptr {value_ty}, ptr {dst_value_base}, i64 %{label}.dst.i\n"
    ));
    ir.push_str(&format!("  store i64 %{label}.stored.key, ptr %{label}.dst.key.slot\n"));
    ir.push_str(&format!(
        "  store {value_ty} %{label}.src.value, ptr %{label}.dst.value.slot\n"
    ));
    ir.push_str(&format!("  %{label}.copy.next.dst = add i64 %{label}.dst.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.cont\n"));
    ir.push_str(&format!("{label}.cont:\n"));
    ir.push_str(&format!(
        "  %{label}.next.dst = phi i64 [ %{label}.dst.i, %{label}.remove ], [ %{label}.copy.next.dst, %{label}.copy ]\n"
    ));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.finish:\n"));
    ir.push_str(&format!("  store i64 %{label}.dst.i, ptr %map{dst_id}.len.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_int_map_iter_key(
    ir: &mut String,
    id: usize,
    dst: u8,
    index_reg: u8,
    tmp_index: &mut usize,
) -> Option<()> {
    let index = next_tmp(tmp_index);
    let key_slot = next_tmp(tmp_index);
    let key = next_tmp(tmp_index);
    ir.push_str(&format!("  {index} = load i64, ptr %r{index_reg}.slot\n"));
    ir.push_str(&format!(
        "  {key_slot} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 {index}\n"
    ));
    ir.push_str(&format!("  {key} = load i64, ptr {key_slot}\n"));
    ir.push_str(&format!("  store i64 {key}, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_int_map_iter_value(
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
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_f64_map_iter_value(
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

pub(in crate::llvm) fn emit_dynamic_i64_ptr_map_iter_value(
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
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_int_map_values(
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

pub(in crate::llvm) fn emit_dynamic_i64_ptr_map_values(
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
        "  {map_base} = getelementptr [4096 x ptr], ptr %map{map_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {list_base} = getelementptr [4096 x ptr], ptr %list{list_id}.ptr.slots, i64 0, i64 0\n"
    ));
    emit_dynamic_value_copy_loop(ir, map_base, list_base, len.clone(), "ptr", list_id, tmp_index);
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_i64_f64_map_values(
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

pub(in crate::llvm) fn emit_dynamic_i64_map_keys(
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
        "  {map_base} = getelementptr [4096 x i64], ptr %map{map_id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {list_base} = getelementptr [4096 x i64], ptr %list{list_id}.value.slots, i64 0, i64 0\n"
    ));
    emit_dynamic_value_copy_loop(ir, map_base, list_base, len.clone(), "i64", list_id, tmp_index);
    Some(())
}

fn emit_dynamic_value_copy_loop(
    ir: &mut String,
    src_base: String,
    dst_base: String,
    len: String,
    ty: &str,
    list_id: usize,
    tmp_index: &mut usize,
) {
    let label = format!("lk.copy.i64.map.values.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  store i64 {len}, ptr %list{list_id}.len.slot\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.loop:\n"));
    ir.push_str(&format!(
        "  %{label}.i = phi i64 [ 0, %bb{list_id} ], [ %{label}.next, %{label}.copy ]\n"
    ));
    ir.push_str(&format!("  %{label}.done = icmp uge i64 %{label}.i, {len}\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.done, label %{label}.done_block, label %{label}.copy\n"
    ));
    ir.push_str(&format!("{label}.copy:\n"));
    ir.push_str(&format!(
        "  %{label}.src = getelementptr {ty}, ptr {src_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst = getelementptr {ty}, ptr {dst_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  %{label}.value = load {ty}, ptr %{label}.src\n"));
    ir.push_str(&format!("  store {ty} %{label}.value, ptr %{label}.dst\n"));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.done_block:\n"));
}

/// The `DynamicMap<i64, V>` lookup/set method bodies used to be hand-written
/// LLVM IR here. They now live in `lkrt` as monomorphized typed helpers
/// (`lkrt_map_i64_{int,f64,ptr}_{lookup,set}`), declared through the native
/// intrinsic registry and linked from `liblkrt.a`. The emitters above call those
/// symbols directly, so no in-module IR is needed. (The map's has/delete/iter/
/// values/keys shapes remain inline IR in the emitters and are not helpers.)
pub(in crate::llvm) fn native_dynamic_i64_map_helpers() -> &'static str {
    ""
}
