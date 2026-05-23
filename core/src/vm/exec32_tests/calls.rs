use super::*;
#[test]
fn execute_module32_calls_closure_function() {
    let callee = Function32 {
        consts: ConstPool32::default(),
        code: vec![
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 2,
        positional_param_count: 2,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![11, 31],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadFunction, 0, 1),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadInt, 2, 1),
            Instr32::abc(Opcode32::Call, 0, 0, 2),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module32(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_module32_calls_closure_with_captured_value() {
    let callee = Function32 {
        consts: ConstPool32::default(),
        code: vec![
            Instr32::abx(Opcode32::LoadCapture, 1, 0),
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 1,
        positional_param_count: 1,
        param_names: Vec::new(),
        capture_count: 1,
        ..Function32::default()
    };
    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![40, 2],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::MakeClosure, 0, 1, 1),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::Call, 0, 0, 1),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module32(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_module32_reuses_shared_stack_for_repeated_closure_calls() {
    let callee = Function32 {
        consts: ConstPool32 {
            ints: vec![1],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 1,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x")],
        capture_count: 0,
        ..Function32::default()
    };
    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![0, 10, 20],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadFunction, 0, 1),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::Call, 0, 0, 1),
            Instr32::abx(Opcode32::LoadFunction, 0, 1),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::Call, 0, 0, 1),
            Instr32::abx(Opcode32::LoadFunction, 0, 1),
            Instr32::abx(Opcode32::LoadInt, 1, 2),
            Instr32::abc(Opcode32::Call, 0, 0, 1),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module32(&module).expect("execute repeated closure calls");

    assert_eq!(result.returns, vec![RuntimeVal::Int(21)]);
    assert_eq!(result.state.stack_top, 2);
    assert_eq!(result.state.stack.len(), 5);
}

#[test]
fn runtime_value_closure_call_uses_active_shared_stack_window() {
    fn invoke_global_closure(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> anyhow::Result<RuntimeVal> {
        let callee = runtime
            .globals()
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing closure global"))?;
        let Some((state, ctx, module)) = runtime.parts_mut() else {
            return Err(anyhow::anyhow!("test native requires full runtime state"));
        };
        call_runtime_value32_runtime(callee, args.as_slice(), state, module, ctx)
    }

    let callee = Function32 {
        consts: ConstPool32 {
            ints: vec![2],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 1,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x")],
        capture_count: 0,
        ..Function32::default()
    };
    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![40],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadNative, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::Call, 0, 0, 1),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry, callee],
        natives: vec![NativeEntry32 {
            name: "invoke_global_closure".to_string(),
            arity: 1,
            function: NativeFunction32::FullState(invoke_global_closure),
        }],
        globals: vec![GlobalSlot32 { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let closure = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index: 1,
        captures: Vec::new(),
    })));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module32_with_globals_heap_and_ctx(&module, vec![closure], heap, &mut ctx)
        .expect("execute native-mediated closure call");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.stack_top, 4);
    assert_eq!(result.state.stack.len(), 7);
}

