use super::*;
use crate::{
    stmt::import::ImportStmt,
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    vm::{
        ConstHeapValue32Data, ConstPool32Data, ConstRuntimeValue32Data, Function32Data, Instr32,
        MODULE32_ARTIFACT_VERSION, Module32Artifact, Module32Data, Opcode32, RuntimeMapKeyData,
    },
};

#[test]
fn llvm_backend_reports_imports_as_unsupported_native_shape() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: vec![ImportStmt::Module {
            module: "os".to_string(),
        }],
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: Vec::new(),
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![Instr32::abc(Opcode32::Return, 0, 0, 0).raw()],
                register_count: 1,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };

    let err = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default())
        .expect_err("imports must be rejected without artifact shell fallback");

    let message = err.to_string();
    assert!(message.contains("imports are not native-lowerable yet"), "{message}");
    assert!(message.contains("os"), "{message}");
    assert!(!message.contains("lk_rt_run_module32_json"), "{message}");
}

#[test]
fn llvm_backend_lowers_i64_instr32_arithmetic_ops_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![20, 6, 3],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadInt, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadInt, 1, 1).raw(),
                    Instr32::abx(Opcode32::LoadInt, 2, 2).raw(),
                    Instr32::abc(Opcode32::DivInt, 3, 0, 1).raw(),
                    Instr32::abc(Opcode32::ModInt, 4, 0, 1).raw(),
                    Instr32::abc(Opcode32::MulInt, 5, 4, 2).raw(),
                    Instr32::abc(Opcode32::AddInt, 6, 3, 5).raw(),
                    Instr32::abc(Opcode32::SubInt, 7, 6, 2).raw(),
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
    assert!(artifact.module.ir.contains("i64 6"));
}

#[test]
fn llvm_backend_lowers_i64_compare_and_branch_without_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![4, 9, 100, 200],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadInt, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadInt, 1, 1).raw(),
                    Instr32::abc(Opcode32::CmpLtInt, 2, 0, 1).raw(),
                    Instr32::abc(Opcode32::Test, 2, 1, 2).raw(),
                    Instr32::abx(Opcode32::LoadInt, 3, 2).raw(),
                    Instr32::sj(Opcode32::Jmp, 1).raw(),
                    Instr32::abx(Opcode32::LoadInt, 3, 3).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("icmp "));
    assert!(artifact.module.ir.contains("%g0.slot = alloca i64"));
    assert!(artifact.module.ir.contains("br label %bb"));
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
fn llvm_backend_lowers_f64_instr32_arithmetic_ops_without_shell() {
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(artifact.module.ir.contains("fdiv double"));
    assert!(artifact.module.ir.contains("frem double"));
    assert!(artifact.module.ir.contains("fmul double"));
    assert!(artifact.module.ir.contains("fadd double"));
    assert!(artifact.module.ir.contains("fsub double"));
    assert!(artifact.module.ir.contains("fcmp oeq double"));
    assert!(artifact.module.ir.contains("label %lk_divisor_zero"));
}

#[test]
fn llvm_backend_lowers_source_f64_global_without_shell() {
    let source = r#"
            let x = 1.5;
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

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("icmp eq i64"));
    assert!(artifact.module.ir.contains("@lk_bool_false"));
}

#[test]
fn llvm_backend_lowers_nil_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return nil;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_nil_text"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
}

#[test]
fn llvm_backend_rejects_non_scalar_runtime_returns_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("fn f() { return 1; }\nreturn f;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let err = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect_err("unsupported llvm shape");

    assert!(
        err.to_string().contains("LLVM native lowering does not support"),
        "unexpected error: {err}"
    );
}

