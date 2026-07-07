use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm, compile_program_to_llvm},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{ConstPoolData, FunctionData, Instr, MODULE_ARTIFACT_VERSION, ModuleArtifact, ModuleData, Opcode},
};

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 42; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_control_flow_call_direct_without_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                FunctionData {
                    consts: ConstPoolData {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr::abc(Opcode::LoadBool, 1, 1, 0).raw(),
                        Instr::abc(Opcode::CallDirect, 0, 1, 1).raw(),
                        Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                    ],
                    performance: Default::default(),
                    register_count: 2,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                    debug_name: None,
                },
                FunctionData {
                    consts: ConstPoolData {
                        ints: Vec::new(),
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr::abc(Opcode::Test, 0, 1, 2).raw(),
                        Instr::abc(Opcode::LoadBool, 1, 0, 0).raw(),
                        Instr::sj(Opcode::Jmp, 1).raw(),
                        Instr::abc(Opcode::LoadBool, 1, 1, 0).raw(),
                        Instr::abc(Opcode::Return, 1, 1, 0).raw(),
                    ],
                    performance: Default::default(),
                    register_count: 2,
                    param_count: 1,
                    positional_param_count: 1,
                    param_names: vec!["value".to_string()],
                    capture_count: 0,
                    debug_name: None,
                },
            ],
        },
    };

    let artifact = compile_module_artifact_to_llvm(
        &artifact,
        LlvmBackendOptions {
            run_optimizations: false,
            ..LlvmBackendOptions::default()
        },
    )
    .expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("br i1"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_i64_arithmetic_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { let x = 40; return x + 2; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_i64_compare_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { let x = 1; return x < 2; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_zero_arg_direct_function_call_f64_compare_without_shell() {
    let tokens = Tokenizer::tokenize("fn f() { let x = 1.5; return x < 2.25; }\nreturn f();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}
