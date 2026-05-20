use super::*;

use std::sync::Mutex;

use crate::vm::analysis::{vm_runtime_metrics_reset, vm_runtime_metrics_snapshot};
use crate::vm::bc32::Bc32Function;
use crate::vm::bytecode::{Function, Op, rk_make_const};
use crate::vm::{Vm, VmContext};

static METRICS_LOCK: Mutex<()> = Mutex::new(());

fn binary_function(op: Op) -> Function {
    Function {
        consts: Vec::new(),
        code: vec![op, Op::Ret { base: 2, retc: 1 }],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: vec![0, 1],
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    }
}

fn add_function() -> Function {
    binary_function(Op::Add(2, 0, 1))
}

fn cmp_branch_function(op: Op) -> Function {
    Function {
        consts: vec![Val::Int(1), Val::Int(2)],
        code: vec![
            op,
            Op::JmpFalse(2, 3),
            Op::LoadK(3, 0),
            Op::Ret { base: 3, retc: 1 },
            Op::LoadK(3, 1),
            Op::Ret { base: 3, retc: 1 },
        ],
        n_regs: 4,
        protos: Vec::new(),
        param_regs: vec![0, 1],
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    }
}

fn pack_bc32(mut function: Function) -> Function {
    let packed = Bc32Function::try_from_function(&function).expect("function should be BC32 packable");
    function.code32 = Some(packed.code32);
    function.bc32_decoded = packed.decoded;
    function
}

fn empty_range_loop_function() -> Function {
    Function {
        consts: Vec::new(),
        code: vec![
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
                ofs: 2,
            },
            Op::ForRangeStep {
                idx: 0,
                step: 2,
                back_ofs: -1,
            },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: vec![0, 1],
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    }
}

fn to_str_add_rhs_function() -> Function {
    Function {
        consts: vec![Val::from_str("key")],
        code: vec![
            Op::ToStr(1, 0),
            Op::Add(2, rk_make_const(0), 1),
            Op::Ret { base: 2, retc: 1 },
        ],
        n_regs: 3,
        protos: Vec::new(),
        param_regs: vec![0],
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    }
}

#[test]
fn generic_add_quickens_int_site_and_reuses_it() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    vm_runtime_metrics_reset();
    let function = add_function();
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();

    for _ in 0..6 {
        let out = vm
            .exec_with(&function, &mut ctx, Some(&[Val::Int(20), Val::Int(22)]))
            .expect("execute add");
        assert_eq!(out, Val::Int(42));
    }

    let metrics = vm_runtime_metrics_snapshot();
    assert!(metrics.quickening_build_successes > 0);
    assert!(metrics.quickening_hits > 0);
    assert_eq!(metrics.quickening_deopts, 0);
}

#[test]
fn generic_add_deopts_on_type_change_and_falls_back() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    vm_runtime_metrics_reset();
    let function = add_function();
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();

    for _ in 0..4 {
        let out = vm
            .exec_with(&function, &mut ctx, Some(&[Val::Int(1), Val::Int(2)]))
            .expect("warm add");
        assert_eq!(out, Val::Int(3));
    }

    let out = vm
        .exec_with(&function, &mut ctx, Some(&[Val::from_str("v"), Val::Int(7)]))
        .expect("fallback add");
    assert_eq!(out.as_str(), Some("v7"));

    let metrics = vm_runtime_metrics_snapshot();
    assert!(metrics.quickening_build_successes > 0);
    assert!(metrics.quickening_deopts > 0);
}

#[test]
fn generic_add_quickens_string_concat_sites_without_value_cache() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    for (args, expected) in [
        ([Val::from_str("v"), Val::Int(7)], "v7"),
        ([Val::Int(7), Val::from_str("v")], "7v"),
    ] {
        vm_runtime_metrics_reset();
        let function = add_function();
        let mut vm = Vm::new();
        let mut ctx = VmContext::new();

        for _ in 0..6 {
            let out = vm.exec_with(&function, &mut ctx, Some(&args)).expect("execute concat");
            assert_eq!(out.as_str(), Some(expected));
        }

        let metrics = vm_runtime_metrics_snapshot();
        assert!(metrics.quickening_build_successes > 0);
        assert!(metrics.quickening_hits > 0);
        assert_eq!(metrics.quickening_deopts, 0);
    }
}

