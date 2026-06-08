use crate::{
    llvm::{
        const_display::llvm_string_constant,
        ir_text::{native_float_display, reg_in_bounds},
        scalar::{block_helpers::local_register_kind_before, emit::emit_native_return_print, facts::NativeScalarFacts},
        straightline_value::{
            NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
            native_static_arg_list_display, native_static_callable_display, native_static_module_display,
        },
    },
    vm::Instr,
};

pub(super) fn emit_return_block(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    if instr.return_count() == 0 {
        ir.push_str("  ret i32 0\n");
        return Some(());
    }
    if instr.return_count() != 1 || !reg_in_bounds(register_count, instr.a()) {
        return None;
    }
    let static_value = static_regs.get(instr.a() as usize).and_then(Clone::clone);
    if let Some(display) = static_value.as_ref().and_then(native_static_callable_display) {
        let symbol = format!("@lk_block_return_callable_{pc}");
        ir.push_str(&format!(
            "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
        ));
        extra_globals.push_str(&llvm_string_constant(&symbol, &display));
        ir.push_str("  ret i32 0\n");
        return Some(());
    }
    if let Some(display) = static_value.as_ref().and_then(native_static_module_display) {
        let symbol = format!("@lk_block_return_module_{pc}");
        ir.push_str(&format!(
            "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
        ));
        extra_globals.push_str(&llvm_string_constant(&symbol, &display));
        ir.push_str("  ret i32 0\n");
        return Some(());
    }
    if let Some(display) = static_value.as_ref().and_then(native_static_arg_list_display) {
        let symbol = "@lk_static_arg_list_return".to_string();
        ir.push_str(&format!(
            "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
        ));
        extra_globals.push_str(&llvm_string_constant(&symbol, &display));
        ir.push_str("  ret i32 0\n");
        return Some(());
    }
    if let Some(NativeStraightlineValue::ArgList { elements }) = static_value.as_ref() {
        if emit_dynamic_arg_list_return(ir, extra_globals, pc, elements) {
            return Some(());
        }
        return None;
    }
    if let Some((symbol, value)) = static_value.as_ref().and_then(static_display_value) {
        let symbol = if symbol.is_empty() {
            let symbol = format!("@lk_block_return_static_{pc}");
            extra_globals.push_str(&llvm_string_constant(&symbol, &value));
            symbol
        } else if symbol.starts_with("@lk_static_") {
            extra_globals.push_str(&llvm_string_constant(&symbol, &value));
            symbol
        } else {
            symbol
        };
        ir.push_str(&format!(
            "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
        ));
        ir.push_str("  ret i32 0\n");
        return Some(());
    }
    if let Some(NativeStraightlineValue::DynamicList { id, element }) = static_value.as_ref()
        && matches!(
            element,
            NativeListElementKind::I64
                | NativeListElementKind::F64
                | NativeListElementKind::Bool
                | NativeListElementKind::StrPtr
        )
    {
        emit_dynamic_numeric_list_return(ir, extra_globals, pc, *id, *element);
        return Some(());
    }
    if let Some(NativeStraightlineValue::DynamicPairList { id, first, second }) = static_value.as_ref() {
        emit_dynamic_pair_list_return(ir, extra_globals, pc, *id, *first, *second);
        return Some(());
    }
    if let Some(NativeStraightlineValue::DynamicMap { id, key, value }) = static_value.as_ref() {
        emit_dynamic_map_return(ir, extra_globals, pc, *id, *key, *value);
        return Some(());
    }
    if matches!(
        static_value,
        Some(NativeStraightlineValue::Builtin(_) | NativeStraightlineValue::Module(_))
    ) {
        return None;
    }
    let kind = facts
        .register_kind_before(pc, instr.a())
        .or_else(|| local_register_kind_before(code, pc, instr.a()))?;
    emit_native_return_print(ir, pc, instr.a(), kind, tmp_index);
    ir.push_str("  ret i32 0\n");
    Some(())
}

fn emit_dynamic_arg_list_return(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    elements: &[NativeStraightlineValue],
) -> bool {
    if !elements.iter().all(dynamic_return_value_supported) {
        return false;
    }
    let open = format!("@lk_block_return_arg_open_{pc}");
    let close = format!("@lk_block_return_arg_close_{pc}");
    let sep = format!("@lk_block_return_arg_sep_{pc}");
    extra_globals.push_str(&llvm_string_constant(&open, "["));
    extra_globals.push_str(&llvm_string_constant(&close, "]"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    let mut current_block = format!("bb{pc}");
    for (index, value) in elements.iter().enumerate() {
        if index > 0 {
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
            ));
        }
        if let Some(next_block) = emit_dynamic_return_value(ir, extra_globals, pc, index, &current_block, value) {
            current_block = next_block;
        }
    }
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {close})\n"
    ));
    ir.push_str("  ret i32 0\n");
    true
}

fn dynamic_return_value_supported(value: &NativeStraightlineValue) -> bool {
    match value {
        NativeStraightlineValue::ArgList { elements } => elements.iter().all(dynamic_return_value_supported),
        _ => matches!(
            value,
            NativeStraightlineValue::I64(_)
                | NativeStraightlineValue::MaybeI64 { .. }
                | NativeStraightlineValue::MaybeF64 { .. }
                | NativeStraightlineValue::MaybeBool { .. }
                | NativeStraightlineValue::MaybeStrPtr { .. }
                | NativeStraightlineValue::Bool(_)
                | NativeStraightlineValue::F64(_)
                | NativeStraightlineValue::StringPtr(_)
                | NativeStraightlineValue::DynamicList {
                    element: NativeListElementKind::I64 | NativeListElementKind::F64 | NativeListElementKind::StrPtr,
                    ..
                }
                | NativeStraightlineValue::DynamicList {
                    element: NativeListElementKind::Bool,
                    ..
                }
                | NativeStraightlineValue::DynamicPairList { .. }
                | NativeStraightlineValue::DynamicMap { .. }
        ),
    }
}

