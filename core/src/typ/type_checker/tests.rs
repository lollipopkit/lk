use super::*;
use crate::{
    expr::Pattern,
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::Val,
};

#[test]
fn test_literal_types() {
    let mut checker = TypeChecker::new();

    assert_eq!(checker.check_expr(&Expr::Val(Val::Nil)).unwrap(), Type::Nil);
    assert_eq!(checker.check_expr(&Expr::Val(Val::Bool(true))).unwrap(), Type::Bool);
    assert_eq!(checker.check_expr(&Expr::Val(Val::Int(42))).unwrap(), Type::Int);
    assert_eq!(
        checker
            .check_expr(&Expr::Val(Val::Float(std::f64::consts::PI)))
            .unwrap(),
        Type::Float
    );
    assert_eq!(
        checker.check_expr(&Expr::Val(Val::Str("hello".into()))).unwrap(),
        Type::String
    );
}

#[test]
fn test_binary_operations() {
    let mut checker = TypeChecker::new();

    let add_expr = Expr::Bin(
        Box::new(Expr::Val(Val::Int(1))),
        BinOp::Add,
        Box::new(Expr::Val(Val::Int(2))),
    );

    let result_type = checker.check_expr(&add_expr).unwrap();
    // Now numeric ops infer Int for Int+Int
    assert!(matches!(result_type, Type::Int));
}

#[test]
fn test_string_addition_type() {
    let mut checker = TypeChecker::new();
    let add_expr = Expr::Bin(
        Box::new(Expr::Val(Val::Str("a".into()))),
        BinOp::Add,
        Box::new(Expr::Val(Val::Str("b".into()))),
    );
    let result_type = checker.check_expr(&add_expr).unwrap();
    assert!(matches!(result_type, Type::String));
}

#[test]
fn test_numeric_auto_promotion() {
    let mut checker = TypeChecker::new();
    let add_expr = Expr::Bin(
        Box::new(Expr::Val(Val::Int(1))),
        BinOp::Add,
        Box::new(Expr::Val(Val::Float(1.5))),
    );
    let result_type = checker.check_expr(&add_expr).unwrap();
    assert_eq!(result_type, Type::Float);
}

#[test]
fn test_division_promotes_float() {
    let mut checker = TypeChecker::new();
    let div_expr = Expr::Bin(
        Box::new(Expr::Val(Val::Int(3))),
        BinOp::Div,
        Box::new(Expr::Val(Val::Int(2))),
    );
    let result_type = checker.check_expr(&div_expr).unwrap();
    assert_eq!(result_type, Type::Float);
}

#[test]
fn test_numeric_type_error_message() {
    let mut checker = TypeChecker::new();
    let bad_expr = Expr::Bin(
        Box::new(Expr::Val(Val::Str("bad".into()))),
        BinOp::Mul,
        Box::new(Expr::Val(Val::Bool(true))),
    );
    let err = checker.check_expr(&bad_expr).unwrap_err();
    assert!(err.to_string().contains("must by numeric types"));
}

#[test]
fn test_list_types() {
    let mut checker = TypeChecker::new();

    let list_expr = Expr::List(vec![
        Box::new(Expr::Val(Val::Int(1))),
        Box::new(Expr::Val(Val::Int(2))),
        Box::new(Expr::Val(Val::Int(3))),
    ]);

    let result_type = checker.check_expr(&list_expr).unwrap();
    if let Type::List(elem_type) = result_type {
        assert_eq!(*elem_type, Type::Int);
    } else {
        panic!("Expected List<Int>");
    }
}

