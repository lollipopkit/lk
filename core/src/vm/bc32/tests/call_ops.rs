use super::*;

fn native_named_answer(
    _args: NativeArgs<'_>,
    named: &[(String, Val)],
    _ctx: &mut crate::vm::VmContext,
) -> anyhow::Result<Val> {
    named
        .iter()
        .find(|(name, _)| name == "answer")
        .map(|(_, value)| value.clone())
        .ok_or_else(|| anyhow::anyhow!("missing answer"))
}

fn const_return_function(value: Val) -> Arc<Function> {
    Arc::new(Function {
        consts: vec![value],
        code: vec![Op::LoadK(0, 0), Op::Ret { base: 0, retc: 1 }],
        n_regs: 1,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    })
}

fn zero_capture_proto(func: Arc<Function>) -> ClosureProto {
    ClosureProto {
        self_name: None,
        params: Arc::new(Vec::new()),
        param_types: Arc::new(Vec::new()),
        named_params: Arc::new(Vec::new()),
        default_funcs: Arc::new(Vec::new()),
        func: Some(Arc::clone(&func)),
        body: Arc::new(Stmt::Block { statements: Vec::new() }),
        captures: Arc::new(Vec::new()),
        capture_names: Arc::<[String]>::from(Vec::new()),
        code: crate::vm::closure_code_cell(Some(&func)),
        empty_env: crate::vm::closure_empty_env(),
        empty_upvalues: crate::vm::closure_empty_upvalues(),
        empty_captures: crate::vm::closure_empty_captures(),
        empty_closure: crate::vm::closure_empty_closure_cell(),
    }
}

#[test]
fn test_bc32_call_native_fast_packed_execution() {
    let out = exec_packed_function(Function {
        consts: vec![Val::RustFastFunction(native_add_one), Val::Int(41)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::CallNativeFast {
                f: 0,
                base: 1,
                argc: 1,
                retc: 1,
            },
            Op::Ret { base: 1, retc: 1 },
        ],
        n_regs: 2,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert_eq!(out, Val::Int(42));
}

#[test]
fn test_bc32_call_closure_exact_packed_execution() {
    let closure_fun = const_return_function(Val::Int(77));
    let out = exec_packed_function(Function {
        consts: vec![],
        code: vec![
            Op::MakeClosure { dst: 0, proto: 0 },
            Op::CallClosureExact {
                f: 0,
                base: 1,
                argc: 0,
                retc: 1,
            },
            Op::Ret { base: 1, retc: 1 },
        ],
        n_regs: 2,
        protos: vec![zero_capture_proto(closure_fun)],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert_eq!(out, Val::Int(77));
}

#[test]
fn test_bc32_call_exact_packed_execution() {
    let out = exec_packed_function(Function {
        consts: vec![Val::RustFastFunction(native_add_one), Val::Int(41)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::CallExact {
                f: 0,
                base: 1,
                argc: 1,
                retc: 1,
            },
            Op::Ret { base: 1, retc: 1 },
        ],
        n_regs: 2,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert_eq!(out, Val::Int(42));
}

#[test]
fn test_bc32_call_named_fallback_packed_execution() {
    let out = exec_packed_function(Function {
        consts: vec![
            Val::RustFastFunctionNamed(native_named_answer),
            Val::from_str("answer"),
            Val::Int(123),
        ],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::LoadK(2, 2),
            Op::CallNamedFallback {
                f: 0,
                base_pos: 3,
                posc: 0,
                base_named: 1,
                namedc: 1,
                retc: 1,
            },
            Op::Ret { base: 3, retc: 1 },
        ],
        n_regs: 4,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert_eq!(out, Val::Int(123));
}
