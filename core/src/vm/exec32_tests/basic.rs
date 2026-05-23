use super::*;
use crate::vm::analysis::{PerfIndexTargetKind, PerfValueKind};
#[test]
fn execute32_returns_int_arithmetic_result() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![7, 5],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::MulInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(35)]);
}

#[test]
fn execute32_branches_with_test_and_jump() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1, 10, 20],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abc(Opcode32::Test, 0, 1, 1),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::sj(Opcode32::Jmp, 1),
            Instr32::abx(Opcode32::LoadInt, 1, 2),
            Instr32::abc(Opcode32::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(10)]);
}

#[test]
fn execute32_materializes_long_string_in_heap() {
    let function = Function32 {
        consts: ConstPool32 {
            heap_values: vec![ConstHeapValue32::LongString(Arc::<str>::from("longer-than-seven"))],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadHeapConst, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns[0].kind(), crate::val::RuntimeValKind::Obj);
    assert_eq!(result.state.heap.len(), 1);
}

#[test]
fn execute32_allocates_mixed_list_on_heap() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1, 2],
            strings: vec!["x".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadString, 1, 0),
            Instr32::abx(Opcode32::LoadInt, 2, 1),
            Instr32::abc(Opcode32::NewList, 3, 0, 3),
            Instr32::abc(Opcode32::Return, 3, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
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
fn execute32_allocates_typed_int_list_on_heap() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![7, 8],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::NewList, 2, 0, 2),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Int(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![7, 8]);
}

#[test]
fn execute32_allocates_typed_int_range_on_heap() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![5, 1, -2],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abx(Opcode32::LoadInt, 2, 2),
            Instr32::abc(Opcode32::NewRange, 3, 0, 1),
            Instr32::abc(Opcode32::Return, 3, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Int(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![5, 3, 1]);
}

#[test]
fn execute32_reads_len_for_typed_list_and_short_string() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1, 2],
            strings: vec!["abc".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::NewList, 2, 0, 2),
            Instr32::abc(Opcode32::Len, 3, 2, 0),
            Instr32::abx(Opcode32::LoadString, 4, 0),
            Instr32::abc(Opcode32::Len, 5, 4, 0),
            Instr32::abc(Opcode32::AddInt, 6, 3, 5),
            Instr32::abc(Opcode32::Return, 6, 1, 0),
        ],
        register_count: 7,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(5)]);
}

#[test]
fn execute32_to_iter_materializes_map_entries_as_pairs() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1, 2, 0, 1],
            strings: vec!["a".to_string(), "b".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadString, 2, 1),
            Instr32::abx(Opcode32::LoadInt, 3, 1),
            Instr32::abc(Opcode32::NewMap, 4, 0, 2),
            Instr32::abc(Opcode32::ToIter, 5, 4, 0),
            Instr32::abx(Opcode32::LoadInt, 6, 2),
            Instr32::abc(Opcode32::GetIndex, 7, 5, 6),
            Instr32::abx(Opcode32::LoadInt, 8, 3),
            Instr32::abc(Opcode32::GetIndex, 9, 7, 8),
            Instr32::abc(Opcode32::Return, 9, 1, 0),
        ],
        register_count: 10,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(1)]);
}

#[test]
fn execute32_allocates_object_and_reads_string_field() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![42],
            strings: vec!["User".to_string(), "score".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadString, 1, 1),
            Instr32::abx(Opcode32::LoadInt, 2, 0),
            Instr32::abc(Opcode32::NewObject, 3, 0, 1),
            Instr32::abc(Opcode32::GetIndex, 4, 3, 1),
            Instr32::abc(Opcode32::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute32_allocates_typed_string_int_map_and_reads_string_key() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![42],
            strings: vec!["answer".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::NewMap, 2, 0, 1),
            Instr32::abx(Opcode32::LoadString, 3, 0),
            Instr32::abc(Opcode32::GetIndex, 4, 2, 3),
            Instr32::abc(Opcode32::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    let RuntimeVal::Obj(handle) = result.state.stack[2] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string-int map");
    };
    assert_eq!(values.get("answer"), Some(&42));
    let cache = result.state.inline_caches.index(4).expect("index cache");
    assert_eq!(cache.target_kind, PerfIndexTargetKind::Map);
    assert_eq!(cache.value_kind, PerfValueKind::Int);
}

