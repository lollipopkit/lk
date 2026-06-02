use crate::llvm::{
    const_display::llvm_string_constant,
    ir_text::native_float_display,
    straightline_value::{
        NativeStraightlineValue, native_static_arg_list_display, native_static_callable_display,
        native_static_display_map_display, native_static_module_display,
    },
};

pub(super) fn emit_native_main_return(ir: &mut String, globals: &mut String, value: &NativeStraightlineValue) {
    match value {
        NativeStraightlineValue::I64(value) => {
            ir.push_str(&format!(
                "  %print = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
            ));
        }
        NativeStraightlineValue::F64(value) => {
            if let Ok(parsed) = value.parse::<f64>() {
                let symbol = "@lk_static_f64_return";
                let display = native_float_display(parsed);
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
                ));
                globals.push_str(&llvm_string_constant(symbol, &display));
            } else {
                ir.push_str(&format!(
                    "  %print = call i32 (ptr, ...) @printf(ptr @lk_f64_fmt, double {value})\n"
                ));
            }
        }
        NativeStraightlineValue::Bool(value) => {
            ir.push_str(&format!("  %bool.text = icmp ne i64 {value}, 0\n"));
            ir.push_str("  %bool.ptr = select i1 %bool.text, ptr @lk_bool_true, ptr @lk_bool_false\n");
            ir.push_str("  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr %bool.ptr)\n");
        }
        NativeStraightlineValue::Nil => {}
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
            let _ = super::emit_native_print_text_parts(ir, parts, true);
        }
        value if native_static_callable_display(value).is_some() => {
            let display = native_static_callable_display(value).expect("callable display");
            let symbol = "@lk_static_callable_return";
            ir.push_str(&format!(
                "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
            ));
            globals.push_str(&llvm_string_constant(symbol, &display));
        }
        value if native_static_module_display(value).is_some() => {
            let display = native_static_module_display(value).expect("module display");
            let symbol = "@lk_static_module_return";
            ir.push_str(&format!(
                "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
            ));
            globals.push_str(&llvm_string_constant(symbol, &display));
        }
        value if native_static_arg_list_display(value).is_some() => {
            let display = native_static_arg_list_display(value).expect("arg list display");
            let symbol = "@lk_static_arg_list_return";
            ir.push_str(&format!(
                "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
            ));
            globals.push_str(&llvm_string_constant(symbol, &display));
        }
        value if native_static_display_map_display(value).is_some() => {
            let display = native_static_display_map_display(value).expect("display map display");
            let symbol = "@lk_static_display_map_return";
            ir.push_str(&format!(
                "  %print = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {symbol})\n"
            ));
            globals.push_str(&llvm_string_constant(symbol, &display));
        }
        NativeStraightlineValue::DynamicSplitText { .. }
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::MaybeI64 { .. }
        | NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::DynamicMap { .. }
        | NativeStraightlineValue::DynamicMapIter { .. }
        | NativeStraightlineValue::DynamicMapEntry { .. }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicPairList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicArgListElement { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::Channel { .. }
        | NativeStraightlineValue::ArgList { .. }
        | NativeStraightlineValue::DisplayMap { .. }
        | NativeStraightlineValue::Cell { .. } => {}
        NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. } => {}
    }
}
