use crate::{
    llvm::{LlvmBackendOptions, compile_module32_artifact_to_llvm, compile_program_to_llvm, ir_text},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        ConstPool32Data, Function32Data, Instr32, MODULE32_ARTIFACT_VERSION, Module32Artifact, Module32Data, Opcode32,
    },
};

#[test]
fn llvm_backend_rejects_unsupported_runtime_value_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 1; }\nreturn f;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let err = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect_err("unsupported llvm shape");

    assert!(
        err.to_string().contains("LLVM native lowering does not support"),
        "unexpected error: {err}"
    );
}

#[test]
fn llvm_backend_lowers_simple_i64_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 1 + 2 * 3;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");
    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("declare i32 @printf(ptr, ...)"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 7"));
}

#[test]
fn llvm_backend_lowers_bool_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return true;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("select i1"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_lowers_simple_f64_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 1.5 + 2.25 * 2.0;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
    assert!(artifact.module.ir.contains("double"));
}

#[test]
fn llvm_backend_folds_static_f64_instr32_arithmetic_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: Vec::new(),
                    floats: vec![20.0, 6.0, 3.0],
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadFloat, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadFloat, 1, 1).raw(),
                    Instr32::abx(Opcode32::LoadFloat, 2, 2).raw(),
                    Instr32::abc(Opcode32::DivFloat, 3, 0, 1).raw(),
                    Instr32::abc(Opcode32::ModFloat, 4, 0, 1).raw(),
                    Instr32::abc(Opcode32::MulFloat, 5, 4, 2).raw(),
                    Instr32::abc(Opcode32::AddFloat, 6, 3, 5).raw(),
                    Instr32::abc(Opcode32::SubFloat, 7, 6, 2).raw(),
                    Instr32::abc(Opcode32::Return, 7, 1, 0).raw(),
                ],
                register_count: 8,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");
    let expected = 20.0_f64 / 6.0 + (20.0_f64 % 6.0) * 3.0 - 3.0;

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(
        artifact
            .module
            .ir
            .contains(&format!("double {}", ir_text::llvm_float_literal(expected)))
    );
    assert!(artifact.module.ir.contains("lk_divisor_zero:"));
}

#[test]
fn llvm_backend_lowers_source_f64_global_without_shell() {
    let source = r#"
            x := 1.5;
            x = x + 2.25;
            return x;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("%g0.slot = alloca i64"));
    assert!(artifact.module.ir.contains("store double"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("fcmp olt double"));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_simple_short_string_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return \"ok\";").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_str_0"));
    assert!(artifact.module.ir.contains("c\"ok\\00\""));
}

#[test]
fn llvm_backend_lowers_source_string_global_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let text = "ok"; return text;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"ok\\00\""));
}

#[test]
fn llvm_backend_lowers_source_static_string_equality_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let text = "ok"; return text == "ok";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_static_conditional_expression_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return 1 < 2 ? 42 : 7;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_static_match_expression_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let value = match 2 {
            1 => 10,
            2 => 42,
            _ => 7,
        };
        return value;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_if_let_range_guard_or_patterns_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let age = 25;
        let status = 201;
        if let 18..65 = age {
            if let x if x > 20 = age {
                if let 200 | 201 | 202 = status {
                    return x + 17;
                }
            }
        }
        return 0;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("icmp "));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_source_match_range_guard_or_patterns_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let x = 25;
        let y = match x {
            0 | 1 => 0,
            n if n < 10 => 1,
            18..65 => 42,
            _ => 2,
        };
        return y;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_source_for_range_i64_loop_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let total = 0;
        for i in 0..4 {
            total = total + i;
        }
        return total;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("icmp "));
    assert!(artifact.module.ir.contains("add i64"));
    assert!(artifact.module.ir.contains("br i1 %"));
}

#[test]
fn llvm_backend_lowers_source_for_range_break_continue_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let sum = 0;
        for i in 0..7 {
            if (i == 3) {
                continue;
            }
            if (i == 6) {
                break;
            }
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("icmp "));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("add i64"));
    assert!(artifact.module.ir.contains("i64 3"));
    assert!(artifact.module.ir.contains("i64 6"));
}

#[test]
fn llvm_backend_lowers_source_for_inclusive_negative_step_range_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let sum = 0;
        for i in 5..=1..0 - 2 {
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("icmp "));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("add i64"));
    assert!(artifact.module.ir.contains("i64 -2"));
}

#[test]
fn llvm_backend_lowers_source_for_static_list_i64_loop_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let total = 0;
        for value in [1, 2, 3, 4] {
            total = total + value;
        }
        return total;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 10"));
}

#[test]
fn llvm_backend_lowers_source_for_static_string_loop_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let count = 0;
        for ch in "abc" {
            count = count + 1;
        }
        return count;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 3"));
}

#[test]
fn llvm_backend_lowers_source_for_static_map_entry_loop_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let total = 0;
        let items = {"a": 1, "b": 2};
        for (key, value) in items {
            total = total + value;
        }
        return total;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 3"));
}

#[test]
fn llvm_backend_lowers_simple_long_string_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return \"longer-than-short\";").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_heap_str_0"));
    assert!(artifact.module.ir.contains("c\"longer-than-short\\00\""));
}
