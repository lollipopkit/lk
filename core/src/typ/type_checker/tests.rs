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
            .check_expr(&Expr::Literal(LiteralVal::Float(core::f64::consts::PI)))
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

/// `/` yields a `Float`, whatever it divides.
///
/// The checker always said this; the executors did not, and the constant
/// folder said it only when the literals did *not* divide evenly. All four
/// paths agree now, which is what this test is for.
#[test]
fn test_division_promotes_float() {
    let mut checker = TypeChecker::new();
    for (lhs, rhs) in [
        (LiteralVal::Int(3), LiteralVal::Int(2)),
        (LiteralVal::Int(20), LiteralVal::Int(4)),
        (LiteralVal::Int(3), LiteralVal::Float(2.0)),
    ] {
        let div_expr = Expr::Bin(
            Box::new(Expr::Literal(lhs.clone())),
            BinOp::Div,
            Box::new(Expr::Literal(rhs.clone())),
        );
        assert_eq!(
            checker.check_expr(&div_expr).unwrap(),
            Type::Float,
            "{lhs:?} / {rhs:?} should be a Float"
        );
    }
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
    assert!(err.to_string().contains("must be numeric types"));
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

/// The three ways a machine integer refuses to mix, and why each is a rule
/// rather than an oversight.
///
/// A `u32` register write has to be exactly 32 bits wide and has to wrap rather
/// than promote — promoting to `Int` would silently give the operation 64-bit
/// semantics, which is the opposite of what asking for a width was for. So the
/// checker refuses three things, and the messages are what a driver author
/// reads when a width is wrong.
///
/// What is *not* refused, and used to be: an integer literal beside a machine
/// An empty list learns its element type from a *container* argument too.
///
/// `xs.push(y)` has parameter `'T` — a bare variable — so the argument was used
/// to bind it. `xs.chain(ys)` has parameter `List<'T>`, which is not a variable,
/// so it was *checked* instead: passing a `List<Int>` reported "expected
/// List<'T0>, got List<Int>" rather than binding `'T0` to `Int`.
///
/// What that cost is a workaround in real code. `bare-metal-x86/program.lk`
/// wrote `let line = [0]; line = [];` — build a list with a placeholder element
/// so the element type is known, then throw the element away — because
/// `let line = []; line = line.chain(…)` did not type-check.
#[test]
fn an_empty_list_learns_its_element_type_from_a_container_argument() {
    for source in [
        // The shape the kernel had to work around.
        "let a = [];
a = a.chain([1]);
",
        // The one that always worked, so a change here cannot have broken it.
        "let a = [];
a.push(1);
",
        // Learned from the far side, and then used: the binding has to reach
        // the reads, not merely silence the argument check.
        "let a = [];
a = a.chain([1]);
let b: Int = a.len();
",
        // Nested one deeper.
        "let a = [];
a = a.chain([[1]]);
",
    ] {
        let program = crate::syntax::parse_program_source(source, Default::default())
            .unwrap_or_else(|e| panic!("should parse: {source}: {e}"));
        let mut checker = TypeChecker::new();
        program
            .type_check(&mut checker)
            .unwrap_or_else(|e| panic!("should type-check: {source:?}: {e}"));
    }
}

/// integer takes its width. `reg + 1` is what driver code is made of. Relaxing
/// the checker alone was a miscompile for one round — the compiler went on
/// materialising the literal as an ordinary `Int`, so `255u8 + 1` answered 256
/// with the type still claiming `u8` — so the literal is now normalised to the
/// width first, in `adopt_machine_width_for_literal`.
#[test]
fn machine_integers_refuse_to_mix() {
    for (source, expected) in [
        // A variable of another numeric type: a width mistake.
        (
            "let a: u8 = 5;\nlet n = 3;\nlet c = a + n;\n",
            "machine integers do not mix",
        ),
        // A literal that does not fit the width it is used with. The literal
        // itself is fine — `a + 1` compiles now, at the width — and this is the
        // range check that comes with having a width at all.
        (
            "let a: u8 = 5;\nlet c = a + 300;\n",
            "out of range for the machine integer",
        ),
        // Two machine integers of different widths.
        (
            "let a: u8 = 5;\nlet b: u16 = 3;\nlet c = a + b;\n",
            "machine integer operands must have the same type",
        ),
        // A literal that does not fit the width it was given.
        ("let a: u8 = 300;\n", "out of range"),
    ] {
        // Parsed and *type-checked*, which is the path `lk FILE` takes.
        // `execute_source` skips the checker and simply runs, so a program that
        // should be refused executes and the test passes for the wrong reason —
        // this one did, answering 8 for `u8 + Int`.
        let program = crate::syntax::parse_program_source(source, Default::default())
            .unwrap_or_else(|e| panic!("should parse: {source}: {e}"));
        let mut checker = TypeChecker::new();
        let error = program
            .type_check(&mut checker)
            .expect_err(&alloc::format!("should be refused: {source}"))
            .to_string();
        assert!(
            error.contains(expected),
            "expected {expected:?} for {source:?}, got {error}"
        );
    }
}
