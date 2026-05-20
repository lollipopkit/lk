use std::sync::Arc;

use super::parse_compile_and_run;
use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::Stmt,
    util::fast_map::fast_hash_map_with_capacity,
    val::Val,
    vm::{
        Op, Vm,
        compiler::{Compiler, FunctionBuilder},
        context::VmContext,
    },
};
use arcstr::ArcStr;

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
fn stdlib_map_get_ne_nil_branch_lowers_to_map_has_dynamic() {
    let source = r#"
        import map;
        let data = {};
        let key = "answer";
        data.set(key, 42);
        if map.get(data, key) != nil {
            return 1;
        }
        return 0;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(1));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::MapHas(_, _, _) | Op::MapHasK(_, _, _))),
        "expected presence-only stdlib map.get(data, key) to lower to MapHas/MapHasK in {:?}",
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
fn homogeneous_float_map_get_feeds_typed_arithmetic() {
    let source = r#"
        import map;
        let data = {"low": 1.5, "high": 2.5};
        return map.get(data, "high") * 2.0;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Float(5.0));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapGetInterned(_, _, _))),
        "expected map.get on literal key to use MapGetInterned in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MulFloat(_, _, _))),
        "expected homogeneous Float map value fact to feed typed multiply in {:?}",
        function.code
    );
}

#[test]
fn direct_call_preserves_homogeneous_int_map_param_fact() {
    let source = r#"
        import map;
        fn total(data, key) {
            return map.get(data, key) + 2;
        }
        let prices = {"basic": 40, "pro": 70};
        return total(prices, "basic");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MapGetDynamic(_, _, _))),
        "direct-call Map param should keep map lowering in {:?}",
        proto.code
    );
    assert!(
        proto
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "direct-call Map<Int> param value fact should feed typed add in {:?}",
        proto.code
    );
}

#[test]
fn direct_call_inside_loop_preserves_homogeneous_int_map_param_fact() {
    let source = r#"
        import map;
        fn total(key, qty, data) {
            let price = map.get(data, key);
            return price * qty;
        }
        let prices = {"basic": 21, "pro": 40};
        let sum = 0;
        for i in 1..=2 {
            sum += total("basic", i, prices);
        }
        return sum;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(63));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MapGetDynamic(_, _, _))),
        "loop direct-call Map param should keep map lowering in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MulInt(_, _, _))),
        "loop direct-call Map<Int> value fact should feed typed multiply in {:?}",
        proto.code
    );
}

#[test]
fn direct_call_in_compound_assign_preserves_homogeneous_int_map_param_fact() {
    let source = r#"
        import map;
        fn total(key, qty, data) {
            let price = map.get(data, key);
            return price * qty;
        }
        let prices = {"basic": 21, "pro": 40};
        let sum = 0;
        for i in 1..=2 {
            sum += total("basic", i, prices);
            sum += total("pro", i, prices);
        }
        return sum;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(183));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MapGetDynamic(_, _, _))),
        "compound direct-call Map param should keep map lowering in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MulInt(_, _, _))),
        "compound direct-call Map<Int> value fact should feed typed multiply in {:?}",
        proto.code
    );
}

#[test]
fn direct_call_with_two_map_params_preserves_homogeneous_int_value_facts() {
    let source = r#"
        import map;
        import math;
        fn total(sku, qty, region, prices, taxes) {
            let price = map.get(prices, sku);
            let tax = map.get(taxes, region);
            let subtotal = price * qty;
            return subtotal + math.floor((subtotal * tax) / 100);
        }
        let prices = {"basic": 20, "pro": 40};
        let taxes = {"us": 10, "eu": 20};
        let sum = 0;
        for i in 1..=2 {
            let region = "us";
            sum += total("basic", i, region, prices, taxes);
            sum += total("pro", i, region, prices, taxes);
        }
        return sum;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(198));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto
            .code
            .iter()
            .filter(|op| matches!(op, Op::MapGetDynamic(_, _, _)))
            .count()
            >= 2,
        "direct-call Map params should keep map lowering in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MulInt(_, _, _))),
        "direct-call Map<Int> value facts should feed typed multiply in {:?}",
        proto.code
    );
}

#[test]
fn direct_call_map_param_facts_survive_branching_helper_body() {
    let source = r#"
        import map;
        import math;
        fn total(sku, qty, region, prices, taxes) {
            let price = map.get(prices, sku);
            let tax = map.get(taxes, region);
            let subtotal = price * qty;
            let discount = 0;
            if qty >= 5 {
                discount = math.floor(subtotal / 10);
            } else if sku.starts_with("pro") {
                discount = math.floor(subtotal / 20);
            }
            return subtotal - discount + math.floor((subtotal * tax) / 100);
        }
        let prices = {"basic": 19, "pro": 49};
        let taxes = {"us": 8, "eu": 20};
        let region = "us";
        return total("basic", 4, region, prices, taxes) + total("pro", 6, region, prices, taxes);
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(370));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto
            .code
            .iter()
            .filter(|op| matches!(op, Op::MapGetDynamic(_, _, _)))
            .count()
            >= 2,
        "branching helper should keep map param fast paths in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MulInt(_, _, _))),
        "branching helper map.get Int facts should feed typed multiply in {:?}",
        proto.code
    );
}

