use crate::{
    llvm::{LlvmBackendOptions, compile_module32_artifact_to_llvm, compile_program_to_llvm},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        ConstHeapValue32Data, ConstPool32Data, Function32Data, Instr32, MODULE32_ARTIFACT_VERSION, Module32Artifact,
        Module32Data, Opcode32,
    },
};

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 42; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_control_flow_call_direct_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::LoadBool, 1, 1, 0).raw(),
                        Instr32::abc(Opcode32::CallDirect, 0, 1, 1).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 2,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::Test, 0, 1, 2).raw(),
                        Instr32::abc(Opcode32::LoadBool, 1, 0, 0).raw(),
                        Instr32::sj(Opcode32::Jmp, 1).raw(),
                        Instr32::abc(Opcode32::LoadBool, 1, 1, 0).raw(),
                        Instr32::abc(Opcode32::Return, 1, 1, 0).raw(),
                    ],
                    register_count: 2,
                    param_count: 1,
                    positional_param_count: 1,
                    param_names: vec!["value".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(
        &artifact,
        LlvmBackendOptions {
            run_optimizations: false,
            ..LlvmBackendOptions::default()
        },
    )
    .expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("br i1"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_control_flow_direct_emit_text_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: vec!["println".to_string()],
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![7],
                        floats: Vec::new(),
                        strings: vec!["workload".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::LoadBool, 0, 1, 0).raw(),
                        Instr32::abc(Opcode32::Test, 0, 1, 1).raw(),
                        Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 2, 0).raw(),
                        Instr32::abc(Opcode32::CallDirect, 0, 1, 2).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: vec![
                            ConstHeapValue32Data::LongString("name=".to_string()),
                            ConstHeapValue32Data::LongString("|count=".to_string()),
                        ],
                    },
                    code: vec![
                        Instr32::abx(Opcode32::GetGlobal, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadHeapConst, 3, 0).raw(),
                        Instr32::abc(Opcode32::ConcatString, 4, 3, 0).raw(),
                        Instr32::abx(Opcode32::LoadHeapConst, 5, 1).raw(),
                        Instr32::abc(Opcode32::ToString, 6, 1, 0).raw(),
                        Instr32::abc(Opcode32::ConcatString, 7, 4, 5).raw(),
                        Instr32::abc(Opcode32::ConcatString, 8, 7, 6).raw(),
                        Instr32::abc(Opcode32::Move, 3, 8, 0).raw(),
                        Instr32::abc(Opcode32::Call, 2, 2, 1).raw(),
                        Instr32::abc(Opcode32::LoadNil, 0, 0, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 9,
                    param_count: 2,
                    positional_param_count: 2,
                    param_names: vec!["name".to_string(), "count".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_raw_fmt"));
    assert!(artifact.module.ir.contains("@lk_str_raw_fmt"));
    assert!(artifact.module.ir.contains("call i32 (ptr, ...) @printf"));
}

#[test]
fn llvm_backend_lowers_control_flow_static_function_call_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![1, 40, 2, 0],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadInt, 4, 0).raw(),
                        Instr32::abc(Opcode32::Test, 4, 1, 6).raw(),
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 1, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 2, 2).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 2).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 0, 3).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 5,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::AddInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 2,
                    positional_param_count: 2,
                    param_names: vec!["lhs".to_string(), "rhs".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(
        &artifact,
        LlvmBackendOptions {
            run_optimizations: false,
            ..LlvmBackendOptions::default()
        },
    )
    .expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact.module.ir.contains("add i64"),
        "expected static function native add: {}",
        artifact.module.ir
    );
}

#[test]
fn llvm_backend_lowers_control_flow_zero_capture_closure_call_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![1, 40, 2, 0],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadInt, 4, 0).raw(),
                        Instr32::abc(Opcode32::Test, 4, 1, 6).raw(),
                        Instr32::abc(Opcode32::MakeClosure, 0, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 1, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 2, 2).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 2).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 0, 3).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 5,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::AddInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 2,
                    positional_param_count: 2,
                    param_names: vec!["lhs".to_string(), "rhs".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(
        &artifact,
        LlvmBackendOptions {
            run_optimizations: false,
            ..LlvmBackendOptions::default()
        },
    )
    .expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact.module.ir.contains("add i64"),
        "expected zero-capture closure native add: {}",
        artifact.module.ir
    );
}

