use super::*;
use crate::{
    expr::Pattern,
    operator::BinOp,
    stmt::{ForPattern, Stmt},
    val::LiteralVal,
};

#[test]
fn test_literal_types() {
    let mut checker = TypeChecker::new();

    assert_eq!(checker.check_expr(&Expr::Literal(LiteralVal::Nil)).unwrap(), Type::Nil);
    assert_eq!(
        checker.check_expr(&Expr::Literal(LiteralVal::Bool(true))).unwrap(),
        Type::Bool
    );
    assert_eq!(
        checker.check_expr(&Expr::Literal(LiteralVal::Int(42))).unwrap(),
        Type::Int
    );
    assert_eq!(
        checker
            .check_expr(&Expr::Literal(LiteralVal::Float(std::f64::consts::PI)))
            .unwrap(),
        Type::Float
    );
    assert_eq!(
        checker
            .check_expr(&Expr::Literal(LiteralVal::from_str("hello")))
            .unwrap(),
        Type::String
    );
}

#[test]
fn test_binary_operations() {
    let mut checker = TypeChecker::new();

    let add_expr = Expr::Bin(
        Box::new(Expr::Literal(LiteralVal::Int(1))),
        BinOp::Add,
        Box::new(Expr::Literal(LiteralVal::Int(2))),
    );

    let result_type = checker.check_expr(&add_expr).unwrap();
    // Now numeric ops infer Int for Int+Int
    assert!(matches!(result_type, Type::Int));
}

#[test]
fn test_string_addition_type() {
    let mut checker = TypeChecker::new();
    let add_expr = Expr::Bin(
        Box::new(Expr::Literal(LiteralVal::from_str("a"))),
        BinOp::Add,
        Box::new(Expr::Literal(LiteralVal::from_str("b"))),
    );
    let result_type = checker.check_expr(&add_expr).unwrap();
    assert!(matches!(result_type, Type::String));
}

#[test]
fn test_numeric_auto_promotion() {
    let mut checker = TypeChecker::new();
    let add_expr = Expr::Bin(
        Box::new(Expr::Literal(LiteralVal::Int(1))),
        BinOp::Add,
        Box::new(Expr::Literal(LiteralVal::Float(1.5))),
    );
    let result_type = checker.check_expr(&add_expr).unwrap();
    assert_eq!(result_type, Type::Float);
}

#[test]
fn test_division_promotes_float() {
    let mut checker = TypeChecker::new();
    let div_expr = Expr::Bin(
        Box::new(Expr::Literal(LiteralVal::Int(3))),
        BinOp::Div,
        Box::new(Expr::Literal(LiteralVal::Int(2))),
    );
    let result_type = checker.check_expr(&div_expr).unwrap();
    assert_eq!(result_type, Type::Float);
}

#[test]
fn test_numeric_type_error_message() {
    let mut checker = TypeChecker::new();
    let bad_expr = Expr::Bin(
        Box::new(Expr::Literal(LiteralVal::from_str("bad"))),
        BinOp::Mul,
        Box::new(Expr::Literal(LiteralVal::Bool(true))),
    );
    let err = checker.check_expr(&bad_expr).unwrap_err();
    assert!(err.to_string().contains("must by numeric types"));
}

#[test]
fn test_list_types() {
    let mut checker = TypeChecker::new();

    let list_expr = Expr::List(vec![
        Box::new(Expr::Literal(LiteralVal::Int(1))),
        Box::new(Expr::Literal(LiteralVal::Int(2))),
        Box::new(Expr::Literal(LiteralVal::Int(3))),
    ]);

    let result_type = checker.check_expr(&list_expr).unwrap();
    if let Type::List(elem_type) = result_type {
        assert_eq!(*elem_type, Type::Int);
    } else {
        panic!("Expected List<Int>");
    }
}

