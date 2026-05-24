use super::options::LlvmBackendOptions;

pub(super) fn llvm_float_literal(value: f64) -> String {
    if value.is_nan() {
        "0x7FF8000000000000".to_string()
    } else if value == f64::INFINITY {
        "0x7FF0000000000000".to_string()
    } else if value == f64::NEG_INFINITY {
        "0xFFF0000000000000".to_string()
    } else {
        let mut out = value.to_string();
        if !out.contains(['.', 'e', 'E']) {
            out.push_str(".0");
        }
        out
    }
}

pub(super) fn emit_branch_to_next(ir: &mut String, pc: usize, code_len: usize) {
    ir.push_str(&format!("  br label {}\n", native_label(pc + 1, code_len)));
}

pub(super) fn native_scalar_main_header(options: &LlvmBackendOptions) -> String {
    let mut ir = String::new();
    ir.push_str(&format!("; ModuleID = '{}'\n", options.module_name));
    if let Some(triple) = &options.target_triple {
        ir.push_str(&format!("target triple = \"{}\"\n", llvm_escape_string(triple)));
    }
    ir.push_str("@lk_i64_fmt = private unnamed_addr constant [5 x i8] c\"%ld\\0A\\00\", align 1\n\n");
    ir.push_str("@lk_f64_fmt = private unnamed_addr constant [4 x i8] c\"%g\\0A\\00\", align 1\n");
    ir.push_str("@lk_str_fmt = private unnamed_addr constant [4 x i8] c\"%s\\0A\\00\", align 1\n");
    ir.push_str("@lk_bool_true = private unnamed_addr constant [5 x i8] c\"true\\00\", align 1\n");
    ir.push_str("@lk_bool_false = private unnamed_addr constant [6 x i8] c\"false\\00\", align 1\n\n");
    ir.push_str("@lk_nil_text = private unnamed_addr constant [4 x i8] c\"nil\\00\", align 1\n\n");
    ir.push_str("declare i32 @printf(ptr, ...)\n\n");
    ir.push_str("define i32 @main() {\n");
    ir.push_str("entry:\n");
    ir
}

pub(super) fn native_relative_target(pc: usize, offset: i32, code_len: usize) -> Option<usize> {
    let target = pc as i64 + 1 + offset as i64;
    if target < 0 || target as usize > code_len {
        return None;
    }
    Some(target as usize)
}

pub(super) fn native_label(pc: usize, code_len: usize) -> String {
    if pc == code_len {
        "%exit".to_string()
    } else {
        format!("%bb{pc}")
    }
}

pub(super) fn next_tmp(index: &mut usize) -> String {
    let name = format!("%t{}", *index);
    *index += 1;
    name
}

pub(super) fn reg_in_bounds(register_count: usize, reg: u8) -> bool {
    (reg as usize) < register_count
}

pub(super) fn llvm_escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &byte in bytes {
        match byte {
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:02X}")),
        }
    }
    out
}

fn llvm_escape_string(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:02X}")),
        }
    }
    out
}