fn emit_dynamic_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    current_block: &str,
    value: &NativeStraightlineValue,
) -> Option<String> {
    match value {
        NativeStraightlineValue::I64(value) => {
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 {value})\n"
            ));
            None
        }
        NativeStraightlineValue::MaybeI64 { value, present } => Some(emit_maybe_i64_return_value(
            ir,
            extra_globals,
            pc,
            index,
            current_block,
            value,
            present,
        )),
        NativeStraightlineValue::MaybeF64 { value, present } => Some(emit_maybe_f64_return_value(
            ir,
            extra_globals,
            pc,
            index,
            current_block,
            value,
            present,
        )),
        NativeStraightlineValue::MaybeBool { value, present } => Some(emit_maybe_bool_return_value(
            ir,
            extra_globals,
            pc,
            index,
            current_block,
            value,
            present,
        )),
        NativeStraightlineValue::MaybeStrPtr { value, present } => Some(emit_maybe_str_return_value(
            ir,
            extra_globals,
            pc,
            index,
            current_block,
            value,
            present,
        )),
        NativeStraightlineValue::F64(value) => {
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double {value})\n"
            ));
            None
        }
        NativeStraightlineValue::Bool(value) => {
            let cond = format!("%ret_arg_list_bool_cond_{pc}_{index}");
            let text = format!("%ret_arg_list_bool_text_{pc}_{index}");
            ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
            ir.push_str(&format!(
                "  {text} = select i1 {cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {text})\n"
            ));
            None
        }
        NativeStraightlineValue::StringPtr(value) => {
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {value})\n"
            ));
            None
        }
        NativeStraightlineValue::DynamicList { id, element } => Some(emit_dynamic_numeric_list_return_value(
            ir,
            extra_globals,
            pc,
            index,
            current_block,
            *id,
            *element,
        )),
        NativeStraightlineValue::DynamicPairList { id, first, second } => Some(emit_dynamic_pair_list_return_value(
            ir,
            extra_globals,
            pc,
            index,
            current_block,
            *id,
            *first,
            *second,
        )),
        NativeStraightlineValue::DynamicMap { id, key, value } => Some(emit_dynamic_map_return_value(
            ir,
            extra_globals,
            pc,
            index,
            current_block,
            *id,
            *key,
            *value,
        )),
        NativeStraightlineValue::ArgList { elements } => {
            emit_nested_dynamic_arg_list_value(ir, extra_globals, pc, index, current_block, elements)
        }
        _ => None,
    }
}

fn emit_maybe_i64_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    _current_block: &str,
    value: &str,
    present: &str,
) -> String {
    let nil = format!("@lk_block_return_maybe_nil_{pc}_{index}");
    let value_label = format!("lk.ret.maybe{pc}.{index}.value");
    let nil_label = format!("lk.ret.maybe{pc}.{index}.nil");
    let done_label = format!("lk.ret.maybe{pc}.{index}.done");
    let cond = format!("%ret_maybe_cond_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&nil, "nil"));
    ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
    ir.push_str(&format!("  br i1 {cond}, label %{value_label}, label %{nil_label}\n"));
    ir.push_str(&format!("{value_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 {value})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{nil_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {nil})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{done_label}:\n"));
    done_label
}

fn emit_maybe_f64_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    _current_block: &str,
    value: &str,
    present: &str,
) -> String {
    let nil = format!("@lk_block_return_maybe_f64_nil_{pc}_{index}");
    let value_label = format!("lk.ret.maybe.f64.{pc}.{index}.value");
    let nil_label = format!("lk.ret.maybe.f64.{pc}.{index}.nil");
    let done_label = format!("lk.ret.maybe.f64.{pc}.{index}.done");
    let cond = format!("%ret_maybe_f64_cond_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&nil, "nil"));
    ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
    ir.push_str(&format!("  br i1 {cond}, label %{value_label}, label %{nil_label}\n"));
    ir.push_str(&format!("{value_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double {value})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{nil_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {nil})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{done_label}:\n"));
    done_label
}

fn emit_maybe_bool_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    _current_block: &str,
    value: &str,
    present: &str,
) -> String {
    let nil = format!("@lk_block_return_maybe_bool_nil_{pc}_{index}");
    let value_label = format!("lk.ret.maybe.bool.{pc}.{index}.value");
    let nil_label = format!("lk.ret.maybe.bool.{pc}.{index}.nil");
    let done_label = format!("lk.ret.maybe.bool.{pc}.{index}.done");
    let cond = format!("%ret_maybe_bool_cond_{pc}_{index}");
    let bool_cond = format!("%ret_maybe_bool_value_cond_{pc}_{index}");
    let bool_text = format!("%ret_maybe_bool_text_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&nil, "nil"));
    ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
    ir.push_str(&format!("  br i1 {cond}, label %{value_label}, label %{nil_label}\n"));
    ir.push_str(&format!("{value_label}:\n"));
    ir.push_str(&format!("  {bool_cond} = icmp ne i64 {value}, 0\n"));
    ir.push_str(&format!(
        "  {bool_text} = select i1 {bool_cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
    ));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {bool_text})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{nil_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {nil})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{done_label}:\n"));
    done_label
}

fn emit_maybe_str_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    _current_block: &str,
    value: &str,
    present: &str,
) -> String {
    let nil = format!("@lk_block_return_maybe_str_nil_{pc}_{index}");
    let value_label = format!("lk.ret.maybe.str.{pc}.{index}.value");
    let nil_label = format!("lk.ret.maybe.str.{pc}.{index}.nil");
    let done_label = format!("lk.ret.maybe.str.{pc}.{index}.done");
    let cond = format!("%ret_maybe_str_cond_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&nil, "nil"));
    ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
    ir.push_str(&format!("  br i1 {cond}, label %{value_label}, label %{nil_label}\n"));
    ir.push_str(&format!("{value_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {value})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{nil_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {nil})\n"
    ));
    ir.push_str(&format!("  br label %{done_label}\n"));
    ir.push_str(&format!("{done_label}:\n"));
    done_label
}

fn emit_nested_dynamic_arg_list_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    current_block: &str,
    elements: &[NativeStraightlineValue],
) -> Option<String> {
    if !elements.iter().all(dynamic_return_value_supported) {
        return None;
    }
    let open = format!("@lk_block_return_nested_arg_open_{pc}_{index}");
    let close = format!("@lk_block_return_nested_arg_close_{pc}_{index}");
    let sep = format!("@lk_block_return_nested_arg_sep_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&open, "["));
    extra_globals.push_str(&llvm_string_constant(&close, "]"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    let mut current_block = current_block.to_string();
    for (child_index, value) in elements.iter().enumerate() {
        if child_index > 0 {
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
            ));
        }
        let nested_index = index.saturating_mul(16).saturating_add(child_index);
        if let Some(next_block) =
            emit_dynamic_return_value(ir, extra_globals, pc, nested_index, current_block.as_str(), value)
        {
            current_block = next_block;
        }
    }
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {close})\n"
    ));
    Some(current_block)
}