#[test]
fn test_index_infers_unannotated_list_element_type() {
    let mut checker = TypeChecker::new();
    let xs_ty = checker.fresh_type_var();
    checker.add_local_type("xs".to_string(), xs_ty.clone());

    let expr = Expr::Bin(
        Box::new(Expr::Access(
            Box::new(Expr::Var("xs".to_string())),
            Box::new(Expr::Literal(LiteralVal::Int(0))),
        )),
        BinOp::Add,
        Box::new(Expr::Literal(LiteralVal::Int(1))),
    );

    assert_eq!(checker.check_expr(&expr).unwrap(), Type::Int);
    let subs = checker.solve_constraints().unwrap();
    let resolved_xs = checker.apply_substitutions(xs_ty, &subs);
    assert_eq!(resolved_xs, Type::List(Box::new(Type::Int)));
}

#[test]
fn test_skip_infers_unannotated_list_type() {
    let mut checker = TypeChecker::new();
    let xs_ty = checker.fresh_type_var();
    checker.add_local_type("xs".to_string(), xs_ty.clone());

    let skip_call = Expr::CallExpr(
        Box::new(Expr::Access(
            Box::new(Expr::Var("xs".to_string())),
            Box::new(Expr::Literal(LiteralVal::from_str("skip"))),
        )),
        vec![Box::new(Expr::Literal(LiteralVal::Int(1)))],
    );
    let expr = Expr::Bin(
        Box::new(Expr::Access(
            Box::new(skip_call),
            Box::new(Expr::Literal(LiteralVal::Int(0))),
        )),
        BinOp::Add,
        Box::new(Expr::Literal(LiteralVal::Int(1))),
    );

    assert_eq!(checker.check_expr(&expr).unwrap(), Type::Int);
    let subs = checker.solve_constraints().unwrap();
    let resolved_xs = checker.apply_substitutions(xs_ty, &subs);
    assert_eq!(resolved_xs, Type::List(Box::new(Type::Int)));
}

#[test]
fn test_type_mismatch_error() {
    let mut checker = TypeChecker::new();

    let logical_expr = Expr::And(
        Box::new(Expr::Literal(LiteralVal::Int(1))), // Should be Bool
        Box::new(Expr::Literal(LiteralVal::Bool(true))),
    );

    let result = checker.check_expr(&logical_expr);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Expected boolean type"));
}

#[test]
fn test_let_statement_type_checking() {
    let mut checker = TypeChecker::new();

    // Test let statement with type annotation
    let let_stmt = Stmt::Let {
        pattern: Pattern::Variable("x".to_string()),
        type_annotation: Some(Type::Int),
        value: Box::new(Expr::Literal(LiteralVal::Int(42))),
        span: None,
        is_const: false,
    };

    // Should pass type checking
    assert!(let_stmt.type_check(&mut checker).is_ok());

    // Test type mismatch
    let let_stmt_mismatch = Stmt::Let {
        pattern: Pattern::Variable("y".to_string()),
        type_annotation: Some(Type::String),
        value: Box::new(Expr::Literal(LiteralVal::Int(42))), // Int assigned to String
        span: None,
        is_const: false,
    };

    let result = let_stmt_mismatch.type_check(&mut checker);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Type mismatch in let statement")
    );
}

#[test]
fn test_assignment_type_checking() {
    let mut checker = TypeChecker::new();

    // First declare a variable
    let let_stmt = Stmt::Let {
        pattern: Pattern::Variable("x".to_string()),
        type_annotation: Some(Type::Int),
        value: Box::new(Expr::Literal(LiteralVal::Int(42))),
        span: None,
        is_const: false,
    };
    let_stmt.type_check(&mut checker).unwrap();

    // Test valid assignment
    let assign_stmt = Stmt::Assign {
        name: "x".to_string(),
        value: Box::new(Expr::Literal(LiteralVal::Int(100))),
        span: None,
    };
    assert!(assign_stmt.type_check(&mut checker).is_ok());

    // Test invalid assignment
    let assign_stmt_invalid = Stmt::Assign {
        name: "x".to_string(),
        value: Box::new(Expr::Literal(LiteralVal::from_str("hello"))), // String assigned to Int
        span: None,
    };
    let result = assign_stmt_invalid.type_check(&mut checker);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Type mismatch in assignment"));
}

