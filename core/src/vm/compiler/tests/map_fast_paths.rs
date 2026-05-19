use super::parse_compile_and_run;
use crate::{val::Val, vm::Op};

#[test]
fn map_values_sum_loop_uses_fold_add_opcode() {
    let source = r#"
        let map = {};
        map.set("a", 2);
        map.set("b", 3);
        let sum = 0;
        for v in map.values() {
            sum += v;
        }
        return sum;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(5));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapValuesFoldAdd { .. })),
        "expected map values fold opcode in {:?}",
        function.code
    );
}

#[test]
fn map_set_consumes_temporary_key_value_registers() {
    let source = r#"
        let map = {};
        let i = 2;
        map.set("key${i}", i * 3);
        return map.get("key2");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(6));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapSetMove { .. })),
        "expected map set move opcode in {:?}",
        function.code
    );
}

#[test]
fn map_set_preserves_variable_key_value_registers() {
    let source = r#"
        let map = {};
        let key = "key";
        let value = 7;
        map.set(key, value);
        return value + map.get(key);
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(14));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapSet { .. })),
        "expected preserving map set opcode in {:?}",
        function.code
    );
}

#[test]
fn stdlib_map_get_on_known_local_map_lowers_to_map_get_dynamic() {
    let source = r#"
        import map;
        let data = {};
        let key = "answer";
        data.set(key, 42);
        return map.get(data, key);
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapGetDynamic(_, _, _))),
        "expected stdlib map.get(data, key) to lower to MapGetDynamic in {:?}",
        function.code
    );
}

#[test]
fn stdlib_map_get_literal_key_lowers_to_map_get_interned() {
    let source = r#"
        import map;
        let data = {};
        data.set("answer", 42);
        return map.get(data, "answer");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapGetInterned(_, _, _))),
        "expected stdlib map.get(data, \"answer\") to lower to MapGetInterned in {:?}",
        function.code
    );
}

#[test]
fn stdlib_map_set_literal_key_lowers_to_map_set_interned() {
    let source = r#"
        import map;
        let data = {};
        map.set(data, "answer", 42);
        return data.get("answer");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapSetInterned(_, _, _))),
        "expected stdlib map.set(data, \"answer\", 42) to lower to MapSetInterned in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "expected stdlib map.set/data.get fast paths to avoid Call in {:?}",
        function.code
    );
}

#[test]
fn stdlib_map_has_literal_key_lowers_to_maphask() {
    let source = r#"
        import map;
        let data = {};
        data.set("answer", 42);
        return map.has(data, "answer");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Bool(true));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapHasK(_, _, _))),
        "expected stdlib map.has(data, \"answer\") to lower to MapHasK in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "expected stdlib map.has fast path to avoid Call in {:?}",
        function.code
    );
}

#[test]
fn map_method_has_dynamic_key_lowers_to_maphas() {
    let source = r#"
        let data = {};
        data.set("answer", 42);
        let key = "answer";
        return data.has(key);
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Bool(true));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapHas(_, _, _))),
        "expected data.has(key) to lower to MapHas in {:?}",
        function.code
    );
}

#[test]
fn map_method_get_dynamic_key_lowers_to_map_get_dynamic() {
    let source = r#"
        let data = {};
        data.set("answer", 42);
        let key = "answer";
        return data.get(key);
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapGetDynamic(_, _, _))),
        "expected data.get(key) to lower to MapGetDynamic in {:?}",
        function.code
    );
}

#[test]
fn map_index_dynamic_key_lowers_to_map_get_dynamic() {
    let source = r#"
        let data = {};
        data.set("answer", 42);
        let key = "answer";
        return data[key];
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapGetDynamic(_, _, _))),
        "expected data[key] to lower to MapGetDynamic in {:?}",
        function.code
    );
}