#[test]
fn loop_compound_call_map_param_facts_survive_branching_helper_body() {
    let source = r#"
        import map;
        import math;
        fn total(sku, qty, region, prices, taxes) {
            let price = map.get(prices, sku);
            let tax = map.get(taxes, region);
            let subtotal = price * qty;
            let discount = 0;
            if qty >= 5 {
                discount = math.floor(subtotal / 10);
            } else if sku.starts_with("pro") {
                discount = math.floor(subtotal / 20);
            }
            return subtotal - discount + math.floor((subtotal * tax) / 100);
        }
        let prices = {"basic": 19, "pro": 49, "team": 99, "addon": 7};
        let taxes = {"us": 8, "eu": 20, "apac": 12};
        let sum = 0;
        for r in 1..=4 {
            let region = "us";
            if ((r % 3) == 1) {
                region = "eu";
            } else if ((r % 3) == 2) {
                region = "apac";
            }
            sum += total("basic", (r % 6) + 1, region, prices, taxes);
            sum += total("pro", (r % 4) + 1, region, prices, taxes);
            sum += total("team", (r % 3) + 1, region, prices, taxes);
            if ((r % 2) == 0) {
                sum += total("addon", (r % 8) + 1, region, prices, taxes);
            }
        }
        return sum;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(1797));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto
            .code
            .iter()
            .filter(|op| matches!(op, Op::MapGetDynamic(_, _, _)))
            .count()
            >= 2,
        "loop compound calls should keep map param fast paths in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MulInt(_, _, _))),
        "loop compound calls should keep map.get Int facts for typed multiply in {:?}",
        proto.code
    );
}

#[test]
fn pricing_helper_keeps_two_map_param_value_facts() {
    let source = r#"
        import map;
        import math;
        let warm = {};
        warm.set("x", 1);
        let warm_key = "x";
        let warm_value = map.get(warm, warm_key);
        fn cart_line_total(sku, qty, region, prices, tax_rates) {
            let price = map.get(prices, sku);
            let tax = map.get(tax_rates, region);
            let subtotal = price * qty;
            let discount = 0;
            if qty >= 5 {
                discount = math.floor(subtotal / 10);
            } else if sku.starts_with("pro") {
                discount = math.floor(subtotal / 20);
            }
            return subtotal - discount + math.floor((subtotal * tax) / 100);
        }
        let prices = {"basic": 19, "pro": 49, "team": 99, "addon": 7};
        let tax_rates = {"us": 8, "eu": 20, "apac": 12};
        return warm_value + cart_line_total("pro", 6, "us", prices, tax_rates);
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(289));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto
            .code
            .iter()
            .filter(|op| matches!(op, Op::MapGetDynamic(_, _, _)))
            .count()
            >= 2,
        "pricing helper should keep map.get lowering in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().filter(|op| matches!(op, Op::MulInt(_, _, _))).count() >= 2,
        "pricing helper Map<Int> facts should feed both typed multiplies in {:?}",
        proto.code
    );
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::CmpGeImmJmp { imm: 5, .. })),
        "pricing helper qty >= 5 guard should fuse to CmpGeImmJmp in {:?}",
        proto.code
    );
    assert!(
        !proto.code.iter().any(|op| matches!(op, Op::Mul(_, _, _))),
        "pricing helper should not need generic multiply after map param facts in {:?}",
        proto.code
    );
}

