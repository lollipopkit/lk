use super::*;
#[test]
fn execute32_triggers_heap_gc_from_runtime_roots() {
    let function = Function32 {
        consts: ConstPool32 {
            strings: vec![
                "keep-long-string".into(),
                "drop-long-string".into(),
                "temp-long-string".into(),
            ],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadString, 1, 1),
            Instr32::abc(Opcode32::LoadNil, 1, 0, 0),
            Instr32::abx(Opcode32::LoadString, 1, 2),
            Instr32::abc(Opcode32::Nop, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let mut heap = HeapStore::new();
    heap.set_gc_threshold(3);

    let result = Executor32::new(function.register_count)
        .run_module_with_globals_and_heap(&Module32::single(function), Vec::new(), heap)
        .expect("execute with gc");

    assert_eq!(result.state.heap.len(), 2);
    assert!(result.state.heap.get(HeapRef::new(1)).is_none());
    assert!(!result.state.heap.should_collect());
    assert!(matches!(result.returns.first(), Some(RuntimeVal::Obj(_))));
}

#[test]
fn execute32_loads_and_stores_upval_cell_values() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![41],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abc(Opcode32::LoadCellVal, 1, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 2, 0),
            Instr32::abc(Opcode32::AddInt, 3, 1, 2),
            Instr32::abc(Opcode32::StoreCellVal, 0, 3, 0),
            Instr32::abc(Opcode32::LoadCellVal, 4, 0, 0),
            Instr32::abc(Opcode32::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![function],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "cell".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Int(1)));

    let result =
        execute_module32_with_globals_heap_and_ctx(&module, vec![RuntimeVal::Obj(cell)], heap, &mut VmContext::new())
            .expect("execute cell ops");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        result.state.heap.get(cell),
        Some(HeapValue::UpvalCell(RuntimeVal::Int(42)))
    ));
}

#[test]
fn execute32_load_cell_rejects_non_cell_objects() {
    let function = Function32 {
        consts: ConstPool32 {
            strings: vec!["not-cell".into()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abc(Opcode32::LoadCellVal, 1, 0, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let err = execute32(&function).expect_err("string is not a cell");

    assert!(err.to_string().contains("LoadCellVal expected UpvalCell"));
}

#[test]
fn execute32_raise_jumps_to_try_handler_with_error_value() {
    let function = Function32 {
        consts: ConstPool32 {
            strings: vec!["boom".into()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::as_bx(Opcode32::TryBegin, 0, 2),
            Instr32::abx(Opcode32::Raise, 0, 0),
            Instr32::abc(Opcode32::LoadNil, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("raise handled");
    let RuntimeVal::Obj(handle) = result.returns.first().expect("return") else {
        panic!("handler return should be error object");
    };
    let Some(HeapValue::ErrorVal(error)) = result.state.heap.get(*handle) else {
        panic!("handler return should be ErrorVal");
    };

    assert_eq!(error.message.as_ref(), "boom");
}

#[test]
fn execute32_try_end_removes_raise_handler() {
    let function = Function32 {
        consts: ConstPool32 {
            strings: vec!["boom".into()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::as_bx(Opcode32::TryBegin, 0, 2),
            Instr32::ax(Opcode32::TryEnd, 0),
            Instr32::abx(Opcode32::Raise, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let err = execute32(&function).expect_err("handler removed");

    assert!(err.to_string().contains("boom"));
}

#[test]
fn execute32_caller_handler_catches_raise_from_callee() {
    let caller = Function32 {
        consts: ConstPool32::default(),
        code: vec![
            Instr32::as_bx(Opcode32::TryBegin, 0, 3),
            Instr32::abx(Opcode32::LoadFunction, 0, 1),
            Instr32::abc(Opcode32::Call, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let callee = Function32 {
        consts: ConstPool32 {
            strings: vec!["boom".into()],
            ..ConstPool32::default()
        },
        code: vec![Instr32::abx(Opcode32::Raise, 0, 0)],
        register_count: 1,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![caller, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module32(&module).expect("caller handler catches callee raise");
    let RuntimeVal::Obj(handle) = result.returns.first().expect("return") else {
        panic!("handler return should be error object");
    };
    let Some(HeapValue::ErrorVal(error)) = result.state.heap.get(*handle) else {
        panic!("handler return should be ErrorVal");
    };

    assert_eq!(error.message.as_ref(), "boom");
}