fn emit_dynamic_numeric_list_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    current_block: &str,
    id: usize,
    element: NativeListElementKind,
) -> String {
    let open = format!("@lk_block_return_list_value_open_{pc}_{index}");
    let close = format!("@lk_block_return_list_value_close_{pc}_{index}");
    let sep = format!("@lk_block_return_list_value_sep_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&open, "["));
    extra_globals.push_str(&llvm_string_constant(&close, "]"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    ir.push_str(&format!(
        "  %ret_arg_list_len_{pc}_{index} = load i64, ptr %list{id}.len.slot\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.list.{pc}.{index}.loop\n"));
    ir.push_str(&format!("ret.arg.list.{pc}.{index}.loop:\n"));
    ir.push_str(&format!("  %ret_arg_list_i_{pc}_{index} = phi i64 [ 0, %{current_block} ], [ %ret_arg_list_next_{pc}_{index}, %ret.arg.list.{pc}.{index}.cont ]\n"));
    ir.push_str(&format!("  %ret_arg_list_done_{pc}_{index} = icmp uge i64 %ret_arg_list_i_{pc}_{index}, %ret_arg_list_len_{pc}_{index}\n"));
    ir.push_str(&format!("  br i1 %ret_arg_list_done_{pc}_{index}, label %ret.arg.list.{pc}.{index}.close, label %ret.arg.list.{pc}.{index}.item\n"));
    ir.push_str(&format!("ret.arg.list.{pc}.{index}.item:\n"));
    ir.push_str(&format!(
        "  %ret_arg_list_need_sep_{pc}_{index} = icmp ne i64 %ret_arg_list_i_{pc}_{index}, 0\n"
    ));
    ir.push_str(&format!("  br i1 %ret_arg_list_need_sep_{pc}_{index}, label %ret.arg.list.{pc}.{index}.sep, label %ret.arg.list.{pc}.{index}.value\n"));
    ir.push_str(&format!("ret.arg.list.{pc}.{index}.sep:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.list.{pc}.{index}.value\n"));
    ir.push_str(&format!("ret.arg.list.{pc}.{index}.value:\n"));
    match element {
        NativeListElementKind::I64 => {
            ir.push_str(&format!("  %ret_arg_list_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_arg_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_list_value_{pc}_{index} = load i64, ptr %ret_arg_list_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_arg_list_value_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::Bool => {
            ir.push_str(&format!("  %ret_arg_list_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_arg_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_list_value_{pc}_{index} = load i64, ptr %ret_arg_list_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  %ret_arg_list_bool_{pc}_{index} = icmp ne i64 %ret_arg_list_value_{pc}_{index}, 0\n"
            ));
            ir.push_str(&format!(
                "  %ret_arg_list_text_{pc}_{index} = select i1 %ret_arg_list_bool_{pc}_{index}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_arg_list_text_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::F64 => {
            ir.push_str(&format!("  %ret_arg_list_slot_{pc}_{index} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 %ret_arg_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_list_value_{pc}_{index} = load double, ptr %ret_arg_list_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double %ret_arg_list_value_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::StrPtr => {
            ir.push_str(&format!("  %ret_arg_list_slot_{pc}_{index} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 %ret_arg_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_list_value_{pc}_{index} = load ptr, ptr %ret_arg_list_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_arg_list_value_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::Text => {}
    }
    ir.push_str(&format!("  br label %ret.arg.list.{pc}.{index}.cont\n"));
    ir.push_str(&format!("ret.arg.list.{pc}.{index}.cont:\n"));
    ir.push_str(&format!(
        "  %ret_arg_list_next_{pc}_{index} = add i64 %ret_arg_list_i_{pc}_{index}, 1\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.list.{pc}.{index}.loop\n"));
    ir.push_str(&format!("ret.arg.list.{pc}.{index}.close:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {close})\n"
    ));
    format!("ret.arg.list.{pc}.{index}.close")
}

fn emit_dynamic_numeric_list_return(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    id: usize,
    element: NativeListElementKind,
) {
    let open = format!("@lk_block_return_list_open_{pc}");
    let close = format!("@lk_block_return_list_close_{pc}");
    let sep = format!("@lk_block_return_list_sep_{pc}");
    extra_globals.push_str(&llvm_string_constant(&open, "["));
    extra_globals.push_str(&llvm_string_constant(&close, "]"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    ir.push_str(&format!("  %ret_list_len_{pc} = load i64, ptr %list{id}.len.slot\n"));
    ir.push_str(&format!("  br label %ret.list.{pc}.loop\n"));
    ir.push_str(&format!("ret.list.{pc}.loop:\n"));
    ir.push_str(&format!(
        "  %ret_list_i_{pc} = phi i64 [ 0, %bb{pc} ], [ %ret_list_next_{pc}, %ret.list.{pc}.cont ]\n"
    ));
    ir.push_str(&format!(
        "  %ret_list_done_{pc} = icmp uge i64 %ret_list_i_{pc}, %ret_list_len_{pc}\n"
    ));
    ir.push_str(&format!(
        "  br i1 %ret_list_done_{pc}, label %ret.list.{pc}.close, label %ret.list.{pc}.item\n"
    ));
    ir.push_str(&format!("ret.list.{pc}.item:\n"));
    ir.push_str(&format!(
        "  %ret_list_need_sep_{pc} = icmp ne i64 %ret_list_i_{pc}, 0\n"
    ));
    ir.push_str(&format!(
        "  br i1 %ret_list_need_sep_{pc}, label %ret.list.{pc}.sep, label %ret.list.{pc}.value\n"
    ));
    ir.push_str(&format!("ret.list.{pc}.sep:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
    ));
    ir.push_str(&format!("  br label %ret.list.{pc}.value\n"));
    ir.push_str(&format!("ret.list.{pc}.value:\n"));
    match element {
        NativeListElementKind::I64 => {
            ir.push_str(&format!("  %ret_list_slot_{pc} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_list_i_{pc}\n"));
            ir.push_str(&format!("  %ret_list_value_{pc} = load i64, ptr %ret_list_slot_{pc}\n"));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_list_value_{pc})\n"
            ));
        }
        NativeListElementKind::Bool => {
            ir.push_str(&format!("  %ret_list_slot_{pc} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_list_i_{pc}\n"));
            ir.push_str(&format!("  %ret_list_value_{pc} = load i64, ptr %ret_list_slot_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_list_bool_{pc} = icmp ne i64 %ret_list_value_{pc}, 0\n"
            ));
            ir.push_str(&format!(
                "  %ret_list_text_{pc} = select i1 %ret_list_bool_{pc}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_list_text_{pc})\n"
            ));
        }
        NativeListElementKind::F64 => {
            ir.push_str(&format!("  %ret_list_slot_{pc} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 %ret_list_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_list_value_{pc} = load double, ptr %ret_list_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double %ret_list_value_{pc})\n"
            ));
        }
        NativeListElementKind::StrPtr => {
            ir.push_str(&format!("  %ret_list_slot_{pc} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 %ret_list_i_{pc}\n"));
            ir.push_str(&format!("  %ret_list_value_{pc} = load ptr, ptr %ret_list_slot_{pc}\n"));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_list_value_{pc})\n"
            ));
        }
        NativeListElementKind::Text => {}
    }
    ir.push_str(&format!("  br label %ret.list.{pc}.cont\n"));
    ir.push_str(&format!("ret.list.{pc}.cont:\n"));
    ir.push_str(&format!("  %ret_list_next_{pc} = add i64 %ret_list_i_{pc}, 1\n"));
    ir.push_str(&format!("  br label %ret.list.{pc}.loop\n"));
    ir.push_str(&format!("ret.list.{pc}.close:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {close})\n"
    ));
    ir.push_str("  ret i32 0\n");
}

fn emit_dynamic_map_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    current_block: &str,
    id: usize,
    key: NativeMapKeyKind,
    value: NativeMapValueKind,
) -> String {
    let open = format!("@lk_block_return_map_value_open_{pc}_{index}");
    let close = format!("@lk_block_return_map_value_close_{pc}_{index}");
    let sep = format!("@lk_block_return_map_value_sep_{pc}_{index}");
    let kv_sep = format!("@lk_block_return_map_value_kv_sep_{pc}_{index}");
    let key_fmt = format!("@lk_block_return_map_value_key_fmt_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&open, "{"));
    extra_globals.push_str(&llvm_string_constant(&close, "}"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    extra_globals.push_str(&llvm_string_constant(&kv_sep, ": "));
    if key == NativeMapKeyKind::Str {
        extra_globals.push_str(&llvm_string_constant(&key_fmt, "%s%ld"));
    }
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    ir.push_str(&format!(
        "  %ret_arg_map_len_{pc}_{index} = load i64, ptr %map{id}.len.slot\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.map.{pc}.{index}.loop\n"));
    ir.push_str(&format!("ret.arg.map.{pc}.{index}.loop:\n"));
    ir.push_str(&format!("  %ret_arg_map_i_{pc}_{index} = phi i64 [ 0, %{current_block} ], [ %ret_arg_map_next_{pc}_{index}, %ret.arg.map.{pc}.{index}.cont ]\n"));
    ir.push_str(&format!(
        "  %ret_arg_map_done_{pc}_{index} = icmp uge i64 %ret_arg_map_i_{pc}_{index}, %ret_arg_map_len_{pc}_{index}\n"
    ));
    ir.push_str(&format!("  br i1 %ret_arg_map_done_{pc}_{index}, label %ret.arg.map.{pc}.{index}.close, label %ret.arg.map.{pc}.{index}.item\n"));
    ir.push_str(&format!("ret.arg.map.{pc}.{index}.item:\n"));
    ir.push_str(&format!(
        "  %ret_arg_map_need_sep_{pc}_{index} = icmp ne i64 %ret_arg_map_i_{pc}_{index}, 0\n"
    ));
    ir.push_str(&format!("  br i1 %ret_arg_map_need_sep_{pc}_{index}, label %ret.arg.map.{pc}.{index}.sep, label %ret.arg.map.{pc}.{index}.key\n"));
    ir.push_str(&format!("ret.arg.map.{pc}.{index}.sep:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.map.{pc}.{index}.key\n"));
    ir.push_str(&format!("ret.arg.map.{pc}.{index}.key:\n"));
    emit_dynamic_map_key_return_value(ir, pc, index, id, key, &key_fmt);
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {kv_sep})\n"
    ));
    emit_dynamic_map_value_return_value(ir, pc, index, id, value);
    ir.push_str(&format!("  br label %ret.arg.map.{pc}.{index}.cont\n"));
    ir.push_str(&format!("ret.arg.map.{pc}.{index}.cont:\n"));
    ir.push_str(&format!(
        "  %ret_arg_map_next_{pc}_{index} = add i64 %ret_arg_map_i_{pc}_{index}, 1\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.map.{pc}.{index}.loop\n"));
    let close_label = format!("ret.arg.map.{pc}.{index}.close");
    ir.push_str(&format!("{close_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {close})\n"
    ));
    close_label
}

fn emit_dynamic_pair_list_return(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    id: usize,
    first: NativeListElementKind,
    second: NativeListElementKind,
) {
    let open = format!("@lk_block_return_pair_list_open_{pc}");
    let close = format!("@lk_block_return_pair_list_close_{pc}");
    let row_open = format!("@lk_block_return_pair_list_row_open_{pc}");
    let row_sep = format!("@lk_block_return_pair_list_row_sep_{pc}");
    let row_close = format!("@lk_block_return_pair_list_row_close_{pc}");
    let sep = format!("@lk_block_return_pair_list_sep_{pc}");
    extra_globals.push_str(&llvm_string_constant(&open, "["));
    extra_globals.push_str(&llvm_string_constant(&close, "]"));
    extra_globals.push_str(&llvm_string_constant(&row_open, "["));
    extra_globals.push_str(&llvm_string_constant(&row_sep, ", "));
    extra_globals.push_str(&llvm_string_constant(&row_close, "]"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    ir.push_str(&format!(
        "  %ret_pair_list_len_{pc} = load i64, ptr %list{id}.len.slot\n"
    ));
    ir.push_str(&format!("  br label %ret.pair.list.{pc}.loop\n"));
    ir.push_str(&format!("ret.pair.list.{pc}.loop:\n"));
    ir.push_str(&format!(
        "  %ret_pair_list_i_{pc} = phi i64 [ 0, %bb{pc} ], [ %ret_pair_list_next_{pc}, %ret.pair.list.{pc}.cont ]\n"
    ));
    ir.push_str(&format!(
        "  %ret_pair_list_done_{pc} = icmp uge i64 %ret_pair_list_i_{pc}, %ret_pair_list_len_{pc}\n"
    ));
    ir.push_str(&format!(
        "  br i1 %ret_pair_list_done_{pc}, label %ret.pair.list.{pc}.close, label %ret.pair.list.{pc}.item\n"
    ));
    ir.push_str(&format!("ret.pair.list.{pc}.item:\n"));
    ir.push_str(&format!(
        "  %ret_pair_list_need_sep_{pc} = icmp ne i64 %ret_pair_list_i_{pc}, 0\n"
    ));
    ir.push_str(&format!(
        "  br i1 %ret_pair_list_need_sep_{pc}, label %ret.pair.list.{pc}.sep, label %ret.pair.list.{pc}.value\n"
    ));
    ir.push_str(&format!("ret.pair.list.{pc}.sep:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
    ));
    ir.push_str(&format!("  br label %ret.pair.list.{pc}.value\n"));
    ir.push_str(&format!("ret.pair.list.{pc}.value:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {row_open})\n"
    ));
    emit_dynamic_pair_field_return(ir, pc, id, first, "first");
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {row_sep})\n"
    ));
    emit_dynamic_pair_field_return(ir, pc, id, second, "second");
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {row_close})\n"
    ));
    ir.push_str(&format!("  br label %ret.pair.list.{pc}.cont\n"));
    ir.push_str(&format!("ret.pair.list.{pc}.cont:\n"));
    ir.push_str(&format!(
        "  %ret_pair_list_next_{pc} = add i64 %ret_pair_list_i_{pc}, 1\n"
    ));
    ir.push_str(&format!("  br label %ret.pair.list.{pc}.loop\n"));
    ir.push_str(&format!("ret.pair.list.{pc}.close:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {close})\n"
    ));
    ir.push_str("  ret i32 0\n");
}

fn emit_dynamic_pair_list_return_value(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    current_block: &str,
    id: usize,
    first: NativeListElementKind,
    second: NativeListElementKind,
) -> String {
    let open = format!("@lk_block_return_pair_list_value_open_{pc}_{index}");
    let close = format!("@lk_block_return_pair_list_value_close_{pc}_{index}");
    let row_open = format!("@lk_block_return_pair_list_value_row_open_{pc}_{index}");
    let row_sep = format!("@lk_block_return_pair_list_value_row_sep_{pc}_{index}");
    let row_close = format!("@lk_block_return_pair_list_value_row_close_{pc}_{index}");
    let sep = format!("@lk_block_return_pair_list_value_sep_{pc}_{index}");
    extra_globals.push_str(&llvm_string_constant(&open, "["));
    extra_globals.push_str(&llvm_string_constant(&close, "]"));
    extra_globals.push_str(&llvm_string_constant(&row_open, "["));
    extra_globals.push_str(&llvm_string_constant(&row_sep, ", "));
    extra_globals.push_str(&llvm_string_constant(&row_close, "]"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    ir.push_str(&format!(
        "  %ret_arg_pair_list_len_{pc}_{index} = load i64, ptr %list{id}.len.slot\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.pair.list.{pc}.{index}.loop\n"));
    ir.push_str(&format!("ret.arg.pair.list.{pc}.{index}.loop:\n"));
    ir.push_str(&format!("  %ret_arg_pair_list_i_{pc}_{index} = phi i64 [ 0, %{current_block} ], [ %ret_arg_pair_list_next_{pc}_{index}, %ret.arg.pair.list.{pc}.{index}.cont ]\n"));
    ir.push_str(&format!(
        "  %ret_arg_pair_list_done_{pc}_{index} = icmp uge i64 %ret_arg_pair_list_i_{pc}_{index}, %ret_arg_pair_list_len_{pc}_{index}\n"
    ));
    ir.push_str(&format!("  br i1 %ret_arg_pair_list_done_{pc}_{index}, label %ret.arg.pair.list.{pc}.{index}.close, label %ret.arg.pair.list.{pc}.{index}.item\n"));
    ir.push_str(&format!("ret.arg.pair.list.{pc}.{index}.item:\n"));
    ir.push_str(&format!(
        "  %ret_arg_pair_list_need_sep_{pc}_{index} = icmp ne i64 %ret_arg_pair_list_i_{pc}_{index}, 0\n"
    ));
    ir.push_str(&format!("  br i1 %ret_arg_pair_list_need_sep_{pc}_{index}, label %ret.arg.pair.list.{pc}.{index}.sep, label %ret.arg.pair.list.{pc}.{index}.value\n"));
    ir.push_str(&format!("ret.arg.pair.list.{pc}.{index}.sep:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.pair.list.{pc}.{index}.value\n"));
    ir.push_str(&format!("ret.arg.pair.list.{pc}.{index}.value:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {row_open})\n"
    ));
    emit_dynamic_pair_field_return_value(ir, pc, index, id, first, "first");
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {row_sep})\n"
    ));
    emit_dynamic_pair_field_return_value(ir, pc, index, id, second, "second");
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {row_close})\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.pair.list.{pc}.{index}.cont\n"));
    ir.push_str(&format!("ret.arg.pair.list.{pc}.{index}.cont:\n"));
    ir.push_str(&format!(
        "  %ret_arg_pair_list_next_{pc}_{index} = add i64 %ret_arg_pair_list_i_{pc}_{index}, 1\n"
    ));
    ir.push_str(&format!("  br label %ret.arg.pair.list.{pc}.{index}.loop\n"));
    let close_label = format!("ret.arg.pair.list.{pc}.{index}.close");
    ir.push_str(&format!("{close_label}:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {close})\n"
    ));
    close_label
}

fn emit_dynamic_pair_field_return(ir: &mut String, pc: usize, id: usize, kind: NativeListElementKind, label: &str) {
    match kind {
        NativeListElementKind::I64 => {
            ir.push_str(&format!("  %ret_pair_list_{label}_slot_{pc} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_pair_list_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_pair_list_{label}_value_{pc} = load i64, ptr %ret_pair_list_{label}_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_pair_list_{label}_value_{pc})\n"
            ));
        }
        NativeListElementKind::Bool => {
            ir.push_str(&format!("  %ret_pair_list_{label}_slot_{pc} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_pair_list_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_pair_list_{label}_value_{pc} = load i64, ptr %ret_pair_list_{label}_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  %ret_pair_list_{label}_bool_{pc} = icmp ne i64 %ret_pair_list_{label}_value_{pc}, 0\n"
            ));
            ir.push_str(&format!(
                "  %ret_pair_list_{label}_text_{pc} = select i1 %ret_pair_list_{label}_bool_{pc}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_pair_list_{label}_text_{pc})\n"
            ));
        }
        NativeListElementKind::F64 => {
            ir.push_str(&format!("  %ret_pair_list_{label}_slot_{pc} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 %ret_pair_list_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_pair_list_{label}_value_{pc} = load double, ptr %ret_pair_list_{label}_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double %ret_pair_list_{label}_value_{pc})\n"
            ));
        }
        NativeListElementKind::StrPtr => {
            ir.push_str(&format!("  %ret_pair_list_{label}_slot_{pc} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 %ret_pair_list_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_pair_list_{label}_value_{pc} = load ptr, ptr %ret_pair_list_{label}_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_pair_list_{label}_value_{pc})\n"
            ));
        }
        NativeListElementKind::Text => {}
    }
}

fn emit_dynamic_pair_field_return_value(
    ir: &mut String,
    pc: usize,
    index: usize,
    id: usize,
    kind: NativeListElementKind,
    label: &str,
) {
    match kind {
        NativeListElementKind::I64 => {
            ir.push_str(&format!("  %ret_arg_pair_list_{label}_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_arg_pair_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_pair_list_{label}_value_{pc}_{index} = load i64, ptr %ret_arg_pair_list_{label}_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_arg_pair_list_{label}_value_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::Bool => {
            ir.push_str(&format!("  %ret_arg_pair_list_{label}_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 %ret_arg_pair_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_pair_list_{label}_value_{pc}_{index} = load i64, ptr %ret_arg_pair_list_{label}_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  %ret_arg_pair_list_{label}_bool_{pc}_{index} = icmp ne i64 %ret_arg_pair_list_{label}_value_{pc}_{index}, 0\n"
            ));
            ir.push_str(&format!(
                "  %ret_arg_pair_list_{label}_text_{pc}_{index} = select i1 %ret_arg_pair_list_{label}_bool_{pc}_{index}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_arg_pair_list_{label}_text_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::F64 => {
            ir.push_str(&format!("  %ret_arg_pair_list_{label}_slot_{pc}_{index} = getelementptr [4096 x double], ptr %list{id}.f64.slots, i64 0, i64 %ret_arg_pair_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_pair_list_{label}_value_{pc}_{index} = load double, ptr %ret_arg_pair_list_{label}_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double %ret_arg_pair_list_{label}_value_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::StrPtr => {
            ir.push_str(&format!("  %ret_arg_pair_list_{label}_slot_{pc}_{index} = getelementptr [4096 x ptr], ptr %list{id}.ptr.slots, i64 0, i64 %ret_arg_pair_list_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_pair_list_{label}_value_{pc}_{index} = load ptr, ptr %ret_arg_pair_list_{label}_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_arg_pair_list_{label}_value_{pc}_{index})\n"
            ));
        }
        NativeListElementKind::Text => {}
    }
}

fn emit_dynamic_map_return(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    id: usize,
    key: NativeMapKeyKind,
    value: NativeMapValueKind,
) {
    let open = format!("@lk_block_return_map_open_{pc}");
    let close = format!("@lk_block_return_map_close_{pc}");
    let sep = format!("@lk_block_return_map_sep_{pc}");
    let kv_sep = format!("@lk_block_return_map_kv_sep_{pc}");
    let key_fmt = format!("@lk_block_return_map_key_fmt_{pc}");
    extra_globals.push_str(&llvm_string_constant(&open, "{"));
    extra_globals.push_str(&llvm_string_constant(&close, "}"));
    extra_globals.push_str(&llvm_string_constant(&sep, ", "));
    extra_globals.push_str(&llvm_string_constant(&kv_sep, ": "));
    if key == NativeMapKeyKind::Str {
        extra_globals.push_str(&llvm_string_constant(&key_fmt, "%s%ld"));
    }
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {open})\n"
    ));
    ir.push_str(&format!("  %ret_map_len_{pc} = load i64, ptr %map{id}.len.slot\n"));
    ir.push_str(&format!("  br label %ret.map.{pc}.loop\n"));
    ir.push_str(&format!("ret.map.{pc}.loop:\n"));
    ir.push_str(&format!(
        "  %ret_map_i_{pc} = phi i64 [ 0, %bb{pc} ], [ %ret_map_next_{pc}, %ret.map.{pc}.cont ]\n"
    ));
    ir.push_str(&format!(
        "  %ret_map_done_{pc} = icmp uge i64 %ret_map_i_{pc}, %ret_map_len_{pc}\n"
    ));
    ir.push_str(&format!(
        "  br i1 %ret_map_done_{pc}, label %ret.map.{pc}.close, label %ret.map.{pc}.item\n"
    ));
    ir.push_str(&format!("ret.map.{pc}.item:\n"));
    ir.push_str(&format!("  %ret_map_need_sep_{pc} = icmp ne i64 %ret_map_i_{pc}, 0\n"));
    ir.push_str(&format!(
        "  br i1 %ret_map_need_sep_{pc}, label %ret.map.{pc}.sep, label %ret.map.{pc}.key\n"
    ));
    ir.push_str(&format!("ret.map.{pc}.sep:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {sep})\n"
    ));
    ir.push_str(&format!("  br label %ret.map.{pc}.key\n"));
    ir.push_str(&format!("ret.map.{pc}.key:\n"));
    emit_dynamic_map_key_return(ir, pc, id, key, &key_fmt);
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {kv_sep})\n"
    ));
    emit_dynamic_map_value_return(ir, pc, id, value);
    ir.push_str(&format!("  br label %ret.map.{pc}.cont\n"));
    ir.push_str(&format!("ret.map.{pc}.cont:\n"));
    ir.push_str(&format!("  %ret_map_next_{pc} = add i64 %ret_map_i_{pc}, 1\n"));
    ir.push_str(&format!("  br label %ret.map.{pc}.loop\n"));
    ir.push_str(&format!("ret.map.{pc}.close:\n"));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {close})\n"
    ));
    ir.push_str("  ret i32 0\n");
}

