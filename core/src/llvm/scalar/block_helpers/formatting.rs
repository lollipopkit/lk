use crate::llvm::{
    const_display::{llvm_string_constant, native_const_list_display},
    ir_text::native_float_display,
    output::emit_native_print_text_parts,
    straightline_value::{NativeBuiltin, NativeStraightlineValue},
};
use crate::vm::ConstRuntimeValueData;

pub(in crate::llvm) fn emit_static_formatted_print(
    ir: &mut String,
    extra_globals: &mut String,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if !matches!(builtin, NativeBuiltin::Print | NativeBuiltin::Println) || args.len() <= 1 {
        return None;
    }
    let NativeStraightlineValue::String { value: format, .. } = args.first()? else {
        return None;
    };
    let mut rest = args.iter().skip(1);
    let mut remaining = format.as_str();
    while let Some(pos) = remaining.find("{}") {
        let (chunk, after_chunk) = remaining.split_at(pos);
        emit_static_print_chunk(ir, extra_globals, chunk, tmp_index);
        let arg = rest.next()?;
        emit_static_print_value_raw(ir, extra_globals, arg, tmp_index)?;
        remaining = &after_chunk[2..];
    }
    if rest.next().is_some() {
        return None;
    }
    emit_static_print_chunk(ir, extra_globals, remaining, tmp_index);
    if builtin == NativeBuiltin::Println {
        ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
    }
    Some(NativeStraightlineValue::Nil)
}

fn emit_static_print_chunk(ir: &mut String, extra_globals: &mut String, chunk: &str, tmp_index: &mut usize) {
    if chunk.is_empty() {
        return;
    }
    let symbol = format!("@lk_fmt_chunk_{}", *tmp_index);
    *tmp_index += 1;
    extra_globals.push_str(&llvm_string_constant(&symbol, chunk));
    ir.push_str(&format!(
        "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
    ));
}

fn emit_static_print_value_raw(
    ir: &mut String,
    extra_globals: &mut String,
    value: &NativeStraightlineValue,
    tmp_index: &mut usize,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 {value})\n"
            ));
        }
        NativeStraightlineValue::F64(value) => {
            if let Ok(parsed) = value.parse::<f64>() {
                let symbol = format!("@lk_fmt_f64_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&symbol, &native_float_display(parsed)));
                ir.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
                ));
                return Some(());
            }
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double {value})\n"
            ));
        }
        NativeStraightlineValue::Bool(value) => {
            if value.starts_with('%') {
                let cond = format!("%fmt_bool_cond_{}", *tmp_index);
                let text = format!("%fmt_bool_text_{}", *tmp_index);
                *tmp_index += 1;
                ir.push_str(&format!("  {cond} = icmp eq i64 {value}, 0\n"));
                ir.push_str(&format!(
                    "  {text} = select i1 {cond}, ptr @lk_bool_false, ptr @lk_bool_true\n"
                ));
                ir.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {text})\n"
                ));
                return Some(());
            }
            let text = if value == "0" {
                "@lk_bool_false"
            } else {
                "@lk_bool_true"
            };
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {text})\n"
            ));
        }
        NativeStraightlineValue::Nil => {
            ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr @lk_nil_text)\n");
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            let symbol = if symbol.is_empty() {
                let generated = format!("@lk_fmt_arg_{}", *tmp_index);
                *tmp_index += 1;
                extra_globals.push_str(&llvm_string_constant(&generated, value));
                generated
            } else {
                symbol.clone()
            };
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
            ));
        }
        NativeStraightlineValue::StringPtr(value) => {
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {value})\n"
            ));
        }
        NativeStraightlineValue::List { value, elements, .. } => {
            let symbol = format!("@lk_fmt_arg_{}", *tmp_index);
            *tmp_index += 1;
            let value = if value.is_empty() {
                native_const_list_display(elements)?
            } else {
                value.clone()
            }
            .replace(", ", ",")
            .replace(": ", ":");
            extra_globals.push_str(&llvm_string_constant(&symbol, &value));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
            ));
        }
        NativeStraightlineValue::Map { value, .. } => {
            let symbol = format!("@lk_fmt_arg_{}", *tmp_index);
            *tmp_index += 1;
            let value = value.replace(", ", ",").replace(": ", ":");
            extra_globals.push_str(&llvm_string_constant(&symbol, &value));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
            ));
        }
        NativeStraightlineValue::Object {
            value,
            type_name,
            fields,
            ..
        } => {
            let symbol = format!("@lk_fmt_arg_{}", *tmp_index);
            *tmp_index += 1;
            let value = native_object_display_text(type_name, fields)
                .unwrap_or_else(|| value.replace(", ", ",").replace(": ", ":"));
            extra_globals.push_str(&llvm_string_constant(&symbol, &value));
            ir.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {symbol})\n"
            ));
        }
        NativeStraightlineValue::Text(parts) => emit_native_print_text_parts(ir, parts, false)?,
        _ => return None,
    }
    Some(())
}

fn native_object_display_text(type_name: &str, fields: &[(String, ConstRuntimeValueData)]) -> Option<String> {
    match type_name {
        "Rect" => {
            let w = native_object_i64_field(fields, "w")?;
            let h = native_object_i64_field(fields, "h")?;
            Some(format!("Rect({w}x{h})"))
        }
        "Circle" => {
            let r = native_object_i64_field(fields, "r")?;
            Some(format!("Circle(r={r})"))
        }
        _ => None,
    }
}

fn native_object_i64_field(fields: &[(String, ConstRuntimeValueData)], key: &str) -> Option<i64> {
    let (_, value) = fields.iter().find(|(field, _)| field == key)?;
    match value {
        ConstRuntimeValueData::Int(value) => Some(*value),
        _ => None,
    }
}
