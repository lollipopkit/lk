use crate::llvm::{
    const_display::llvm_string_constant,
    ir_text::next_tmp,
    straightline_value::{NativeStraightlineValue, NativeTextPart},
};

pub(in crate::llvm) fn emit_dynamic_string_map_has(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let len = next_tmp(tmp_index);
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let label = format!("lk.has.string.map.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
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
    emit_string_key_match(ir, &label, &prefix_base, &number_base, &prefix, &number);
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

pub(in crate::llvm) fn emit_dynamic_string_int_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_string_map_delete(
        ir,
        extra_globals,
        src_id,
        dst_id,
        dst,
        key,
        pc,
        "i64",
        "value",
        "0",
        tmp_index,
    )
}

pub(in crate::llvm) fn emit_dynamic_string_f64_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_string_map_delete(
        ir,
        extra_globals,
        src_id,
        dst_id,
        dst,
        key,
        pc,
        "double",
        "f64",
        "0.0",
        tmp_index,
    )
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_set(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    pc: usize,
    value_reg: u8,
    key: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let value = next_tmp(tmp_index);
    let value_copy = next_tmp(tmp_index);
    let len = next_tmp(tmp_index);
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    let label = format!("lk.set.string.ptr.map.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {value} = load ptr, ptr %r{value_reg}.slot\n"));
    ir.push_str(&format!("  {value_copy} = call ptr @strdup(ptr {value})\n"));
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 0\n"
    ));
    emit_string_map_set_loop(
        ir,
        &label,
        id,
        &prefix_base,
        &number_base,
        &value_base,
        &prefix,
        &number,
        "ptr",
        &value_copy,
        &len,
        pc,
    );
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_get(
    ir: &mut String,
    extra_globals: &mut String,
    id: usize,
    pc: usize,
    dst: u8,
    key: NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, id, tmp_index)?;
    let len = next_tmp(tmp_index);
    let prefix_base = next_tmp(tmp_index);
    let number_base = next_tmp(tmp_index);
    let value_base = next_tmp(tmp_index);
    let label = format!("lk.get.string.ptr.map.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {len} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  store i64 0, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  store ptr @lk_nil_text, ptr %r{dst}.slot\n"));
    ir.push_str(&format!(
        "  {prefix_base} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {number_base} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {value_base} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 0\n"
    ));
    emit_string_map_get_loop(
        ir,
        &label,
        dst,
        &prefix_base,
        &number_base,
        &value_base,
        &prefix,
        &number,
        "ptr",
        &len,
        pc,
    );
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_iter_value(
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
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_values(
    ir: &mut String,
    map_id: usize,
    list_id: usize,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    let len = next_tmp(tmp_index);
    let map_base = next_tmp(tmp_index);
    let list_base = next_tmp(tmp_index);
    let label = format!("lk.copy.string.ptr.map.values.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {len} = load i64, ptr %map{map_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {map_base} = getelementptr [4096 x ptr], ptr %map{map_id}.ptr.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {list_base} = getelementptr [4096 x ptr], ptr %list{list_id}.ptr.slots, i64 0, i64 0\n"
    ));
    emit_ptr_copy_loop(ir, &label, &map_base, &list_base, &len, list_id, pc);
    Some(())
}

