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
