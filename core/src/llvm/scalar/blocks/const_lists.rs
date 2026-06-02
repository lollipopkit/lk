use crate::{
    llvm::{
        const_display::llvm_string_constant,
        ir_text::next_tmp,
        straightline_value::{NativeStraightlineValue, native_const_runtime_string},
    },
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data},
};

pub(super) fn static_const_list_elements(
    elements: &[ConstRuntimeValue32Data],
) -> Option<Vec<&[ConstRuntimeValue32Data]>> {
    elements
        .iter()
        .map(|value| match value {
            ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
                ConstHeapValue32Data::List(elements) => Some(elements.as_slice()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

pub(super) fn emit_const_list_element_index(
    ir: &mut String,
    extra_globals: &mut String,
    elements: &[ConstRuntimeValue32Data],
    outer_index: &str,
    inner_index: usize,
    dst: u8,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let nested = static_const_list_elements(elements)?;
    let values = nested
        .iter()
        .map(|elements| elements.get(inner_index).cloned())
        .collect::<Option<Vec<_>>>()?;
    if let Some(ints) = values
        .iter()
        .map(|value| match value {
            ConstRuntimeValue32Data::Int(value) => Some(*value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
    {
        let selected = emit_i64_select(ir, &ints, outer_index, tmp_index);
        ir.push_str(&format!("  store i64 {selected}, ptr %r{dst}.slot\n"));
        return Some(NativeStraightlineValue::I64(selected));
    }
    if let Some(strings) = values
        .into_iter()
        .map(native_const_runtime_string)
        .collect::<Option<Vec<_>>>()
    {
        let selected = emit_string_select(ir, extra_globals, &strings, outer_index, pc, tmp_index);
        ir.push_str(&format!("  store ptr {selected}, ptr %r{dst}.slot\n"));
        return Some(NativeStraightlineValue::StringPtr(selected));
    }
    None
}

pub(super) fn emit_const_list_element_dynamic_index(
    ir: &mut String,
    extra_globals: &mut String,
    elements: &[ConstRuntimeValue32Data],
    outer_index: &str,
    inner_index: &str,
    dst: u8,
    pc: usize,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let nested = static_const_list_elements(elements)?;
    if nested
        .iter()
        .all(|row| row.iter().all(|value| matches!(value, ConstRuntimeValue32Data::Int(_))))
    {
        let mut selected = "0".to_string();
        for (outer_idx, row) in nested.iter().enumerate() {
            let mut row_selected = "0".to_string();
            for (inner_idx, value) in row.iter().enumerate() {
                let ConstRuntimeValue32Data::Int(value) = value else {
                    return None;
                };
                let cmp = next_tmp(tmp_index);
                let next = next_tmp(tmp_index);
                ir.push_str(&format!("  {cmp} = icmp eq i64 {inner_index}, {inner_idx}\n"));
                ir.push_str(&format!(
                    "  {next} = select i1 {cmp}, i64 {value}, i64 {row_selected}\n"
                ));
                row_selected = next;
            }
            let cmp = next_tmp(tmp_index);
            let next = next_tmp(tmp_index);
            ir.push_str(&format!("  {cmp} = icmp eq i64 {outer_index}, {outer_idx}\n"));
            ir.push_str(&format!(
                "  {next} = select i1 {cmp}, i64 {row_selected}, i64 {selected}\n"
            ));
            selected = next;
        }
        ir.push_str(&format!("  store i64 {selected}, ptr %r{dst}.slot\n"));
        return Some(NativeStraightlineValue::I64(selected));
    }
    if nested.iter().all(|row| {
        row.iter()
            .cloned()
            .map(native_const_runtime_string)
            .all(|value| value.is_some())
    }) {
        let mut selected = "@lk_empty_text".to_string();
        for (outer_idx, row) in nested.iter().enumerate() {
            let mut row_selected = "@lk_empty_text".to_string();
            for (inner_idx, value) in row.iter().cloned().enumerate() {
                let value = native_const_runtime_string(value)?;
                let symbol = format!("@lk_const_list_dyn_{pc}_{outer_idx}_{inner_idx}");
                extra_globals.push_str(&llvm_string_constant(&symbol, &value));
                let cmp = next_tmp(tmp_index);
                let next = next_tmp(tmp_index);
                ir.push_str(&format!("  {cmp} = icmp eq i64 {inner_index}, {inner_idx}\n"));
                ir.push_str(&format!(
                    "  {next} = select i1 {cmp}, ptr {symbol}, ptr {row_selected}\n"
                ));
                row_selected = next;
            }
            let cmp = next_tmp(tmp_index);
            let next = next_tmp(tmp_index);
            ir.push_str(&format!("  {cmp} = icmp eq i64 {outer_index}, {outer_idx}\n"));
            ir.push_str(&format!(
                "  {next} = select i1 {cmp}, ptr {row_selected}, ptr {selected}\n"
            ));
            selected = next;
        }
        ir.push_str(&format!("  store ptr {selected}, ptr %r{dst}.slot\n"));
        return Some(NativeStraightlineValue::StringPtr(selected));
    }
    Some(NativeStraightlineValue::DynamicConstListElement {
        elements: elements.to_vec(),
        index: outer_index.to_string(),
    })
}

pub(super) fn emit_const_list_element_len(
    ir: &mut String,
    elements: &[ConstRuntimeValue32Data],
    outer_index: &str,
    dst: u8,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let lengths = static_const_list_elements(elements)?
        .iter()
        .map(|elements| i64::try_from(elements.len()).ok())
        .collect::<Option<Vec<_>>>()?;
    let selected = emit_i64_select(ir, &lengths, outer_index, tmp_index);
    ir.push_str(&format!("  store i64 {selected}, ptr %r{dst}.slot\n"));
    Some(NativeStraightlineValue::I64(selected))
}

fn emit_i64_select(ir: &mut String, values: &[i64], outer_index: &str, tmp_index: &mut usize) -> String {
    let mut selected = "0".to_string();
    for (idx, value) in values.iter().enumerate() {
        let cmp = next_tmp(tmp_index);
        let next = next_tmp(tmp_index);
        ir.push_str(&format!("  {cmp} = icmp eq i64 {outer_index}, {idx}\n"));
        ir.push_str(&format!("  {next} = select i1 {cmp}, i64 {value}, i64 {selected}\n"));
        selected = next;
    }
    selected
}

fn emit_string_select(
    ir: &mut String,
    extra_globals: &mut String,
    values: &[String],
    outer_index: &str,
    pc: usize,
    tmp_index: &mut usize,
) -> String {
    let mut selected = "@lk_empty_text".to_string();
    for (idx, value) in values.iter().enumerate() {
        let symbol = format!("@lk_const_list_{pc}_{}_{}", idx, *tmp_index);
        extra_globals.push_str(&llvm_string_constant(&symbol, value));
        let cmp = next_tmp(tmp_index);
        let next = next_tmp(tmp_index);
        ir.push_str(&format!("  {cmp} = icmp eq i64 {outer_index}, {idx}\n"));
        ir.push_str(&format!("  {next} = select i1 {cmp}, ptr {symbol}, ptr {selected}\n"));
        selected = next;
    }
    selected
}
