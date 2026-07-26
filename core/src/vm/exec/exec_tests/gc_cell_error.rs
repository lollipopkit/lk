use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::vm::analysis::{PerfCellMoveFact, PerformanceFacts};
#[test]
fn execute_triggers_heap_gc_from_runtime_roots() {
    let function = Function {
        consts: ConstPool {
            strings: vec![
                "keep-long-string".into(),
                "drop-long-string".into(),
                "temp-long-string".into(),
            ],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 1),
            Instr::abc(Opcode::LoadNil, 1, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 2),
            Instr::abc(Opcode::Nop, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let mut heap = HeapStore::new();
    heap.set_gc_threshold(3);

    let result = Executor::new(function.register_count)
        .run_module_with_globals_and_heap(&Module::single(function), Vec::new(), heap)
        .expect("execute with gc");

    assert_eq!(result.state.heap.len(), 2);
    assert!(result.state.heap.get(HeapRef::new(1)).is_none());
    assert!(!result.state.heap.should_collect());
    assert!(matches!(result.returns.first(), Some(RuntimeVal::Obj(_))));
}

#[test]
fn execute_loads_and_stores_upval_cell_values() {
    let function = Function {
        consts: ConstPool {
            ints: vec![41],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::GetGlobal, 0, 0),
            Instr::abc(Opcode::LoadCellVal, 1, 0, 0),
            Instr::abx(Opcode::LoadInt, 2, 0),
            Instr::abc(Opcode::AddInt, 3, 1, 2),
            Instr::abc(Opcode::StoreCellVal, 0, 3, 0),
            Instr::abc(Opcode::LoadCellVal, 4, 0, 0),
            Instr::abc(Opcode::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![function],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "cell".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };
    let mut heap = HeapStore::new();
    let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Int(1)));

    let result =
        execute_module_with_globals_heap_and_ctx(&module, vec![RuntimeVal::Obj(cell)], heap, &mut VmContext::new())
            .expect("execute cell ops");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        result.state.heap.get(cell),
        Some(HeapValue::UpvalCell(RuntimeVal::Int(42)))
    ));
}

#[test]
fn execute_store_cell_clones_source_without_move_fact() {
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("stored-long-string"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::GetGlobal, 0, 0),
            Instr::abx(Opcode::LoadHeapConst, 1, 0),
            Instr::abc(Opcode::StoreCellVal, 0, 1, 0),
            Instr::abc(Opcode::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![function],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "cell".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };
    let mut heap = HeapStore::new();
    let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Nil));

    let result =
        execute_module_with_globals_heap_and_ctx(&module, vec![RuntimeVal::Obj(cell)], heap, &mut VmContext::new())
            .expect("execute cell store clone");

    assert!(matches!(result.returns[0], RuntimeVal::Obj(_)));
    assert!(matches!(
        result.state.heap.get(cell),
        Some(HeapValue::UpvalCell(RuntimeVal::Obj(_)))
    ));
}

#[test]
fn execute_store_cell_move_fact_consumes_source_register() {
    let mut performance = PerformanceFacts::default();
    performance.set_cell_move_fact(2, PerfCellMoveFact { move_value: true });
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("stored-long-string"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::GetGlobal, 0, 0),
            Instr::abx(Opcode::LoadHeapConst, 1, 0),
            Instr::abc(Opcode::StoreCellVal, 0, 1, 0),
            Instr::abc(Opcode::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        performance,
        ..Function::default()
    };
    let module = Module {
        functions: vec![function],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "cell".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };
    let mut heap = HeapStore::new();
    let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Nil));

    let result =
        execute_module_with_globals_heap_and_ctx(&module, vec![RuntimeVal::Obj(cell)], heap, &mut VmContext::new())
            .expect("execute cell store move");

    assert_eq!(result.returns, vec![RuntimeVal::Nil]);
    assert!(matches!(
        result.state.heap.get(cell),
        Some(HeapValue::UpvalCell(RuntimeVal::Obj(_)))
    ));
}