#[test]
fn test_const_assignment_type_error() {
    let mut checker = TypeChecker::new();

    let const_stmt = Stmt::Let {
        pattern: Pattern::Variable("x".to_string()),
        type_annotation: Some(Type::Int),
        value: Box::new(Expr::Literal(LiteralVal::Int(1))),
        span: None,
        is_const: true,
    };
    const_stmt
        .type_check(&mut checker)
        .expect("const binding should type check");

    let assign_stmt = Stmt::Assign {
        name: "x".to_string(),
        value: Box::new(Expr::Literal(LiteralVal::Int(2))),
        span: None,
    };
    let result = assign_stmt.type_check(&mut checker);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("const variable"));
}

#[test]
fn test_if_statement_type_checking() {
    let mut checker = TypeChecker::new();

    // Test if statement with boolean condition
    let if_stmt = Stmt::If {
        condition: Box::new(Expr::Literal(LiteralVal::Bool(true))),
        then_stmt: Box::new(Stmt::Let {
            pattern: Pattern::Variable("x".to_string()),
            type_annotation: None,
            value: Box::new(Expr::Literal(LiteralVal::Int(42))),
            span: None,
            is_const: false,
        }),
        else_stmt: None,
    };
    assert!(if_stmt.type_check(&mut checker).is_ok());

    // Test if statement with non-boolean condition
    let if_stmt_invalid = Stmt::If {
        condition: Box::new(Expr::Literal(LiteralVal::Int(42))), // Int instead of Bool
        then_stmt: Box::new(Stmt::Let {
            pattern: Pattern::Variable("x".to_string()),
            type_annotation: None,
            value: Box::new(Expr::Literal(LiteralVal::Int(42))),
            span: None,
            is_const: false,
        }),
        else_stmt: None,
    };
    assert!(if_stmt_invalid.type_check(&mut checker).is_ok());
}

#[test]
fn test_while_statement_type_checking() {
    let mut checker = TypeChecker::new();

    // Test while statement with boolean condition
    let while_stmt = Stmt::While {
        condition: Box::new(Expr::Literal(LiteralVal::Bool(true))),
        body: Box::new(Stmt::Expr(Box::new(Expr::Literal(LiteralVal::Int(42))))),
    };
    assert!(while_stmt.type_check(&mut checker).is_ok());

    // Test while statement with non-boolean condition
    let while_stmt_invalid = Stmt::While {
        condition: Box::new(Expr::Literal(LiteralVal::Int(42))), // Int instead of Bool
        body: Box::new(Stmt::Expr(Box::new(Expr::Literal(LiteralVal::Int(42))))),
    };
    assert!(while_stmt_invalid.type_check(&mut checker).is_ok());
}

#[test]
fn test_for_statement_type_checking() {
    let mut checker = TypeChecker::new();

    // Test for statement with list iterable
    let for_stmt = Stmt::For {
        pattern: ForPattern::Variable("item".to_string()),
        iterable: Box::new(Expr::List(vec![
            Box::new(Expr::Literal(LiteralVal::Int(1))),
            Box::new(Expr::Literal(LiteralVal::Int(2))),
        ])),
        body: Box::new(Stmt::Expr(Box::new(Expr::Literal(LiteralVal::Nil)))),
    };
    assert!(for_stmt.type_check(&mut checker).is_ok());

    // Test for statement with non-iterable
    let for_stmt_invalid = Stmt::For {
        pattern: ForPattern::Variable("item".to_string()),
        iterable: Box::new(Expr::Literal(LiteralVal::Int(42))), // Int is not iterable
        body: Box::new(Stmt::Expr(Box::new(Expr::Literal(LiteralVal::Nil)))),
    };
    let result = for_stmt_invalid.type_check(&mut checker);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("For loop iterable must be List, String, Map, or Set")
    );
}
