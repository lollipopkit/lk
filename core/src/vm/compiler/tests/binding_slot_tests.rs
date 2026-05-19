use super::{compile_and_run, make_add1_function, make_let};
use crate::{expr::Expr, op::BinOp, stmt::Stmt, val::Val, vm::Op};

#[test]
fn define_non_call_expression_writes_directly_to_global_slot() {
    let (function, _ctx, result) = compile_and_run(vec![
        Stmt::Define {
            name: "value".to_string(),
            value: Box::new(Expr::Bin(
                Box::new(Expr::Val(Val::Int(2))),
                BinOp::Add,
                Box::new(Expr::Val(Val::Int(3))),
            )),
        },
        Stmt::Return {
            value: Some(Box::new(Expr::Var("value".to_string()))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(5));
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::StoreLocal(_, _))),
        "define should initialize its local/global slot without a StoreLocal copy"
    );
}

#[test]
fn function_definition_writes_closure_directly_to_global_slot() {
    let (function, _ctx, result) = compile_and_run(vec![
        make_add1_function(),
        Stmt::Return {
            value: Some(Box::new(Expr::Call(
                "add1".to_string(),
                vec![Box::new(Expr::Val(Val::Int(4)))],
            ))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(5));
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::StoreLocal(_, _))),
        "function definition should materialize the closure in its final slot"
    );
}

#[test]
fn let_call_result_stays_in_return_slot_without_storelocal() {
    let (function, _ctx, result) = compile_and_run(vec![
        make_add1_function(),
        make_let(
            "first",
            Expr::Call("add1".to_string(), vec![Box::new(Expr::Val(Val::Int(4)))]),
            false,
        ),
        make_let(
            "second",
            Expr::Call("add1".to_string(), vec![Box::new(Expr::Val(Val::Int(10)))]),
            false,
        ),
        Stmt::Return {
            value: Some(Box::new(Expr::Bin(
                Box::new(Expr::Var("first".to_string())),
                BinOp::Add,
                Box::new(Expr::Var("second".to_string())),
            ))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(16));
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::StoreLocal(_, _))),
        "let call result should bind to the call return slot without StoreLocal"
    );
}

#[test]
fn zero_arg_call_let_binds_reserved_return_slot_without_storelocal() {
    let getter = Stmt::Function {
        name: "get_value".to_string(),
        params: Vec::new(),
        param_types: Vec::new(),
        return_type: None,
        body: Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Var("value".to_string()))),
        }),
        named_params: Vec::new(),
    };
    let (function, _ctx, result) = compile_and_run(vec![
        Stmt::Define {
            name: "value".to_string(),
            value: Box::new(Expr::Val(Val::Int(1))),
        },
        getter,
        make_let("before", Expr::Call("get_value".to_string(), Vec::new()), false),
        Stmt::Define {
            name: "value".to_string(),
            value: Box::new(Expr::Val(Val::Int(2))),
        },
        make_let("after", Expr::Call("get_value".to_string(), Vec::new()), false),
        Stmt::Return {
            value: Some(Box::new(Expr::List(vec![
                Box::new(Expr::Var("before".to_string())),
                Box::new(Expr::Var("after".to_string())),
            ]))),
        },
    ]);
    let result = result.expect("vm exec");
    let Val::List(items) = result else {
        panic!("expected list result");
    };
    assert_eq!(items.as_slice(), &[Val::Int(1), Val::Int(2)]);
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::StoreLocal(_, _))),
        "zero-arg call results should have a stable reserved return slot"
    );
}
