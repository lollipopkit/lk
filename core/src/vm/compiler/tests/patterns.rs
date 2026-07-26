use super::*;

#[test]
fn compiler_lowers_match_range_guard_and_or_patterns() {
    let function = compile_source(
        r#"
        let x = 25;
        let y = match x {
            0 | 1 => 0,
            n if n < 10 => 1,
            18..65 => 42,
            _ => 2,
        };
        return y;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_if_let_list_destructuring_binding() {
    let function = compile_source(
        r#"
        if let [a, [b]] = [40, [2]] {
            return a + b;
        }
        return 0;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_if_let_list_rest_binding() {
    let function = compile_source(
        r#"
        if let [head, ..tail] = [40, 1, 2] {
            return head + tail.1;
        }
        return 0;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_match_map_destructuring_binding() {
    let function = compile_source(
        r#"
        let data = {"left": 40, "right": {"value": 2}};
        let y = match data {
            {"left": a, "right": {"value": b}} => a + b,
        };
        return y;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_match_map_rest_binding_to_map_rest() {
    let function = compile_source(
        r#"
        let data = {"a": 40, "b": 2};
        let y = match data {
            {"a": a, ..rest} => a + rest.b,
        };
        return y;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MapRest),
        "expected MapRest in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_match_fallback_to_nil() {
    let function = compile_source(
        r#"
        let y = match 7 {
            0 => 1,
        };
        return y == nil;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Bool(true)]);
}
