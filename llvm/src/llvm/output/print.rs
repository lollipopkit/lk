use super::{emit_local_or_global_string_ptr, native_float_display};
use crate::llvm::straightline_value::{NativeStraightlineValue, NativeTextPart, native_runtime_string_key_kind};

use super::object_methods::emit_native_object_method;

pub(super) fn emit_native_print_value(body: &mut String, value: &NativeStraightlineValue, line: bool) -> Option<()> {
    let i64_fmt = if line { "@lk_i64_fmt" } else { "@lk_i64_raw_fmt" };
    let f64_fmt = if line { "@lk_f64_fmt" } else { "@lk_f64_raw_fmt" };
    let str_fmt = if line { "@lk_str_fmt" } else { "@lk_str_raw_fmt" };
    match value {
        NativeStraightlineValue::I64(value) => {
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {i64_fmt}, i64 {value})\n"))
        }
        NativeStraightlineValue::MaybeI64 { .. }
        | NativeStraightlineValue::MaybeF64 { .. }
        | NativeStraightlineValue::MaybeBool { .. }
        | NativeStraightlineValue::MaybeStrPtr { .. } => return None,
        NativeStraightlineValue::F64(value) => {
            if let Ok(parsed) = value.parse::<f64>() {
                let display = native_float_display(parsed);
                let _len = display.chars().count();
                let _key_kind = native_runtime_string_key_kind(&display);
                let symbol = String::new();
                let ptr = emit_local_or_global_string_ptr(body, &symbol, &display)?;
                body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {ptr})\n"));
            } else {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr {f64_fmt}, double {value})\n"
                ));
            }
        }
        NativeStraightlineValue::Bool(value) => {
            if value == "0" {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr @lk_bool_false)\n"
                ));
            } else {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr @lk_bool_true)\n"
                ));
            }
        }
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
        NativeStraightlineValue::String { symbol, value, .. } => {
            let ptr = emit_local_or_global_string_ptr(body, symbol, value)?;
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {ptr})\n"));
        }
        NativeStraightlineValue::Object { .. } => {
            let NativeStraightlineValue::String { symbol, value, .. } = emit_native_object_method(value, "show")?
            else {
                return None;
            };
            let ptr = emit_local_or_global_string_ptr(body, &symbol, &value)?;
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {ptr})\n"));
        }
        NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::Set { .. }
        | NativeStraightlineValue::DisplayMap { .. }
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
        | NativeStraightlineValue::Error { .. } => return None,
        NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Cell { .. } => return None,
    }
    Some(())
}

pub(in crate::llvm) fn emit_native_print_text_parts(
    body: &mut String,
    parts: &[NativeTextPart],
    line: bool,
) -> Option<()> {
    let mut text_index = 0usize;
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
                let bool_ptr = format!("%text_bool_{text_index}");
                text_index += 1;
                let condition = if value == "0" {
                    "false".to_string()
                } else if value == "1" {
                    "true".to_string()
                } else if value.starts_with('%') {
                    let cond = format!("%text_bool_cond_{text_index}");
                    text_index += 1;
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
                let ptr = emit_local_or_global_string_ptr(body, symbol, value)?;
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {ptr})\n"
                ));
            }
        }
    }
    if line {
        body.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
    }
    Some(())
}
