use super::*;
use std::sync::Arc;

use crate::{
    val::{CallableValue, HeapRef, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal},
    vm::{
        ConstHeapValue32, ConstPool32, Instr32, NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32,
        Opcode32, RuntimeCallable32, VmContext,
    },
};

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

#[test]
fn execute32_compares_int_ordering() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![3, 5],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abc(Opcode32::CmpLtInt, 2, 0, 1),
            Instr32::abc(Opcode32::CmpGeInt, 3, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 2, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Bool(true), RuntimeVal::Bool(false)]);
}

#[test]
fn execute32_checks_contains_for_typed_list_map_and_string() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![2, 9, 1],
            strings: vec![
                "ab".to_string(),
                "z".to_string(),
                "abc".to_string(),
                "answer".to_string(),
            ],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 2),
            Instr32::abx(Opcode32::LoadInt, 2, 0),
            Instr32::abc(Opcode32::NewList, 3, 1, 2),
            Instr32::abx(Opcode32::LoadInt, 14, 1),
            Instr32::abc(Opcode32::Contains, 4, 0, 3),
            Instr32::abc(Opcode32::Contains, 5, 14, 3),
            Instr32::abx(Opcode32::LoadString, 6, 0),
            Instr32::abx(Opcode32::LoadString, 7, 1),
            Instr32::abx(Opcode32::LoadString, 8, 2),
            Instr32::abc(Opcode32::Contains, 9, 6, 8),
            Instr32::abc(Opcode32::Contains, 10, 7, 8),
            Instr32::abx(Opcode32::LoadString, 11, 3),
            Instr32::abc(Opcode32::NewMap, 12, 11, 1),
            Instr32::abc(Opcode32::Contains, 13, 11, 12),
            Instr32::abc(Opcode32::Move, 0, 4, 0),
            Instr32::abc(Opcode32::Move, 1, 5, 0),
            Instr32::abc(Opcode32::Move, 2, 9, 0),
            Instr32::abc(Opcode32::Move, 3, 10, 0),
            Instr32::abc(Opcode32::Move, 4, 13, 0),
            Instr32::abc(Opcode32::Return, 0, 5, 0),
        ],
        register_count: 15,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(
        result.returns,
        vec![
            RuntimeVal::Bool(true),
            RuntimeVal::Bool(false),
            RuntimeVal::Bool(true),
            RuntimeVal::Bool(false),
            RuntimeVal::Bool(true),
        ]
    );
}

#[test]
fn execute32_slices_typed_list_suffix_with_slice_from() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![40, 1, 2],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 1),
            Instr32::abx(Opcode32::LoadInt, 2, 2),
            Instr32::abc(Opcode32::NewList, 3, 0, 3),
            Instr32::abx(Opcode32::LoadInt, 4, 1),
            Instr32::abc(Opcode32::SliceFrom, 5, 3, 4),
            Instr32::abc(Opcode32::Len, 6, 5, 0),
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

    assert_eq!(result.returns, vec![RuntimeVal::Int(2)]);
}

#[test]
fn execute32_builds_map_rest_without_removed_keys() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![40, 2, 9],
            strings: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadString, 2, 1),
            Instr32::abx(Opcode32::LoadInt, 3, 1),
            Instr32::abx(Opcode32::LoadString, 4, 2),
            Instr32::abx(Opcode32::LoadInt, 5, 2),
            Instr32::abc(Opcode32::NewMap, 6, 0, 3),
            Instr32::abc(Opcode32::Move, 7, 6, 0),
            Instr32::abc(Opcode32::Move, 8, 0, 0),
            Instr32::abc(Opcode32::MapRest, 9, 7, 1),
            Instr32::abc(Opcode32::GetIndex, 10, 9, 2),
            Instr32::abc(Opcode32::GetIndex, 11, 9, 0),
            Instr32::abc(Opcode32::Return, 10, 2, 0),
        ],
        register_count: 12,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(2), RuntimeVal::Nil]);
}

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

