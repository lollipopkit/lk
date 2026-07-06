use super::*;
use crate::{
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        Compiler, ConstHeapValueData, ConstPoolData, ConstRuntimeValueData, FunctionData, Instr,
        MODULE_ARTIFACT_VERSION, ModuleArtifact, ModuleData, Opcode,
    },
};

#[test]
fn llvm_backend_rejects_mir_unsupported_shape_with_precise_reason() {
    // Range values are (still) outside the MIR subset; with the MIR
    // pipeline as the only backend the compile must fail loudly with the
    // lowering's precise `Unsupported` reason. (Nested container literals,
    // the previous specimen here, lower natively since the Dyn work.)
    let tokens = Tokenizer::tokenize("let r = 0..3;\nreturn r;\n").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let err = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect_err("unsupported must fail");
    let message = err.to_string();
    assert!(message.contains("MIR lowering:"), "{message}");
}

#[test]
fn llvm_backend_lowers_static_println_runtime_global_without_shell() {
    let source = r#"
        println(40 + 2);
        return 7;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["println"]).expect("compile module");
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    // `println` now lowers through the MIR pipeline: the argument display goes
    // through the i64-to-string helper and prints via the shared string format.
    assert!(artifact.module.ir.contains("; ModuleID = 'lk_aot'"));
    assert!(artifact.module.ir.contains("@lkrt_i64_to_str"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_lowers_static_const_contains_without_artifact_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![2],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![ConstHeapValueData::List(vec![
                        ConstRuntimeValueData::Int(1),
                        ConstRuntimeValueData::Int(2),
                    ])],
                },
                code: vec![
                    Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                    Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
                    Instr::abc(Opcode::Contains, 2, 0, 1).raw(),
                    Instr::abc(Opcode::Return, 2, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 3,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
                debug_name: None,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_const_contains_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 2 in [1, 2];").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_static_string_contains_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "bc" in "abcd";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

mod basic;
mod direct_calls;
mod strings;
