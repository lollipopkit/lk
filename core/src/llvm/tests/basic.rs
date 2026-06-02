use crate::{
    llvm::{LlvmBackendOptions, compile_module32_artifact_to_llvm, compile_program_to_llvm, ir_text},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        Compiler32, ConstHeapValue32Data, ConstPool32Data, ConstRuntimeValue32Data, Function32Data, Instr32,
        MODULE32_ARTIFACT_VERSION, Module32Artifact, Module32Data, Opcode32, RuntimeMapKeyData, VmContext,
        compile_program32_module_with_ctx,
    },
};

#[test]
fn llvm_backend_lowers_static_function_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 1; }\nreturn f;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<fn #1(0 captures)>\\00\""));
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
fn llvm_backend_lowers_static_map_rest_has_without_shell() {
    let tokens = Tokenizer::tokenize(
        r#"let user = {"name": "Bob", "age": 25}; let {"name": who, ..remaining} = user; return remaining.has("age");"#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let mut ctx = VmContext::new();
    let module = compile_program32_module_with_ctx(&program, &mut ctx).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_range_indexing_without_shell() {
    for (source, expected) in [
        (r#"let s = "hello"; return s[1..3];"#, "c\"el\\00\""),
        ("let xs = [10, 20, 30]; return xs[0..2];", "c\"[10, 20]\\00\""),
        ("let xs = [10, 20, 30]; return xs[-1];", "i64 30"),
    ] {
        let tokens = Tokenizer::tokenize(source).expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");
        let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");
        assert!(!artifact.module.ir.contains("@lk_module32_json"));
        assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
        assert!(
            artifact.module.ir.contains(expected),
            "{source}\n{}",
            artifact.module.ir
        );
    }
}

#[test]
fn llvm_backend_lowers_static_map_module_delete_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: vec!["map".to_string()],
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![1],
                    floats: Vec::new(),
                    strings: vec!["delete".to_string(), "a".to_string()],
                    heap_values: vec![ConstHeapValue32Data::Map(vec![
                        (
                            RuntimeMapKeyData::ShortStr("a".to_string()),
                            ConstRuntimeValue32Data::Int(1),
                        ),
                        (
                            RuntimeMapKeyData::ShortStr("b".to_string()),
                            ConstRuntimeValue32Data::Int(2),
                        ),
                    ])],
                },
                code: vec![
                    Instr32::abx(Opcode32::GetGlobal, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                    Instr32::abc(Opcode32::GetIndex, 2, 0, 1).raw(),
                    Instr32::abx(Opcode32::LoadHeapConst, 3, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 4, 1).raw(),
                    Instr32::abc(Opcode32::Move, 5, 2, 0).raw(),
                    Instr32::abc(Opcode32::Move, 6, 3, 0).raw(),
                    Instr32::abc(Opcode32::Move, 7, 4, 0).raw(),
                    Instr32::abc(Opcode32::Call, 5, 5, 2).raw(),
                    Instr32::abx(Opcode32::LoadInt, 6, 0).raw(),
                    Instr32::abc(Opcode32::GetIndex, 7, 5, 6).raw(),
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 1"));
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
fn llvm_backend_lowers_source_mixed_int_float_arithmetic_without_shell() {
    let source = r#"
            let answer = 40;
            let precise = answer + 2.5;
            return precise / 2;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("sitofp i64"));
    assert!(artifact.module.ir.contains("fadd double"));
    assert!(artifact.module.ir.contains("fdiv double"));
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
fn llvm_backend_lowers_control_flow_static_contains_without_shell() {
    let source = r#"
        if (!("ell" in "hello")) { return 1; }
        if ("x" in [1, 2, 3]) { return 2; }
        return 0;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
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
    assert!(artifact.module.ir.contains("list1.len.slot"));
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
    assert!(artifact.module.ir.contains("select i1"));
    assert!(artifact.module.ir.contains("add i64"));
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
    assert!(artifact.module.ir.contains("srem"), "should use modulo operation");
}

#[test]
fn llvm_backend_lowers_recursive_list_contains_bool_without_artifact_shell() {
    let source = r#"
        fn contains(xs: List<Int>, target: Int) -> Bool {
            if (xs.len() == 0) { return false; }
            if (xs[0] == target) { return true; }
            return contains(xs.skip(1), target);
        }
        return contains([1, 3, 5, 7], 5);
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let mut ctx = VmContext::new();
    let module = compile_program32_module_with_ctx(&program, &mut ctx).expect("module32");
    let module = Module32Artifact::new(Vec::new(), &module).expect("module32 artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact.module.ir.contains("@lk_bool_true"),
        "Bool recursive hint should keep the entry return printable as bool"
    );
}

#[test]
fn llvm_backend_lowers_dynamic_i64_list_return_without_artifact_shell() {
    let source = r#"
        let out = [];
        for n in [1, 2] {
            out.push(n);
        }
        return out;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_raw_fmt"));
    assert!(artifact.module.ir.contains("%ret_list_i_"));
}

#[test]
fn llvm_backend_lowers_nested_static_i64_list_iteration_without_artifact_shell() {
    let source = r#"
        let matrix = [[1, 2], [3, 4]];
        let flat = [];
        for row in matrix {
            for cell in row {
                flat.push(cell);
            }
        }
        return flat == [1, 2, 3, 4];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(
        artifact.module.ir.contains("load i64, ptr %r10.slot"),
        "{}",
        artifact.module.ir
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
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["println"]).expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("load ptr, ptr %r5.slot"));
    assert!(!artifact.module.ir.contains("@lk_fmt_arg_"));
}

#[test]
fn llvm_backend_lowers_dynamic_f64_list_return_without_artifact_shell() {
    let source = r#"
        let out = [];
        for n in [1, 2] {
            out.push(n + 0.5);
        }
        return out;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("[4096 x double]"));
    assert!(artifact.module.ir.contains("@lk_f64_raw_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_f64_list_methods_without_artifact_shell() {
    let source = r#"
        let xs = [];
        for n in [1, 2] {
            xs = xs.push(n + 0.5);
        }
        return [xs.take(1), xs.skip(1), xs.concat([3.5])];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_take_f64_list"));
    assert!(artifact.module.ir.contains("@lk_slice_f64_list"));
    assert!(artifact.module.ir.contains("@lk_concat_f64_list"));
    assert!(artifact.module.ir.contains("%ret_arg_list_value_"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_list_return_without_artifact_shell() {
    let source = r#"
        let out = [];
        for n in [1, 2] {
            out.push("v${n}");
        }
        return out;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("declare ptr @strdup(ptr)"));
    assert!(artifact.module.ir.contains("@lk_str_raw_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_get_without_artifact_shell() {
    let source = r#"
        import map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        return map.get(m, "k1") + map.get(m, "k2");
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_set_string_f64_map"));
    assert!(artifact.module.ir.contains("@lk_lookup_string_f64_map"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_values_without_artifact_shell() {
    let source = r#"
        import map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        return [map.get(m, "k1"), map.get(m, "k2"), map.values(m)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_set_string_f64_map"));
    assert!(artifact.module.ir.contains("%ret_arg_list_value_"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_keys_values_without_artifact_shell() {
    let source = r#"
        import map;
        let weights = {};
        for n in [1, 2] {
            weights = map.set(weights, "k${n}", n + 0.5);
        }
        return [map.keys(weights), map.values(weights)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_set_string_f64_map"));
    assert!(artifact.module.ir.contains("@lk_map_keys_fmt_"));
    assert!(artifact.module.ir.contains("%ret_arg_list_value_"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_iteration_without_artifact_shell() {
    let source = r#"
        import map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        let total = 0.0;
        for pair in m {
            total = total + pair[1];
        }
        return total;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_set_string_f64_map"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %map"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_pair_list_return_without_artifact_shell() {
    let source = r#"
        import map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        let out = [];
        for pair in m {
            out = out.push([pair[0], pair[1]]);
        }
        return out;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_map_iter_key_fmt_"));
    assert!(artifact.module.ir.contains("ret_pair_list_"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %list"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_i64_map_return_without_artifact_shell() {
    let source = r#"
        import map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n * 10);
        }
        return m;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_block_return_map_key_fmt_"));
    assert!(artifact.module.ir.contains("ret_map_value_"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x i64], ptr %map"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_return_without_artifact_shell() {
    let source = r#"
        import map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        return m;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_block_return_map_key_fmt_"));
    assert!(artifact.module.ir.contains("ret_map_value_"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %map"));
}

#[test]
fn llvm_backend_lowers_dynamic_f64_list_module_methods_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 1] {
            xs = xs.push(n + 0.5);
        }
        return [list.contains(xs, 1.5), list.index_of(xs, 2.5), list.sort(xs), list.reverse(xs), list.pop(xs)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_contains_f64_list"));
    assert!(artifact.module.ir.contains("@lk_index_of_f64_list"));
    assert!(artifact.module.ir.contains("@lk_sort_f64_list"));
    assert!(artifact.module.ir.contains("@lk_reverse_f64_list"));
    assert!(artifact.module.ir.contains("@lk_pop_f64_list"));
    assert!(artifact.module.ir.contains("fcmp oeq double"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_list_module_methods_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 3] {
            xs = xs.push("v${n}");
        }
        return [
            list.contains(xs, "v2"),
            list.index_of(xs, "v3"),
            list.reverse(xs),
            list.sort(xs),
            list.pop(xs)
        ];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_contains_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_index_of_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_sort_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_reverse_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_pop_ptr_list"));
    assert!(artifact.module.ir.contains("call i32 @strcmp"));
}

#[test]
fn llvm_backend_lowers_dynamic_f64_list_module_mutators_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 3] {
            xs = xs.push(n + 0.5);
        }
        return [
            list.push(xs, 4.5),
            list.slice(xs, 1, 3),
            list.insert(xs, 1, 9.5),
            list.remove_at(xs, 1),
            list.set(xs, 1, 8.5)
        ];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_push_f64_list"));
    assert!(artifact.module.ir.contains("@lk_slice_range_f64_list"));
    assert!(artifact.module.ir.contains("@lk_insert_f64_list"));
    assert!(artifact.module.ir.contains("@lk_remove_at_f64_list"));
    assert!(artifact.module.ir.contains("@lk_set_f64_list"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_list_module_mutators_without_artifact_shell() {
    let source = r#"
        import list;
        let xs = [];
        for n in [1, 2, 3] {
            xs.push("v${n}");
        }
        return [
            list.push(xs, "v4"),
            list.slice(xs, 1, 3),
            list.insert(xs, 1, "vx"),
            list.remove_at(xs, 1),
            list.set(xs, 1, "vy")
        ];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
            .expect("compile module");
    let module = Module32Artifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module32_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_push_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_slice_range_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_insert_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_remove_at_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_set_ptr_list"));
}
