use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm, compile_program_to_llvm},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{Compiler, ModuleArtifact},
};

#[test]
fn llvm_backend_lowers_bool_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return true;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("select i1"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_lowers_simple_f64_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 1.5 + 2.25 * 2.0;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
    assert!(artifact.module.ir.contains("double"));
}

#[test]
fn llvm_backend_lowers_source_mixed_int_float_arithmetic_without_shell() {
    let source = r#"
            let answer = 40;
            let precise = answer + 2.5;
            return precise / 2;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("sitofp i64"));
    assert!(artifact.module.ir.contains("fadd double"));
    // Float division routes through the divisor-guarded lkrt helper (abort on /0)
    // rather than a raw `fdiv` that would silently produce infinity.
    assert!(artifact.module.ir.contains("call double @lkrt_f64_div_checked"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
}

#[test]
fn llvm_backend_lowers_source_f64_branch_without_shell() {
    let source = r#"
            let x = 1.5;
            let y = 2.25;
            if (x < y) {
                return true;
            }
            return false;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("fcmp olt double"));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_source_static_string_equality_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let text = "ok"; return text == "ok";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_simple_long_string_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return \"longer-than-short\";").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    // Long-string literals now lower through the MIR pipeline as interned
    // string globals (hex-escaped bytes).
    assert!(artifact.module.ir.contains("; ModuleID = 'lk_aot'"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_str_0"));
}

#[test]
fn llvm_backend_lowers_recursive_factorial_without_artifact_shell() {
    let source = "fn factorial(n) { if (n <= 1) { return 1; } return n * factorial(n - 1); }\nreturn factorial(5);";
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");
    assert!(
        artifact.module.ir.contains("@lk_fn_1"),
        "should generate subfunction for recursive call"
    );
    assert!(
        artifact.module.ir.contains("call i64 @lk_fn_1"),
        "should emit recursive call instruction"
    );
}

#[test]
fn llvm_backend_lowers_recursive_fibonacci_without_artifact_shell() {
    let source = "fn fib(n) { if (n <= 0) { return 0; } if (n == 1) { return 1; } return fib(n - 1) + fib(n - 2); }\nreturn fib(10);";
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");
    assert!(
        artifact.module.ir.contains("@lk_fn_1"),
        "should generate subfunction for recursive call"
    );
}

#[test]
fn llvm_backend_lowers_recursive_gcd_without_artifact_shell() {
    let source = "fn gcd(a, b) { if (b == 0) { return a; } return gcd(b, a % b); }\nreturn gcd(100, 75);";
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");
    assert!(
        artifact.module.ir.contains("@lk_fn_1"),
        "should generate subfunction for recursive call"
    );
    assert!(
        artifact.module.ir.contains("@lkrt_i64_mod_checked"),
        "modulo should route through the divisor-guarded lkrt helper"
    );
}

#[test]
fn llvm_backend_formats_control_flow_assigned_string_from_slot_without_artifact_shell() {
    let source = r#"
        let score = 75;
        let g = "";
        if (score >= 90) {
            g = "A";
        } else if (score >= 70) {
            g = "C";
        } else {
            g = "F";
        }
        println("g = {}", g);
        return g == "C";
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["println"]).expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    // The branch-assigned string and its `println` now lower through the MIR
    // pipeline: the merged string is a block-param phi and the formatted line
    // is assembled via the string-concat helper.
    assert!(artifact.module.ir.contains("; ModuleID = 'lk_aot'"));
    assert!(artifact.module.ir.contains("phi ptr"));
    assert!(artifact.module.ir.contains("@lkrt_str_concat"));
}