#[test]
fn generic_int_arithmetic_quickens_common_binary_sites() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    for (op, args, expected) in [
        (Op::Sub(2, 0, 1), [Val::Int(50), Val::Int(8)], Val::Int(42)),
        (Op::Mul(2, 0, 1), [Val::Int(6), Val::Int(7)], Val::Int(42)),
        (Op::Mod(2, 0, 1), [Val::Int(142), Val::Int(100)], Val::Int(42)),
    ] {
        vm_runtime_metrics_reset();
        let function = binary_function(op);
        let mut vm = Vm::new();
        let mut ctx = VmContext::new();

        for _ in 0..6 {
            let out = vm
                .exec_with(&function, &mut ctx, Some(&args))
                .expect("execute arithmetic");
            assert_eq!(out, expected);
        }

        let metrics = vm_runtime_metrics_snapshot();
        assert!(metrics.quickening_build_successes > 0);
        assert!(metrics.quickening_hits > 0);
        assert_eq!(metrics.quickening_deopts, 0);
    }
}

#[test]
fn generic_numeric_arithmetic_quickens_float_sites() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    for (op, args, expected) in [
        (Op::Add(2, 0, 1), [Val::Float(40.0), Val::Int(2)], Val::Float(42.0)),
        (Op::Sub(2, 0, 1), [Val::Float(50.0), Val::Int(8)], Val::Float(42.0)),
        (Op::Mul(2, 0, 1), [Val::Float(6.0), Val::Int(7)], Val::Float(42.0)),
        (Op::Mod(2, 0, 1), [Val::Float(142.0), Val::Int(100)], Val::Float(42.0)),
    ] {
        vm_runtime_metrics_reset();
        let function = binary_function(op);
        let mut vm = Vm::new();
        let mut ctx = VmContext::new();

        for _ in 0..6 {
            let out = vm
                .exec_with(&function, &mut ctx, Some(&args))
                .expect("execute float arithmetic");
            assert_eq!(out, expected);
        }

        let metrics = vm_runtime_metrics_snapshot();
        assert!(metrics.quickening_build_successes > 0);
        assert!(metrics.quickening_hits > 0);
        assert_eq!(metrics.quickening_deopts, 0);
    }
}

#[test]
fn generic_int_compare_quickens_ordering_sites() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    for (op, args, expected) in [
        (Op::CmpEq(2, 0, 1), [Val::Int(9), Val::Int(9)], Val::Bool(true)),
        (Op::CmpNe(2, 0, 1), [Val::Int(9), Val::Int(4)], Val::Bool(true)),
        (Op::CmpLt(2, 0, 1), [Val::Int(4), Val::Int(9)], Val::Bool(true)),
        (Op::CmpLe(2, 0, 1), [Val::Int(9), Val::Int(9)], Val::Bool(true)),
        (Op::CmpGt(2, 0, 1), [Val::Int(11), Val::Int(9)], Val::Bool(true)),
        (Op::CmpGe(2, 0, 1), [Val::Int(11), Val::Int(11)], Val::Bool(true)),
    ] {
        vm_runtime_metrics_reset();
        let function = binary_function(op);
        let mut vm = Vm::new();
        let mut ctx = VmContext::new();

        for _ in 0..6 {
            let out = vm.exec_with(&function, &mut ctx, Some(&args)).expect("execute compare");
            assert_eq!(out, expected);
        }

        let metrics = vm_runtime_metrics_snapshot();
        assert!(metrics.quickening_build_successes > 0);
        assert!(metrics.quickening_hits > 0);
        assert_eq!(metrics.quickening_deopts, 0);
    }
}

