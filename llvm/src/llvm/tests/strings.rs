use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm, compile_program_to_llvm},
    stmt::import::collect_program_imports,
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        Compiler, ConstHeapValueData, ConstPoolData, ConstRuntimeValueData, FunctionData, Instr,
        MODULE_ARTIFACT_VERSION, ModuleArtifact, ModuleData, Opcode, RuntimeMapKeyData,
    },
};

#[test]
fn llvm_backend_lowers_static_string_not_to_match_exec() {
    let tokens = Tokenizer::tokenize(r#"return !("ok");"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["println"]).expect("compile module");
    let module = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_lowers_optional_template_string_print_without_artifact_shell() {
    let source = r#"
        fn check_range(n, lo, hi, label) {
            if (n < lo) { return "${label} too low (min ${lo})"; }
            if (n > hi) { return "${label} too high (max ${hi})"; }
            return nil;
        }
        println(check_range(-1, 0, 10, "age"));
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["println"]).expect("compile module");
    let module = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("present.slot"));
    assert!(artifact.module.ir.contains("select i1"));
    assert!(artifact.module.ir.contains("@lk_nil_text"));
}

#[test]
fn llvm_backend_rejects_static_list_not_to_match_exec() {
    let tokens = Tokenizer::tokenize("return !([1, 2, 3]);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let err = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect_err("unsupported llvm shape");

    assert!(
        err.to_string().contains("LLVM native lowering does not support"),
        "unexpected error: {err}"
    );
}

#[test]
fn llvm_backend_lowers_non_string_contains_string_to_false_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return 1 in "123";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
    assert!(artifact.module.ir.contains("i64 0"));
}

#[test]
fn llvm_backend_lowers_static_split_join_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "a|b|c".split("|").join("|");"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"a|b|c\\00\""));
}

#[test]
fn llvm_backend_lowers_static_string_list_get_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let xs = ["a", "b"]; return xs.get(1);"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"b\\00\""));
}

#[test]
fn llvm_backend_lowers_source_static_string_get_index_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "abcd".1;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"b\\00\""));
}

#[test]
fn llvm_backend_lowers_static_string_list_take_skip_concat_without_artifact_shell() {
    let source = r#"
        let xs = ["a", "b"];
        let taken = xs.take(1);
        let skipped = xs.skip(1);
        return taken.concat(skipped).concat(["c"]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"[a, b, c]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_list_unique_zip_flatten_chunk_without_artifact_shell() {
    let source = r#"
        let unique = [1, 2, 1].unique();
        let zipped = [1, 2].zip(["a", "b"]);
        let flat = [[1, 2], [3]].flatten();
        let chunked = [1, 2, 3].chunk(2);
        return [unique, zipped, flat, chunked];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(
        artifact
            .module
            .ir
            .contains("c\"[[1, 2], [[1, a], [2, b]], [1, 2, 3], [[1, 2], [3]]]\\00\"")
    );
}

#[test]
fn llvm_backend_lowers_string_key_map_contains_int_key_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return 1 in {"1": 42};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_simple_const_map_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return {"a": 1, "b": true};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_heap_map_0"));
    assert!(artifact.module.ir.contains("c\"{a: 1, b: true}\\00\""));
}

#[test]
fn llvm_backend_lowers_static_new_map_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let value = 42; return {"answer": value};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_new_map_"));
    assert!(artifact.module.ir.contains("c\"{answer: 42}\\00\""));
}

