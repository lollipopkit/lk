use crate::{
    llvm::{LlvmBackendOptions, compile_module_artifact_to_llvm},
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{Compiler, ModuleArtifact},
};

#[test]
fn llvm_backend_lowers_assert_runtime_globals_without_artifact_shell() {
    let source = r#"
        assert(1);
        assert_eq(40 + 2, 42);
        assert_ne("left", "right");
        return 7;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["assert", "assert_eq", "assert_ne"])
            .expect("compile module");
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("lk_assert_fail"));
    assert!(artifact.module.ir.contains("i64 7"));
}
