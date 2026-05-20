use super::parse_compile_and_run;
use crate::{val::Val, vm::Op};

#[test]
fn stdlib_list_set_constant_index_lowers_to_list_set_i() {
    let source = r#"
        import list;
        let data = [1, 2, 3];
        let pair = list.set(data, 1, 42);
        let updated = pair[0];
        return [data[1], updated[1], pair[1]];
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    let Val::List(values) = result.expect("vm exec") else {
        panic!("expected list");
    };
    assert_eq!(values.as_slice(), [Val::Int(2), Val::Int(42), Val::Int(2)]);
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListSetI { .. })),
        "expected list.set(data, 1, 42) to lower to ListSetI in {:?}",
        function.code
    );
}

#[test]
fn list_method_set_constant_index_lowers_to_list_set_i() {
    let source = r#"
        let data = [1, 2, 3];
        let pair = data.set(1, 42);
        let updated = pair[0];
        return [data[1], updated[1], pair[1]];
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    let Val::List(values) = result.expect("vm exec") else {
        panic!("expected list");
    };
    assert_eq!(values.as_slice(), [Val::Int(2), Val::Int(42), Val::Int(2)]);
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListSetI { .. })),
        "expected data.set(1, 42) to lower to ListSetI in {:?}",
        function.code
    );
}

#[test]
fn homogeneous_int_list_index_feeds_typed_arithmetic() {
    let source = r#"
        let data = [10, 20, 30];
        return data[1] + 22;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListIndexI(_, _, _))),
        "expected constant list index to lower to ListIndexI in {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "expected homogeneous Int list element fact to feed typed add in {:?}",
        function.code
    );
}

#[test]
fn homogeneous_int_list_get_feeds_typed_arithmetic() {
    let source = r#"
        import list;
        let data = [10, 20, 30];
        return list.get(data, 1) + 22;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "expected list.get on homogeneous Int list to feed typed add in {:?}",
        function.code
    );
}

#[test]
fn homogeneous_float_list_index_feeds_typed_arithmetic() {
    let source = r#"
        let data = [1.5, 2.5, 3.5];
        return data[1] * 2.0;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Float(5.0));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListIndexI(_, _, _))),
        "expected constant list index to lower to ListIndexI in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MulFloat(_, _, _))),
        "expected homogeneous Float list element fact to feed typed multiply in {:?}",
        function.code
    );
}

#[test]
fn list_push_invalidates_homogeneous_value_fact() {
    let source = r#"
        let data = [1, 2];
        data.push("x");
        let inc = 1;
        return data[0] + inc;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(2));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::ListPush { .. } | Op::ListPushMove { .. })),
        "expected list.push to lower to a list push opcode in {:?}",
        function.code
    );
    assert!(
        !function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "list.push should invalidate homogeneous element facts before later add in {:?}",
        function.code
    );
}

#[test]
fn list_push_temporary_value_lowers_to_move_push() {
    let source = r#"
        let data = [];
        data.push("sku-${1}");
        return data[0];
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::from_str("sku-1"));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListPushMove { .. })),
        "expected temporary list.push argument to lower to ListPushMove in {:?}",
        function.code
    );
}

#[test]
fn list_push_variable_value_keeps_non_move_push() {
    let source = r#"
        let data = [];
        let value = "sku-${1}";
        data.push(value);
        return [data[0], value];
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    let expected = Val::from_str("sku-1");
    let Val::List(values) = result.expect("vm exec") else {
        panic!("expected list");
    };
    assert_eq!(values.as_slice(), [expected.clone(), expected]);
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListPush { .. })),
        "expected variable list.push argument to keep ListPush in {:?}",
        function.code
    );
}

#[test]
fn empty_list_push_adopts_homogeneous_int_fact() {
    let source = r#"
        let data = [];
        data.push(40);
        data.push(1);
        return data[0] + data[1] + 1;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function
            .code
            .iter()
            .filter(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _)))
            .count()
            >= 2,
        "empty list followed by homogeneous Int pushes should feed typed adds in {:?}",
        function.code
    );
}
