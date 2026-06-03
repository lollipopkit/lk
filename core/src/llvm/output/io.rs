use super::{emit_local_or_global_string_ptr, emit_native_object_method};
use crate::llvm::straightline_value::{NativeStraightlineValue, NativeTextPart};

pub(super) fn emit_native_stderr_value(body: &mut String, value: &NativeStraightlineValue, line: bool) -> Option<()> {
    body.push_str("  call i32 @fflush(ptr null)\n");
    emit_native_fd_value(body, 2, value, line)
}

fn emit_native_fd_value(body: &mut String, fd: i32, value: &NativeStraightlineValue, line: bool) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => emit_formatted_fd_write(
            body,
            fd,
            if line { "@lk_i64_fmt" } else { "@lk_i64_raw_fmt" },
            "i64",
            value,
        ),
        NativeStraightlineValue::F64(value) => emit_formatted_fd_write(
            body,
            fd,
            if line { "@lk_f64_fmt" } else { "@lk_f64_raw_fmt" },
            "double",
            value,
        ),
        NativeStraightlineValue::Bool(value) => emit_bool_fd_write(body, fd, value, line),
        NativeStraightlineValue::StringPtr(value) => emit_ptr_fd_write(body, fd, value, None, line),
        NativeStraightlineValue::Text(parts) => emit_text_fd_write(body, fd, parts, line)?,
        NativeStraightlineValue::Nil => emit_ptr_fd_write(body, fd, "@lk_nil_text", Some(3), line),
        NativeStraightlineValue::String { symbol, value, .. } => {
            let ptr = emit_local_or_global_string_ptr(body, symbol, value)?;
            emit_ptr_fd_write(body, fd, &ptr, Some(value.len()), line)
        }
        NativeStraightlineValue::Object { .. } => {
            let NativeStraightlineValue::String { symbol, value, .. } = emit_native_object_method(value, "show")?
            else {
                return None;
            };
            let ptr = emit_local_or_global_string_ptr(body, &symbol, &value)?;
            emit_ptr_fd_write(body, fd, &ptr, Some(value.len()), line)
        }
        NativeStraightlineValue::DynamicSplitText { .. }
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::MaybeI64 { .. }
        | NativeStraightlineValue::MaybeF64 { .. }
        | NativeStraightlineValue::MaybeBool { .. }
        | NativeStraightlineValue::MaybeStrPtr { .. }
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
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
        | NativeStraightlineValue::Error { .. }
        | NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Cell { .. } => return None,
    }
    Some(())
}

fn emit_formatted_fd_write(body: &mut String, fd: i32, fmt: &str, ty: &str, value: &str) {
    let id = body.len();
    let buf = format!("%lk_fd_fmt_buf_{id}");
    let len32 = format!("%lk_fd_fmt_len32_{id}");
    let len = format!("%lk_fd_fmt_len_{id}");
    body.push_str(&format!("  {buf} = alloca [128 x i8]\n"));
    body.push_str(&format!(
        "  {len32} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 128, ptr {fmt}, {ty} {value})\n"
    ));
    body.push_str(&format!("  {len} = sext i32 {len32} to i64\n"));
    body.push_str(&format!("  call i64 @write(i32 {fd}, ptr {buf}, i64 {len})\n"));
}

fn emit_bool_fd_write(body: &mut String, fd: i32, value: &str, line: bool) {
    if value == "0" {
        emit_ptr_fd_write(body, fd, "@lk_bool_false", Some(5), line);
    } else if value == "1" {
        emit_ptr_fd_write(body, fd, "@lk_bool_true", Some(4), line);
    } else {
        let id = body.len();
        let cond = format!("%lk_fd_bool_cond_{id}");
        let ptr = format!("%lk_fd_bool_ptr_{id}");
        let len = format!("%lk_fd_bool_len_{id}");
        body.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
        body.push_str(&format!(
            "  {ptr} = select i1 {cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
        ));
        body.push_str(&format!("  {len} = select i1 {cond}, i64 4, i64 5\n"));
        body.push_str(&format!("  call i64 @write(i32 {fd}, ptr {ptr}, i64 {len})\n"));
        emit_newline_fd_write(body, fd, line);
    }
}

fn emit_text_fd_write(body: &mut String, fd: i32, parts: &[NativeTextPart], line: bool) -> Option<()> {
    for part in parts {
        match part {
            NativeTextPart::I64(value) => emit_formatted_fd_write(body, fd, "@lk_i64_raw_fmt", "i64", value),
            NativeTextPart::F64(value) => emit_formatted_fd_write(body, fd, "@lk_f64_raw_fmt", "double", value),
            NativeTextPart::Bool(value) => emit_bool_fd_write(body, fd, value, false),
            NativeTextPart::Nil => emit_ptr_fd_write(body, fd, "@lk_nil_text", Some(3), false),
            NativeTextPart::StrPtr(value) => emit_ptr_fd_write(body, fd, value, None, false),
            NativeTextPart::String { symbol, value } => {
                let ptr = emit_local_or_global_string_ptr(body, symbol, value)?;
                emit_ptr_fd_write(body, fd, &ptr, Some(value.len()), false);
            }
        }
    }
    emit_newline_fd_write(body, fd, line);
    Some(())
}

fn emit_ptr_fd_write(body: &mut String, fd: i32, ptr: &str, known_len: Option<usize>, line: bool) {
    let len = if let Some(len) = known_len {
        len.to_string()
    } else {
        let id = body.len();
        let len = format!("%lk_fd_strlen_{id}");
        body.push_str(&format!("  {len} = call i64 @strlen(ptr {ptr})\n"));
        len
    };
    body.push_str(&format!("  call i64 @write(i32 {fd}, ptr {ptr}, i64 {len})\n"));
    emit_newline_fd_write(body, fd, line);
}

fn emit_newline_fd_write(body: &mut String, fd: i32, line: bool) {
    if line {
        body.push_str(&format!("  call i64 @write(i32 {fd}, ptr @lk_newline, i64 1)\n"));
    }
}