#[test]
fn execute_module32_calls_native_function_with_same_call_opcode() {
    fn native_add(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let [RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)] = args.as_slice() else {
            bail!("native_add expects two ints");
        };
        Ok(RuntimeVal::Int(lhs + rhs))
    }

    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![13, 29],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadNative, 0, 0),
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
        functions: vec![entry],
        natives: vec![NativeEntry32 {
            name: "native_add".to_string(),
            arity: 2,
            function: NativeFunction32::Plain(native_add),
        }],
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module32(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_module32_calls_runtime32_callable_from_heap() {
    let callee = Function32 {
        consts: ConstPool32 {
            ints: vec![40],
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
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32::single(callee));
    let callable = RuntimeCallable32::new(Arc::clone(&callee_module), 0, Vec::new(), HeapStore::new(), Vec::new());

    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![2],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
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
    let caller_module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module32_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("call runtime32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute32_caller_handler_catches_raise_from_runtime32_callable() {
    let callee = Function32 {
        consts: ConstPool32 {
            strings: vec!["boom".into()],
            ..ConstPool32::default()
        },
        code: vec![Instr32::abx(Opcode32::Raise, 0, 0)],
        register_count: 1,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32 {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    });
    let callable = RuntimeCallable32::new(callee_module, 0, Vec::new(), HeapStore::new(), Vec::new());
    let entry = Function32 {
        code: vec![
            Instr32::as_bx(Opcode32::TryBegin, 0, 3),
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abc(Opcode32::Call, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        ..Function32::default()
    };
    let caller_module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module32_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("caller handler catches runtime32 raise");
    let RuntimeVal::Obj(handle) = result.returns.first().expect("return") else {
        panic!("handler return should be error object");
    };
    let Some(HeapValue::ErrorVal(error)) = result.state.heap.get(*handle) else {
        panic!("handler return should be ErrorVal");
    };

    assert_eq!(error.message.as_ref(), "boom");
}

#[test]
fn execute_module32_calls_runtime32_callable_with_named_args() {
    let callee = Function32 {
        code: vec![
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 2,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x"), Arc::<str>::from("y")],
        capture_count: 0,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32 {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    });
    let callable = RuntimeCallable32::new(Arc::clone(&callee_module), 0, Vec::new(), HeapStore::new(), Vec::new());

    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![40, 2],
            strings: vec!["y".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadString, 2, 0),
            Instr32::abx(Opcode32::LoadInt, 3, 1),
            Instr32::abx(Opcode32::CallNamed, 0, (1 << 7) | 1),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let caller_module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module32_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("call runtime32 named");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn runtime32_callable_error_keeps_shared_module_state() {
    let callee = Function32 {
        consts: ConstPool32 {
            ints: vec![41],
            strings: vec!["boom".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::SetGlobal, 0, 0),
            Instr32::abx(Opcode32::Raise, 0, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32 {
        functions: vec![callee],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "counter".into() }],
        entry: 0,
    });
    let callable = RuntimeCallable32::new(
        Arc::clone(&callee_module),
        0,
        Vec::new(),
        HeapStore::new(),
        vec![RuntimeVal::Int(1)],
    );
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_callable32_raw(&callable, &[], &mut ctx).expect_err("call should raise");

    assert!(err.to_string().contains("boom"));
    let state = callable.state.lock().expect("callable state");
    assert_eq!(state.globals, vec![RuntimeVal::Int(41)]);
}

#[test]
fn execute_source32_runs_public_source_entry_on_new_vm() {
    let result = execute_source32(
        r#"
        let data = {"a": 40, "b": 2};
        let y = match data {
            {"a": a, ..rest} => a + rest.b,
        };
        return y;
        "#,
    )
    .expect("execute source");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_module32_context_native_can_use_vm_context() {
    fn add_seed(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let [RuntimeVal::Int(delta)] = args.as_slice() else {
            bail!("add_seed expects one int");
        };
        let delta = *delta;
        let ctx = runtime.ctx_mut().ok_or_else(|| anyhow!("missing VmContext"))?;
        let Some(seed_export) = ctx.get_runtime_global("seed") else {
            bail!("seed must be a runtime global");
        };
        let RuntimeVal::Int(seed) = seed_export.value else {
            bail!("seed must be an int runtime global");
        };
        let value = seed + delta;
        ctx.define_runtime_value("seen", RuntimeVal::Int(value), HeapStore::new());
        Ok(RuntimeVal::Int(value))
    }

    let module = Compiler32::compile_source_module_with_natives(
        "return add_seed(2);",
        vec![NativeEntry32 {
            name: "add_seed".to_string(),
            arity: 1,
            function: NativeFunction32::Context(add_seed),
        }],
    )
    .expect("compile module");
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.define_runtime_value("seed", RuntimeVal::Int(40), HeapStore::new());

    let result = execute_module32_with_globals_and_ctx(&module, Vec::new(), &mut ctx).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        ctx.get_runtime_global("seen").map(|export| &export.value),
        Some(RuntimeVal::Int(42))
    ));
}

#[test]
fn execute_program32_with_ctx_reads_external_slots_without_syncing_back_to_context() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        total := seed + 2;
        seed = total + 1;
        return seed;
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.define_runtime_value("seed", RuntimeVal::Int(39), HeapStore::new());

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        ctx.get_runtime_global("seed").map(|export| &export.value),
        Some(RuntimeVal::Int(39))
    ));
    assert!(ctx.get_runtime_global("total").is_none());
}

#[test]
fn program_execute32_with_ctx_uses_new_vm_context_path() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let value = seed + 2;
        return value;
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.define_runtime_value("seed", RuntimeVal::Int(40), HeapStore::new());

    let result = program.execute32_with_ctx(&mut ctx).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_program32_imports_core_bit_builtins_as_runtime_native32() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        return (6 & 3) + (4 | 1) + (~0);
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(6)]);
}

#[test]
fn execute_program32_imports_core_object_builtins_as_runtime_native32() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let boxed = __lk_make_struct("Box", {"x": 1});
        let updated = __lk_set_field(boxed, "y", 2);
        return __lk_merge_fields(updated, {"z": 3});
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");
    let map = result.first_return_map().expect("result map");

    assert_eq!(
        map.get(&RuntimeMapKey::String(Arc::from("x"))),
        Some(RuntimeVal::Int(1))
    );
    assert_eq!(
        map.get(&RuntimeMapKey::String(Arc::from("y"))),
        Some(RuntimeVal::Int(2))
    );
    assert_eq!(
        map.get(&RuntimeMapKey::String(Arc::from("z"))),
        Some(RuntimeVal::Int(3))
    );
}

#[test]
fn execute_program32_imports_typeof_as_runtime_native32() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        return typeof(__lk_make_struct("Box", {"x": 1}));
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");

    assert!(matches!(result.first_return(), RuntimeVal::ShortStr(value) if value.as_str() == "Object"));
}
