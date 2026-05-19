use std::sync::Arc;

use crate::resolve::slots::SlotResolver;
use crate::util::fast_map::fast_hash_map_with_capacity;
use crate::vm::{AllocationRegion, EscapeClass};

use super::{
    BinOp, Compiler, Expr, ForPattern, Op, Stmt, Val, compile_and_run, compile_and_run_with_ctx, make_assign,
    make_const_let, parse_compile_and_run,
};

#[test]
fn assign_to_const_binding_falls_back_without_modify() {
    let (_, _ctx, result) = compile_and_run(vec![
        make_const_let("counter", Val::Int(5), true),
        make_assign("counter", Expr::Val(Val::Int(7))),
        Stmt::Return {
            value: Some(Box::new(Expr::Var("counter".to_string()))),
        },
    ]);
    let err = result.expect_err("assigning to const binding should fail at runtime");
    assert!(
        err.to_string().to_lowercase().contains("const"),
        "error should mention const binding, got {err:?}"
    );
}

#[test]
fn return_local_reads_slot_without_loadlocal_copy() {
    let function = Compiler::new().compile_function(
        &["value".to_string()],
        &[],
        &Stmt::Return {
            value: Some(Box::new(Expr::Var("value".to_string()))),
        },
    );
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::LoadLocal(_, _))),
        "returning a local should not copy it through a temporary LoadLocal"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::Ret { base, retc: 1 } if *base == function.param_regs[0])),
        "Ret should read directly from the parameter slot"
    );
}

#[test]
fn local_condition_branches_without_loadlocal_copy() {
    let function = Compiler::new().compile_function(
        &["flag".to_string()],
        &[],
        &Stmt::If {
            condition: Box::new(Expr::Var("flag".to_string())),
            then_stmt: Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Val(Val::Int(1)))),
            }),
            else_stmt: Some(Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Val(Val::Int(2)))),
            })),
        },
    );
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::LoadLocal(_, _))),
        "branching on a local should not copy it through a temporary LoadLocal"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::BoolBranch(reg, _) if *reg == function.param_regs[0])),
        "BoolBranch should read directly from the parameter slot"
    );
}

#[test]
fn function_local_let_does_not_export_global() {
    let function = Compiler::new().compile_function(
        &["input".to_string()],
        &[],
        &Stmt::Block {
            statements: vec![
                Box::new(Stmt::Let {
                    pattern: crate::expr::Pattern::Variable("local".to_string()),
                    type_annotation: None,
                    value: Box::new(Expr::Var("input".to_string())),
                    span: None,
                    is_const: false,
                }),
                Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Var("local".to_string()))),
                }),
            ],
        },
    );

    assert!(
        function.code.iter().all(|op| !matches!(op, Op::DefineGlobal(_, _))),
        "function-local let should stay in frame slots, not sync through globals: {:?}",
        function.code
    );
}