fn emit_dynamic_map_key_return(ir: &mut String, pc: usize, id: usize, key: NativeMapKeyKind, key_fmt: &str) {
    match key {
        NativeMapKeyKind::Str => {
            ir.push_str(&format!(
                "  %ret_map_key_slot_{pc} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 %ret_map_i_{pc}\n"
            ));
            ir.push_str(&format!(
                "  %ret_map_key_prefix_{pc} = load ptr, ptr %ret_map_key_slot_{pc}\n"
            ));
            ir.push_str(&format!("  %ret_map_number_slot_{pc} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 %ret_map_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_map_key_number_{pc} = load i64, ptr %ret_map_number_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  %ret_map_key_has_number_{pc} = icmp ne i64 %ret_map_key_number_{pc}, 0\n"
            ));
            ir.push_str(&format!(
                "  br i1 %ret_map_key_has_number_{pc}, label %ret.map.{pc}.key.format, label %ret.map.{pc}.key.raw\n"
            ));
            ir.push_str(&format!("ret.map.{pc}.key.format:\n"));
            ir.push_str(&format!(
                "  %ret_map_key_buf_{pc} = getelementptr [4096 x i8], ptr %r0.text.buf, i64 0, i64 0\n"
            ));
            ir.push_str(&format!("  call i32 (ptr, i64, ptr, ...) @snprintf(ptr %ret_map_key_buf_{pc}, i64 4096, ptr {key_fmt}, ptr %ret_map_key_prefix_{pc}, i64 %ret_map_key_number_{pc})\n"));
            ir.push_str(&format!("  br label %ret.map.{pc}.key.print\n"));
            ir.push_str(&format!("ret.map.{pc}.key.raw:\n"));
            ir.push_str(&format!("  br label %ret.map.{pc}.key.print\n"));
            ir.push_str(&format!("ret.map.{pc}.key.print:\n"));
            ir.push_str(&format!("  %ret_map_key_ptr_{pc} = phi ptr [ %ret_map_key_buf_{pc}, %ret.map.{pc}.key.format ], [ %ret_map_key_prefix_{pc}, %ret.map.{pc}.key.raw ]\n"));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_map_key_ptr_{pc})\n"
            ));
        }
        NativeMapKeyKind::I64 => {
            ir.push_str(&format!("  %ret_map_key_slot_{pc} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 %ret_map_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_map_key_value_{pc} = load i64, ptr %ret_map_key_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_map_key_value_{pc})\n"
            ));
        }
    }
}