#[test]
fn execute_load_cell_rejects_non_cell_objects() {
    let function = Function {
        consts: ConstPool {
            strings: vec!["not-cell".into()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abc(Opcode::LoadCellVal, 1, 0, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let err = execute(&function).expect_err("string is not a cell");

    assert!(err.to_string().contains("LoadCellVal expected UpvalCell"));
}

#[test]
fn execute_raise_jumps_to_try_handler_with_error_value() {
    let function = Function {
        consts: ConstPool {
            strings: vec!["boom".into()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::as_bx(Opcode::TryBegin, 0, 2),
            Instr::abx(Opcode::Raise, 0, 0),
            Instr::abc(Opcode::LoadNil, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("raise handled");
    // A message-only raise binds the message *string*, which is what
    // `typeof(e) == "String"` in `try { 1/0 } catch e` reports today. It used to
    // bind an `ErrorVal` here, disagreeing with the `pcall` path this replaces.
    assert_eq!(
        result.returns.first().expect("return"),
        &RuntimeVal::ShortStr(crate::val::ShortStr::new("boom").expect("short"))
    );
}

/// A raise message that cannot be an inline `ShortStr`, so the value a `catch`
/// binds is a heap object the collector has to keep alive.
const RAISE_MESSAGE_OVER_INLINE: &str = "boom-well-past-seven-bytes";

/// A first-class `error(v)` raise binds **`v` itself**, not its rendering.
///
/// This is the round trip `pcall` has always guaranteed (plan M2.2). The opcode
/// path did not implement it *at all*: every catch site downcast `LanguageRaise`
/// only, so a `LkRaisedValue` crossing a `TryBegin` handler escaped it. Driven
/// directly here because nothing emits these opcodes yet — the end-to-end path
/// arrives with the compiler change.
#[test]
fn caught_first_class_raise_binds_the_raised_value() {
    let function = Function {
        code: vec![
            Instr::as_bx(Opcode::TryBegin, 0, 1),
            Instr::abc(Opcode::LoadNil, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let mut executor = Executor::new(function.register_count);
    executor
        .state
        .stack
        .resize(function.register_count as usize, RuntimeVal::Nil);
    executor.state.stack_top = function.register_count as usize;
    // The handler the `TryBegin` above would install.
    executor.begin_try(0, 1).expect("install handler");

    let raised = crate::vm::LkRaisedValue {
        value: RuntimeVal::Int(7),
        rendered: alloc::sync::Arc::<str>::from("7"),
    };
    executor
        .handle_raised_value(&raised)
        .expect("first-class raise is caught");

    assert_eq!(
        executor.read(0).expect("catch register"),
        &RuntimeVal::Int(7),
        "the catch binds the raised value, not its rendering"
    );
    assert!(
        executor.state.pending_raise_root.is_none(),
        "the GC pin is dropped once a live register holds the value"
    );
}

#[test]
fn execute_gc_keeps_caught_raise_error_value_alive() {
    let function = Function {
        consts: ConstPool {
            // Longer than `ShortStr`'s 7 inline bytes on purpose: the caught
            // value must land on the heap for this test to be about GC at all.
            strings: vec![RAISE_MESSAGE_OVER_INLINE.into()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::as_bx(Opcode::TryBegin, 0, 2),
            Instr::abx(Opcode::Raise, 0, 0),
            Instr::abc(Opcode::LoadNil, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let mut heap = HeapStore::new();
    let first_garbage = heap.alloc(HeapValue::String("dead-before-raise-1".into()));
    let second_garbage = heap.alloc(HeapValue::String("dead-before-raise-2".into()));
    heap.set_gc_threshold(1);

    let result = Executor::new(function.register_count)
        .run_module_with_globals_and_heap(&Module::single(function), Vec::new(), heap)
        .expect("raise handled across gc");
    let RuntimeVal::Obj(handle) = result.returns.first().expect("return") else {
        panic!("handler return should be error object");
    };
    let Some(HeapValue::String(message)) = result.state.heap.get(*handle) else {
        panic!("handler return should survive GC as a heap string");
    };

    assert_eq!(message.as_ref(), RAISE_MESSAGE_OVER_INLINE);
    let unreused_garbage = if *handle == first_garbage {
        second_garbage
    } else {
        first_garbage
    };
    assert!(result.state.heap.get(unreused_garbage).is_none());
    assert!(!result.state.heap.should_collect());
}

#[test]
fn execute_try_end_removes_raise_handler() {
    let function = Function {
        consts: ConstPool {
            strings: vec!["boom".into()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::as_bx(Opcode::TryBegin, 0, 2),
            Instr::ax(Opcode::TryEnd, 0),
            Instr::abx(Opcode::Raise, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let err = execute(&function).expect_err("handler removed");

    assert!(err.to_string().contains("boom"));
}

#[test]
fn execute_caller_handler_catches_raise_from_callee() {
    let caller = Function {
        consts: ConstPool::default(),
        code: vec![
            Instr::as_bx(Opcode::TryBegin, 0, 3),
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abc(Opcode::Call, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let callee = Function {
        consts: ConstPool {
            strings: vec!["boom".into()],
            ..ConstPool::default()
        },
        code: vec![Instr::abx(Opcode::Raise, 0, 0)],
        register_count: 1,
        ..Function::default()
    };
    let module = Module {
        functions: vec![caller, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };

    let result = execute_module(&module).expect("caller handler catches callee raise");
    // Same value contract across a frame boundary as within one frame.
    assert_eq!(
        result.returns.first().expect("return"),
        &RuntimeVal::ShortStr(crate::val::ShortStr::new("boom").expect("short"))
    );
}

#[test]
fn execute_callee_return_unwinds_its_try_handlers_before_next_call() {
    let caller = Function {
        code: vec![
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abc(Opcode::Call, 0, 0, 0),
            Instr::abx(Opcode::LoadFunction, 0, 2),
            Instr::abc(Opcode::Call, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let returns_inside_try = Function {
        code: vec![
            Instr::as_bx(Opcode::TryBegin, 0, 2),
            Instr::abc(Opcode::LoadNil, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        ..Function::default()
    };
    let raises_without_handler = Function {
        consts: ConstPool {
            strings: vec!["boom".into()],
            ..ConstPool::default()
        },
        code: vec![Instr::abx(Opcode::Raise, 0, 0)],
        register_count: 1,
        ..Function::default()
    };
    let module = Module {
        functions: vec![caller, returns_inside_try, raises_without_handler],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };

    let err = execute_module(&module).expect_err("stale callee handler must not catch later raise");

    assert!(err.to_string().contains("boom"));
}
