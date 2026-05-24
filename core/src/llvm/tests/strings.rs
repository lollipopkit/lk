use crate::{
    llvm::{LlvmBackendOptions, compile_module32_artifact_to_llvm, compile_program_to_llvm},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        ConstHeapValue32Data, ConstPool32Data, ConstRuntimeValue32Data, Function32Data, Instr32,
        MODULE32_ARTIFACT_VERSION, Module32Artifact, Module32Data, Opcode32, RuntimeMapKeyData,
    },
};

#[test]
fn llvm_backend_rejects_static_string_not_to_match_exec32() {
    let tokens = Tokenizer::tokenize(r#"return !("ok");"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let err = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect_err("unsupported llvm shape");

    assert!(
        err.to_string().contains("LLVM native lowering does not support"),
        "unexpected error: {err}"
    );
}

#[test]
fn llvm_backend_rejects_static_list_not_to_match_exec32() {
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
    assert!(artifact.module.ir.contains("i64 0"));
}

#[test]
fn llvm_backend_lowers_string_key_map_contains_int_key_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return 1 in {"1": 42};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_string_key_map_index_across_short_and_heap_keys_without_artifact_shell() {
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
                    floats: Vec::new(),
                    strings: vec!["a".to_string()],
                    heap_values: vec![ConstHeapValue32Data::Map(vec![(
                        RuntimeMapKeyData::String("a".to_string()),
                        ConstRuntimeValue32Data::Int(42),
                    )])],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                    Instr32::abc(Opcode32::GetIndex, 2, 0, 1).raw(),
                    Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                ],
                register_count: 3,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_string_key_map_equality_across_short_and_heap_keys_without_artifact_shell() {
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
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![
                        ConstHeapValue32Data::Map(vec![(
                            RuntimeMapKeyData::ShortStr("a".to_string()),
                            ConstRuntimeValue32Data::Int(42),
                        )]),
                        ConstHeapValue32Data::Map(vec![(
                            RuntimeMapKeyData::String("a".to_string()),
                            ConstRuntimeValue32Data::Int(42),
                        )]),
                    ],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadHeapConst, 1, 1).raw(),
                    Instr32::abc(Opcode32::CmpInt, 2, 0, 1).raw(),
                    Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                ],
                register_count: 3,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_mixed_map_set_index_with_exact_string_key_semantics_without_artifact_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![9],
                    floats: Vec::new(),
                    strings: vec!["a".to_string()],
                    heap_values: vec![
                        ConstHeapValue32Data::Map(vec![
                            (
                                RuntimeMapKeyData::String("a".to_string()),
                                ConstRuntimeValue32Data::Int(1),
                            ),
                            (RuntimeMapKeyData::Int(7), ConstRuntimeValue32Data::Int(0)),
                        ]),
                        ConstHeapValue32Data::LongString("a".to_string()),
                    ],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                    Instr32::abx(Opcode32::LoadInt, 2, 0).raw(),
                    Instr32::abc(Opcode32::SetIndex, 0, 1, 2).raw(),
                    Instr32::abx(Opcode32::LoadHeapConst, 3, 1).raw(),
                    Instr32::abc(Opcode32::GetIndex, 4, 0, 3).raw(),
                    Instr32::abc(Opcode32::Return, 4, 1, 0).raw(),
                ],
                register_count: 5,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_const_string_concat_without_artifact_shell() {
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
                    floats: Vec::new(),
                    strings: vec!["hello, ".into(), "native".into()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadString, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 1, 1).raw(),
                    Instr32::abc(Opcode32::ConcatString, 2, 0, 1).raw(),
                    Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                ],
                register_count: 3,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_concat_str_0"));
    assert!(artifact.module.ir.contains("c\"hello, native\\00\""));
}

#[test]
fn llvm_backend_lowers_static_tostring_concat_without_artifact_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![42],
                    floats: Vec::new(),
                    strings: vec!["answer ".into()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadString, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                    Instr32::abc(Opcode32::ToString, 2, 1, 0).raw(),
                    Instr32::abc(Opcode32::ConcatString, 3, 0, 2).raw(),
                    Instr32::abc(Opcode32::Return, 3, 1, 0).raw(),
                ],
                register_count: 4,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_concat_str_1"));
    assert!(artifact.module.ir.contains("c\"answer 42\\00\""));
}

#[test]
fn llvm_backend_rejects_static_float_divisor_zero_tostring_without_artifact_shell() {
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
                    floats: vec![1.0, 0.0],
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadFloat, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadFloat, 1, 1).raw(),
                    Instr32::abc(Opcode32::DivFloat, 2, 0, 1).raw(),
                    Instr32::abc(Opcode32::ToString, 3, 2, 0).raw(),
                    Instr32::abc(Opcode32::Return, 3, 1, 0).raw(),
                ],
                register_count: 4,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let err = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect_err("unsupported");

    assert!(err.to_string().contains("does not support"));
}

#[test]
fn llvm_backend_rejects_static_list_tostring_to_match_exec32() {
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
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![ConstHeapValue32Data::List(vec![ConstRuntimeValue32Data::Int(1)])],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abc(Opcode32::ToString, 1, 0, 0).raw(),
                    Instr32::abc(Opcode32::Return, 1, 1, 0).raw(),
                ],
                register_count: 2,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let err = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default())
        .expect_err("unsupported llvm shape");

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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("c\"answer=42, ratio=1.5, ok=true\\00\""));
}

#[test]
fn llvm_backend_lowers_source_template_string_with_static_numeric_arithmetic_without_shell() {
    let tokens = Tokenizer::tokenize(r#"return "sum=${1 + 2}, ratio=${1.5 + 2.25}";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("c\"sum=3, ratio=3.75\\00\""));
}

#[test]
fn llvm_backend_lowers_source_template_string_with_static_comparisons_without_shell() {
    let tokens = Tokenizer::tokenize(r#"return "lt=${1 < 2}, eq=${1.5 == 1.5}, ne=${"a" != "b"}";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("c\"lt=true, eq=true, ne=true\\00\""));
}