#[test]
fn opcode_compare_branch_fusion_skips_temp_bool_register_write() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    vm_runtime_metrics_reset();
    let function = cmp_branch_function(Op::CmpLt(2, 0, 1));
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();

    let out = vm
        .exec_with(&function, &mut ctx, Some(&[Val::Int(1), Val::Int(2)]))
        .expect("execute fused compare branch");
    assert_eq!(out, Val::Int(1));

    let metrics = vm_runtime_metrics_snapshot();
    assert_eq!(
        metrics.register_writes, 3,
        "fused compare+branch should write the two argument registers and selected return constant, not the temporary bool"
    );
}

#[test]
fn opcode_compare_imm_branch_fusion_skips_temp_bool_register_write() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    vm_runtime_metrics_reset();
    let function = cmp_branch_function(Op::CmpGtImm(2, 0, 900));
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();

    let out = vm
        .exec_with(&function, &mut ctx, Some(&[Val::Int(901), Val::Nil]))
        .expect("execute fused immediate compare branch");
    assert_eq!(out, Val::Int(1));

    let metrics = vm_runtime_metrics_snapshot();
    assert_eq!(
        metrics.register_writes, 3,
        "fused immediate compare+branch should write arguments and selected return constant, not the temporary bool"
    );
}

#[test]
fn packed_compare_imm_branch_records_typed_branch_metric() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    vm_runtime_metrics_reset();
    let function = pack_bc32(cmp_branch_function(Op::CmpGtImm(2, 0, 900)));
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();

    let out = vm
        .exec_with(&function, &mut ctx, Some(&[Val::Int(901), Val::Nil]))
        .expect("execute packed fused immediate compare branch");
    assert_eq!(out, Val::Int(1));

    let metrics = vm_runtime_metrics_snapshot();
    assert!(
        metrics.typed_branch_ops > 0,
        "packed immediate compare+branch should be counted as a typed branch"
    );
}

#[test]
fn opcode_for_range_loop_fuses_adjacent_step() {
    let function = empty_range_loop_function();
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();

    let out = vm
        .exec_with(&function, &mut ctx, Some(&[Val::Int(0), Val::Int(3)]))
        .expect("execute empty range loop");
    assert_eq!(out, Val::Int(3));
}

#[test]
fn opcode_to_str_add_rhs_fusion_skips_temp_string_register_write() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    vm_runtime_metrics_reset();
    let function = to_str_add_rhs_function();
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();

    let out = vm
        .exec_with(&function, &mut ctx, Some(&[Val::Int(42)]))
        .expect("execute fused to_str add");
    assert_eq!(out.as_str(), Some("key42"));

    let metrics = vm_runtime_metrics_snapshot();
    assert_eq!(
        metrics.register_writes, 2,
        "fused ToStr+Add should write the argument register and final result, not the temporary string"
    );
}

#[test]
fn generic_index_quickens_list_and_string_int_sites_without_value_cache() {
    let _guard = METRICS_LOCK.lock().expect("metrics lock");
    for (args, expected) in [
        (
            [
                Val::List(vec![Val::Int(10), Val::Int(42), Val::Int(99)].into()),
                Val::Int(1),
            ],
            Val::Int(42),
        ),
        ([Val::from_str("x42"), Val::Int(1)], Val::from_str("4")),
    ] {
        vm_runtime_metrics_reset();
        let function = binary_function(Op::Index {
            dst: 2,
            base: 0,
            idx: 1,
        });
        let mut vm = Vm::new();
        let mut ctx = VmContext::new();

        for _ in 0..6 {
            let out = vm.exec_with(&function, &mut ctx, Some(&args)).expect("execute index");
            assert_eq!(out, expected);
        }

        let metrics = vm_runtime_metrics_snapshot();
        assert!(metrics.quickening_build_successes > 0);
        assert!(metrics.quickening_hits > 0);
        assert_eq!(metrics.quickening_deopts, 0);
    }
}
