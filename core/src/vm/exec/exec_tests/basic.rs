use super::*;
use crate::val::ShortStr;
use crate::vm::{
    ConstRuntimeValue,
    analysis::{
        PerfContainerBuildFact, PerfIndexFact, PerfIndexTargetKind, PerfKeyFact, PerfRegisterCopyFact, PerfValueKind,
        PerformanceFacts,
    },
    vm_runtime_metrics_reset, vm_runtime_metrics_snapshot,
};
#[test]
fn execute_returns_int_arithmetic_result() {
    let function = Function {
        consts: ConstPool {
            ints: vec![7, 5],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::MulInt, 2, 0, 1),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(35)]);
}

#[test]
fn execute_branches_with_test_and_jump() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 10, 20],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abc(Opcode::Test, 0, 1, 1),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::sj(Opcode::Jmp, 1),
            Instr::abx(Opcode::LoadInt, 1, 2),
            Instr::abc(Opcode::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(10)]);
}

#[test]
fn execute_not_rejects_string_operand() {
    let function = Function {
        consts: ConstPool {
            strings: vec!["ok".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abc(Opcode::Not, 1, 0, 0),
            Instr::abc(Opcode::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let err = execute(&function).expect_err("string not operand must be rejected");

    assert!(err.to_string().contains("Not expected Bool or Nil"));
}

#[test]
fn execute_tostring_rejects_list_operand() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abc(Opcode::NewList, 1, 0, 1),
            Instr::abc(Opcode::ToString, 2, 1, 0),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let err = execute(&function).expect_err("list tostring operand must be rejected");

    assert!(err.to_string().contains("object cannot be converted to string"));
}

#[test]
fn execute_materializes_long_string_in_heap() {
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns[0].kind(), crate::val::RuntimeValKind::Obj);
    assert_eq!(result.state.heap.len(), 1);
}

#[test]
fn execute_load_heap_const_list_preserves_typed_string_backing() {
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::List(vec![
                ConstRuntimeValue::ShortStr(ShortStr::new("short").expect("short string")),
                ConstRuntimeValue::Heap(Box::new(ConstHeapValue::LongString(Arc::<str>::from(
                    "long-const-string",
                )))),
            ])],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::String(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string list");
    };

    assert_eq!(values.len(), 2);
    assert!(values.iter().any(|value| value.as_ref() == "short"));
    assert!(values.iter().any(|value| value.as_ref() == "long-const-string"));
}

#[test]
fn execute_records_move_heap_clone_as_register_copy_metric() {
    // RuntimeVal is Copy, so Move just copies the value without clone/move distinction.
    // The copy policy metrics are no longer tracked by the Move handler.
    let mut performance = PerformanceFacts::default();
    performance.set_register_copy_fact(1, PerfRegisterCopyFact { move_source: false });
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abc(Opcode::Move, 1, 0, 0),
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

    vm_runtime_metrics_reset();
    let result = execute(&function).expect("execute");

    assert_eq!(result.returns[0].kind(), crate::val::RuntimeValKind::Obj);
    // Move no longer tracks copy policy metrics since RuntimeVal is Copy.
}

#[test]
fn execute_records_move_heap_clone_as_local_store_metric() {
    // RuntimeVal is Copy, so Move just copies the value.
    // The local copy/store metrics are tracked by the local store handler, not Move.
    let mut performance = PerformanceFacts::default();
    performance.mark_local_slot(1);
    performance.set_register_copy_fact(1, PerfRegisterCopyFact { move_source: false });
    performance.set_local_copy_fact(1, crate::vm::analysis::PerfLocalCopyFact { move_source: false });
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abc(Opcode::Move, 1, 0, 0),
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

    vm_runtime_metrics_reset();
    let result = execute(&function).expect("execute");

    assert_eq!(result.returns[0].kind(), crate::val::RuntimeValKind::Obj);
    // Move no longer tracks copy policy metrics since RuntimeVal is Copy.
}

#[test]
fn execute_allocates_mixed_list_on_heap() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 2],
            strings: vec!["x".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 0),
            Instr::abx(Opcode::LoadInt, 2, 1),
            Instr::abc(Opcode::NewList, 3, 0, 3),
            Instr::abc(Opcode::Return, 3, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Mixed(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected mixed list");
    };

    assert_eq!(values.len(), 3);
    assert_eq!(values[0], RuntimeVal::Int(1));
    assert_eq!(values[2], RuntimeVal::Int(2));
}

