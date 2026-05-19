use super::encoding::{self, BOOL_FALSE_LITERAL};
use super::*;
use crate::expr::Expr;
use crate::stmt::{Program, Stmt};
use crate::val::Val;
use crate::vm::{Function, Op};

mod aot;

#[test]
fn emits_addition_ir() {
    let func = Function {
        consts: vec![Val::Int(40), Val::Int(2)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::Add(2, 0, 1),
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "add", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains(" = add i64 "),
        "expected known-int Add to lower to native integer add:\n{}",
        ir
    );
    assert!(ir.contains("ret i64"), "expected return in IR:\n{}", ir);
}

#[test]
fn emits_known_int_compare_ir() {
    let func = Function {
        consts: vec![Val::Int(40), Val::Int(2)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::CmpGt(2, 0, 1),
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "cmp_int", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains(" = icmp sgt i64 "),
        "expected known-int compare to lower to native icmp:\n{}",
        ir
    );
    assert!(
        !ir.contains("@lk_rt_cmp"),
        "known-int compare should not require runtime compare helper:\n{}",
        ir
    );
}

#[test]
fn emits_branching_ir() {
    let program = Program::new(vec![Box::new(Stmt::If {
        condition: Box::new(Expr::Val(Val::Bool(true))),
        then_stmt: Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Val(Val::Int(1)))),
        }),
        else_stmt: Some(Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Val(Val::Int(0)))),
        })),
    })])
    .expect("program");

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_program_to_llvm(&program, options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(ir.contains("icmp"), "expected condition lowering via icmp:\n{}", ir);
    assert!(ir.contains("br i1"), "expected conditional branch lowering:\n{}", ir);
}