#[test]
fn direct_call_infers_folded_map_value_fact() {
    let mut values = fast_hash_map_with_capacity(2);
    values.insert(ArcStr::from("a"), Val::Int(20));
    values.insert(ArcStr::from("b"), Val::Int(21));
    let folded_map = Val::Map(Arc::new(values));
    let body = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "lookup".to_string(),
                params: vec!["data".to_string(), "key".to_string()],
                param_types: Vec::new(),
                return_type: None,
                named_params: Vec::new(),
                body: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Bin(
                        Box::new(Expr::CallExpr(
                            Box::new(Expr::Access(
                                Box::new(Expr::Var("map".to_string())),
                                Box::new(Expr::Val(Val::from_str("get"))),
                            )),
                            vec![
                                Box::new(Expr::Var("data".to_string())),
                                Box::new(Expr::Var("key".to_string())),
                            ],
                        )),
                        BinOp::Add,
                        Box::new(Expr::Val(Val::Int(1))),
                    ))),
                }),
            }),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("data".to_string()),
                type_annotation: None,
                value: Box::new(Expr::Val(folded_map)),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Call(
                    "lookup".to_string(),
                    vec![
                        Box::new(Expr::Var("data".to_string())),
                        Box::new(Expr::Val(Val::from_str("b"))),
                    ],
                ))),
            }),
        ],
    };
    let function = Compiler::new().compile_stmt(&body);
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let result = vm.exec_with(&function, &mut ctx, None);

    assert_eq!(result.expect("vm exec"), Val::Int(22));
    let proto = function
        .protos
        .iter()
        .find_map(|proto| proto.func.as_ref())
        .expect("compiled function proto");
    assert!(
        proto.code.iter().any(|op| matches!(op, Op::MapGetDynamic(_, _, _))),
        "folded Map<Int> direct-call param should keep map.get lowering in {:?}",
        proto.code
    );
    assert!(
        proto
            .code
            .iter()
            .any(|op| matches!(op, Op::AddInt(_, _, _) | Op::AddIntImm(_, _, _))),
        "folded Map<Int> direct-call param should feed typed add in {:?}",
        proto.code
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
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::MapSetInterned(_, _, _) | Op::MapSetInternedMove(_, _, _))),
        "expected map.set literal key to use an interned map set opcode in {:?}",
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
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::MapSetInterned(_, _, _) | Op::MapSetInternedMove(_, _, _))),
        "expected stdlib map.set(data, \"answer\", 42) to lower to an interned map set opcode in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "expected stdlib map.set/data.get fast paths to avoid Call in {:?}",
        function.code
    );
}

#[test]
fn map_set_literal_key_temporary_value_lowers_to_move_set() {
    let source = r#"
        let data = {};
        data.set("sku", "sku-${1}");
        return data.get("sku");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::from_str("sku-1"));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::MapSetInternedMove(_, _, _))),
        "expected literal-key map.set with temporary value to lower to MapSetInternedMove in {:?}",
        function.code
    );
}

#[test]
fn map_set_literal_key_variable_value_keeps_non_move_set() {
    let source = r#"
        let data = {};
        let value = "sku-${1}";
        data.set("sku", value);
        return [data.get("sku"), value];
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    let expected = Val::from_str("sku-1");
    let Val::List(values) = result.expect("vm exec") else {
        panic!("expected list");
    };
    assert_eq!(values.as_slice(), [expected.clone(), expected]);
    assert!(
        function.code.iter().any(|op| matches!(op, Op::MapSetInterned(_, _, _))),
        "expected literal-key map.set with variable value to keep MapSetInterned in {:?}",
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

#[test]
fn map_upsert_with_pure_delta_delays_delta_until_after_nil_branch() {
    let source = r#"
        let stock = {};
        let r = 3;
        for i in 1..=4 {
            let sku = "sku-${i % 2}";
            let current = map.get(stock, sku);
            let delta = ((i * 11) + r) % 37;
            if current == nil {
                stock.set(sku, delta);
            } else {
                stock.set(sku, current + delta);
            }
        }
        return map.get(stock, "sku-1") + map.get(stock, "sku-0");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(85));
    let get_pos = function
        .code
        .iter()
        .position(|op| matches!(op, Op::MapGetDynamic(_, _, _)))
        .expect("expected dynamic map get");
    assert!(
        matches!(function.code.get(get_pos + 1), Some(Op::CmpEq(..))),
        "expected nil compare immediately after MapGetDynamic for packed branch fusion in {:?}",
        function.code
    );
    assert!(
        matches!(
            function.code.get(get_pos + 2),
            Some(Op::BoolBranch(..) | Op::JmpFalse(..))
        ),
        "expected branch immediately after MapGetDynamic nil compare in {:?}",
        function.code
    );
}

#[test]
fn map_upsert_with_default_increment_delays_default_until_after_nil_branch() {
    let source = r#"
        let hist = {};
        for i in 1..=4 {
            let bucket = "b${i % 2}";
            let current = map.get(hist, bucket);
            if current == nil {
                hist.set(bucket, 1);
            } else {
                hist.set(bucket, current + 1);
            }
        }
        return map.get(hist, "b1") + map.get(hist, "b0");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(4));
    let get_pos = function
        .code
        .iter()
        .position(|op| matches!(op, Op::MapGetDynamic(_, _, _)))
        .expect("expected dynamic map get");
    assert!(
        matches!(function.code.get(get_pos + 1), Some(Op::CmpEq(..))),
        "expected nil compare immediately after MapGetDynamic for packed branch fusion in {:?}",
        function.code
    );
    assert!(
        matches!(
            function.code.get(get_pos + 2),
            Some(Op::BoolBranch(..) | Op::JmpFalse(..))
        ),
        "expected branch immediately after MapGetDynamic nil compare in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::AddIntImm(..))),
        "expected current + 1 to lower to AddIntImm in {:?}",
        function.code
    );
}