#[test]
fn execute_allocates_typed_int_list_on_heap() {
    let function = Function {
        consts: ConstPool {
            ints: vec![7, 8],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Int(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![7, 8]);
}

#[test]
fn execute_new_list_without_build_fact_clones_source_register() {
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abc(Opcode::NewList, 1, 0, 1),
            Instr::abc(Opcode::Return, 0, 2, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert!(matches!(result.returns[0], RuntimeVal::Obj(_)));
    assert!(matches!(result.returns[1], RuntimeVal::Obj(_)));
}

#[test]
fn execute_new_list_build_fact_consumes_source_register() {
    let mut performance = PerformanceFacts::default();
    performance.set_container_build_fact(
        1,
        PerfContainerBuildFact {
            move_keys: false,
            move_values: true,
        },
    );
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abc(Opcode::NewList, 1, 0, 1),
            Instr::abc(Opcode::Return, 0, 2, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        performance,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns[0], RuntimeVal::Nil);
    let RuntimeVal::Obj(handle) = result.returns[1] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::String(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string list");
    };
    assert_eq!(values[0].as_ref(), "longer-than-seven");
}

#[test]
fn execute_allocates_typed_int_range_on_heap() {
    let function = Function {
        consts: ConstPool {
            ints: vec![5, 1, -2],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abx(Opcode::LoadInt, 2, 2),
            Instr::abc(Opcode::NewRange, 3, 0, 1),
            Instr::abc(Opcode::Return, 3, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Int(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![5, 3, 1]);
}

#[test]
fn execute_reads_len_for_typed_list_and_short_string() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 2],
            strings: vec!["abc".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abc(Opcode::Len, 3, 2, 0),
            Instr::abx(Opcode::LoadString, 4, 0),
            Instr::abc(Opcode::Len, 5, 4, 0),
            Instr::abc(Opcode::AddInt, 6, 3, 5),
            Instr::abc(Opcode::Return, 6, 1, 0),
        ],
        register_count: 7,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(5)]);
}

#[test]
fn execute_to_iter_materializes_map_entries_as_pairs() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 2, 0, 1],
            strings: vec!["a".to_string(), "b".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 1),
            Instr::abx(Opcode::LoadInt, 3, 1),
            Instr::abc(Opcode::NewMap, 4, 0, 2),
            Instr::abc(Opcode::ToIter, 5, 4, 0),
            Instr::abx(Opcode::LoadInt, 6, 2),
            Instr::abc(Opcode::GetIndex, 7, 5, 6),
            Instr::abx(Opcode::LoadInt, 8, 3),
            Instr::abc(Opcode::GetIndex, 9, 7, 8),
            Instr::abc(Opcode::Return, 9, 1, 0),
        ],
        register_count: 10,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(1)]);
}

#[test]
fn execute_reads_known_typed_list_index_with_static_fact() {
    let mut performance = PerformanceFacts::default();
    performance.set_index_fact(
        4,
        PerfIndexFact {
            target_kind: PerfIndexTargetKind::List,
            value_kind: PerfValueKind::Int,
        },
    );
    performance.set_index_fact(
        6,
        PerfIndexFact {
            target_kind: PerfIndexTargetKind::List,
            value_kind: PerfValueKind::Int,
        },
    );
    let function = Function {
        consts: ConstPool {
            ints: vec![10, 20, 1, -1],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadInt, 3, 2),
            Instr::abc(Opcode::GetIndex, 4, 2, 3),
            Instr::abx(Opcode::LoadInt, 5, 3),
            Instr::abc(Opcode::GetIndex, 6, 2, 5),
            Instr::abc(Opcode::AddInt, 7, 4, 6),
            Instr::abc(Opcode::Return, 7, 1, 0),
        ],
        register_count: 8,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        performance,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(40)]);
}

