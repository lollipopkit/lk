use super::{
    const_display::llvm_string_constant,
    ir_text::native_scalar_main_header,
    options::LlvmBackendOptions,
    straightline_value::{NativeBuiltin, NativeModule, NativeStraightlineValue, NativeTextPart},
};

pub(super) fn emit_native_builtin_call(
    body: &mut String,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match builtin {
        NativeBuiltin::CoreCallMethod => return emit_native_core_call_method(body, args, ssa_index),
        NativeBuiltin::OsClock => return emit_native_os_clock(body, args, ssa_index),
        NativeBuiltin::OsEpoch => return emit_native_os_epoch(body, args, ssa_index),
        NativeBuiltin::Print | NativeBuiltin::Println => {}
    }
    if args.len() > 1 {
        return None;
    }
    let line = builtin == NativeBuiltin::Println;
    if let Some(arg) = args.first() {
        emit_native_print_value(body, arg, line)?;
    } else if line {
        body.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
    }
    Some(NativeStraightlineValue::Nil)
}

fn emit_native_core_call_method(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [
        NativeStraightlineValue::Module(module),
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
    else {
        return None;
    };
    if !elements.is_empty() {
        return None;
    }
    match (module, method.as_str()) {
        (NativeModule::Os, "clock") => emit_native_os_clock(body, &[], ssa_index),
        (NativeModule::Os, "epoch") => emit_native_os_epoch(body, &[], ssa_index),
        _ => None,
    }
}

fn emit_native_os_clock(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if !args.is_empty() {
        return None;
    }
    let ticks = format!("%os_clock_ticks_{}", *ssa_index);
    *ssa_index += 1;
    let ticks_f64 = format!("%os_clock_ticks_f64_{}", *ssa_index);
    *ssa_index += 1;
    let seconds = format!("%os_clock_seconds_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {ticks} = call i64 @clock()\n"));
    body.push_str(&format!("  {ticks_f64} = sitofp i64 {ticks} to double\n"));
    body.push_str(&format!("  {seconds} = fdiv double {ticks_f64}, 1000000.0\n"));
    Some(NativeStraightlineValue::F64(seconds))
}

fn emit_native_os_epoch(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if !args.is_empty() {
        return None;
    }
    let seconds = format!("%os_epoch_seconds_{}", *ssa_index);
    *ssa_index += 1;
    let millis = format!("%os_epoch_millis_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {seconds} = call i64 @time(ptr null)\n"));
    body.push_str(&format!("  {millis} = mul i64 {seconds}, 1000\n"));
    Some(NativeStraightlineValue::I64(millis))
}

fn emit_native_print_value(body: &mut String, value: &NativeStraightlineValue, line: bool) -> Option<()> {
    let i64_fmt = if line { "@lk_i64_fmt" } else { "@lk_i64_raw_fmt" };
    let f64_fmt = if line { "@lk_f64_fmt" } else { "@lk_f64_raw_fmt" };
    let str_fmt = if line { "@lk_str_fmt" } else { "@lk_str_raw_fmt" };
    match value {
        NativeStraightlineValue::I64(value) => {
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {i64_fmt}, i64 {value})\n"))
        }
        NativeStraightlineValue::F64(value) => body.push_str(&format!(
            "  call i32 (ptr, ...) @printf(ptr {f64_fmt}, double {value})\n"
        )),
        NativeStraightlineValue::Bool(_) => return None,
        NativeStraightlineValue::StringPtr(value) => {
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {value})\n"))
        }
        NativeStraightlineValue::Text(parts) => emit_native_print_text_parts(body, parts, line)?,
        NativeStraightlineValue::DynamicSplitText { .. } => return None,
        NativeStraightlineValue::DynamicTextChar => return None,
        NativeStraightlineValue::Nil => {
            body.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr @lk_nil_text)\n"
            ));
        }
        NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::DynamicStringIntMap { .. }
        | NativeStraightlineValue::DynamicIntList { .. }
        | NativeStraightlineValue::DynamicTextList { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::Object { .. }
        | NativeStraightlineValue::Error { .. } => return None,
        NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Cell { .. } => return None,
    }
    Some(())
}

