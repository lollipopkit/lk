use super::*;
use crate::vm::analysis::{PerfCellMoveFact, PerformanceFacts};
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
fn execute32_store_cell_clones_source_without_move_fact() {
    let function = Function32 {
        consts: ConstPool32 {
            heap_values: vec![ConstHeapValue32::LongString(Arc::<str>::from("stored-long-string"))],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abx(Opcode32::LoadHeapConst, 1, 0),
            Instr32::abc(Opcode32::StoreCellVal, 0, 1, 0),
            Instr32::abc(Opcode32::Return, 1, 1, 0),
        ],
        register_count: 2,
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
    let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Nil));

    let result =
        execute_module32_with_globals_heap_and_ctx(&module, vec![RuntimeVal::Obj(cell)], heap, &mut VmContext::new())
            .expect("execute cell store clone");

    assert!(matches!(result.returns[0], RuntimeVal::Obj(_)));
    assert!(matches!(
        result.state.heap.get(cell),
        Some(HeapValue::UpvalCell(RuntimeVal::Obj(_)))
    ));
}

#[test]
fn execute32_store_cell_move_fact_consumes_source_register() {
    let mut performance = PerformanceFacts::default();
    performance.set_cell_move_fact(2, PerfCellMoveFact { move_value: true });
    let function = Function32 {
        consts: ConstPool32 {
            heap_values: vec![ConstHeapValue32::LongString(Arc::<str>::from("stored-long-string"))],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abx(Opcode32::LoadHeapConst, 1, 0),
            Instr32::abc(Opcode32::StoreCellVal, 0, 1, 0),
            Instr32::abc(Opcode32::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        performance,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![function],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "cell".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Nil));

    let result =
        execute_module32_with_globals_heap_and_ctx(&module, vec![RuntimeVal::Obj(cell)], heap, &mut VmContext::new())
            .expect("execute cell store move");

    assert_eq!(result.returns, vec![RuntimeVal::Nil]);
    assert!(matches!(
        result.state.heap.get(cell),
        Some(HeapValue::UpvalCell(RuntimeVal::Obj(_)))
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
fn execute32_gc_keeps_caught_raise_error_value_alive() {
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
    let mut heap = HeapStore::new();
    let first_garbage = heap.alloc(HeapValue::String("dead-before-raise-1".into()));
    let second_garbage = heap.alloc(HeapValue::String("dead-before-raise-2".into()));
    heap.set_gc_threshold(1);

    let result = Executor32::new(function.register_count)
        .run_module_with_globals_and_heap(&Module32::single(function), Vec::new(), heap)
        .expect("raise handled across gc");
    let RuntimeVal::Obj(handle) = result.returns.first().expect("return") else {
        panic!("handler return should be error object");
    };
    let Some(HeapValue::ErrorVal(error)) = result.state.heap.get(*handle) else {
        panic!("handler return should survive GC as ErrorVal");
    };

    assert_eq!(error.message.as_ref(), "boom");
    let unreused_garbage = if *handle == first_garbage {
        second_garbage
    } else {
        first_garbage
    };
    assert!(result.state.heap.get(unreused_garbage).is_none());
    assert!(!result.state.heap.should_collect());
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

#[test]
fn execute32_callee_return_unwinds_its_try_handlers_before_next_call() {
    let caller = Function32 {
        code: vec![
            Instr32::abx(Opcode32::LoadFunction, 0, 1),
            Instr32::abc(Opcode32::Call, 0, 0, 0),
            Instr32::abx(Opcode32::LoadFunction, 0, 2),
            Instr32::abc(Opcode32::Call, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let returns_inside_try = Function32 {
        code: vec![
            Instr32::as_bx(Opcode32::TryBegin, 0, 2),
            Instr32::abc(Opcode32::LoadNil, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        ..Function32::default()
    };
    let raises_without_handler = Function32 {
        consts: ConstPool32 {
            strings: vec!["boom".into()],
            ..ConstPool32::default()
        },
        code: vec![Instr32::abx(Opcode32::Raise, 0, 0)],
        register_count: 1,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![caller, returns_inside_try, raises_without_handler],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let err = execute_module32(&module).expect_err("stale callee handler must not catch later raise");

    assert!(err.to_string().contains("boom"));
}
