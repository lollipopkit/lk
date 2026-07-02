use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm, compile_program_to_llvm, ir_text},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        Compiler, ConstHeapValueData, ConstPoolData, ConstRuntimeValueData, FunctionData, Instr,
        MODULE_ARTIFACT_VERSION, ModuleArtifact, ModuleData, Opcode, RuntimeMapKeyData, VmContext,
        compile_program_module_with_ctx,
    },
};

#[test]
fn llvm_backend_lowers_static_function_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 1; }\nreturn f;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<fn #1(0 captures)>\\00\""));
}

#[test]
fn llvm_backend_lowers_simple_i64_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 1 + 2 * 3;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");
    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("declare i32 @printf(ptr, ...)"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 7"));
}

#[test]
fn llvm_backend_lowers_bool_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return true;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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
    let module = compile_program_module_with_ctx(&program, &mut ctx).expect("module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("module artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_dynamic_map_get_field_k_without_shell() {
    let tokens = Tokenizer::tokenize(
        r#"
        let config = {};
        config.set("retries", 7);
        let flag = 1;
        if flag == 1 {
            return config.retries;
        }
        return 0;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module(&program).expect("module");
    assert!(
        module.functions[module.entry as usize]
            .code
            .iter()
            .any(|instr| instr.opcode() == Opcode::GetFieldK),
        "expected compiler to emit GetFieldK for const string map field"
    );
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("module artifact");
    let artifact = compile_module_artifact_to_llvm(&artifact, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_int_lookup"));
    assert!(artifact.module.ir.contains("store i64 7"));
}

#[test]
fn llvm_backend_keeps_dynamic_map_get_field_k_nil_branch_runtime_checked() {
    let tokens = Tokenizer::tokenize(
        r#"
        let config = {};
        let flag = 1;
        if flag == 1 {
            let retries = config.retries;
            if retries != nil {
                return retries;
            }
            return 3;
        }
        return 0;
        "#,
    )
    .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module(&program).expect("module");
    assert!(
        module.functions[module.entry as usize]
            .code
            .iter()
            .any(|instr| instr.opcode() == Opcode::GetFieldK),
        "expected compiler to emit GetFieldK for const string map field"
    );
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("module artifact");
    let artifact = compile_module_artifact_to_llvm(&artifact, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_int_lookup"));
    assert!(artifact.module.ir.contains(".present.slot"));
    assert!(artifact.module.ir.contains("icmp eq i64") || artifact.module.ir.contains("icmp ne i64"));
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
        let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");
        assert!(!artifact.module.ir.contains("@lk_module_json"));
        assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
        assert!(
            artifact.module.ir.contains(expected),
            "{source}\n{}",
            artifact.module.ir
        );
    }
}

#[test]
fn llvm_backend_lowers_static_map_module_delete_without_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["map".to_string()],
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![1],
                    floats: Vec::new(),
                    strings: vec!["delete".to_string(), "a".to_string()],
                    heap_values: vec![ConstHeapValueData::Map(vec![
                        (
                            RuntimeMapKeyData::ShortStr("a".to_string()),
                            ConstRuntimeValueData::Int(1),
                        ),
                        (
                            RuntimeMapKeyData::ShortStr("b".to_string()),
                            ConstRuntimeValueData::Int(2),
                        ),
                    ])],
                },
                code: vec![
                    Instr::abx(Opcode::GetGlobal, 0, 0).raw(),
                    Instr::abx(Opcode::LoadString, 1, 0).raw(),
                    Instr::abc(Opcode::GetIndex, 2, 0, 1).raw(),
                    Instr::abx(Opcode::LoadHeapConst, 3, 0).raw(),
                    Instr::abx(Opcode::LoadString, 4, 1).raw(),
                    Instr::abc(Opcode::Move, 5, 2, 0).raw(),
                    Instr::abc(Opcode::Move, 6, 3, 0).raw(),
                    Instr::abc(Opcode::Move, 7, 4, 0).raw(),
                    Instr::abc(Opcode::Call, 5, 5, 2).raw(),
                    Instr::abx(Opcode::LoadInt, 6, 0).raw(),
                    Instr::abc(Opcode::GetIndex, 7, 5, 6).raw(),
                    Instr::abc(Opcode::Return, 7, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 8,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_simple_f64_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 1.5 + 2.25 * 2.0;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
    assert!(artifact.module.ir.contains("double"));
}

#[test]
fn llvm_backend_folds_static_f64_instr_arithmetic_without_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: Vec::new(),
                    floats: vec![20.0, 6.0, 3.0],
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
                    Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
                    Instr::abx(Opcode::LoadFloat, 2, 2).raw(),
                    Instr::abc(Opcode::DivFloat, 3, 0, 1).raw(),
                    Instr::abc(Opcode::ModFloat, 4, 0, 1).raw(),
                    Instr::abc(Opcode::MulFloat, 5, 4, 2).raw(),
                    Instr::abc(Opcode::AddFloat, 6, 3, 5).raw(),
                    Instr::abc(Opcode::SubFloat, 7, 6, 2).raw(),
                    Instr::abc(Opcode::Return, 7, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 8,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact =
        compile_module_artifact_to_llvm(&artifact, super::legacy_text_backend_options()).expect("llvm artifact");
    let expected = 20.0_f64 / 6.0 + (20.0_f64 % 6.0) * 3.0 - 3.0;

    assert!(!artifact.module.ir.contains("@lk_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("fcmp olt double"));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_simple_short_string_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return \"ok\";").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_str_0"));
    assert!(artifact.module.ir.contains("c\"ok\\00\""));
}

#[test]
fn llvm_backend_lowers_source_string_global_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let text = "ok"; return text;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"ok\\00\""));
}

#[test]
fn llvm_backend_lowers_source_static_string_equality_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let text = "ok"; return text == "ok";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_source_static_conditional_expression_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return 1 < 2 ? 42 : 7;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_text_backend_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("select i1"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_simple_long_string_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return \"longer-than-short\";").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_heap_str_0"));
    assert!(artifact.module.ir.contains("c\"longer-than-short\\00\""));
}