#[test]
fn for_loop_using_index_falls_back_to_runtime() {
    let iterable = Expr::Range {
        start: Some(Box::new(Expr::Val(Val::Int(0)))),
        end: Some(Box::new(Expr::Var("iters".to_string()))),
        inclusive: false,
        step: None,
    };
    let loop_stmt = Stmt::For {
        pattern: ForPattern::Variable("i".to_string()),
        iterable: Box::new(iterable),
        body: Box::new(Stmt::Block {
            statements: vec![Box::new(make_assign(
                "acc",
                Expr::Bin(
                    Box::new(Expr::Var("acc".to_string())),
                    BinOp::Add,
                    Box::new(Expr::Var("i".to_string())),
                ),
            ))],
        }),
    };
    let (function, _ctx, result) = compile_and_run(vec![
        make_const_let("iters", Val::Int(3), true),
        make_const_let("acc", Val::Int(0), false),
        loop_stmt,
        Stmt::Return {
            value: Some(Box::new(Expr::Var("acc".to_string()))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(3));
    assert!(function.code.iter().any(|op| matches!(
        op,
        Op::ForRangePrep { .. } | Op::RangeLoopI { .. } | Op::ForRangeStep { .. } | Op::ToIter { .. }
    )));
}

#[test]
fn slot_coherence_matches_resolver() {
    use crate::stmt::NamedParamDecl;

    let compiler = Compiler::new();
    let params = vec!["x".to_string(), "y".to_string()];
    let named_params = vec![NamedParamDecl {
        name: "limit".to_string(),
        type_annotation: None,
        default: Some(Expr::Val(Val::Int(10))),
    }];
    let body = Stmt::Return {
        value: Some(Box::new(Expr::Bin(
            Box::new(Expr::Var("x".to_string())),
            BinOp::Add,
            Box::new(Expr::Var("limit".to_string())),
        ))),
    };

    let function = compiler.compile_function(&params, &named_params, &body);

    let mut resolver = SlotResolver::new();
    let layout = resolver.resolve_function_slots(&params, &named_params, &body);

    for (idx, param) in params.iter().enumerate() {
        let expected = layout
            .decls
            .iter()
            .find(|decl| decl.name == *param && decl.is_param)
            .map(|decl| decl.index)
            .expect("param index");
        assert_eq!(
            function.param_regs[idx], expected,
            "param register mismatch for {}",
            param
        );
    }

    for (idx, decl) in named_params.iter().enumerate() {
        let expected = layout
            .decls
            .iter()
            .find(|d| d.name == decl.name && d.is_param)
            .map(|d| d.index)
            .expect("named param index");
        assert_eq!(
            function.named_param_regs[idx], expected,
            "named param register mismatch for {}",
            decl.name
        );
    }
}

#[test]
fn ssa_pipeline_smoke_test() {
    let compiler = Compiler::new();
    let expr = Expr::Call(
        "make".to_string(),
        vec![Box::new(Expr::Val(Val::Int(1))), Box::new(Expr::Val(Val::Int(2)))],
    );

    let func = compiler.compile_expr(&expr);
    let analysis = func.analysis.as_ref().expect("expected SSA analysis artifacts");

    assert!(
        analysis.ssa.is_some(),
        "SSA lowering should succeed for call expression"
    );
    assert_eq!(
        analysis.escape.return_class,
        EscapeClass::Escapes,
        "call expression should be classified as escaping"
    );
    assert!(
        !analysis.escape.escaping_values.is_empty(),
        "escape summary should record escaping values"
    );
    assert_eq!(
        analysis.region_plan.return_region,
        AllocationRegion::Heap,
        "escaping return should reserve heap allocation"
    );
}

#[test]
fn region_plan_marks_trivial_expr_as_thread_local() {
    let compiler = Compiler::new();
    let expr = Expr::Val(Val::Int(5));
    let func = compiler.compile_expr(&expr);
    let analysis = func.analysis.expect("analysis available");
    assert!(
        analysis.escape.escaping_values.is_empty(),
        "constant expression must not escape"
    );
    assert_eq!(analysis.region_plan.return_region, AllocationRegion::ThreadLocal);
}

#[test]
fn list_sum_loop_uses_fold_add_opcode() {
    let source = r#"
        let list = [];
        for i in 1..=4 {
            list.push(i);
        }
        let sum = 0;
        for v in list {
            sum += v;
        }
        return sum;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(10));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListFoldAdd { .. })),
        "expected list fold opcode in {:?}",
        function.code
    );
}

#[test]
fn string_contains_literal_lowers_to_containsk() {
    let source = r#"
        let line = "alpha-beta";
        return line.contains("ha-b");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Bool(true));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ContainsK(_, _, _))),
        "expected str.contains(\"literal\") to lower to ContainsK in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "expected contains literal fast path to avoid Call in {:?}",
        function.code
    );
}

#[test]
fn len_on_expression_result_lowers_to_len_without_call() {
    let source = r#"
        let prefix = "ab";
        return (prefix + "cd").len();
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(4));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::Len { .. })),
        "expected expression .len() to lower to Len in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "expected expression .len() fast path to avoid Call in {:?}",
        function.code
    );
}

