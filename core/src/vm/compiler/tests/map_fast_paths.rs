use super::parse_compile_and_run;
use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::Stmt,
    val::Val,
    vm::{Op, Vm, compiler::FunctionBuilder, context::VmContext},
};

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
fn homogeneous_int_map_get_feeds_typed_arithmetic() {
    let source = r#"
        import map;
        let data = {"low": 2, "high": 40};
        return map.get(data, "high") + 2;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapGetInterned(_, _, _))),
        "expected map.get on literal key to use MapGetInterned in {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "expected homogeneous Int map value fact to feed typed add in {:?}",
        function.code
    );
}

#[test]
fn homogeneous_int_map_get_feeds_chained_typed_arithmetic() {
    let source = r#"
        import map;
        let data = {"low": 2, "high": 40};
        return 1 + map.get(data, "high") + "x".len();
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
        "expected map.get in a chained numeric expression to keep typed adds in {:?}",
        function.code
    );
}

#[test]
fn map_set_invalidates_homogeneous_value_fact() {
    let body = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("data".to_string()),
                type_annotation: None,
                value: Box::new(Expr::Map(vec![(
                    Box::new(Expr::Val(Val::from_str("x"))),
                    Box::new(Expr::Val(Val::Int(1))),
                )])),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Expr(Box::new(Expr::CallExpr(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("data".to_string())),
                    Box::new(Expr::Val(Val::from_str("set"))),
                )),
                vec![
                    Box::new(Expr::Val(Val::from_str("y"))),
                    Box::new(Expr::Val(Val::from_str("text"))),
                ],
            )))),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("inc".to_string()),
                type_annotation: None,
                value: Box::new(Expr::Val(Val::Int(1))),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Bin(
                    Box::new(Expr::CallExpr(
                        Box::new(Expr::Access(
                            Box::new(Expr::Var("data".to_string())),
                            Box::new(Expr::Val(Val::from_str("get"))),
                        )),
                        vec![Box::new(Expr::Val(Val::from_str("x")))],
                    )),
                    BinOp::Add,
                    Box::new(Expr::Var("inc".to_string())),
                ))),
            }),
        ],
    };
    let mut builder = FunctionBuilder::new();
    builder.stmt(&body);
    let function = builder.finish();
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let result = vm.exec_with(&function, &mut ctx, None);

    assert_eq!(result.expect("vm exec"), Val::Int(2));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapSetInterned(_, _, _))),
        "expected map.set literal key to use MapSetInterned in {:?}",
        function.code
    );
    assert!(
        !function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "map.set should invalidate homogeneous value facts before later add in {:?}",
        function.code
    );
}

#[test]
fn empty_map_set_adopts_homogeneous_int_fact() {
    let source = r#"
        let data = {};
        data.set("x", 40);
        data.set("y", 1);
        return data.get("x") + data.get("y") + 1;
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
        "empty map followed by homogeneous Int sets should feed typed adds in {:?}",
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
