use super::*;
use crate::vm::{FunctionAnalysis, PerfKeyFact, PerfStringIntKeyFact, PerformanceFacts};

#[test]
fn lowers_const_string_map_get_feeding_add_to_single_helper() {
    let func = Function {
        consts: vec![Val::from_str("count"), Val::Int(1)],
        code: vec![
            Op::MapGetInterned(0, 2, 0),
            Op::LoadK(1, 1),
            Op::Add(3, 0, 1),
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
    let artifact = compile_function_to_llvm(&func, "map_get_const_add", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_add_map_get_const_str"),
        "expected const-string map.get feeding add to fuse into helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_get_const_str"),
        "deferred const-string map.get should not materialize intermediate get:\n{}",
        ir
    );
}

#[test]
fn lowers_dynamic_known_string_map_get_feeding_add_to_single_helper() {
    let func = Function {
        consts: vec![Val::from_str("count"), Val::Int(1)],
        code: vec![
            Op::LoadK(4, 0),
            Op::MapGetDynamic(0, 2, 4),
            Op::LoadK(1, 1),
            Op::Add(3, 1, 0),
            Op::Ret { base: 3, retc: 1 },
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
    let artifact =
        compile_function_to_llvm(&func, "map_get_dynamic_const_add", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_add_map_get_const_str"),
        "expected dynamic known-string map.get feeding add to fuse into helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_get_const_str"),
        "deferred dynamic known-string map.get should not materialize intermediate get:\n{}",
        ir
    );
}

#[test]
fn lowers_const_string_map_get_feeding_mul_to_single_helper() {
    let func = Function {
        consts: vec![Val::from_str("tax"), Val::Int(25)],
        code: vec![
            Op::MapGetInterned(0, 2, 0),
            Op::LoadK(1, 1),
            Op::Mul(3, 1, 0),
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
    let artifact = compile_function_to_llvm(&func, "map_get_const_mul", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_mul_map_get_const_str"),
        "expected const-string map.get feeding mul to fuse into helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_get_const_str"),
        "deferred const-string map.get should not materialize intermediate get:\n{}",
        ir
    );
}

#[test]
fn lowers_typed_const_string_map_get_mul_feeding_floor_div_without_floor_helper() {
    let func = Function {
        consts: vec![Val::from_str("tax"), Val::Int(25)],
        code: vec![
            Op::MapGetInterned(0, 2, 0),
            Op::LoadK(1, 1),
            Op::MulInt(3, 1, 0),
            Op::FloorDivImm {
                dst: 4,
                src: 3,
                imm: 100,
            },
            Op::Ret { base: 4, retc: 1 },
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
    let artifact =
        compile_function_to_llvm(&func, "map_get_const_mul_floor_div", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_mul_map_get_const_str"),
        "expected typed const-string map.get feeding mul to fuse into helper:\n{}",
        ir
    );
    assert!(
        ir.contains("sdiv i64") && ir.contains("srem i64"),
        "typed floor-div after fused map.get multiply should lower to LLVM integer division:\n{}",
        ir
    );
    assert!(
        !ir.contains("lk_rt_floor_div_imm"),
        "typed floor-div after fused map.get multiply should not call runtime helper:\n{}",
        ir
    );
}

#[test]
fn lowers_string_int_map_get_feeding_mul_to_single_helper() {
    let func = Function {
        consts: vec![Val::from_str("tax"), Val::Int(7), Val::Int(25)],
        code: vec![
            Op::LoadK(4, 0),
            Op::LoadK(5, 1),
            Op::StrConcatToStr(6, 4, 5),
            Op::MapGetDynamic(0, 2, 6),
            Op::LoadK(1, 2),
            Op::Mul(3, 0, 1),
            Op::Ret { base: 3, retc: 1 },
        ],
        n_regs: 7,
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
    let artifact =
        compile_function_to_llvm(&func, "map_get_str_int_mul", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_mul_map_get_str_int"),
        "expected string-int map.get feeding mul to fuse into helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_get_str_int"),
        "deferred string-int map.get should not materialize intermediate get:\n{}",
        ir
    );
}

#[test]
fn lowers_string_int_key_from_performance_facts() {
    let mut perf = PerformanceFacts::default();
    perf.key_ops.resize_with(4, Option::default);
    perf.key_ops[2] = Some(PerfKeyFact {
        const_key: None,
        string_int: Some(PerfStringIntKeyFact {
            prefix_key: 0,
            suffix_reg: 1,
        }),
    });
    perf.key_ops[3] = perf.key_ops[2];
    let func = Function {
        consts: vec![Val::from_str("tax"), Val::Int(7)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::StrConcatToStr(2, 0, 1),
            Op::MapGetDynamic(3, 4, 2),
            Op::Ret { base: 3, retc: 1 },
        ],
        n_regs: 5,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: Some(FunctionAnalysis {
            perf,
            ..FunctionAnalysis::default()
        }),
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "str_int_key_fact", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_map_get_str_int"),
        "expected PerformanceFacts string-int key to lower dynamic map.get:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_to_string") && !ir.contains("call i64 @lk_rt_add("),
        "facts-backed string-int key should not materialize concat:\n{}",
        ir
    );
}

#[test]
fn lowers_const_string_map_counter_update_to_single_set_helper() {
    let func = Function {
        consts: vec![Val::from_str("count"), Val::Int(1)],
        code: vec![
            Op::MapGetInterned(0, 2, 0),
            Op::LoadK(1, 1),
            Op::Add(3, 0, 1),
            Op::MapSetInterned(2, 0, 3),
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
    let artifact = compile_function_to_llvm(&func, "map_counter_const", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_map_set_add_map_get_const_str"),
        "expected const-string map counter update to fuse into set-add helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_add_map_get_const_str"),
        "counter update should defer add into set helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_set_const_str"),
        "counter update should not use generic const-string set helper:\n{}",
        ir
    );
}

#[test]
fn lowers_dynamic_known_string_map_counter_update_to_single_set_helper() {
    let func = Function {
        consts: vec![Val::from_str("count"), Val::Int(1)],
        code: vec![
            Op::LoadK(4, 0),
            Op::MapGetDynamic(0, 2, 4),
            Op::LoadK(1, 1),
            Op::Add(3, 1, 0),
            Op::MapSet { map: 2, key: 4, val: 3 },
            Op::Ret { base: 2, retc: 1 },
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
    let artifact =
        compile_function_to_llvm(&func, "map_counter_dynamic_const", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_map_set_add_map_get_const_str"),
        "expected dynamic known-string map counter update to fuse into set-add helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_add_map_get_const_str"),
        "counter update should defer add into set helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_set_const_str"),
        "counter update should not use generic const-string set helper:\n{}",
        ir
    );
}

#[test]
fn lowers_string_int_map_counter_update_to_single_set_helper() {
    let func = Function {
        consts: vec![Val::from_str("b"), Val::Int(7), Val::Int(1)],
        code: vec![
            Op::LoadK(4, 0),
            Op::LoadK(5, 1),
            Op::Add(6, 4, 5),
            Op::MapGetDynamic(0, 2, 6),
            Op::LoadK(1, 2),
            Op::Add(3, 0, 1),
            Op::MapSet { map: 2, key: 6, val: 3 },
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 7,
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
    let artifact =
        compile_function_to_llvm(&func, "map_counter_str_int", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_map_set_add_map_get_str_int"),
        "expected string-int map counter update to fuse into set-add helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_add_map_get_str_int"),
        "counter update should defer string-int add into set helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_set_str_int"),
        "counter update should not use generic string-int set helper:\n{}",
        ir
    );
}

#[test]
fn lowers_string_int_nil_init_counter_update_to_single_helper() {
    let nil = crate::vm::rk_make_const(3);
    let func = Function {
        consts: vec![Val::from_str("b"), Val::Int(7), Val::Int(1), Val::Nil],
        code: vec![
            Op::LoadK(4, 0),
            Op::LoadK(5, 1),
            Op::StrConcatToStr(6, 4, 5),
            Op::MapGetDynamic(0, 2, 6),
            Op::CmpEq(7, 0, nil),
            Op::BoolBranch(7, 4),
            Op::LoadK(1, 2),
            Op::MapSet { map: 2, key: 6, val: 1 },
            Op::Jmp(3),
            Op::AddIntImm(3, 0, 1),
            Op::MapSet { map: 2, key: 6, val: 3 },
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 8,
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
    let artifact =
        compile_function_to_llvm(&func, "map_counter_nil_init_str_int", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_map_update_int_str_int"),
        "expected nil-init string-int counter update to fuse into update helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_get_str_int"),
        "fused nil-init update should not materialize map.get helper:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_map_set_str_int"),
        "fused nil-init update should not emit branch-local map.set helper:\n{}",
        ir
    );
}
