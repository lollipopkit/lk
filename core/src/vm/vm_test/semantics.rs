use super::*;

#[test]
fn test_vm_int_add() {
    let compiler = Compiler::new();
    let expr = Expr::Bin(
        Box::new(Expr::Val(Val::Int(2))),
        BinOp::Add,
        Box::new(Expr::Val(Val::Int(40))),
    );
    let fun = compiler.compile_expr(&expr);
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let out = vm.exec(&fun, &mut ctx).unwrap();
    assert_eq!(out, Val::Int(42));
}

#[test]
fn test_vm_optional_and_nullish_nested() {
    use Compiler;

    // Optional access: {"a": {}}?.a?.b -> nil
    let expr = Expr::OptionalAccess(
        Box::new(Expr::OptionalAccess(
            Box::new(Expr::Map(vec![(
                Box::new(Expr::Val(Val::Str("a".into()))),
                Box::new(Expr::Map(vec![])),
            )])),
            Box::new(Expr::Val(Val::Str("a".into()))),
        )),
        Box::new(Expr::Val(Val::Str("b".into()))),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Nil);

    // Nullish coalescing with nesting: nil ?? (nil ?? 3) -> 3
    let expr = Expr::NullishCoalescing(
        Box::new(Expr::Val(Val::Nil)),
        Box::new(Expr::NullishCoalescing(
            Box::new(Expr::Val(Val::Nil)),
            Box::new(Expr::Val(Val::Int(3))),
        )),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(3));

    // Short-circuit: true || (expensive) -> true; false && (expensive) -> false
    let expr = Expr::Or(
        Box::new(Expr::Val(Val::Bool(true))),
        Box::new(Expr::Bin(
            Box::new(Expr::Val(Val::Int(1))),
            BinOp::Div,
            Box::new(Expr::Val(Val::Int(0))),
        )),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Bool(true));

    let expr = Expr::And(
        Box::new(Expr::Val(Val::Bool(false))),
        Box::new(Expr::Bin(
            Box::new(Expr::Val(Val::Int(1))),
            BinOp::Div,
            Box::new(Expr::Val(Val::Int(0))),
        )),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Bool(false));
}

#[test]
fn test_vm_nullish_non_nil_values() {
    use Compiler;

    // false ?? 5 -> false (do not coalesce)
    let expr = Expr::NullishCoalescing(Box::new(Expr::Val(Val::Bool(false))), Box::new(Expr::Val(Val::Int(5))));
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Bool(false));

    // 0 ?? 5 -> 0
    let expr = Expr::NullishCoalescing(Box::new(Expr::Val(Val::Int(0))), Box::new(Expr::Val(Val::Int(5))));
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(0));

    // "" ?? 7 -> ""
    let expr = Expr::NullishCoalescing(
        Box::new(Expr::Val(Val::Str("".into()))),
        Box::new(Expr::Val(Val::Int(7))),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Str("".into()));
}

#[test]
fn test_vm_nullish_short_circuit_right_not_evaluated() {
    use Compiler;

    // 1 ?? (1 / 0) -> 1, and must not evaluate right (division by zero)
    let expr = Expr::NullishCoalescing(
        Box::new(Expr::Val(Val::Int(1))),
        Box::new(Expr::Bin(
            Box::new(Expr::Val(Val::Int(1))),
            BinOp::Div,
            Box::new(Expr::Val(Val::Int(0))),
        )),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(1));
}

#[test]
fn test_vm_indexk_negative_and_overflow() {
    use Compiler;

    // [1,2][ -1 ] -> nil
    let expr = Expr::Access(
        Box::new(Expr::List(vec![
            Box::new(Expr::Val(Val::Int(1))),
            Box::new(Expr::Val(Val::Int(2))),
        ])),
        Box::new(Expr::Val(Val::Int(-1))),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Nil);

    // "ab"[ 5 ] -> nil
    let expr = Expr::Access(
        Box::new(Expr::Val(Val::Str("ab".into()))),
        Box::new(Expr::Val(Val::Int(5))),
    );
    let fun = Compiler::new().compile_expr(&expr);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Nil);
}

#[test]
fn test_vm_index_dynamic_list_negative_overflow() {
    // l = [1,2]; i = -1; return l[i] -> nil
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "l".into(),
                value: Box::new(Expr::List(vec![
                    Box::new(Expr::Val(Val::Int(1))),
                    Box::new(Expr::Val(Val::Int(2))),
                ])),
            }),
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(-1))),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Access(
                    Box::new(Expr::Var("l".into())),
                    Box::new(Expr::Var("i".into())),
                ))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Nil);

    // i = 5 (overflow) -> nil
    let program2 = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "l".into(),
                value: Box::new(Expr::List(vec![
                    Box::new(Expr::Val(Val::Int(1))),
                    Box::new(Expr::Val(Val::Int(2))),
                ])),
            }),
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(5))),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Access(
                    Box::new(Expr::Var("l".into())),
                    Box::new(Expr::Var("i".into())),
                ))),
            }),
        ],
    };
    let fun2 = Compiler::new().compile_stmt(&program2);
    let out2 = exec_with_new_vm(&fun2);
    assert_eq!(out2, Val::Nil);
}

#[test]
fn test_vm_index_dynamic_string_negative_overflow() {
    // s = "ab"; i = -1; return s[i] -> nil
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "s".into(),
                value: Box::new(Expr::Val(Val::Str("ab".into()))),
            }),
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(-1))),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Access(
                    Box::new(Expr::Var("s".into())),
                    Box::new(Expr::Var("i".into())),
                ))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Nil);

    // i = 5 (overflow) -> nil
    let program2 = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "s".into(),
                value: Box::new(Expr::Val(Val::Str("ab".into()))),
            }),
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(5))),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Access(
                    Box::new(Expr::Var("s".into())),
                    Box::new(Expr::Var("i".into())),
                ))),
            }),
        ],
    };
    let fun2 = Compiler::new().compile_stmt(&program2);
    let out2 = exec_with_new_vm(&fun2);
    assert_eq!(out2, Val::Nil);
}