fn emit_native_print_text_parts(body: &mut String, parts: &[NativeTextPart], line: bool) -> Option<()> {
    for part in parts {
        match part {
            NativeTextPart::I64(value) => {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 {value})\n"
                ));
            }
            NativeTextPart::F64(value) => {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double {value})\n"
                ));
            }
            NativeTextPart::Bool(value) => {
                let bool_ptr = format!("%text_bool_{}", body.len());
                let condition = if value == "0" {
                    "false".to_string()
                } else if value == "1" {
                    "true".to_string()
                } else if value.starts_with('%') {
                    let cond = format!("%text_bool_cond_{}", body.len());
                    body.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                    cond
                } else {
                    return None;
                };
                body.push_str(&format!(
                    "  {bool_ptr} = select i1 {}, ptr @lk_bool_true, ptr @lk_bool_false\n",
                    condition
                ));
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {bool_ptr})\n"
                ));
            }
            NativeTextPart::Nil => {
                body.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr @lk_nil_text)\n");
            }
            NativeTextPart::StrPtr(value) => {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {value})\n"
                ));
            }
            NativeTextPart::String { symbol, value } => {
                let _ = value;
                if symbol.is_empty() {
                    return None;
                }
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
                ));
            }
        }
    }
    if line {
        body.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
    }
    Some(())
}

pub(super) fn native_scalar_main_ir(options: &LlvmBackendOptions, body: &str, return_value: Option<&str>) -> String {
    let mut ir = native_scalar_main_header(options);
    ir.push_str(body);
    if let Some(value) = return_value {
        ir.push_str(&format!(
            "  %print = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
        ));
    }
    ir.push_str("  ret i32 0\n");
    ir.push_str("}\n");
    ir
}

pub(super) fn native_straightline_main_ir(
    options: &LlvmBackendOptions,
    body: &str,
    return_value: Option<&NativeStraightlineValue>,
) -> String {
    let mut ir = native_scalar_main_header(options);
    ir.push_str(body);
    let mut globals = String::new();
    if let Some(value) = return_value {
        match value {
            NativeStraightlineValue::I64(value) => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
                ));
            }
            NativeStraightlineValue::F64(value) => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_f64_fmt, double {value})\n"
                ));
            }
            NativeStraightlineValue::Bool(value) => {
                ir.push_str(&format!("  %bool.text = icmp ne i64 {value}, 0\n"));
                ir.push_str("  %bool.ptr = select i1 %bool.text, ptr @lk_bool_true, ptr @lk_bool_false\n");
                ir.push_str("  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr %bool.ptr)\n");
            }
            NativeStraightlineValue::Nil => {
                ir.push_str("  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_nil_text)\n");
            }
            NativeStraightlineValue::String { symbol, value, .. }
            | NativeStraightlineValue::List { symbol, value, .. }
            | NativeStraightlineValue::Map { symbol, value, .. }
            | NativeStraightlineValue::Object { symbol, value, .. } => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
                ));
                globals.push_str(&llvm_string_constant(symbol, value));
            }
            NativeStraightlineValue::Error { symbol } => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
                ));
                globals.push_str(&llvm_string_constant(symbol, "<value>"));
            }
            NativeStraightlineValue::StringPtr(value) => {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {value})\n"
                ));
            }
            NativeStraightlineValue::Text(parts) => {
                let _ = emit_native_print_text_parts(&mut ir, parts, true);
            }
            NativeStraightlineValue::DynamicSplitText { .. } => {}
            NativeStraightlineValue::DynamicTextChar => {}
            NativeStraightlineValue::Function(_)
            | NativeStraightlineValue::Builtin(_)
            | NativeStraightlineValue::Module(_)
            | NativeStraightlineValue::DynamicStringIntMap { .. }
            | NativeStraightlineValue::DynamicIntList { .. }
            | NativeStraightlineValue::DynamicTextList { .. }
            | NativeStraightlineValue::DynamicJoinedText { .. }
            | NativeStraightlineValue::Closure { .. }
            | NativeStraightlineValue::Cell { .. } => {}
        }
    }
    ir.push_str("  ret i32 0\n");
    ir.push_str("}\n");
    ir.push_str(&globals);
    ir
}
