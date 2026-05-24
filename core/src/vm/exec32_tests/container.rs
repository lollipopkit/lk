use super::*;
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
                "1".to_string(),
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
            Instr32::abc(Opcode32::Contains, 15, 0, 8),
            Instr32::abx(Opcode32::LoadString, 16, 4),
            Instr32::abx(Opcode32::LoadInt, 17, 0),
            Instr32::abc(Opcode32::NewMap, 18, 16, 1),
            Instr32::abc(Opcode32::Contains, 19, 1, 18),
            Instr32::abc(Opcode32::Move, 0, 4, 0),
            Instr32::abc(Opcode32::Move, 1, 5, 0),
            Instr32::abc(Opcode32::Move, 2, 9, 0),
            Instr32::abc(Opcode32::Move, 3, 10, 0),
            Instr32::abc(Opcode32::Move, 4, 13, 0),
            Instr32::abc(Opcode32::Move, 5, 15, 0),
            Instr32::abc(Opcode32::Move, 6, 19, 0),
            Instr32::abc(Opcode32::Return, 0, 7, 0),
        ],
        register_count: 20,
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
            RuntimeVal::Bool(false),
            RuntimeVal::Bool(true),
        ]
    );
}

#[test]
fn execute32_to_iter_reads_typed_string_int_map_backing_as_pairs() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![10, 20],
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
            Instr32::abc(Opcode32::Return, 4, 2, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };

    let result = execute32(&function).expect("execute32");

    let RuntimeVal::Obj(map_handle) = result.returns[0] else {
        panic!("expected returned map object");
    };
    let HeapValue::Map(TypedMap::StringInt(entries)) = result.state.heap.get(map_handle).expect("map heap object")
    else {
        panic!("expected typed string-int map backing");
    };
    assert_eq!(entries.get("a").copied(), Some(10));
    assert_eq!(entries.get("b").copied(), Some(20));

    let RuntimeVal::Obj(iter_handle) = result.returns[1] else {
        panic!("expected returned iterator list object");
    };
    let HeapValue::List(crate::val::TypedList::Mixed(pairs)) =
        result.state.heap.get(iter_handle).expect("iter heap object")
    else {
        panic!("expected mixed pair list");
    };
    assert_eq!(pairs.len(), 2);

    let RuntimeVal::Obj(first_pair_handle) = pairs[0] else {
        panic!("expected first pair object");
    };
    let HeapValue::List(crate::val::TypedList::Mixed(first_pair)) = result
        .state
        .heap
        .get(first_pair_handle)
        .expect("first pair heap object")
    else {
        panic!("expected first pair mixed list");
    };
    assert_eq!(
        first_pair,
        &vec![
            RuntimeVal::ShortStr(crate::val::ShortStr::new("a").expect("short key")),
            RuntimeVal::Int(10),
        ]
    );
}

#[test]
fn execute32_compares_const_string_key_maps_across_short_and_heap_keys() {
    let mut short_key_map = std::collections::BTreeMap::new();
    short_key_map.insert(
        RuntimeMapKey::ShortStr(crate::val::ShortStr::new("a").expect("short key")),
        crate::vm::ConstRuntimeValue32::Int(42),
    );
    let mut heap_key_map = std::collections::BTreeMap::new();
    heap_key_map.insert(
        RuntimeMapKey::String(std::sync::Arc::<str>::from("a")),
        crate::vm::ConstRuntimeValue32::Int(42),
    );
    let function = Function32 {
        consts: ConstPool32 {
            heap_values: vec![
                ConstHeapValue32::Map(short_key_map),
                ConstHeapValue32::Map(heap_key_map),
            ],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadHeapConst, 0, 0),
            Instr32::abx(Opcode32::LoadHeapConst, 1, 1),
            Instr32::abc(Opcode32::CmpInt, 2, 0, 1),
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

    assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
}

#[test]
fn execute32_mixed_map_set_index_uses_exact_string_key_semantics() {
    let mut map = std::collections::BTreeMap::new();
    map.insert(
        RuntimeMapKey::String(std::sync::Arc::<str>::from("a")),
        crate::vm::ConstRuntimeValue32::Int(1),
    );
    map.insert(RuntimeMapKey::Int(7), crate::vm::ConstRuntimeValue32::Int(0));
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![9],
            strings: vec!["a".to_string()],
            heap_values: vec![
                ConstHeapValue32::Map(map),
                ConstHeapValue32::LongString(std::sync::Arc::<str>::from("a")),
            ],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadHeapConst, 0, 0),
            Instr32::abx(Opcode32::LoadString, 1, 0),
            Instr32::abx(Opcode32::LoadInt, 2, 0),
            Instr32::abc(Opcode32::SetIndex, 0, 1, 2),
            Instr32::abx(Opcode32::LoadHeapConst, 3, 1),
            Instr32::abc(Opcode32::GetIndex, 4, 0, 3),
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

    assert_eq!(result.returns, vec![RuntimeVal::Int(1)]);
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
fn execute32_map_rest_preserves_typed_string_int_backing() {
    let function = Function32 {
        consts: ConstPool32 {
            ints: vec![40, 2],
            strings: vec!["a".to_string(), "b".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadString, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadString, 2, 1),
            Instr32::abx(Opcode32::LoadInt, 3, 1),
            Instr32::abc(Opcode32::NewMap, 4, 0, 2),
            Instr32::abc(Opcode32::Move, 5, 4, 0),
            Instr32::abc(Opcode32::Move, 6, 0, 0),
            Instr32::abc(Opcode32::MapRest, 7, 5, 1),
            Instr32::abc(Opcode32::Return, 7, 1, 0),
        ],
        register_count: 8,
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

    assert_eq!(values.len(), 1);
    assert_eq!(values.get("b"), Some(&2));
}