pub(in crate::llvm) fn emit_dynamic_string_ptr_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<()> {
    emit_dynamic_string_map_delete(
        ir,
        extra_globals,
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
fn emit_dynamic_string_map_delete(
    ir: &mut String,
    extra_globals: &mut String,
    src_id: usize,
    dst_id: usize,
    dst: u8,
    key: NativeStraightlineValue,
    pc: usize,
    value_ty: &str,
    value_slot_name: &str,
    missing_value: &str,
    tmp_index: &mut usize,
) -> Option<()> {
    let (prefix, number) = dynamic_string_int_key_parts(extra_globals, key, src_id, tmp_index)?;
    let len = next_tmp(tmp_index);
    let src_prefix_base = next_tmp(tmp_index);
    let src_number_base = next_tmp(tmp_index);
    let src_value_base = next_tmp(tmp_index);
    let dst_prefix_base = next_tmp(tmp_index);
    let dst_number_base = next_tmp(tmp_index);
    let dst_value_base = next_tmp(tmp_index);
    let label = format!("lk.delete.string.map.{}", *tmp_index);
    *tmp_index += 1;
    ir.push_str(&format!("  {len} = load i64, ptr %map{src_id}.len.slot\n"));
    ir.push_str(&format!(
        "  {src_prefix_base} = getelementptr [4096 x ptr], ptr %map{src_id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {src_number_base} = getelementptr [4096 x i64], ptr %map{src_id}.number.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {src_value_base} = getelementptr [4096 x {value_ty}], ptr %map{src_id}.{value_slot_name}.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_prefix_base} = getelementptr [4096 x ptr], ptr %map{dst_id}.prefix.slots, i64 0, i64 0\n"
    ));
    ir.push_str(&format!(
        "  {dst_number_base} = getelementptr [4096 x i64], ptr %map{dst_id}.number.slots, i64 0, i64 0\n"
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
    emit_string_key_match(ir, &label, &src_prefix_base, &src_number_base, &prefix, &number);
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
    emit_string_map_copy_item(
        ir,
        &label,
        value_ty,
        &src_value_base,
        &dst_prefix_base,
        &dst_number_base,
        &dst_value_base,
    );
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

#[allow(clippy::too_many_arguments)]
fn emit_string_map_set_loop(
    ir: &mut String,
    label: &str,
    id: usize,
    prefix_base: &str,
    number_base: &str,
    value_base: &str,
    prefix: &str,
    number: &str,
    value_ty: &str,
    value: &str,
    len: &str,
    pc: usize,
) {
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.loop:\n"));
    ir.push_str(&format!(
        "  %{label}.i = phi i64 [ 0, %bb{pc} ], [ %{label}.next, %{label}.cont ]\n"
    ));
    ir.push_str(&format!("  %{label}.done = icmp uge i64 %{label}.i, {len}\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.done, label %{label}.append, label %{label}.check\n"
    ));
    ir.push_str(&format!("{label}.check:\n"));
    emit_string_key_match(ir, label, prefix_base, number_base, prefix, number);
    ir.push_str(&format!(
        "  br i1 %{label}.matched, label %{label}.update, label %{label}.cont\n"
    ));
    ir.push_str(&format!("{label}.update:\n"));
    ir.push_str(&format!(
        "  %{label}.update.value.slot = getelementptr {value_ty}, ptr {value_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  store {value_ty} {value}, ptr %{label}.update.value.slot\n"));
    ir.push_str(&format!("  br label %{label}.finish\n"));
    ir.push_str(&format!("{label}.cont:\n"));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.append:\n"));
    ir.push_str(&format!(
        "  %{label}.append.prefix.slot = getelementptr ptr, ptr {prefix_base}, i64 {len}\n"
    ));
    ir.push_str(&format!(
        "  %{label}.append.number.slot = getelementptr i64, ptr {number_base}, i64 {len}\n"
    ));
    ir.push_str(&format!(
        "  %{label}.append.value.slot = getelementptr {value_ty}, ptr {value_base}, i64 {len}\n"
    ));
    ir.push_str(&format!("  store ptr {prefix}, ptr %{label}.append.prefix.slot\n"));
    ir.push_str(&format!("  store i64 {number}, ptr %{label}.append.number.slot\n"));
    ir.push_str(&format!("  store {value_ty} {value}, ptr %{label}.append.value.slot\n"));
    ir.push_str(&format!("  %{label}.next.len = add i64 {len}, 1\n"));
    ir.push_str(&format!("  store i64 %{label}.next.len, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  br label %{label}.finish\n"));
    ir.push_str(&format!("{label}.finish:\n"));
}

#[allow(clippy::too_many_arguments)]
fn emit_string_map_get_loop(
    ir: &mut String,
    label: &str,
    dst: u8,
    prefix_base: &str,
    number_base: &str,
    value_base: &str,
    prefix: &str,
    number: &str,
    value_ty: &str,
    len: &str,
    pc: usize,
) {
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
    emit_string_key_match(ir, label, prefix_base, number_base, prefix, number);
    ir.push_str(&format!(
        "  br i1 %{label}.matched, label %{label}.found, label %{label}.cont\n"
    ));
    ir.push_str(&format!("{label}.found:\n"));
    ir.push_str(&format!(
        "  %{label}.value.slot = getelementptr {value_ty}, ptr {value_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.value = load {value_ty}, ptr %{label}.value.slot\n"
    ));
    ir.push_str(&format!("  store {value_ty} %{label}.value, ptr %r{dst}.slot\n"));
    ir.push_str(&format!("  store i64 1, ptr %r{dst}.present.slot\n"));
    ir.push_str(&format!("  br label %{label}.finish\n"));
    ir.push_str(&format!("{label}.cont:\n"));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.missing:\n"));
    ir.push_str(&format!("  br label %{label}.finish\n"));
    ir.push_str(&format!("{label}.finish:\n"));
}

fn emit_ptr_copy_loop(
    ir: &mut String,
    label: &str,
    src_base: &str,
    dst_base: &str,
    len: &str,
    list_id: usize,
    pc: usize,
) {
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.loop:\n"));
    ir.push_str(&format!(
        "  %{label}.i = phi i64 [ 0, %bb{pc} ], [ %{label}.next, %{label}.copy ]\n"
    ));
    ir.push_str(&format!("  %{label}.done = icmp uge i64 %{label}.i, {len}\n"));
    ir.push_str(&format!(
        "  br i1 %{label}.done, label %{label}.finish, label %{label}.copy\n"
    ));
    ir.push_str(&format!("{label}.copy:\n"));
    ir.push_str(&format!(
        "  %{label}.src.slot = getelementptr ptr, ptr {src_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.slot = getelementptr ptr, ptr {dst_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!("  %{label}.value = load ptr, ptr %{label}.src.slot\n"));
    ir.push_str(&format!("  store ptr %{label}.value, ptr %{label}.dst.slot\n"));
    ir.push_str(&format!("  %{label}.next = add i64 %{label}.i, 1\n"));
    ir.push_str(&format!("  br label %{label}.loop\n"));
    ir.push_str(&format!("{label}.finish:\n"));
    ir.push_str(&format!("  store i64 {len}, ptr %list{list_id}.len.slot\n"));
}

fn emit_string_key_match(
    ir: &mut String,
    label: &str,
    prefix_base: &str,
    number_base: &str,
    prefix: &str,
    number: &str,
) {
    ir.push_str(&format!(
        "  %{label}.prefix.slot = getelementptr ptr, ptr {prefix_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.stored.prefix = load ptr, ptr %{label}.prefix.slot\n"
    ));
    ir.push_str(&format!(
        "  %{label}.prefix.cmp = call i32 @strcmp(ptr %{label}.stored.prefix, ptr {prefix})\n"
    ));
    ir.push_str(&format!("  %{label}.prefix.eq = icmp eq i32 %{label}.prefix.cmp, 0\n"));
    ir.push_str(&format!(
        "  %{label}.number.slot = getelementptr i64, ptr {number_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.stored.number = load i64, ptr %{label}.number.slot\n"
    ));
    ir.push_str(&format!(
        "  %{label}.number.eq = icmp eq i64 %{label}.stored.number, {number}\n"
    ));
    ir.push_str(&format!(
        "  %{label}.matched = and i1 %{label}.prefix.eq, %{label}.number.eq\n"
    ));
}

fn emit_string_map_copy_item(
    ir: &mut String,
    label: &str,
    value_ty: &str,
    src_value_base: &str,
    dst_prefix_base: &str,
    dst_number_base: &str,
    dst_value_base: &str,
) {
    ir.push_str(&format!(
        "  %{label}.src.value.slot = getelementptr {value_ty}, ptr {src_value_base}, i64 %{label}.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.src.value = load {value_ty}, ptr %{label}.src.value.slot\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.prefix.slot = getelementptr ptr, ptr {dst_prefix_base}, i64 %{label}.dst.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.number.slot = getelementptr i64, ptr {dst_number_base}, i64 %{label}.dst.i\n"
    ));
    ir.push_str(&format!(
        "  %{label}.dst.value.slot = getelementptr {value_ty}, ptr {dst_value_base}, i64 %{label}.dst.i\n"
    ));
    ir.push_str(&format!(
        "  store ptr %{label}.stored.prefix, ptr %{label}.dst.prefix.slot\n"
    ));
    ir.push_str(&format!(
        "  store i64 %{label}.stored.number, ptr %{label}.dst.number.slot\n"
    ));
    ir.push_str(&format!(
        "  store {value_ty} %{label}.src.value, ptr %{label}.dst.value.slot\n"
    ));
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