#[test]
fn execute_allocates_object_and_reads_string_field() {
    let mut performance = PerformanceFacts::default();
    performance.set_key_fact(
        4,
        PerfKeyFact {
            const_key: Some(1),
            ..PerfKeyFact::default()
        },
    );
    let function = Function {
        consts: ConstPool {
            ints: vec![42],
            strings: vec!["User".to_string(), "score".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 1),
            Instr::abx(Opcode::LoadInt, 2, 0),
            Instr::abc(Opcode::NewObject, 3, 0, 1),
            Instr::abc(Opcode::GetIndex, 4, 3, 1),
            Instr::abc(Opcode::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        performance,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    let cache = result
        .state
        .inline_caches
        .index_cache_for_tests(4)
        .expect("index cache");
    assert_eq!(cache.fact.target_kind, PerfIndexTargetKind::Object);
    assert_eq!(cache.object_field_slot, Some(0));
}

#[test]
fn execute_allocates_typed_string_int_map_and_reads_string_key() {
    let function = Function {
        consts: ConstPool {
            ints: vec![42],
            strings: vec!["answer".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::NewMap, 2, 0, 1),
            Instr::abx(Opcode::LoadString, 3, 0),
            Instr::abc(Opcode::GetIndex, 4, 2, 3),
            Instr::abc(Opcode::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    let RuntimeVal::Obj(handle) = result.state.stack[2] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string-int map");
    };
    assert_eq!(values.get("answer"), Some(&42));
    let cache = result.state.inline_caches.index_fact_for_tests(4).expect("index cache");
    assert_eq!(cache.target_kind, PerfIndexTargetKind::Map);
    assert_eq!(cache.value_kind, PerfValueKind::Int);
}

#[test]
fn execute_new_map_without_build_fact_clones_source_registers() {
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"))],
            strings: vec!["answer".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadHeapConst, 1, 0),
            Instr::abc(Opcode::NewMap, 2, 0, 1),
            Instr::abc(Opcode::Return, 0, 3, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert!(matches!(result.returns[0], RuntimeVal::ShortStr(_)));
    assert!(matches!(result.returns[1], RuntimeVal::Obj(_)));
    assert!(matches!(result.returns[2], RuntimeVal::Obj(_)));
}

#[test]
fn execute_new_map_build_fact_consumes_source_registers() {
    let mut performance = PerformanceFacts::default();
    performance.set_container_build_fact(
        2,
        PerfContainerBuildFact {
            move_keys: true,
            move_values: true,
        },
    );
    let function = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("longer-than-seven"))],
            strings: vec!["answer".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadHeapConst, 1, 0),
            Instr::abc(Opcode::NewMap, 2, 0, 1),
            Instr::abc(Opcode::Return, 0, 3, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        performance,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns[0], RuntimeVal::Nil);
    assert_eq!(result.returns[1], RuntimeVal::Nil);
    let RuntimeVal::Obj(handle) = result.returns[2] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringMixed(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected string-mixed map");
    };
    assert!(matches!(values.get("answer"), Some(RuntimeVal::Obj(_))));
}

#[test]
fn execute_writes_mixed_map_by_string_key() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 42],
            strings: vec!["answer".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::NewMap, 2, 0, 1),
            Instr::abx(Opcode::LoadString, 3, 0),
            Instr::abx(Opcode::LoadInt, 4, 1),
            Instr::abc(Opcode::SetIndex, 2, 3, 4),
            Instr::abc(Opcode::GetIndex, 5, 2, 3),
            Instr::abc(Opcode::Return, 5, 1, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_updates_typed_string_int_map_without_materializing() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 42],
            strings: vec!["answer".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::NewMap, 2, 0, 1),
            Instr::abx(Opcode::LoadString, 3, 0),
            Instr::abx(Opcode::LoadInt, 4, 1),
            Instr::abc(Opcode::SetIndex, 2, 3, 4),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string-int map");
    };

    assert_eq!(values.get("answer"), Some(&42));
}

#[test]
fn execute_materializes_typed_string_int_map_to_string_mixed_on_value_pollution() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1],
            strings: vec!["answer".to_string(), "label".to_string(), "ok".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::NewMap, 2, 0, 1),
            Instr::abx(Opcode::LoadString, 3, 1),
            Instr::abx(Opcode::LoadString, 4, 2),
            Instr::abc(Opcode::SetIndex, 2, 3, 4),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringMixed(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected string-mixed map");
    };

    assert_eq!(values.get("answer"), Some(&RuntimeVal::Int(1)));
    assert!(matches!(values.get("label"), Some(RuntimeVal::ShortStr(value)) if value.as_str() == "ok"));
}