#[test]
fn split_join_same_separator_lowers_to_original_value() {
    let source = r#"
        let line = "a|b|c";
        return line.split("|").join("|").len();
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(5));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::Len { .. })),
        "expected split/join/len peephole to keep direct Len in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "expected split/join same-separator peephole to avoid method calls in {:?}",
        function.code
    );
}

#[test]
fn dynamic_list_access_stays_current_after_mutation() {
    let source = r#"
        let values = [];
        values.push(10);
        let idx = 0;
        let first = values[idx];
        values.push(32);
        return first + values[1];
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::Access(_, _, _))),
        "expected dynamic list access to use Access in {:?}",
        function.code
    );
}

#[test]
fn zero_arg_method_call_uses_call_method0_opcode() {
    let source = r#"
        let data = { "answer": 42 };
        return data.answer();
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::CallMethod0 { .. } | Op::CallGlobalMethod0 { .. })),
        "expected zero-arg method call to use a method fast opcode in {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .all(|op| !matches!(op, Op::BuildList { len: 0, .. })),
        "CallMethod0 should not build an empty positional arg list in {:?}",
        function.code
    );
}

#[test]
fn zero_arg_global_method_call_fuses_receiver_load() {
    let (function, _ctx, result) = compile_and_run_with_ctx(
        vec![Stmt::Return {
            value: Some(Box::new(Expr::CallExpr(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("module".to_string())),
                    Box::new(Expr::Val(Val::from_str("answer"))),
                )),
                Vec::new(),
            ))),
        }],
        |ctx| {
            let mut module = fast_hash_map_with_capacity(1);
            module.insert("answer".into(), Val::Int(42));
            ctx.set("module".to_string(), Val::Map(Arc::new(module)));
        },
    );

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::CallGlobalMethod0 { .. })),
        "expected global zero-arg method call to fuse LoadGlobal in {:?}",
        function.code
    );
}

#[test]
fn template_string_starts_from_first_literal() {
    let source = r#"
        let i = 42;
        return "key${i}";
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::from_str("key42"));
    assert!(
        !function
            .consts
            .iter()
            .any(|value| matches!(value, Val::Str(s) if s.is_empty())),
        "template lowering should not force an empty leading string in {:?}",
        function.consts
    );
    assert_eq!(
        function.code.iter().filter(|op| matches!(op, Op::Add(_, _, _))).count(),
        1,
        "template lowering should append only once in {:?}",
        function.code
    );
}

#[test]
fn template_string_numeric_expr_uses_direct_concat() {
    let source = r#"
        let r = 42;
        return "key-${r % 7}-${r}";
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::from_str("key-0-42"));
    assert_eq!(
        function.code.iter().filter(|op| matches!(op, Op::ToStr(_, _))).count(),
        0,
        "known-int interpolations should concatenate without ToStr in {:?}",
        function.code
    );
}

#[test]
fn known_int_register_compare_lowers_to_cmp_i() {
    let source = r#"
        let i = 0;
        let limit = 3;
        limit = limit + 0;
        while (i < limit) {
            i = i + 1;
        }
        return i;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(3));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::CmpI { .. })),
        "known int register comparison should lower to CmpI in {:?}",
        function.code
    );
}

#[test]
fn range_loop_hoists_invariant_arithmetic_subexpr() {
    let source = r#"
        let total = 0;
        for outer in 1..=3 {
            let list = [];
            for i in 1..=4 {
                list.push(i + (outer % 2));
            }
            for v in list {
                total += v;
            }
        }
        return total;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(38));
    let mod_pos = function
        .code
        .iter()
        .position(|op| matches!(op, Op::ModInt(_, _, _)))
        .expect("expected invariant modulo");
    let inner_loop_pos = function
        .code
        .iter()
        .enumerate()
        .filter_map(|(idx, op)| matches!(op, Op::RangeLoopI { .. }).then_some(idx))
        .nth(1)
        .expect("expected nested range loop");
    assert!(
        mod_pos < inner_loop_pos,
        "expected invariant modulo before inner loop guard in {:?}",
        function.code
    );
}