fn emit_dynamic_map_key_return_value(
    ir: &mut String,
    pc: usize,
    index: usize,
    id: usize,
    key: NativeMapKeyKind,
    key_fmt: &str,
) {
    match key {
        NativeMapKeyKind::Str => {
            ir.push_str(&format!("  %ret_arg_map_key_slot_{pc}_{index} = getelementptr [4096 x ptr], ptr %map{id}.prefix.slots, i64 0, i64 %ret_arg_map_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_key_prefix_{pc}_{index} = load ptr, ptr %ret_arg_map_key_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!("  %ret_arg_map_number_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 %ret_arg_map_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_key_number_{pc}_{index} = load i64, ptr %ret_arg_map_number_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  %ret_arg_map_key_has_number_{pc}_{index} = icmp ne i64 %ret_arg_map_key_number_{pc}_{index}, 0\n"
            ));
            ir.push_str(&format!("  br i1 %ret_arg_map_key_has_number_{pc}_{index}, label %ret.arg.map.{pc}.{index}.key.format, label %ret.arg.map.{pc}.{index}.key.raw\n"));
            ir.push_str(&format!("ret.arg.map.{pc}.{index}.key.format:\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_key_buf_{pc}_{index} = getelementptr [4096 x i8], ptr %r0.text.buf, i64 0, i64 0\n"
            ));
            ir.push_str(&format!("  call i32 (ptr, i64, ptr, ...) @snprintf(ptr %ret_arg_map_key_buf_{pc}_{index}, i64 4096, ptr {key_fmt}, ptr %ret_arg_map_key_prefix_{pc}_{index}, i64 %ret_arg_map_key_number_{pc}_{index})\n"));
            ir.push_str(&format!("  br label %ret.arg.map.{pc}.{index}.key.print\n"));
            ir.push_str(&format!("ret.arg.map.{pc}.{index}.key.raw:\n"));
            ir.push_str(&format!("  br label %ret.arg.map.{pc}.{index}.key.print\n"));
            ir.push_str(&format!("ret.arg.map.{pc}.{index}.key.print:\n"));
            ir.push_str(&format!("  %ret_arg_map_key_ptr_{pc}_{index} = phi ptr [ %ret_arg_map_key_buf_{pc}_{index}, %ret.arg.map.{pc}.{index}.key.format ], [ %ret_arg_map_key_prefix_{pc}_{index}, %ret.arg.map.{pc}.{index}.key.raw ]\n"));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_arg_map_key_ptr_{pc}_{index})\n"
            ));
        }
        NativeMapKeyKind::I64 => {
            ir.push_str(&format!("  %ret_arg_map_key_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %map{id}.number.slots, i64 0, i64 %ret_arg_map_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_key_value_{pc}_{index} = load i64, ptr %ret_arg_map_key_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_arg_map_key_value_{pc}_{index})\n"
            ));
        }
    }
}

