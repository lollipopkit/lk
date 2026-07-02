use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm, compile_program_to_llvm},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        ConstHeapValueData, ConstPoolData, ConstRuntimeValueData, FunctionData, Instr, MODULE_ARTIFACT_VERSION,
        ModuleArtifact, ModuleData, Opcode,
    },
};

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
    // Float division is divisor-guarded by the lkrt helper (which aborts on a zero
    // divisor) rather than an inline `fcmp`/`lk_divisor_zero` branch.
    assert!(artifact.module.ir.contains("call double @lkrt_f64_div_checked"));
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
    // Constant-folded template strings are long-string literals now lowered
    // by the MIR pipeline as hex-escaped interned globals.
    assert!(artifact.module.ir.contains("; ModuleID = 'lk_aot'"));
    assert!(artifact.module.ir.contains("@lk_str_0"));
}

#[test]
fn llvm_backend_lowers_source_template_string_with_static_numeric_arithmetic_without_shell() {
    let tokens = Tokenizer::tokenize(r#"return "sum=${1 + 2}, ratio=${1.5 + 2.25}";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("; ModuleID = 'lk_aot'"));
    assert!(artifact.module.ir.contains("@lk_str_0"));
}

#[test]
fn llvm_backend_lowers_source_template_string_with_static_comparisons_without_shell() {
    let tokens = Tokenizer::tokenize(r#"return "lt=${1 < 2}, eq=${1.5 == 1.5}, ne=${"a" != "b"}";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("; ModuleID = 'lk_aot'"));
    assert!(artifact.module.ir.contains("@lk_str_0"));
}
