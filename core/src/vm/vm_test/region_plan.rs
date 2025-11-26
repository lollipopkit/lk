use crate::util::fast_map::fast_hash_map_with_capacity;
use crate::val::Val;
use crate::vm::{
    AllocationRegion, RegionPlan, Vm, VmContext,
    analysis::FunctionAnalysis,
    bc32::{Bc32Decoded, Bc32Function},
    bytecode::{Function, Op},
};
use std::sync::Arc;

fn list_slice_function() -> Function {
    let const_list = Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)].into());
    let start_idx = Val::Int(1);
    let region_plan = RegionPlan {
        values: vec![
            AllocationRegion::Heap,        // r0 holds list constant
            AllocationRegion::Heap,        // r1 holds start index
            AllocationRegion::ThreadLocal, // r2 should reuse TLS buffer
        ],
        return_region: AllocationRegion::Heap,
    };
    Function {
        consts: vec![const_list, start_idx],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::ListSlice {
                dst: 2,
                src: 0,
                start: 1,
            },
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
        analysis: Some(FunctionAnalysis {
            region_plan: Arc::new(region_plan),
            ..FunctionAnalysis::default()
        }),
    }
}

fn map_to_iter_function() -> Function {
    let mut raw = fast_hash_map_with_capacity(2);
    raw.insert(Arc::from("b"), Val::Int(2));
    raw.insert(Arc::from("a"), Val::Int(1));
    let const_map = Val::Map(Arc::new(raw));
    let region_plan = RegionPlan {
        values: vec![
            AllocationRegion::Heap,        // r0 map constant
            AllocationRegion::ThreadLocal, // r1 ToIter result
        ],
        return_region: AllocationRegion::Heap,
    };
    Function {
        consts: vec![const_map],
        code: vec![
            Op::LoadK(0, 0),
            Op::ToIter { dst: 1, src: 0 },
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
        analysis: Some(FunctionAnalysis {
            region_plan: Arc::new(region_plan),
            ..FunctionAnalysis::default()
        }),
    }
}

fn build_list_function() -> Function {
    let region_plan = RegionPlan {
        values: vec![
            AllocationRegion::Heap,        // r0
            AllocationRegion::Heap,        // r1
            AllocationRegion::ThreadLocal, // r2 should use TLS buffer
        ],
        return_region: AllocationRegion::Heap,
    };
    Function {
        consts: vec![Val::Int(1), Val::Int(2)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::BuildList {
                dst: 2,
                base: 0,
                len: 2,
            },
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
        analysis: Some(FunctionAnalysis {
            region_plan: Arc::new(region_plan),
            ..FunctionAnalysis::default()
        }),
    }
}

fn build_map_function() -> Function {
    let region_plan = RegionPlan {
        values: vec![
            AllocationRegion::Heap,        // r0 key
            AllocationRegion::Heap,        // r1 value
            AllocationRegion::ThreadLocal, // r2 result map
        ],
        return_region: AllocationRegion::Heap,
    };
    Function {
        consts: vec![Val::Str(Arc::from("k")), Val::Int(10)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::BuildMap {
                dst: 2,
                base: 0,
                len: 1,
            },
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
        analysis: Some(FunctionAnalysis {
            region_plan: Arc::new(region_plan),
            ..FunctionAnalysis::default()
        }),
    }
}

fn exec_and_expect(fun: &Function, expected: &Val) {
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let out = vm.exec(fun, &mut ctx).expect("vm exec");
    assert_eq!(out, expected.clone());
}

fn pack_bc32(fun: &Function) -> Option<(Vec<u32>, Option<Arc<Bc32Decoded>>)> {
    Bc32Function::try_from_function(fun).map(|packed| (packed.code32, packed.decoded))
}

#[test]
fn list_slice_region_plan_thread_local_executes() {
    let mut fun = list_slice_function();
    let expected = Val::List(vec![Val::Int(2), Val::Int(3)].into());

    exec_and_expect(&fun, &expected);

    if let Some((code32, decoded)) = pack_bc32(&fun) {
        fun.code32 = Some(code32);
        fun.bc32_decoded = decoded;
        exec_and_expect(&fun, &expected);
    }
}

#[test]
fn build_list_region_plan_thread_local_executes() {
    let mut fun = build_list_function();
    let expected = Val::List(vec![Val::Int(1), Val::Int(2)].into());

    exec_and_expect(&fun, &expected);

    if let Some((code32, decoded)) = pack_bc32(&fun) {
        fun.code32 = Some(code32);
        fun.bc32_decoded = decoded;
        exec_and_expect(&fun, &expected);
    }
}

#[test]
fn build_map_region_plan_thread_local_executes() {
    let mut fun = build_map_function();
    let expected = {
        let mut raw = fast_hash_map_with_capacity(1);
        raw.insert(Arc::from("k"), Val::Int(10));
        Val::Map(Arc::new(raw))
    };

    exec_and_expect(&fun, &expected);

    if let Some((code32, decoded)) = pack_bc32(&fun) {
        fun.code32 = Some(code32);
        fun.bc32_decoded = decoded;
        exec_and_expect(&fun, &expected);
    }
}

#[test]
fn to_iter_region_plan_thread_local_executes() {
    let mut fun = map_to_iter_function();
    let expected_pairs = vec![
        Val::List(vec![Val::Str(Arc::from("a")), Val::Int(1)].into()),
        Val::List(vec![Val::Str(Arc::from("b")), Val::Int(2)].into()),
    ];
    let expected = Val::List(expected_pairs.into());

    exec_and_expect(&fun, &expected);

    if let Some((code32, decoded)) = pack_bc32(&fun) {
        fun.code32 = Some(code32);
        fun.bc32_decoded = decoded;
        exec_and_expect(&fun, &expected);
    }
}