#[test]
fn llvm_backend_lowers_simple_const_list_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [1, true, \"longer-than-short\"];").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_heap_list_0"));
    assert!(artifact.module.ir.contains("c\"[1, true, longer-than-short]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_const_list_is_list_without_artifact_shell() {
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
                    Instr32::abc(Opcode32::IsList, 1, 0, 0).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_string_is_list_like_vm_without_artifact_shell() {
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
                    strings: vec!["ab".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadString, 0, 0).raw(),
                    Instr32::abc(Opcode32::IsList, 1, 0, 0).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_const_list_len_without_artifact_shell() {
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
                    heap_values: vec![ConstHeapValue32Data::List(vec![
                        ConstRuntimeValue32Data::Int(1),
                        ConstRuntimeValue32Data::Int(2),
                    ])],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abc(Opcode32::Len, 1, 0, 0).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_list_get_index_without_artifact_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![1],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![ConstHeapValue32Data::List(vec![
                        ConstRuntimeValue32Data::Int(10),
                        ConstRuntimeValue32Data::Int(20),
                    ])],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
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
    assert!(artifact.module.ir.contains("i64 20"));
}

#[test]
fn llvm_backend_lowers_source_const_list_get_index_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [10, 20].1;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 20"));
}

#[test]
fn llvm_backend_lowers_source_const_list_len_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [1, 2].len();").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_map_is_map_without_artifact_shell() {
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
                    heap_values: vec![ConstHeapValue32Data::Map(vec![(
                        RuntimeMapKeyData::String("a".to_string()),
                        ConstRuntimeValue32Data::Int(1),
                    )])],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abc(Opcode32::IsMap, 1, 0, 0).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_const_map_len_without_artifact_shell() {
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
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abc(Opcode32::Len, 1, 0, 0).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_map_get_index_without_artifact_shell() {
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
                    strings: vec!["b".to_string()],
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
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_static_const_map_rest_without_artifact_shell() {
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
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                    Instr32::abc(Opcode32::MapRest, 2, 0, 1).raw(),
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_map_rest_"));
    assert!(artifact.module.ir.contains("c\"{b: 2}\\00\""));
}

#[test]
fn llvm_backend_lowers_static_map_to_iter_without_artifact_shell() {
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
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abc(Opcode32::ToIter, 1, 0, 0).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_to_iter_"));
    assert!(artifact.module.ir.contains("c\"[[a, 1], [b, 2]]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_string_to_iter_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "ab";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = crate::vm::Compiler32::compile_module(&program).expect("module");
    let mut artifact = Module32Artifact::new(Vec::new(), &module).expect("artifact");
    artifact.module.functions[0]
        .code
        .insert(1, Instr32::abc(Opcode32::ToIter, 0, 0, 0).raw());

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[a, b]\\00\""));
}

#[test]
fn llvm_backend_lowers_source_const_map_get_index_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return {"a": 1, "b": 2}.b;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 2"));
}

#[test]
fn llvm_backend_lowers_source_const_list_equality_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return [[1, 2], [3, 4]] == [[1, 2], [3, 4]];").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_const_map_inequality_without_artifact_shell() {
    let tokens =
        Tokenizer::tokenize(r#"return {"user": {"name": "Alice"}} != {"user": {"name": "Bob"}};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_const_contains_without_artifact_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![2],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![ConstHeapValue32Data::List(vec![
                        ConstRuntimeValue32Data::Int(1),
                        ConstRuntimeValue32Data::Int(2),
                    ])],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadInt, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadHeapConst, 1, 0).raw(),
                    Instr32::abc(Opcode32::Contains, 2, 0, 1).raw(),
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
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_const_contains_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 2 in [1, 2];").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_static_string_contains_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "bc" in "abcd";"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_source_const_map_contains_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "b" in {"a": 1, "b": 2};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_bool_true"));
    assert!(artifact.module.ir.contains("i64 1"));
}

#[test]
fn llvm_backend_lowers_static_string_slice_from_without_artifact_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![2],
                    floats: Vec::new(),
                    strings: Vec::new(),
                    heap_values: vec![ConstHeapValue32Data::LongString("hello".to_string())],
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadHeapConst, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                    Instr32::abc(Opcode32::SliceFrom, 2, 0, 1).raw(),
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"llo\\00\""));
}

