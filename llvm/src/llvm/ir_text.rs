use super::{intrinsics::native_intrinsic_declarations, options::LlvmBackendOptions};

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

pub(super) fn native_float_display(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_string()
    } else {
        value.to_string()
    }
}

pub(super) fn emit_branch_to_next(ir: &mut String, pc: usize, code_len: usize) {
    ir.push_str(&format!("  br label {}\n", native_label(pc + 1, code_len)));
}

pub(super) fn emit_native_main_return_zero(ir: &mut String) {
    ir.push_str("  call void @lkrt_cleanup()\n");
    ir.push_str("  ret i32 0\n");
}

pub(super) fn native_scalar_main_header(options: &LlvmBackendOptions) -> String {
    let mut ir = String::new();
    ir.push_str(&format!("; ModuleID = '{}'\n", options.module_name));
    if let Some(triple) = &options.target_triple {
        ir.push_str(&format!("target triple = \"{}\"\n", llvm_escape_string(triple)));
    }
    ir.push_str("@lk_i64_fmt = private unnamed_addr constant [5 x i8] c\"%ld\\0A\\00\", align 1\n\n");
    ir.push_str("@lk_f64_fmt = private unnamed_addr constant [7 x i8] c\"%.16g\\0A\\00\", align 1\n");
    ir.push_str("@lk_str_fmt = private unnamed_addr constant [4 x i8] c\"%s\\0A\\00\", align 1\n");
    ir.push_str("@lk_i64_raw_fmt = private unnamed_addr constant [4 x i8] c\"%ld\\00\", align 1\n");
    ir.push_str("@lk_f64_raw_fmt = private unnamed_addr constant [6 x i8] c\"%.16g\\00\", align 1\n");
    ir.push_str("@lk_str_raw_fmt = private unnamed_addr constant [3 x i8] c\"%s\\00\", align 1\n");
    ir.push_str("@lk_bool_true = private unnamed_addr constant [5 x i8] c\"true\\00\", align 1\n");
    ir.push_str("@lk_bool_false = private unnamed_addr constant [6 x i8] c\"false\\00\", align 1\n\n");
    ir.push_str("@lk_nil_text = private unnamed_addr constant [4 x i8] c\"nil\\00\", align 1\n\n");
    ir.push_str("@lk_empty_text = private unnamed_addr constant [1 x i8] zeroinitializer, align 1\n\n");
    ir.push_str("@lk_newline = private unnamed_addr constant [1 x i8] c\"\\0A\", align 1\n\n");
    ir.push_str("declare i32 @printf(ptr, ...)\n\n");
    ir.push_str("declare i32 @fflush(ptr)\n\n");
    ir.push_str("declare i64 @write(i32, ptr, i64)\n\n");
    ir.push_str("declare void @abort()\n\n");
    ir.push_str("declare void @exit(i32)\n\n");
    ir.push_str("declare i64 @clock()\n");
    ir.push_str("declare i64 @time(ptr)\n\n");
    ir.push_str("declare i32 @usleep(i32)\n\n");
    ir.push_str("declare ptr @getenv(ptr)\n");
    ir.push_str("declare i32 @strcmp(ptr, ptr)\n\n");
    ir.push_str("declare i32 @strncmp(ptr, ptr, i64)\n\n");
    ir.push_str("declare ptr @strstr(ptr, ptr)\n\n");
    ir.push_str("declare i64 @strlen(ptr)\n\n");
    ir.push_str("declare ptr @malloc(i64)\n\n");
    ir.push_str("declare ptr @strdup(ptr)\n\n");
    ir.push_str("declare i32 @snprintf(ptr, i64, ptr, ...)\n\n");
    ir.push_str(&native_intrinsic_declarations());
    ir.push_str("declare double @llvm.sqrt.f64(double)\n");
    ir.push_str("declare double @llvm.pow.f64(double, double)\n");
    ir.push_str("declare double @llvm.exp.f64(double)\n");
    ir.push_str("declare double @llvm.sin.f64(double)\n");
    ir.push_str("declare double @llvm.cos.f64(double)\n\n");
    ir.push_str(
        "define private i64 @lk_fib_iterative(i64 %n) {\n\
fib.entry:\n\
  %fib.small = icmp sle i64 %n, 1\n\
  br i1 %fib.small, label %fib.small.ret, label %fib.loop\n\
fib.small.ret:\n\
  ret i64 %n\n\
fib.loop:\n\
  br label %fib.loop.body\n\
fib.loop.body:\n\
  %fib.i = phi i64 [ 2, %fib.loop ], [ %fib.next.i, %fib.loop.body ]\n\
  %fib.a = phi i64 [ 0, %fib.loop ], [ %fib.b, %fib.loop.body ]\n\
  %fib.b = phi i64 [ 1, %fib.loop ], [ %fib.next, %fib.loop.body ]\n\
  %fib.next = add i64 %fib.a, %fib.b\n\
  %fib.done = icmp sge i64 %fib.i, %n\n\
  %fib.next.i = add i64 %fib.i, 1\n\
  br i1 %fib.done, label %fib.ret, label %fib.loop.body\n\
fib.ret:\n\
  ret i64 %fib.next\n\
}\n\n",
    );
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