#[test]
fn test_type_mismatch_error() {
    let mut checker = TypeChecker::new();

    let logical_expr = Expr::And(
        Box::new(Expr::Val(Val::Int(1))), // Should be Bool
        Box::new(Expr::Val(Val::Bool(true))),
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
        value: Box::new(Expr::Val(Val::Int(42))),
        span: None,
        is_const: false,
    };

    // Should pass type checking
    assert!(let_stmt.type_check(&mut checker).is_ok());

    // Test type mismatch
    let let_stmt_mismatch = Stmt::Let {
        pattern: Pattern::Variable("y".to_string()),
        type_annotation: Some(Type::String),
        value: Box::new(Expr::Val(Val::Int(42))), // Int assigned to String
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
        value: Box::new(Expr::Val(Val::Int(42))),
        span: None,
        is_const: false,
    };
    let_stmt.type_check(&mut checker).unwrap();

    // Test valid assignment
    let assign_stmt = Stmt::Assign {
        name: "x".to_string(),
        value: Box::new(Expr::Val(Val::Int(100))),
        span: None,
    };
    assert!(assign_stmt.type_check(&mut checker).is_ok());

    // Test invalid assignment
    let assign_stmt_invalid = Stmt::Assign {
        name: "x".to_string(),
        value: Box::new(Expr::Val(Val::Str("hello".into()))), // String assigned to Int
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
        value: Box::new(Expr::Val(Val::Int(1))),
        span: None,
        is_const: true,
    };
    const_stmt
        .type_check(&mut checker)
        .expect("const binding should type check");

    let assign_stmt = Stmt::Assign {
        name: "x".to_string(),
        value: Box::new(Expr::Val(Val::Int(2))),
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
        condition: Box::new(Expr::Val(Val::Bool(true))),
        then_stmt: Box::new(Stmt::Let {
            pattern: Pattern::Variable("x".to_string()),
            type_annotation: None,
            value: Box::new(Expr::Val(Val::Int(42))),
            span: None,
            is_const: false,
        }),
        else_stmt: None,
    };
    assert!(if_stmt.type_check(&mut checker).is_ok());

    // Test if statement with non-boolean condition
    let if_stmt_invalid = Stmt::If {
        condition: Box::new(Expr::Val(Val::Int(42))), // Int instead of Bool
        then_stmt: Box::new(Stmt::Let {
            pattern: Pattern::Variable("x".to_string()),
            type_annotation: None,
            value: Box::new(Expr::Val(Val::Int(42))),
            span: None,
            is_const: false,
        }),
        else_stmt: None,
    };
    let result = if_stmt_invalid.type_check(&mut checker);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("If condition must be Bool"));
}

#[test]
fn test_while_statement_type_checking() {
    let mut checker = TypeChecker::new();

    // Test while statement with boolean condition
    let while_stmt = Stmt::While {
        condition: Box::new(Expr::Val(Val::Bool(true))),
        body: Box::new(Stmt::Expr(Box::new(Expr::Val(Val::Int(42))))),
    };
    assert!(while_stmt.type_check(&mut checker).is_ok());

    // Test while statement with non-boolean condition
    let while_stmt_invalid = Stmt::While {
        condition: Box::new(Expr::Val(Val::Int(42))), // Int instead of Bool
        body: Box::new(Stmt::Expr(Box::new(Expr::Val(Val::Int(42))))),
    };
    let result = while_stmt_invalid.type_check(&mut checker);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("While condition must be Bool"));
}

#[test]
fn test_for_statement_type_checking() {
    let mut checker = TypeChecker::new();

    // Test for statement with list iterable
    let for_stmt = Stmt::For {
        pattern: ForPattern::Variable("item".to_string()),
        iterable: Box::new(Expr::List(vec![
            Box::new(Expr::Val(Val::Int(1))),
            Box::new(Expr::Val(Val::Int(2))),
        ])),
        body: Box::new(Stmt::Expr(Box::new(Expr::Val(Val::Nil)))),
    };
    assert!(for_stmt.type_check(&mut checker).is_ok());

    // Test for statement with non-iterable
    let for_stmt_invalid = Stmt::For {
        pattern: ForPattern::Variable("item".to_string()),
        iterable: Box::new(Expr::Val(Val::Int(42))), // Int is not iterable
        body: Box::new(Stmt::Expr(Box::new(Expr::Val(Val::Nil)))),
    };
    let result = for_stmt_invalid.type_check(&mut checker);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("For loop iterable must be List, String, or Map")
    );
}
