use super::*;
use crate::{
    stmt::import::ImportStmt,
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        Compiler, ConstHeapValueData, ConstPoolData, ConstRuntimeValueData, FunctionData, Instr,
        MODULE_ARTIFACT_VERSION, ModuleArtifact, ModuleData, Opcode, RuntimeMapKeyData,
    },
};

#[test]
fn llvm_backend_allows_unused_import_metadata_for_native_shape() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: vec![ImportStmt::Module {
            module: "os".to_string(),
        }],
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: Vec::new(),
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![Instr::abc(Opcode::Return, 0, 0, 0).raw()],
                performance: Default::default(),
                register_count: 1,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("i64 7"));
}

#[test]
fn llvm_backend_reports_imported_runtime_globals_as_unsupported_native_shape() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: vec![ImportStmt::Module {
            module: "process".to_string(),
        }],
        module: ModuleData {
            entry: 0,
            globals: vec!["process".to_string()],
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: Vec::new(),
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::GetGlobal, 0, 0).raw(),
                    Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 1,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let err = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default())
        .expect_err("runtime globals must be rejected without native runtime seeding");

    let message = err.to_string();
    assert!(
        message.contains("runtime globals are not native-lowerable yet"),
        "{message}"
    );
    assert!(message.contains("process"), "{message}");
    assert!(!message.contains("lk_rt_run_module_json"), "{message}");
}

#[test]
fn llvm_backend_lowers_os_clock_and_epoch_without_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: vec![ImportStmt::Module {
            module: "os".to_string(),
        }],
        module: ModuleData {
            entry: 0,
            globals: vec!["os".to_string()],
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: Vec::new(),
                    floats: Vec::new(),
                    strings: vec!["clock".to_string(), "epoch".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::GetGlobal, 1, 0).raw(),
                    Instr::abx(Opcode::LoadString, 2, 0).raw(),
                    Instr::abc(Opcode::GetIndex, 3, 1, 2).raw(),
                    Instr::abc(Opcode::Move, 4, 3, 0).raw(),
                    Instr::abc(Opcode::Call, 4, 4, 0).raw(),
                    Instr::abx(Opcode::GetGlobal, 2, 0).raw(),
                    Instr::abx(Opcode::LoadString, 3, 1).raw(),
                    Instr::abc(Opcode::GetIndex, 4, 2, 3).raw(),
                    Instr::abc(Opcode::Move, 5, 4, 0).raw(),
                    Instr::abc(Opcode::Call, 5, 5, 0).raw(),
                    Instr::abc(Opcode::Move, 1, 5, 0).raw(),
                    Instr::abc(Opcode::SubInt, 2, 1, 1).raw(),
                    Instr::abc(Opcode::Return, 2, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 6,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("declare i64 @clock()"));
    assert!(artifact.module.ir.contains("declare i64 @time(ptr)"));
}

#[test]
fn llvm_backend_lowers_os_string_builtin_without_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: vec![ImportStmt::Module {
            module: "os".to_string(),
        }],
        module: ModuleData {
            entry: 0,
            globals: vec!["os".to_string()],
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: Vec::new(),
                    floats: Vec::new(),
                    strings: vec!["hostname".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::GetGlobal, 0, 0).raw(),
                    Instr::abx(Opcode::LoadString, 1, 0).raw(),
                    Instr::abc(Opcode::GetIndex, 2, 0, 1).raw(),
                    Instr::abc(Opcode::Move, 3, 2, 0).raw(),
                    Instr::abc(Opcode::Call, 3, 3, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("lk-host"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_lowers_i64_instr_arithmetic_ops_without_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![20, 6, 3],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                    Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                    Instr::abx(Opcode::LoadInt, 2, 2).raw(),
                    Instr::abc(Opcode::DivInt, 3, 0, 1).raw(),
                    Instr::abc(Opcode::ModInt, 4, 0, 1).raw(),
                    Instr::abc(Opcode::MulInt, 5, 4, 2).raw(),
                    Instr::abc(Opcode::AddInt, 6, 3, 5).raw(),
                    Instr::abc(Opcode::SubInt, 7, 6, 2).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 6"));
}

#[test]
fn llvm_backend_lowers_i64_compare_and_branch_without_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![4, 9, 100, 200],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                    Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                    Instr::abc(Opcode::CmpLtInt, 2, 0, 1).raw(),
                    Instr::abc(Opcode::Test, 2, 1, 2).raw(),
                    Instr::abx(Opcode::LoadInt, 3, 2).raw(),
                    Instr::sj(Opcode::Jmp, 1).raw(),
                    Instr::abx(Opcode::LoadInt, 3, 3).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(artifact.module.ir.contains("icmp slt i64"));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("label %bb4"));
    assert!(artifact.module.ir.contains("label %bb6"));
    assert!(artifact.module.ir.contains("call i32 (ptr, ...) @printf"));
}