fn emit_dynamic_map_value_return(ir: &mut String, pc: usize, id: usize, value: NativeMapValueKind) {
    match value {
        NativeMapValueKind::I64 => {
            ir.push_str(&format!("  %ret_map_value_slot_{pc} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 %ret_map_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_map_value_{pc} = load i64, ptr %ret_map_value_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_map_value_{pc})\n"
            ));
        }
        NativeMapValueKind::Bool => {
            ir.push_str(&format!("  %ret_map_value_slot_{pc} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 %ret_map_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_map_value_{pc} = load i64, ptr %ret_map_value_slot_{pc}\n"
            ));
            ir.push_str(&format!("  %ret_map_bool_{pc} = icmp ne i64 %ret_map_value_{pc}, 0\n"));
            ir.push_str(&format!(
                "  %ret_map_text_{pc} = select i1 %ret_map_bool_{pc}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_map_text_{pc})\n"
            ));
        }
        NativeMapValueKind::F64 => {
            ir.push_str(&format!("  %ret_map_value_slot_{pc} = getelementptr [4096 x double], ptr %map{id}.f64.slots, i64 0, i64 %ret_map_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_map_value_{pc} = load double, ptr %ret_map_value_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double %ret_map_value_{pc})\n"
            ));
        }
        NativeMapValueKind::StrPtr => {
            ir.push_str(&format!("  %ret_map_value_slot_{pc} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 %ret_map_i_{pc}\n"));
            ir.push_str(&format!(
                "  %ret_map_value_{pc} = load ptr, ptr %ret_map_value_slot_{pc}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_map_value_{pc})\n"
            ));
        }
    }
}

