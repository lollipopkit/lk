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