#[test]
fn llvm_backend_lowers_source_if_i64_branch_without_shell() {
    let source = r#"
            if (4 < 9) {
                return 100;
            } else {
                return 200;
            }
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(artifact.module.ir.contains("icmp "));
    assert!(artifact.module.ir.contains("br i1 %"));
}

#[test]
fn llvm_backend_lowers_source_if_i64_truthy_branch_without_shell() {
    let source = r#"
            if 0 {
                return 100;
            } else {
                return 200;
            }
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("br label %bb"));
    assert!(artifact.module.ir.contains("i64 100"));
}

#[test]
fn llvm_backend_lowers_source_if_nil_falsy_branch_without_shell() {
    let source = r#"
            if nil {
                return 100;
            } else {
                return 200;
            }
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("br label %bb"));
    assert!(artifact.module.ir.contains("i64 200"));
}

#[test]
fn llvm_backend_lowers_source_if_static_string_truthy_branch_without_shell() {
    let source = r#"
            if "ok" {
                return 100;
            } else {
                return 200;
            }
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("i64 100"));
    assert!(!artifact.module.ir.contains("i64 200"));
}

#[test]
fn llvm_backend_lowers_source_if_static_list_truthy_branch_without_shell() {
    let source = r#"
            if [1, 2, 3] {
                return 100;
            } else {
                return 200;
            }
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("i64 100"));
    assert!(!artifact.module.ir.contains("i64 200"));
}

#[test]
fn llvm_backend_lowers_source_static_nullish_coalescing_without_shell() {
    let source = r#"
            let missing = nil;
            let found = 7;
            return (missing ?? 40) + (found ?? 2);
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("add i64"));
    assert!(artifact.module.ir.contains("br i1 %"));
}

#[test]
fn llvm_backend_lowers_source_static_logical_short_circuit_without_shell() {
    let source = r#"
            let a = false && [1, 2, 3];
            let b = true || [4, 5, 6];
            if a { return 10; }
            if b { return 20; }
            return 30;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("br i1 %"));
    assert!(artifact.module.ir.contains("i64 20"));
}

#[test]
fn llvm_backend_lowers_source_while_i64_loop_without_shell() {
    let source = r#"
            let i = 0;
            let sum = 0;
            while (i < 4) {
                sum = sum + i;
                i = i + 1;
            }
            return sum;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("icmp "));
    assert!(!artifact.module.ir.contains("%g0.slot = alloca"));
    assert!(artifact.module.ir.contains("%r0.slot = alloca i64"));
    assert!(artifact.module.ir.contains("br label %bb"));
}

#[test]
fn llvm_backend_lowers_source_not_and_is_nil_without_shell() {
    let source = r#"
            let missing = nil;
            let ok = !(1 < 2);
            if (missing == nil) {
                return ok;
            }
            return true;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("icmp eq i64"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
}

#[test]
fn llvm_backend_lowers_nil_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return nil;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_nil_text"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_lowers_static_function_return_display_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 1; }\nreturn f;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<fn #1(0 captures)>\\00\""));
}

#[test]
fn llvm_backend_lowers_static_builtin_return_display_without_artifact_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["print".to_string()],
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: Vec::new(),
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::GetGlobal, 0, 0).raw(),
                    Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 1,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<native fn print(...)>\\00\""));
}

#[test]
fn llvm_backend_lowers_static_module_builtin_return_display_without_artifact_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["math".to_string()],
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: Vec::new(),
                    floats: Vec::new(),
                    strings: vec!["abs".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::GetGlobal, 0, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<native fn abs(1 args)>\\00\""));
}