#[test]
fn lowers_short_circuit_and() {
    let func = Function {
        consts: vec![Val::Bool(true), Val::Bool(false)],
        code: vec![
            Op::LoadK(0, 0),
            Op::JmpFalseSet { r: 0, dst: 2, ofs: 3 },
            Op::LoadK(1, 1),
            Op::ToBool(2, 1),
            Op::Ret { base: 2, retc: 1 },
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "and_gate", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("and_false"),
        "expected AND short-circuit label in IR:\n{}",
        ir
    );
    let expected_false = format!("store i64 {}", BOOL_FALSE_LITERAL);
    assert!(ir.contains(&expected_false), "expected false assignment in IR:\n{}", ir);
}

#[test]
fn to_bool_compares_against_sentinels() {
    let func = Function {
        consts: vec![Val::Int(0)],
        code: vec![Op::LoadK(0, 0), Op::ToBool(1, 0), Op::Ret { base: 1, retc: 1 }],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "truthy_int", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;

    assert!(ir.contains(encoding::BOOL_FALSE_LITERAL));
    assert!(ir.contains(encoding::NIL_LITERAL));

    for line in ir.lines().filter(|line| line.contains("icmp eq i64")) {
        assert!(
            line.contains(encoding::BOOL_FALSE_LITERAL) || line.contains(encoding::NIL_LITERAL),
            "unexpected equality comparison found in IR: {line}"
        );
    }
}

#[test]
fn lowers_nullish_pick() {
    let func = Function {
        consts: vec![Val::Int(7), Val::Int(42)],
        code: vec![
            Op::LoadK(0, 0),
            Op::NullishPick { l: 0, dst: 1, ofs: 2 },
            Op::LoadK(1, 1),
            Op::Ret { base: 1, retc: 1 },
            Op::Ret { base: 1, retc: 1 },
        ],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "nullish", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("nullish_taken"),
        "expected nullish taken label in IR:\n{}",
        ir
    );
    assert!(ir.contains("br label"), "expected branching structure in IR:\n{}", ir);
}

#[test]
fn lowers_jmp_if_nil() {
    let func = Function {
        consts: vec![Val::Int(1), Val::Int(2)],
        code: vec![
            Op::JmpIfNil(0, 2),
            Op::LoadK(1, 0),
            Op::Ret { base: 1, retc: 1 },
            Op::LoadK(1, 1),
            Op::Ret { base: 1, retc: 1 },
        ],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "maybe", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(ir.contains("isnil"), "expected nil comparison in IR:\n{}", ir);
    assert!(ir.contains("br i1"), "expected conditional branch in IR:\n{}", ir);
}

#[test]
fn lowers_for_range_loop() {
    let func = Function {
        consts: vec![Val::Int(0), Val::Int(3)],
        code: vec![
            Op::LoadK(0, 0), // idx start
            Op::LoadK(1, 1), // limit
            Op::ForRangePrep {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: false,
                explicit: false,
            },
            Op::RangeLoopI {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: false,
                write_idx: true,
                ofs: 1,
            },
            Op::ForRangeStep {
                idx: 0,
                step: 2,
                back_ofs: -2,
            },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "for_range", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(ir.contains("forprep_step"), "expected step selection in IR:\n{}", ir);
    assert!(
        ir.contains("forguard_cont"),
        "expected guard continuation selection in IR:\n{}",
        ir
    );
    assert!(ir.contains("forstep_next"), "expected loop increment in IR:\n{}", ir);
}

#[test]
fn lowers_specialised_int_ops() {
    let func = Function {
        consts: vec![Val::Int(10), Val::Int(4)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::AddInt(2, 0, 1),
            Op::ModInt(3, 0, 1),
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 4,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "ints", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("srem i64"),
        "expected integer remainder lowering in IR:\n{}",
        ir
    );
    assert!(ir.contains("add i64"), "expected integer add lowering in IR:\n{}", ir);
}

#[test]
fn lowers_float_ops() {
    let func = Function {
        consts: vec![Val::Float(1.5), Val::Float(2.25)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::AddFloat(2, 0, 1),
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "float_add", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_add"),
        "expected float addition to preserve Val semantics through runtime helper:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_float(double 0x3FF8000000000000)"),
        "expected first float constant lowering through runtime helper:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_float(double 0x4002000000000000)"),
        "expected second float constant lowering through runtime helper:\n{}",
        ir
    );
}

#[test]
fn lowers_string_constants() {
    let func = Function {
        consts: vec![Val::Str("hello".into())],
        code: vec![Op::LoadK(0, 0), Op::Ret { base: 0, retc: 1 }],
        n_regs: 1,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "strings", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(ir.contains("@.str"), "expected string global definition in IR:\n{}", ir);
    assert!(
        ir.contains("call i64 @lk_rt_intern_string"),
        "expected string interning helper call in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("getelementptr inbounds"),
        "expected GEP when materialising string literal in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_to_string_helper() {
    let func = Function {
        consts: vec![Val::Int(42)],
        code: vec![Op::LoadK(0, 0), Op::ToStr(1, 0), Op::Ret { base: 1, retc: 1 }],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "tostr", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_to_string"),
        "expected call into runtime to_string helper in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_load_global() {
    let func = Function {
        consts: vec![Val::Str("g".into())],
        code: vec![Op::LoadGlobal(0, 0), Op::Ret { base: 0, retc: 1 }],
        n_regs: 1,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "load_global", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_intern_string"),
        "expected string interning before global load in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_load_global"),
        "expected runtime load_global helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_define_global() {
    let func = Function {
        consts: vec![Val::Str("g".into()), Val::Int(1)],
        code: vec![Op::LoadK(1, 1), Op::DefineGlobal(0, 1), Op::Ret { base: 0, retc: 0 }],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "define_global", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_intern_string"),
        "expected string interning before defining global in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("call void @lk_rt_define_global"),
        "expected runtime define_global helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_build_list() {
    let func = Function {
        consts: vec![],
        code: vec![
            Op::BuildList {
                dst: 0,
                base: 1,
                len: 2,
            },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "build_list", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("alloca [2 x i64]"),
        "expected stack buffer allocation for list elements in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_build_list"),
        "expected runtime build_list helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_call_instruction() {
    let func = Function {
        consts: vec![],
        code: vec![
            Op::Call {
                f: 0,
                base: 1,
                argc: 2,
                retc: 1,
            },
            Op::Ret { base: 1, retc: 1 },
        ],
        n_regs: 4,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "call", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("alloca [2 x i64]"),
        "expected argument buffer allocation for call in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_call"),
        "expected runtime call helper invocation in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_zero_arg_method_call_without_nil_trampoline() {
    let func = Function {
        consts: vec![Val::Str("clock".into()), Val::Str("os".into())],
        code: vec![
            Op::CallGlobalMethod0 {
                dst: 0,
                receiver: 1,
                method: 0,
            },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 1,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "zero_arg_method", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_call_method"),
        "expected zero-argument method call lowering in IR:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_call("),
        "zero-argument method call should not fall back to calling the nil trampoline:\n{}",
        ir
    );
}

#[test]
fn lowers_build_map_and_access() {
    let func = Function {
        consts: vec![Val::Str("key".into()), Val::Int(1)],
        code: vec![
            Op::LoadK(0, 1),
            Op::BuildMap {
                dst: 1,
                base: 2,
                len: 1,
            },
            Op::AccessK(2, 1, 0),
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 4,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "map_access", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_build_map"),
        "expected runtime build_map helper call in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_access"),
        "expected runtime access helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_list_push_and_map_set() {
    let func = Function {
        consts: vec![],
        code: vec![
            Op::ListPush { list: 0, val: 1 },
            Op::MapSet { map: 2, key: 3, val: 4 },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 5,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "mutate_collections", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_list_push"),
        "expected runtime list_push helper call in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_map_set"),
        "expected runtime map_set helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_fused_integer_jump_ops() {
    let func = Function {
        consts: vec![Val::Int(0)],
        code: vec![
            Op::LoadK(0, 0),
            Op::CmpLtImmJmp { r: 0, imm: 5, ofs: 2 },
            Op::AddIntImmJmp { r: 0, imm: 1, ofs: -1 },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 1,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "fused_jumps", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("br i1") && ir.contains("br label"),
        "expected fused jump ops to lower to LLVM branches:\n{}",
        ir
    );
    assert!(
        ir.contains("add i64"),
        "expected AddIntImmJmp to lower to integer add:\n{}",
        ir
    );
}

#[test]
fn fused_add_jump_to_for_range_guard_advances_index() {
    let func = Function {
        consts: vec![Val::Int(1), Val::Int(3), Val::Int(0)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::ForRangePrep {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: true,
                explicit: false,
            },
            Op::RangeLoopI {
                idx: 0,
                limit: 1,
                step: 2,
                inclusive: true,
                write_idx: true,
                ofs: 2,
            },
            Op::AddIntImmJmp { r: 3, imm: 1, ofs: -1 },
            Op::Ret { base: 3, retc: 1 },
        ],
        n_regs: 4,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "fused_for_jump", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("forjmp_next"),
        "expected AddIntImmJmp targeting RangeLoopI to advance the loop index:\n{}",
        ir
    );
}

#[test]
fn lowers_index_and_len() {
    let func = Function {
        consts: vec![Val::Int(3)],
        code: vec![
            Op::Len { dst: 0, src: 1 },
            Op::IndexK(1, 2, 0),
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "index_len", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_len"),
        "expected runtime len helper call in IR:\n{}",
        ir
    );
    assert!(
        ir.contains("call i64 @lk_rt_index"),
        "expected runtime index helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_in_membership() {
    let func = Function {
        consts: vec![],
        code: vec![Op::In(0, 1, 2), Op::Ret { base: 0, retc: 1 }],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "contains", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_in"),
        "expected runtime membership helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_map_has_typed_op() {
    let func = Function {
        consts: vec![Val::from_str("needle")],
        code: vec![Op::MapHasK(0, 1, 0), Op::Ret { base: 0, retc: 1 }],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "map_has", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_map_has"),
        "expected runtime map.has helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_list_slice() {
    let func = Function {
        consts: vec![],
        code: vec![
            Op::ListSlice {
                dst: 0,
                src: 1,
                start: 2,
            },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "list_slice", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_list_slice"),
        "expected runtime list_slice helper call in IR:\n{}",
        ir
    );
}

#[test]
fn lowers_to_iter() {
    let func = Function {
        consts: vec![],
        code: vec![Op::ToIter { dst: 0, src: 1 }, Op::Ret { base: 0, retc: 1 }],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "to_iter", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_to_iter"),
        "expected runtime to_iter helper call in IR:\n{}",
        ir
    );
}