#[test]
fn llvm_backend_lowers_string_key_map_index_across_short_and_heap_keys_without_artifact_shell() {
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
                    floats: Vec::new(),
                    strings: vec!["a".to_string()],
                    heap_values: vec![ConstHeapValueData::Map(vec![(
                        RuntimeMapKeyData::String("a".to_string()),
                        ConstRuntimeValueData::Int(42),
                    )])],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abx(Opcode::LoadString, 1, 0).raw(),
                    Instr::abc(Opcode::GetIndex, 2, 0, 1).raw(),
                    Instr::abc(Opcode::Return, 2, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 3,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_string_key_map_equality_across_short_and_heap_keys_without_artifact_shell() {
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
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![
                        ConstHeapValueData::Map(vec![(
                            RuntimeMapKeyData::ShortStr("a".to_string()),
                            ConstRuntimeValueData::Int(42),
                        )]),
                        ConstHeapValueData::Map(vec![(
                            RuntimeMapKeyData::String("a".to_string()),
                            ConstRuntimeValueData::Int(42),
                        )]),
                    ],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abx(Opcode::LoadHeapConst, 1, 1).raw(),
                    Instr::abc(Opcode::CmpInt, 2, 0, 1).raw(),
                    Instr::abc(Opcode::Return, 2, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 3,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_mixed_map_set_index_with_exact_string_key_semantics_without_artifact_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![9],
                    floats: Vec::new(),
                    strings: vec!["a".to_string()],
                    heap_values: vec![
                        ConstHeapValueData::Map(vec![
                            (
                                RuntimeMapKeyData::String("a".to_string()),
                                ConstRuntimeValueData::Int(1),
                            ),
                            (RuntimeMapKeyData::Int(7), ConstRuntimeValueData::Int(0)),
                        ]),
                        ConstHeapValueData::LongString("a".to_string()),
                    ],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abx(Opcode::LoadString, 1, 0).raw(),
                    Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                    Instr::abc(Opcode::SetIndex, 0, 1, 2).raw(),
                    Instr::abx(Opcode::LoadHeapConst, 3, 1).raw(),
                    Instr::abc(Opcode::GetIndex, 4, 0, 3).raw(),
                    Instr::abc(Opcode::Return, 4, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 5,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_const_string_concat_without_artifact_shell() {
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
                    floats: Vec::new(),
                    strings: vec!["hello, ".into(), "native".into()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadString, 0, 0).raw(),
                    Instr::abx(Opcode::LoadString, 1, 1).raw(),
                    Instr::abc(Opcode::ConcatString, 2, 0, 1).raw(),
                    Instr::abc(Opcode::Return, 2, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 3,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_concat_str_0"));
    assert!(artifact.module.ir.contains("c\"hello, native\\00\""));
}

#[test]
fn llvm_backend_lowers_static_tostring_concat_without_artifact_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![42],
                    floats: Vec::new(),
                    strings: vec!["answer ".into()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadString, 0, 0).raw(),
                    Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                    Instr::abc(Opcode::ToString, 2, 1, 0).raw(),
                    Instr::abc(Opcode::ConcatString, 3, 0, 2).raw(),
                    Instr::abc(Opcode::Return, 3, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 4,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_concat_str_1"));
    assert!(artifact.module.ir.contains("c\"answer 42\\00\""));
}

#[test]
fn llvm_backend_lowers_static_float_divisor_zero_tostring_guard_without_artifact_shell() {
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
                    floats: vec![1.0, 0.0],
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
                    Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
                    Instr::abc(Opcode::DivFloat, 2, 0, 1).raw(),
                    Instr::abc(Opcode::ToString, 3, 2, 0).raw(),
                    Instr::abc(Opcode::Return, 3, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 4,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("fcmp oeq double"));
    assert!(artifact.module.ir.contains("lk_divisor_zero:"));
}

#[test]
fn llvm_backend_rejects_static_list_tostring_to_match_exec() {
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
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![ConstHeapValueData::List(vec![ConstRuntimeValueData::Int(1)])],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abc(Opcode::ToString, 1, 0, 0).raw(),
                    Instr::abc(Opcode::Return, 1, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 2,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let err =
        compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect_err("unsupported llvm shape");

    assert!(
        err.to_string().contains("LLVM native lowering does not support"),
        "unexpected error: {err}"
    );
}

#[test]
fn llvm_backend_lowers_source_template_string_with_static_values_without_shell() {
    let tokens = Tokenizer::tokenize(r#"return "answer=${42}, ratio=${1.5}, ok=${true}";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"answer=42, ratio=1.5, ok=true\\00\""));
}

#[test]
fn llvm_backend_lowers_source_template_string_with_static_numeric_arithmetic_without_shell() {
    let tokens = Tokenizer::tokenize(r#"return "sum=${1 + 2}, ratio=${1.5 + 2.25}";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"sum=3, ratio=3.75\\00\""));
}

#[test]
fn llvm_backend_lowers_source_template_string_with_static_comparisons_without_shell() {
    let tokens = Tokenizer::tokenize(r#"return "lt=${1 < 2}, eq=${1.5 == 1.5}, ne=${"a" != "b"}";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("c\"lt=true, eq=true, ne=true\\00\""));
}

#[test]
fn llvm_backend_lowers_branch_template_string_optional_compare_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        fn check_range(n, lo, hi, label) {
            if (n < lo) { return "${label} too low (min ${lo})"; }
            if (n > hi) { return "${label} too high (max ${hi})"; }
            return nil;
        }
        assert(check_range(-1, 0, 10, "age") == "age too low (min 0)");
        println("ok");
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "println"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("call i32 @strcmp"));
    assert!(
        !artifact
            .module
            .ir
            .contains("store i64 0, ptr %r3.slot\n  br label %bb43")
    );
}

#[test]
fn llvm_backend_lowers_direct_missing_map_get_nil_compare_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        fn get_nested(data, key1, key2) {
            let level1 = data.get(key1);
            if (level1 == nil) { return nil; }
            return level1.get(key2);
        }
        let obj = { "user": { "email": "alice@example.com" } };
        assert(get_nested(obj, "admin", "email") == nil);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains(".present.slot"));
}

#[test]
fn llvm_backend_lowers_template_expression_compare_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let x = 10;
        let y = 20;
        let text = "${x} + ${y} = ${x + y}";
        assert(text == "10 + 20 = 30");
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic"]).expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("call i32 @strcmp"));
}

#[test]
fn llvm_backend_lowers_dynamic_string_int_map_set_call_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let text = "the quick brown fox jumps over the lazy dog the fox was quick";
        let words = text.split(" ");
        let lower_words = words.map(|w| w.lower());
        use map;
        let freq = {};
        for word in lower_words {
            let current = freq.get(word);
            if (current == nil) {
                freq = map.set(freq, word, 1);
            } else {
                freq = map.set(freq, word, current + 1);
            }
        }
        assert(freq.get("the") == 3);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method", "map"])
            .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_set_string_int_map"));
}

#[test]
fn llvm_backend_lowers_static_map_set_call_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        use map;
        let m = { "a": 1, "b": 2 };
        let m2 = map.set(m, "c", 3);
        assert(m2.has("c"));
        assert(m2.len() == 3);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method", "map"])
            .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_string_value_map_set_get_without_shell() {
    let source = r#"
        use map;
        let m = {};
        let m2 = map.set(m, "a", "x");
        return [map.has(m2, "a"), map.get(m2, "a"), map.values(m2), map.delete(m2, "a")];
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method", "map"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("[true, x, [x], [{}, x]]"));
}

#[test]
fn llvm_backend_lowers_string_list_sort_ptr_list_call_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        fn sort_words(xs) {
            let result = [];
            let remaining = xs;
            while (remaining.len() > 0) {
                let min_idx = 0;
                let i = 1;
                while (i < remaining.len()) {
                    if (remaining[i] < remaining[min_idx]) {
                        min_idx = i;
                    }
                    i += 1;
                }
                result.push(remaining[min_idx]);
                remaining = remaining.take(min_idx).concat(remaining.skip(min_idx + 1));
            }
            return result;
        }
        let sorted = sort_words(["world", "hello", "test"]);
        assert(sorted[0] == "hello");
        assert(sorted.len() == 3);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_fn_2_list") || artifact.module.ir.contains("@lk_fn_1_list"));
    assert!(artifact.module.ir.contains("@lk_take_ptr_list"));
    assert!(artifact.module.ir.contains("@lk_concat_ptr_list"));
}

#[test]
fn llvm_backend_lowers_i64_list_direct_call_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        fn merge(a, b) {
            let result = [];
            let i = 0;
            let j = 0;
            while (i < a.len() && j < b.len()) {
                if (a[i] <= b[j]) {
                    result.push(a[i]);
                    i += 1;
                } else {
                    result.push(b[j]);
                    j += 1;
                }
            }
            while (i < a.len()) {
                result.push(a[i]);
                i += 1;
            }
            while (j < b.len()) {
                result.push(b[j]);
                j += 1;
            }
            return result;
        }
        let merged = merge([1, 3, 5], [2, 4, 6]);
        assert(merged == [1, 2, 3, 4, 5, 6]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_fn_2_i64_list") || artifact.module.ir.contains("@lk_fn_1_i64_list"));
    assert!(artifact.module.ir.contains("@lk_slice_i64_list"));
}

#[test]
fn llvm_backend_lowers_i64_list_take_concat_inline_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        fn insert_sorted(sorted, item) {
            let i = 0;
            while (i < sorted.len() && sorted[i] < item) {
                i += 1;
            }
            return sorted.take(i).concat([item]).concat(sorted.skip(i));
        }
        let sorted = insert_sorted([1, 3, 5], 4);
        assert(sorted == [1, 3, 4, 5]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_take_i64_list"));
    assert!(artifact.module.ir.contains("@lk_concat_i64_list"));
}

#[test]
fn llvm_backend_lowers_i64_range_chain_arglist_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let left = 1..=3;
        let right = [10, 11, 12];
        let chained = left.chain(right);
        assert(chained == [1, 2, 3, 10, 11, 12]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_concat_i64_list"));
}

#[test]
fn llvm_backend_lowers_i64_list_map_filter_zip_arglists_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let nums = [1, 2, 3, 4];
        let squares = nums.map(|x| x * x);
        let evens = nums.filter(|x| x % 2 == 0);
        let zipped = [1, 2, 3].zip(["x", "y", "z"]);
        assert(squares == [1, 4, 9, 16]);
        assert(evens == [2, 4]);
        assert(zipped == [[1, "x"], [2, "y"], [3, "z"]]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
        .expect("compile module");
    let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
}

#[test]
fn llvm_backend_lowers_static_list_display_return_methods_without_shell() {
    for (source, expected) in [
        (r#"return ["a", "b"].map(|x| x.upper());"#, "c\"[A, B]\\00\""),
        ("return [1, 2, 3].map(|x| x * 2);", "c\"[2, 4, 6]\\00\""),
        ("return [1, 2, 3, 4].filter(|x| x % 2 == 0);", "c\"[2, 4]\\00\""),
        ("return \"ab\".chars();", "c\"[a, b]\\00\""),
    ] {
        let tokens = Tokenizer::tokenize(source).expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");

        let module =
            Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method"])
                .expect("compile module");
        let module = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
        let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default())
            .unwrap_or_else(|err| panic!("llvm artifact for {source}: {err}"));

        assert!(!artifact.module.ir.contains("@lk_module_json"));
        assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
        assert!(artifact.module.ir.contains(expected), "{source}");
    }
}

#[test]
fn llvm_backend_lowers_iter_flatten_mixed_arglist_without_shell() {
    let source = r#"
        use iter;
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let nested_left = [1, 2];
        let nested_right = [3];
        let flat = iter.flatten([nested_left, nested_right, 4]);
        assert(flat == [1, 2, 3, 4]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic", "__lk_call_method", "iter"])
            .expect("compile module");
    let module = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
}

#[test]
fn llvm_backend_lowers_for_destructured_string_list_compare_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let pairs = [[1, "a"], [2, "b"], [3, "c"]];
        let labels = [];
        for (n, label) in pairs {
            labels.push(label);
        }
        assert(labels == ["a", "b", "c"]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic"]).expect("compile module");
    let module = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("ptr_list_cmp"));
    assert!(artifact.module.ir.contains("call i32 @strcmp"));
}

#[test]
fn llvm_backend_lowers_string_iteration_char_list_compare_without_shell() {
    let source = r#"
        fn assert(cond) { if (!cond) { panic("assertion failed"); } }
        let chars = [];
        for ch in "abc" {
            chars.push(ch);
        }
        assert(chars == ["a", "b", "c"]);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["panic"]).expect("compile module");
    let module = ModuleArtifact::new(collect_program_imports(&program), &module).expect("artifact");
    let artifact = compile_module_artifact_to_llvm(&module, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("ptr_list_cmp"));
    assert!(artifact.module.ir.contains("call i32 @strcmp"));
}