#[test]
fn llvm_backend_lowers_control_flow_static_capture_closure_call_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![2, 0],
                        floats: Vec::new(),
                        strings: vec!["value".to_string(), "key".to_string()],
                        heap_values: vec![ConstHeapValue32Data::Map(Vec::new())],
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadHeapConst, 5, 0).raw(),
                        Instr32::abx(Opcode32::LoadString, 6, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                        Instr32::abc(Opcode32::SetIndex, 5, 6, 1).raw(),
                        Instr32::abc(Opcode32::GetIndex, 1, 5, 6).raw(),
                        Instr32::abc(Opcode32::Test, 1, 1, 5).raw(),
                        Instr32::abx(Opcode32::LoadString, 3, 0).raw(),
                        Instr32::abc(Opcode32::MakeClosure, 0, 1, 3).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 7,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: vec!["value".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadCapture, 0, 0).raw(),
                        Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                        Instr32::abc(Opcode32::CmpInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 1,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(
        &artifact,
        LlvmBackendOptions {
            run_optimizations: false,
            ..LlvmBackendOptions::default()
        },
    )
    .expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_inline_zero_capture_closure_arg_call_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![40, 2],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::MakeClosure, 1, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 3, 1).raw(),
                        Instr32::abc(Opcode32::CallDirect, 0, 1, 3).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 4,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::Call, 0, 0, 2).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 3,
                    positional_param_count: 3,
                    param_names: vec!["f".to_string(), "lhs".to_string(), "rhs".to_string()],
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::AddInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 2,
                    positional_param_count: 2,
                    param_names: vec!["lhs".to_string(), "rhs".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
}

#[test]
fn llvm_backend_lowers_inline_static_capture_closure_arg_call_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: vec!["value".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadString, 0, 0).raw(),
                        Instr32::abc(Opcode32::MakeClosure, 1, 2, 0).raw(),
                        Instr32::abc(Opcode32::CallDirect, 0, 1, 1).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 2,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::Call, 0, 0, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 1,
                    param_count: 1,
                    positional_param_count: 1,
                    param_names: vec!["f".to_string()],
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: vec!["value".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadCapture, 0, 0).raw(),
                        Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                        Instr32::abc(Opcode32::CmpInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 1,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(
        &artifact,
        LlvmBackendOptions {
            run_optimizations: false,
            ..LlvmBackendOptions::default()
        },
    )
    .expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(
        artifact.module.ir.contains("@lk_bool_true") && artifact.module.ir.contains("select i1"),
        "expected inline captured closure argument native lowering: {}",
        artifact.module.ir
    );
}

#[test]
fn llvm_backend_lowers_inline_static_capture_closure_arg_dynamic_call_named_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![40, 1],
                        floats: Vec::new(),
                        strings: vec!["delta".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadInt, 0, 0).raw(),
                        Instr32::abc(Opcode32::MakeClosure, 1, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadString, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 3, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 4, 1).raw(),
                        Instr32::abc(Opcode32::AddInt, 3, 3, 4).raw(),
                        Instr32::abc(Opcode32::CallDirect, 0, 1, 3).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 5,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::CallNamed, 0, 1 << 7).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 3,
                    positional_param_count: 3,
                    param_names: vec!["f".to_string(), "name".to_string(), "value".to_string()],
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadCapture, 1, 0).raw(),
                        Instr32::abc(Opcode32::AddInt, 2, 1, 0).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 1,
                    positional_param_count: 0,
                    param_names: vec!["delta".to_string()],
                    capture_count: 1,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(
        &artifact,
        LlvmBackendOptions {
            run_optimizations: false,
            ..LlvmBackendOptions::default()
        },
    )
    .expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(
        artifact.module.ir.contains("i64 42"),
        "expected inline captured closure named call native lowering: {}",
        artifact.module.ir
    );
}