#[test]
fn llvm_backend_lowers_simple_const_list_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [1, true, \"longer-than-short\"];").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_heap_list_0"));
    assert!(artifact.module.ir.contains("c\"[1, true, longer-than-short]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_const_list_is_list_without_artifact_shell() {
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
                    Instr::abc(Opcode::IsList, 1, 0, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_string_is_list_like_vm_without_artifact_shell() {
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
                    strings: vec!["ab".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadString, 0, 0).raw(),
                    Instr::abc(Opcode::IsList, 1, 0, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_const_list_len_without_artifact_shell() {
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
                    heap_values: vec![ConstHeapValueData::List(vec![
                        ConstRuntimeValueData::Int(1),
                        ConstRuntimeValueData::Int(2),
                    ])],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abc(Opcode::Len, 1, 0, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_list_get_index_without_artifact_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![1],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![ConstHeapValueData::List(vec![
                        ConstRuntimeValueData::Int(10),
                        ConstRuntimeValueData::Int(20),
                    ])],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abx(Opcode::LoadInt, 1, 0).raw(),
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
    assert!(artifact.module.ir.contains("i64 20"));
}

#[test]
fn llvm_backend_lowers_source_const_list_get_index_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [10, 20].1;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 20"));
}

#[test]
fn llvm_backend_lowers_source_const_list_len_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [1, 2].len();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_map_is_map_without_artifact_shell() {
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
                    heap_values: vec![ConstHeapValueData::Map(vec![(
                        RuntimeMapKeyData::String("a".to_string()),
                        ConstRuntimeValueData::Int(1),
                    )])],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abc(Opcode::IsMap, 1, 0, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_const_map_len_without_artifact_shell() {
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
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abc(Opcode::Len, 1, 0, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_map_get_index_without_artifact_shell() {
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
                    strings: vec!["b".to_string()],
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
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_map_rest_without_artifact_shell() {
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
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abx(Opcode::LoadString, 1, 0).raw(),
                    Instr::abc(Opcode::MapRest, 2, 0, 1).raw(),
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_map_rest_"));
    assert!(artifact.module.ir.contains("c\"{b: 2}\\00\""));
}

#[test]
fn llvm_backend_lowers_static_map_to_iter_without_artifact_shell() {
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
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abc(Opcode::ToIter, 1, 0, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_to_iter_"));
    assert!(artifact.module.ir.contains("c\"[[a, 1], [b, 2]]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_string_to_iter_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "ab";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = crate::vm::Compiler::compile_module(&program).expect("module");
    let mut artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    artifact.module.functions[0]
        .code
        .insert(1, Instr::abc(Opcode::ToIter, 0, 0, 0).raw());

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[a, b]\\00\""));
}

#[test]
fn llvm_backend_lowers_source_const_map_get_index_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return {"a": 1, "b": 2}.b;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_source_const_list_equality_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [[1, 2], [3, 4]] == [[1, 2], [3, 4]];").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_const_map_inequality_without_artifact_shell() {
    let tokens =
        Tokenizer::tokenize(r#"return {"user": {"name": "Alice"}} != {"user": {"name": "Bob"}};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
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

#[test]
fn llvm_backend_lowers_source_const_map_contains_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "b" in {"a": 1, "b": 2};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_string_slice_from_without_artifact_shell() {
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
                    heap_values: vec![ConstHeapValueData::LongString("hello".to_string())],
                },
                code: vec![
                    Instr::abx(Opcode::LoadHeapConst, 0, 0).raw(),
                    Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                    Instr::abc(Opcode::SliceFrom, 2, 0, 1).raw(),
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"llo\\00\""));
}

#[test]
fn llvm_backend_lowers_static_new_list_without_artifact_shell() {
    let artifact = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![1],
                    floats: Vec::new(),
                    strings: vec!["ok".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                    Instr::abx(Opcode::LoadString, 1, 0).raw(),
                    Instr::abc(Opcode::LoadBool, 2, 1, 0).raw(),
                    Instr::abc(Opcode::NewList, 3, 0, 3).raw(),
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[1, ok, true]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_new_range_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 5..=1..0 - 2;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_new_range_"));
    assert!(artifact.module.ir.contains("c\"[5, 3, 1]\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_list_slice_from_without_shell() {
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
                        heap_values: vec![ConstHeapValueData::List(vec![
                            ConstRuntimeValueData::Int(1),
                            ConstRuntimeValueData::Int(2),
                            ConstRuntimeValueData::Int(3),
                        ])],
                    },
                    code: vec![
                        Instr::abx(Opcode::LoadFunction, 0, 1).raw(),
                        Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
                        Instr::abc(Opcode::Call, 0, 0, 1).raw(),
                        Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                    ],
                    performance: Default::default(),
                    register_count: 2,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                FunctionData {
                    consts: ConstPoolData {
                        ints: vec![1],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                        Instr::abc(Opcode::SliceFrom, 2, 0, 1).raw(),
                        Instr::abc(Opcode::Return, 2, 1, 0).raw(),
                    ],
                    performance: Default::default(),
                    register_count: 3,
                    param_count: 1,
                    positional_param_count: 1,
                    param_names: vec!["xs".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[2, 3]\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_new_list_without_shell() {
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
                        ints: vec![1],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr::abx(Opcode::LoadFunction, 0, 1).raw(),
                        Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                        Instr::abc(Opcode::Call, 0, 0, 1).raw(),
                        Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                    ],
                    performance: Default::default(),
                    register_count: 2,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                },
                FunctionData {
                    consts: ConstPoolData {
                        ints: vec![2],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                        Instr::abc(Opcode::NewList, 2, 0, 2).raw(),
                        Instr::abc(Opcode::Return, 2, 1, 0).raw(),
                    ],
                    performance: Default::default(),
                    register_count: 3,
                    param_count: 1,
                    positional_param_count: 1,
                    param_names: vec!["x".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[1, 2]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_try_success_path_without_artifact_shell() {
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
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::as_bx(Opcode::TryBegin, 0, 3).raw(),
                    Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                    Instr::ax(Opcode::TryEnd, 0).raw(),
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

    let artifact = compile_module_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_static_raise_handler_path_without_artifact_shell() {
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
                    strings: vec!["boom".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr::as_bx(Opcode::TryBegin, 0, 1).raw(),
                    Instr::abx(Opcode::Raise, 0, 0).raw(),
                    Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                ],
                performance: Default::default(),
                register_count: 1,
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<value>\\00\""));
}

mod basic;
mod direct_calls;
mod modules;
mod objects;
mod strings;