#[test]
fn llvm_backend_lowers_recursive_factorial_without_artifact_shell() {
    let source = "fn factorial(n) { if (n <= 1) { return 1; } return n * factorial(n - 1); }\nreturn factorial(5);";
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");
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
    let module = compile_program_module_with_ctx(&program, &mut ctx).expect("module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("module artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["println"]).expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_take"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_slice"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_concat"));
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

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("declare ptr @strdup(ptr)"));
    assert!(artifact.module.ir.contains("@lk_str_raw_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_bool_list_return_without_artifact_shell() {
    let source = r#"
        let out = [];
        for n in [1, 2, 3] {
            out.push(n > 1);
        }
        return out;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_get_without_artifact_shell() {
    let source = r#"
        use map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        return map.get(m, "k1") + map.get(m, "k2");
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_f64_set"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_f64_lookup"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_values_without_artifact_shell() {
    let source = r#"
        use map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        return [map.get(m, "k1"), map.get(m, "k2"), map.values(m)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_f64_set"));
    assert!(artifact.module.ir.contains("%ret_arg_list_value_"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_keys_values_without_artifact_shell() {
    let source = r#"
        use map;
        let weights = {};
        for n in [1, 2] {
            weights = map.set(weights, "k${n}", n + 0.5);
        }
        return [map.keys(weights), map.values(weights)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_f64_set"));
    assert!(artifact.module.ir.contains("@lk_map_keys_fmt_"));
    assert!(artifact.module.ir.contains("%ret_arg_list_value_"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_iteration_without_artifact_shell() {
    let source = r#"
        use map;
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
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_f64_set"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %map"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_pair_list_return_without_artifact_shell() {
    let source = r#"
        use map;
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
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_map_iter_key_fmt_"));
    assert!(artifact.module.ir.contains("ret_pair_list_"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %list"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_i64_map_return_without_artifact_shell() {
    let source = r#"
        use map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n * 10);
        }
        return m;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_block_return_map_key_fmt_"));
    assert!(artifact.module.ir.contains("ret_map_value_"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x i64], ptr %map"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_f64_map_return_without_artifact_shell() {
    let source = r#"
        use map;
        let m = {};
        for n in [1, 2] {
            m = map.set(m, "k${n}", n + 0.5);
        }
        return m;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_block_return_map_key_fmt_"));
    assert!(artifact.module.ir.contains("ret_map_value_"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %map"));
}

#[test]
fn llvm_backend_lowers_dynamic_f64_list_module_methods_without_artifact_shell() {
    let source = r#"
        use list;
        let xs = [];
        for n in [1, 2, 1] {
            xs = xs.push(n + 0.5);
        }
        return [list.contains(xs, 1.5), list.index_of(xs, 2.5), list.sort(xs), list.reverse(xs), list.pop(xs)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_contains"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_index_of"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_sort"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_reverse"));
    assert!(artifact.module.ir.contains("@lkrt_list_f64_pop"));
    // The `fcmp oeq` comparison now lives inside the lkrt helper, so it no
    // longer appears in the generated module IR.
}

#[test]
fn llvm_backend_lowers_dynamic_string_list_module_methods_without_artifact_shell() {
    let source = r#"
        use list;
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
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_list_str_contains"));
    assert!(artifact.module.ir.contains("@lkrt_list_str_index_of"));
    assert!(artifact.module.ir.contains("@lkrt_list_str_sort"));
    assert!(artifact.module.ir.contains("@lkrt_list_str_reverse"));
    assert!(artifact.module.ir.contains("@lkrt_list_str_pop"));
    // The `strcmp` comparison now lives inside the lkrt helper, so it no longer
    // appears in the generated module IR for these string-list methods.
}

#[test]
fn llvm_backend_lowers_dynamic_i64_i64_map_return_without_artifact_shell() {
    let source = r#"
        use map;
        let counts = {};
        for n in [1, 2] {
            counts = map.set(counts, n, n * 10);
        }
        return counts;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_int_set"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_int_lookup"));
    assert!(artifact.module.ir.contains("@lk_i64_raw_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_i64_map_get_values_without_artifact_shell() {
    let source = r#"
        use map;
        let counts = {};
        for n in [1, 2] {
            counts = map.set(counts, n, n * 10);
        }
        return [map.get(counts, 1), map.get(counts, 2), map.values(counts)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_int_set"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_int_lookup"));
    assert!(artifact.module.ir.contains("@lk_block_return_maybe_nil"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_i64_map_iteration_without_artifact_shell() {
    let source = r#"
        use map;
        let counts = {};
        for n in [1, 2] {
            counts = map.set(counts, n, n * 10);
        }
        let out = [];
        for pair in counts {
            out = out.push([pair[0], pair[1]]);
        }
        return out;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("[[1, 10], [2, 20]]"));
    assert!(artifact.module.ir.contains("@lk_i64_raw_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_f64_map_return_without_artifact_shell() {
    let source = r#"
        use map;
        let weights = {};
        for n in [1, 2] {
            weights = map.set(weights, n, n + 0.5);
        }
        return weights;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_f64_set"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_f64_lookup"));
    assert!(artifact.module.ir.contains("@lk_f64_raw_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_f64_map_get_keys_values_without_artifact_shell() {
    let source = r#"
        use map;
        let weights = {};
        for n in [1, 2] {
            weights = map.set(weights, n, n + 0.5);
        }
        return [map.get(weights, 1), map.get(weights, 2), map.keys(weights), map.values(weights)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_f64_set"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_f64_lookup"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x i64], ptr %list"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %list"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_f64_map_iteration_without_artifact_shell() {
    let source = r#"
        use map;
        let weights = {};
        for n in [1, 2] {
            weights = map.set(weights, n, n + 0.5);
        }
        let total = 0.0;
        for pair in weights {
            total = total + pair[1];
        }
        return total;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_f64_set"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %map"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_bool_map_without_artifact_shell() {
    let source = r#"
        use map;
        let flags = {};
        for n in [1, 2] {
            flags = map.set(flags, "k${n}", n > 1);
        }
        return [map.get(flags, "k1"), map.get(flags, "k2"), map.values(flags)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_int_set"));
    assert!(artifact.module.ir.contains("@lkrt_map_str_int_lookup"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
}

#[test]
fn llvm_backend_lowers_dynamic_i64_bool_map_without_artifact_shell() {
    let source = r#"
        use map;
        let flags = {};
        for n in [1, 2] {
            flags = map.set(flags, n, n > 1);
        }
        let out = [];
        for pair in flags {
            out = out.push([pair[0], pair[1]]);
        }
        return [map.get(flags, 1), map.get(flags, 2), map.keys(flags), map.values(flags), out];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
    assert!(
        artifact
            .module
            .ir
            .contains("[false, true, [1, 2], [false, true], [[1, false], [2, true]]]")
    );
}

#[test]
fn llvm_backend_lowers_dynamic_i64_string_map_without_artifact_shell() {
    let source = r#"
        use map;
        let names = {};
        for n in [1, 2] {
            names = map.set(names, n, "v${n}");
        }
        return [map.get(names, 1), names[2], map.keys(names), map.values(names), names];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_ptr_set"));
    assert!(artifact.module.ir.contains("@lkrt_map_i64_ptr_lookup"));
    assert!(artifact.module.ir.contains("call ptr @strdup"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x ptr], ptr %list"));
    assert!(artifact.module.ir.contains("ret.arg.map."));
}

#[test]
fn llvm_backend_lowers_nested_dynamic_map_return_without_artifact_shell() {
    let source = r#"
        use map;
        let flags = {};
        for n in [1, 2] {
            flags = map.set(flags, n, n > 1);
        }
        return [flags, map.values(flags)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("ret.arg.map."));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
}

#[test]
fn llvm_backend_lowers_nested_dynamic_pair_list_return_without_artifact_shell() {
    let source = r#"
        use map;
        let weights = {};
        for n in [1, 2] {
            weights = map.set(weights, "k${n}", n + 0.5);
        }
        let pairs = [];
        for pair in weights {
            pairs = pairs.push([pair[0], pair[1]]);
        }
        return [pairs, map.values(weights)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("ret.arg.pair.list."));
    assert!(artifact.module.ir.contains("getelementptr [4096 x ptr], ptr %list"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %list"));
}

#[test]
fn llvm_backend_lowers_nested_dynamic_i64_f64_pair_list_return_without_artifact_shell() {
    let source = r#"
        use map;
        let weights = {};
        for n in [1, 2] {
            weights = map.set(weights, n, n + 0.5);
        }
        let pairs = [];
        for pair in weights {
            pairs = pairs.push([pair[0], pair[1]]);
        }
        return [pairs, map.values(weights)];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("ret.arg.pair.list."));
    assert!(artifact.module.ir.contains("getelementptr [4096 x i64], ptr %list"));
    assert!(artifact.module.ir.contains("getelementptr [4096 x double], ptr %list"));
}

#[test]
fn llvm_backend_lowers_dynamic_f64_list_module_mutators_without_artifact_shell() {
    let source = r#"
        use list;
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
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("call void @lkrt_list_f64_push"));
    assert!(artifact.module.ir.contains("call void @lkrt_list_f64_slice_range"));
    assert!(artifact.module.ir.contains("call void @lkrt_list_f64_insert"));
    assert!(artifact.module.ir.contains("call double @lkrt_list_f64_remove_at"));
    assert!(artifact.module.ir.contains("call double @lkrt_list_f64_set"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_list_module_mutators_without_artifact_shell() {
    let source = r#"
        use list;
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
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "list"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&module, super::legacy_fallback_options()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("call void @lkrt_list_str_push"));
    assert!(artifact.module.ir.contains("call void @lkrt_list_str_slice_range"));
    assert!(artifact.module.ir.contains("call void @lkrt_list_str_insert"));
    assert!(artifact.module.ir.contains("call ptr @lkrt_list_str_remove_at"));
    assert!(artifact.module.ir.contains("call ptr @lkrt_list_str_set"));
}
