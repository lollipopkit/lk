use std::sync::Arc;

use super::*;

fn empty_function() -> Function {
    Function {
        consts: Vec::new(),
        code: Vec::new(),
        n_regs: 0,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    }
}

#[test]
fn call_site_plan_matches_arity_and_return_layout() {
    let fun = empty_function();
    let runtime = FunctionRuntimePlan::from_function(&fun, None);
    let plan = CallSitePlan::positional(
        42,
        &fun,
        runtime,
        2,
        CallReturnLayout::new(3, 1),
        None,
        None,
        None,
        FrameInfo::new("test", None::<&str>),
    );

    assert!(plan.matches_layout(2, 3, 1));
    assert!(!plan.matches_layout(1, 3, 1));
    assert!(!plan.matches_layout(2, 4, 1));
    assert!(!plan.matches_layout(2, 3, 0));
    assert_eq!(plan.runtime.func_key, &fun as *const Function as usize);
    assert_eq!(plan.runtime.reg_count, fun.n_regs as usize);
}

#[test]
fn closure_call_ic_clones_shared_call_site_plan() {
    let fun = empty_function();
    let plan = Arc::new(CallSitePlan::positional(
        42,
        &fun,
        FunctionRuntimePlan::from_function(&fun, None),
        2,
        CallReturnLayout::new(3, 1),
        None,
        None,
        None,
        FrameInfo::new("test", None::<&str>),
    ));
    let ic = CallIc::ClosurePositional {
        plan: Arc::clone(&plan),
        cache: ClosureFastCache::new(),
    };

    match ic.clone() {
        CallIc::ClosurePositional { plan: cloned, .. } => assert!(Arc::ptr_eq(&plan, &cloned)),
        _ => panic!("expected positional closure call ic"),
    }
}

#[test]
fn named_call_site_plan_matches_closure_named_layout() {
    let named = Arc::new(NamedCallPlan {
        provided_indices: Arc::from([0usize].as_slice()),
        defaults_to_eval: Arc::from([1usize].as_slice()),
        optional_nil: Arc::from([2usize].as_slice()),
    });
    let plan = NamedCallSitePlan::closure_named(42, 2, CallReturnLayout::new(3, 1), named);

    assert!(plan.matches_closure_layout(42, 2, 3, 1));
    assert!(!plan.matches_closure_layout(41, 2, 3, 1));
    assert!(!plan.matches_closure_layout(42, 1, 3, 1));
    assert!(!plan.matches_closure_layout(42, 2, 4, 1));
    assert!(!plan.matches_closure_layout(42, 2, 3, 0));
}