#[test]
fn llvm_backend_lowers_control_flow_static_map_set_index_without_shell() {
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
                    strings: vec!["answer".to_string()],
                    heap_values: vec![ConstHeapValue32Data::Map(Vec::new())],
                },
                code: vec![
                    Instr32::abc(Opcode32::LoadBool, 0, 1, 0).raw(),
                    Instr32::abc(Opcode32::Test, 0, 1, 1).raw(),
                    Instr32::abx(Opcode32::LoadHeapConst, 1, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 2, 0).raw(),
                    Instr32::abx(Opcode32::LoadInt, 3, 0).raw(),
                    Instr32::abc(Opcode32::SetIndex, 1, 2, 3).raw(),
                    Instr32::abc(Opcode32::GetIndex, 4, 1, 2).raw(),
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
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_const_list_get_index_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return [10, 20].1; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 20"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_const_contains_without_shell() {
    let tokens = Tokenizer::tokenize(r#"fn f() { return "b" in {"a": 1, "b": 2}; } return f();"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_const_list_equality_without_shell() {
    let tokens = Tokenizer::tokenize("fn same(x) { return x == [1, 2]; } return same([1, 2]);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_new_map_without_shell() {
    let tokens = Tokenizer::tokenize(r#"fn f(v) { return {"answer": v}; } return f(42);"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_func"));
    assert!(artifact.module.ir.contains("_new_map_"));
    assert!(artifact.module.ir.contains("c\"{answer: 42}\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_new_range_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 1..4; } return f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("_new_range_"));
    assert!(artifact.module.ir.contains("c\"[1, 2, 3]\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_map_rest_without_shell() {
    let tokens =
        Tokenizer::tokenize(r#"fn f(data) { let {"a": _, ..rest} = data; return rest; } return f({"a": 1, "b": 2});"#)
            .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("_map_rest_"));
    assert!(artifact.module.ir.contains("c\"{b: 2}\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_to_iter_without_shell() {
    let tokens = Tokenizer::tokenize(r#"fn f(value) { return value; } return f({"a": 1});"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = crate::vm::Compiler32::compile_module(&program).expect("module");
    let mut artifact = crate::vm::Module32Artifact::new(Vec::new(), &module).expect("artifact");
    artifact.module.functions[1]
        .code
        .insert(0, crate::vm::Instr32::abc(crate::vm::Opcode32::ToIter, 0, 0, 0).raw());

    let artifact = crate::llvm::compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default())
        .expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[[a, 1]]\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_new_object_without_shell() {
    let source = r#"
        struct User { name: String, score: Int }
        fn make(score) { return User { name: "Ada", score: score }; }
        return make(42).score;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_set_index_without_shell() {
    let source = r#"
        fn f(value) {
            let values = [1, 2, 3];
            values[1] = value;
            return values.1;
        }
        return f(42);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_i64_arithmetic_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { let x = 40; return x + 2; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_f64_arithmetic_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { let x = 1.5; return x + 2.25; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
    assert!(artifact.module.ir.contains("double 3.75"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_bool_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return true; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("select i1"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_nil_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return nil; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(!artifact.module.ir.contains("ptr @lk_nil_text"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_i64_compare_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { let x = 1; return x < 2; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_f64_compare_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { let x = 1.5; return x < 2.25; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_simple_positional_direct_function_call_without_shell() {
    let tokens = Tokenizer::tokenize("fn f(x) { return x + 1; }\nreturn f(41);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_reading_static_global_without_shell() {
    let tokens = Tokenizer::tokenize("let offset = 2; fn f(x) { return x + offset; } return f(40);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_writing_static_global_without_shell() {
    let tokens =
        Tokenizer::tokenize("counter := 1; fn set_counter() { counter = 2; return counter; } return set_counter();")
            .expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_i64_branch_without_shell() {
    let tokens =
        Tokenizer::tokenize("fn pick(x) { if x < 2 { return 10; } return 20; } return pick(1);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 10"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_i64_truthy_branch_without_shell() {
    let tokens = Tokenizer::tokenize("fn pick(x) { if x { return 10; } return 20; } return pick(0);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 10"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_nil_falsy_branch_without_shell() {
    let tokens =
        Tokenizer::tokenize("fn pick(x) { if x { return 10; } return 20; } return pick(nil);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 20"));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_string_not_to_match_exec32() {
    let tokens = Tokenizer::tokenize(r#"fn no(x) { return !x; } return no("ok");"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_lowers_f64_positional_direct_function_call_without_shell() {
    let tokens = Tokenizer::tokenize("fn f(x) { return x + 2.25; }\nreturn f(1.5);").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_f64_fmt"));
    assert!(artifact.module.ir.contains("double 3.75"));
}

#[test]
fn llvm_backend_lowers_string_positional_direct_function_call_without_shell() {
    let tokens = Tokenizer::tokenize(r#"fn same(x) { return x == "ok"; } return same("ok");"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_direct_function_static_try_success_path_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 1,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![42],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::as_bx(Opcode32::TryBegin, 0, 3).raw(),
                        Instr32::abx(Opcode32::LoadInt, 0, 0).raw(),
                        Instr32::ax(Opcode32::TryEnd, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 1,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_direct_function_static_raise_handler_path_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 1,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: vec!["boom".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::as_bx(Opcode32::TryBegin, 0, 1).raw(),
                        Instr32::abx(Opcode32::Raise, 0, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 1,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<value>\\00\""));
}

#[test]
fn llvm_backend_lowers_static_call_named_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![40, 2],
                        floats: Vec::new(),
                        strings: vec!["delta".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadString, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 3, 1).raw(),
                        Instr32::abx(Opcode32::CallNamed, 0, (1 << 7) | 1).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 4,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::AddInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 2,
                    positional_param_count: 1,
                    param_names: vec!["base".to_string(), "delta".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_control_flow_static_call_named_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![1, 40, 2, 0],
                        floats: Vec::new(),
                        strings: vec!["delta".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadInt, 4, 0).raw(),
                        Instr32::abc(Opcode32::Test, 4, 1, 6).raw(),
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 1, 1).raw(),
                        Instr32::abx(Opcode32::LoadString, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 3, 2).raw(),
                        Instr32::abx(Opcode32::CallNamed, 0, (1 << 7) | 1).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 0, 3).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 5,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::AddInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 2,
                    positional_param_count: 1,
                    param_names: vec!["base".to_string(), "delta".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(
        artifact.module.ir.contains("add i64"),
        "expected control-flow static named call native lowering: {}",
        artifact.module.ir
    );
}

#[test]
fn llvm_backend_lowers_direct_function_call_named_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 0).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 1,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![40, 2],
                        floats: Vec::new(),
                        strings: vec!["delta".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadFunction, 0, 2).raw(),
                        Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadString, 2, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 3, 1).raw(),
                        Instr32::abx(Opcode32::CallNamed, 0, (1 << 7) | 1).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 4,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abc(Opcode32::AddInt, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 2,
                    positional_param_count: 1,
                    param_names: vec!["base".to_string(), "delta".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_direct_closure_call_with_static_capture_without_shell() {
    let source = r#"
        fn make(base) {
            return |value| base + value;
        }

        let add40 = make(40);
        return add40(2);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_direct_closure_call_named_with_static_capture_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                Function32Data {
                    consts: ConstPool32Data {
                        ints: vec![40, 2],
                        floats: Vec::new(),
                        strings: vec!["delta".to_string()],
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                        Instr32::abc(Opcode32::MakeClosure, 0, 1, 1).raw(),
                        Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                        Instr32::abx(Opcode32::LoadInt, 2, 1).raw(),
                        Instr32::abx(Opcode32::CallNamed, 0, 1 << 7).raw(),
                        Instr32::abc(Opcode32::Return, 0, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                Function32Data {
                    consts: ConstPool32Data {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadCapture, 1, 0).raw(),
                        Instr32::abc(Opcode32::AddInt, 2, 1, 0).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 1,
                    positional_param_count: 0,
                    param_names: vec!["delta".to_string()],
                    capture_count: 1,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}