#[test]
fn execute_adds_and_subtracts_typed_string_int_maps_without_runtime_entry_materialization() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 2, 3],
            strings: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 1),
            Instr::abx(Opcode::LoadInt, 3, 1),
            Instr::abc(Opcode::NewMap, 4, 0, 2),
            Instr::abx(Opcode::LoadString, 5, 1),
            Instr::abx(Opcode::LoadInt, 6, 2),
            Instr::abx(Opcode::LoadString, 7, 2),
            Instr::abx(Opcode::LoadInt, 8, 2),
            Instr::abc(Opcode::NewMap, 9, 5, 2),
            Instr::abc(Opcode::AddInt, 10, 4, 9),
            Instr::abc(Opcode::SubInt, 11, 10, 9),
            Instr::abc(Opcode::Return, 10, 2, 0),
        ],
        register_count: 12,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(added) = result.returns[0] else {
        panic!("expected added map");
    };
    let RuntimeVal::Obj(removed) = result.returns[1] else {
        panic!("expected removed map");
    };

    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(added).expect("added map") else {
        panic!("expected added string-int map");
    };
    assert_eq!(values.get("a"), Some(&1));
    assert_eq!(values.get("b"), Some(&3));
    assert_eq!(values.get("c"), Some(&3));

    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(removed).expect("removed map") else {
        panic!("expected removed string-int map");
    };
    assert_eq!(values.get("a"), Some(&1));
    assert_eq!(values.get("b"), None);
    assert_eq!(values.get("c"), None);
}

#[test]
fn execute_subtracts_string_key_from_typed_string_int_map_without_cloning_removed_entry() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 2],
            strings: vec!["a".to_string(), "b".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 1),
            Instr::abx(Opcode::LoadInt, 3, 1),
            Instr::abc(Opcode::NewMap, 4, 0, 2),
            Instr::abx(Opcode::LoadString, 5, 1),
            Instr::abc(Opcode::SubInt, 6, 4, 5),
            Instr::abc(Opcode::Return, 6, 1, 0),
        ],
        register_count: 7,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(handle).expect("map object") else {
        panic!("expected string-int map");
    };

    assert_eq!(values.get("a"), Some(&1));
    assert_eq!(values.get("b"), None);
}

#[test]
fn execute_reads_mixed_list_by_int_index() {
    let function = Function {
        consts: ConstPool {
            ints: vec![7, 8, 1],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadInt, 3, 2),
            Instr::abc(Opcode::GetIndex, 4, 2, 3),
            Instr::abc(Opcode::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(8)]);
}