#[test]
fn execute32_writes_mixed_map_by_string_key() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1, 42],
            strings: vec!["answer".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::NewMap, 2, 0, 1),
            Instr32::abx(Opcode32::LoadString, 3, 0),
            Instr32::abx(Opcode32::LoadInt, 4, 1),
            Instr32::abc(Opcode32::SetIndex, 2, 3, 4),
            Instr32::abc(Opcode32::GetIndex, 5, 2, 3),
            Instr32::abc(Opcode32::Return, 5, 1, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute32_updates_typed_string_int_map_without_materializing() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1, 42],
            strings: vec!["answer".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::NewMap, 2, 0, 1),
            Instr32::abx(Opcode32::LoadString, 3, 0),
            Instr32::abx(Opcode32::LoadInt, 4, 1),
            Instr32::abc(Opcode32::SetIndex, 2, 3, 4),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected map object");
    };
    let HeapValue::Map(TypedMap::StringInt(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected typed string-int map");
    };

    assert_eq!(values.get("answer"), Some(&42));
}

#[test]
fn execute32_materializes_typed_string_int_map_to_string_mixed_on_value_pollution() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1],
            strings: vec!["answer".to_string(), "label".to_string(), "ok".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::NewMap, 2, 0, 1),
            Instr32::abx(Opcode32::LoadString, 3, 1),
            Instr32::abx(Opcode32::LoadString, 4, 2),
            Instr32::abc(Opcode32::SetIndex, 2, 3, 4),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
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
fn execute32_reads_mixed_list_by_int_index() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![7, 8, 1],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::NewList, 2, 0, 2),
            Instr32::abx(Opcode32::LoadInt, 3, 2),
            Instr32::abc(Opcode32::GetIndex, 4, 2, 3),
            Instr32::abc(Opcode32::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(8)]);
}

#[test]
fn execute32_writes_mixed_list_by_int_index() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![7, 8, 1, 42],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::NewList, 2, 0, 2),
            Instr32::abx(Opcode32::LoadInt, 3, 2),
            Instr32::abx(Opcode32::LoadInt, 4, 3),
            Instr32::abc(Opcode32::SetIndex, 2, 3, 4),
            Instr32::abc(Opcode32::GetIndex, 5, 2, 3),
            Instr32::abc(Opcode32::Return, 5, 1, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute32_updates_typed_string_list_without_materializing() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![1],
            strings: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadString, 1, 1),
            Instr32::abc(Opcode32::NewList, 2, 0, 2),
            Instr32::abx(Opcode32::LoadInt, 3, 0),
            Instr32::abx(Opcode32::LoadString, 4, 2),
            Instr32::abc(Opcode32::SetIndex, 2, 3, 4),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
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
fn execute32_adds_typed_string_lists_without_materializing_items() {
    let function = Function32 {
        consts: ConstPool32 {
            heap_values: vec![
                ConstHeapValue32::LongString(Arc::<str>::from("long-left-value")),
                ConstHeapValue32::LongString(Arc::<str>::from("long-right-value")),
            ],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadHeapConst, 0, 0),
            Instr32::abc(Opcode32::NewList, 1, 0, 1),
            Instr32::abx(Opcode32::LoadHeapConst, 2, 1),
            Instr32::abc(Opcode32::NewList, 3, 2, 1),
            Instr32::abc(Opcode32::AddInt, 4, 1, 3),
            Instr32::abc(Opcode32::Return, 4, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
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
fn execute32_materializes_typed_string_list_on_non_string_write() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![0, 42],
            strings: vec!["short".to_string(), "longer-than-seven".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadString, 1, 1),
            Instr32::abc(Opcode32::NewList, 2, 0, 2),
            Instr32::abx(Opcode32::LoadInt, 3, 0),
            Instr32::abx(Opcode32::LoadInt, 4, 1),
            Instr32::abc(Opcode32::SetIndex, 2, 3, 4),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 5,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");
    let RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let HeapValue::List(TypedList::Mixed(values)) = result.state.heap.get(handle).expect("heap object") else {
        panic!("expected mixed list");
    };

    assert_eq!(values[0], RuntimeVal::Int(42));
    assert_eq!(values[1].kind(), crate::val::RuntimeValKind::Obj);
}
