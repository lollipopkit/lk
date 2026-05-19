use crate::{
    expr::Expr,
    op::BinOp,
    stmt::Stmt,
    val::Val,
    vm::{Op, compiler::Compiler},
};

#[test]
fn known_global_method_return_feeds_typed_arithmetic() {
    let function = Compiler::new().compile_stmt(&Stmt::Block {
        statements: vec![
            Box::new(Stmt::Let {
                pattern: crate::expr::Pattern::Variable("seed".to_string()),
                type_annotation: None,
                value: Box::new(Expr::CallExpr(
                    Box::new(Expr::Access(
                        Box::new(Expr::Var("os".to_string())),
                        Box::new(Expr::Val(Val::from_str("epoch"))),
                    )),
                    Vec::new(),
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Bin(
                    Box::new(Expr::Val(Val::Int(100))),
                    BinOp::Add,
                    Box::new(Expr::Bin(
                        Box::new(Expr::Var("seed".to_string())),
                        BinOp::Sub,
                        Box::new(Expr::Var("seed".to_string())),
                    )),
                ))),
            }),
        ],
    });

    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::CallGlobalMethod0 { .. })),
        "zero-arg global method call should lower directly to CallGlobalMethod0 in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::SubInt(_, _, _))),
        "known os.epoch Int return should feed typed subtraction in {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "known os.epoch Int return should feed typed addition in {:?}",
        function.code
    );
}

#[test]
fn known_global_clock_return_feeds_typed_float_arithmetic() {
    let function = Compiler::new().compile_stmt(&Stmt::Block {
        statements: vec![
            Box::new(Stmt::Let {
                pattern: crate::expr::Pattern::Variable("t0".to_string()),
                type_annotation: None,
                value: Box::new(Expr::CallExpr(
                    Box::new(Expr::Access(
                        Box::new(Expr::Var("os".to_string())),
                        Box::new(Expr::Val(Val::from_str("clock"))),
                    )),
                    Vec::new(),
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Let {
                pattern: crate::expr::Pattern::Variable("t1".to_string()),
                type_annotation: None,
                value: Box::new(Expr::CallExpr(
                    Box::new(Expr::Access(
                        Box::new(Expr::Var("os".to_string())),
                        Box::new(Expr::Val(Val::from_str("clock"))),
                    )),
                    Vec::new(),
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Bin(
                    Box::new(Expr::Bin(
                        Box::new(Expr::Var("t1".to_string())),
                        BinOp::Sub,
                        Box::new(Expr::Var("t0".to_string())),
                    )),
                    BinOp::Mul,
                    Box::new(Expr::Val(Val::Float(1000.0))),
                ))),
            }),
        ],
    });

    assert!(
        function
            .code
            .iter()
            .filter(|op| matches!(op, Op::CallGlobalMethod0 { .. }))
            .count()
            >= 2,
        "zero-arg os.clock calls should lower directly to CallGlobalMethod0 in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::SubFloat(_, _, _))),
        "known os.clock Float return should feed typed subtraction in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MulFloat(_, _, _))),
        "known os.clock Float return should feed typed multiplication in {:?}",
        function.code
    );
}

#[test]
fn direct_call_infers_global_clock_float_arguments() {
    let function = Compiler::new().compile_stmt(&Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "elapsed_ms".to_string(),
                params: vec!["t0".to_string(), "t1".to_string()],
                param_types: Vec::new(),
                return_type: None,
                named_params: Vec::new(),
                body: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Bin(
                        Box::new(Expr::Bin(
                            Box::new(Expr::Var("t1".to_string())),
                            BinOp::Sub,
                            Box::new(Expr::Var("t0".to_string())),
                        )),
                        BinOp::Mul,
                        Box::new(Expr::Val(Val::Float(1000.0))),
                    ))),
                }),
            }),
            Box::new(Stmt::Let {
                pattern: crate::expr::Pattern::Variable("t0".to_string()),
                type_annotation: None,
                value: Box::new(Expr::CallExpr(
                    Box::new(Expr::Access(
                        Box::new(Expr::Var("os".to_string())),
                        Box::new(Expr::Val(Val::from_str("clock"))),
                    )),
                    Vec::new(),
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Let {
                pattern: crate::expr::Pattern::Variable("t1".to_string()),
                type_annotation: None,
                value: Box::new(Expr::CallExpr(
                    Box::new(Expr::Access(
                        Box::new(Expr::Var("os".to_string())),
                        Box::new(Expr::Val(Val::from_str("clock"))),
                    )),
                    Vec::new(),
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Call(
                    "elapsed_ms".to_string(),
                    vec![
                        Box::new(Expr::Var("t0".to_string())),
                        Box::new(Expr::Var("t1".to_string())),
                    ],
                ))),
            }),
        ],
    });

    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled elapsed_ms proto");
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::SubFloat(_, _, _))),
        "direct-call os.clock argument facts should feed typed subtraction in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MulFloat(_, _, _))),
        "direct-call os.clock argument facts should feed typed multiplication in {:?}",
        proto.code
    );
}