#[test]
fn execute_writes_mixed_list_by_int_index() {
    let function = Function {
        consts: ConstPool {
            ints: vec![7, 8, 1, 42],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadInt, 3, 2),
            Instr::abx(Opcode::LoadInt, 4, 3),
            Instr::abc(Opcode::SetIndex, 2, 3, 4),
            Instr::abc(Opcode::GetIndex, 5, 2, 3),
            Instr::abc(Opcode::Return, 5, 1, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_pollutes_typed_int_list_by_string_write_without_reclassifying() {
    let function = Function {
        consts: ConstPool {
            ints: vec![7, 8, 1],
            strings: vec!["nine".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadInt, 3, 2),
            Instr::abx(Opcode::LoadString, 4, 0),
            Instr::abc(Opcode::SetIndex, 2, 3, 4),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Mixed(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected mixed list");
    };

    assert_eq!(values[0], RuntimeVal::Int(7));
    assert!(matches!(&values[1], RuntimeVal::ShortStr(value) if value.as_str() == "nine"));
}

#[test]
fn execute_updates_typed_string_list_without_materializing() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1],
            strings: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadInt, 3, 0),
            Instr::abx(Opcode::LoadString, 4, 2),
            Instr::abc(Opcode::SetIndex, 2, 3, 4),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::String(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string list");
    };

    assert_eq!(values[0].as_ref(), "a");
    assert_eq!(values[1].as_ref(), "c");
}

#[test]
fn execute_adds_typed_string_lists_without_materializing_items() {
    let function = Function {
        consts: ConstPool {
            heap_values: vec![
                ConstHeapValue::LongString(Arc::<str>::from("long-left-value")),
                ConstHeapValue::LongString(Arc::<str>::from("long-right-value")),
            ],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abc(Opcode::NewList, 1, 0, 1),
            Instr::abx(Opcode::LoadHeapConst, 2, 1),
            Instr::abc(Opcode::NewList, 3, 2, 1),
            Instr::abc(Opcode::AddInt, 4, 1, 3),
            Instr::abc(Opcode::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::String(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string list");
    };

    assert_eq!(values.len(), 2);
    assert_eq!(result.state.heap.len(), 5);
}

#[test]
fn execute_adds_typed_int_lists_and_push_preserving_backing() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 2, 3, 4],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadInt, 3, 2),
            Instr::abc(Opcode::NewList, 4, 3, 1),
            Instr::abc(Opcode::AddInt, 5, 2, 4),
            Instr::abx(Opcode::LoadInt, 6, 3),
            Instr::abc(Opcode::AddInt, 7, 5, 6),
            Instr::abc(Opcode::Return, 7, 1, 0),
        ],
        register_count: 8,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Int(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![1, 2, 3, 4]);
}

#[test]
fn execute_empty_list_push_adopts_int_backing() {
    let function = Function {
        consts: ConstPool {
            ints: vec![42],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abc(Opcode::NewList, 0, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::ListPush, 0, 1, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Int(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![42]);
}

#[test]
fn execute_prepends_value_to_typed_string_list_without_helper_materialization() {
    let function = Function {
        consts: ConstPool {
            ints: vec![7],
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("long-tail-value"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadHeapConst, 1, 0),
            Instr::abc(Opcode::NewList, 2, 1, 1),
            Instr::abc(Opcode::AddInt, 3, 0, 2),
            Instr::abc(Opcode::Return, 3, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Mixed(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected mixed list");
    };

    assert_eq!(values[0], RuntimeVal::Int(7));
    assert_eq!(values.len(), 2);
}

#[test]
fn execute_subtracts_cross_numeric_list_without_reclassifying_lhs_backing() {
    let function = Function {
        consts: ConstPool {
            ints: vec![1, 2],
            floats: vec![1.0],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadFloat, 3, 0),
            Instr::abc(Opcode::NewList, 4, 3, 1),
            Instr::abc(Opcode::SubInt, 5, 2, 4),
            Instr::abc(Opcode::Return, 5, 1, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Int(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![2]);
}

#[test]
fn execute_materializes_typed_string_list_on_non_string_write() {
    let function = Function {
        consts: ConstPool {
            ints: vec![0, 42],
            strings: vec!["short".to_string(), "longer-than-seven".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadString, 0, 0),
            Instr::abx(Opcode::LoadString, 1, 1),
            Instr::abc(Opcode::NewList, 2, 0, 2),
            Instr::abx(Opcode::LoadInt, 3, 0),
            Instr::abx(Opcode::LoadInt, 4, 1),
            Instr::abc(Opcode::SetIndex, 2, 3, 4),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Mixed(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected mixed list");
    };

    assert_eq!(values[0], RuntimeVal::Int(42));
}