#[test]
fn llvm_backend_lowers_static_new_list_without_artifact_shell() {
    let artifact = Module32Artifact {
        format: "lk.module32".to_string(),
        version: MODULE32_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: Module32Data {
            entry: 0,
            globals: Vec::new(),
            functions: vec![Function32Data {
                consts: ConstPool32Data {
                    ints: vec![1],
                    floats: Vec::new(),
                    strings: vec!["ok".to_string()],
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::abx(Opcode32::LoadInt, 0, 0).raw(),
                    Instr32::abx(Opcode32::LoadString, 1, 0).raw(),
                    Instr32::abc(Opcode32::LoadBool, 2, 1, 0).raw(),
                    Instr32::abc(Opcode32::NewList, 3, 0, 3).raw(),
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
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[1, ok, true]\\00\""));
}

#[test]
fn llvm_backend_lowers_static_new_range_without_artifact_shell() {
    let tokens = Tokenizer::tokenize("return 5..=1..0 - 2;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_new_range_"));
    assert!(artifact.module.ir.contains("c\"[5, 3, 1]\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_list_slice_from_without_shell() {
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
                        heap_values: vec![ConstHeapValue32Data::List(vec![
                            ConstRuntimeValue32Data::Int(1),
                            ConstRuntimeValue32Data::Int(2),
                            ConstRuntimeValue32Data::Int(3),
                        ])],
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abx(Opcode32::LoadHeapConst, 1, 0).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 1).raw(),
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
                        ints: vec![1],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                        Instr32::abc(Opcode32::SliceFrom, 2, 0, 1).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 1,
                    positional_param_count: 1,
                    param_names: vec!["xs".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[2, 3]\\00\""));
}

#[test]
fn llvm_backend_lowers_direct_function_call_static_new_list_without_shell() {
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
                        ints: vec![1],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadFunction, 0, 1).raw(),
                        Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                        Instr32::abc(Opcode32::Call, 0, 0, 1).raw(),
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
                        ints: vec![2],
                        floats: Vec::new(),
                        strings: Vec::new(),
                        heap_values: Vec::new(),
                    },
                    code: vec![
                        Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                        Instr32::abc(Opcode32::NewList, 2, 0, 2).raw(),
                        Instr32::abc(Opcode32::Return, 2, 1, 0).raw(),
                    ],
                    register_count: 3,
                    param_count: 1,
                    positional_param_count: 1,
                    param_names: vec!["x".to_string()],
                    capture_count: 0,
                },
            ],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"[1, 2]\\00\""));
}

#[test]
fn llvm_backend_lowers_source_static_string_get_index_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return "abcd".1;"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"b\\00\""));
}

#[test]
fn llvm_backend_lowers_simple_const_map_return_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"return {"a": 1, "b": true};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_const_heap_map_0"));
    assert!(artifact.module.ir.contains("c\"{a: 1, b: true}\\00\""));
}

#[test]
fn llvm_backend_lowers_static_new_map_without_artifact_shell() {
    let tokens = Tokenizer::tokenize(r#"let value = 42; return {"answer": value};"#).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");

    let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("@lk_new_map_"));
    assert!(artifact.module.ir.contains("c\"{answer: 42}\\00\""));
}

#[test]
fn llvm_backend_lowers_static_try_success_path_without_artifact_shell() {
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
                    strings: Vec::new(),
                    heap_values: Vec::new(),
                },
                code: vec![
                    Instr32::as_bx(Opcode32::TryBegin, 0, 3).raw(),
                    Instr32::abx(Opcode32::LoadInt, 1, 0).raw(),
                    Instr32::ax(Opcode32::TryEnd, 0).raw(),
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

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_i64_fmt"));
    assert!(artifact.module.ir.contains("i64 42"));
}

#[test]
fn llvm_backend_lowers_static_raise_handler_path_without_artifact_shell() {
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
            }],
        },
    };

    let artifact = compile_module32_artifact_to_llvm(&artifact, LlvmBackendOptions::default()).expect("llvm");

    assert!(!artifact.module.ir.contains("@lk_module32_json"));
    assert!(!artifact.module.ir.contains("lk_rt_run_module32_json"));
    assert!(artifact.module.ir.contains("@lk_str_fmt"));
    assert!(artifact.module.ir.contains("c\"<value>\\00\""));
}

mod basic;
mod direct_calls;
mod objects;
mod strings;