fn emit_dynamic_map_value_return_value(ir: &mut String, pc: usize, index: usize, id: usize, value: NativeMapValueKind) {
    match value {
        NativeMapValueKind::I64 => {
            ir.push_str(&format!("  %ret_arg_map_value_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 %ret_arg_map_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_value_{pc}_{index} = load i64, ptr %ret_arg_map_value_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 %ret_arg_map_value_{pc}_{index})\n"
            ));
        }
        NativeMapValueKind::Bool => {
            ir.push_str(&format!("  %ret_arg_map_value_slot_{pc}_{index} = getelementptr [4096 x i64], ptr %map{id}.value.slots, i64 0, i64 %ret_arg_map_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_value_{pc}_{index} = load i64, ptr %ret_arg_map_value_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  %ret_arg_map_bool_{pc}_{index} = icmp ne i64 %ret_arg_map_value_{pc}_{index}, 0\n"
            ));
            ir.push_str(&format!(
                "  %ret_arg_map_text_{pc}_{index} = select i1 %ret_arg_map_bool_{pc}_{index}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_arg_map_text_{pc}_{index})\n"
            ));
        }
        NativeMapValueKind::F64 => {
            ir.push_str(&format!("  %ret_arg_map_value_slot_{pc}_{index} = getelementptr [4096 x double], ptr %map{id}.f64.slots, i64 0, i64 %ret_arg_map_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_value_{pc}_{index} = load double, ptr %ret_arg_map_value_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double %ret_arg_map_value_{pc}_{index})\n"
            ));
        }
        NativeMapValueKind::StrPtr => {
            ir.push_str(&format!("  %ret_arg_map_value_slot_{pc}_{index} = getelementptr [4096 x ptr], ptr %map{id}.ptr.slots, i64 0, i64 %ret_arg_map_i_{pc}_{index}\n"));
            ir.push_str(&format!(
                "  %ret_arg_map_value_{pc}_{index} = load ptr, ptr %ret_arg_map_value_slot_{pc}_{index}\n"
            ));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr %ret_arg_map_value_{pc}_{index})\n"
            ));
        }
    }
}

fn static_display_value(value: &NativeStraightlineValue) -> Option<(String, String)> {
    match value {
        NativeStraightlineValue::String { symbol, value, .. }
        | NativeStraightlineValue::List { symbol, value, .. }
        | NativeStraightlineValue::Map { symbol, value, .. }
        | NativeStraightlineValue::Object { symbol, value, .. } => Some((symbol.clone(), value.clone())),
        NativeStraightlineValue::F64(value) if !value.starts_with('%') => {
            Some((String::new(), native_float_display(value.parse().ok()?)))
        }
        NativeStraightlineValue::Bool(value) if !value.starts_with('%') => {
            let text = if value.parse::<i64>().ok()? != 0 {
                "true"
            } else {
                "false"
            };
            Some((String::new(), text.to_string()))
        }
        NativeStraightlineValue::Error { symbol } => Some((symbol.clone(), "<value>".to_string())),
        _ => None,
    }
}
