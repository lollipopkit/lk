use crate::expr::{Expr, Pattern};
use crate::stmt::{ForPattern, Program, Stmt};
use crate::vm::AllocationRegion;
use crate::vm::compiler::Compiler;
use crate::vm::{Function, Op, Vm, compile_program};
use crate::{op::BinOp, val::Val, vm::context::VmContext};
use crate::{resolve::slots::SlotResolver, vm::EscapeClass};

fn make_add1_function() -> Stmt {
    Stmt::Function {
        name: "add1".to_string(),
        params: vec!["x".to_string()],
        param_types: Vec::new(),
        return_type: None,
        body: Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Bin(
                Box::new(Expr::Var("x".to_string())),
                BinOp::Add,
                Box::new(Expr::Val(Val::Int(1))),
            ))),
        }),
        named_params: Vec::new(),
    }
}

fn make_const_let(name: &str, value: Val, is_const: bool) -> Stmt {
    Stmt::Let {
        pattern: Pattern::Variable(name.to_string()),
        type_annotation: None,
        value: Box::new(Expr::Val(value)),
        span: None,
        is_const,
    }
}

fn make_assign(name: &str, value: Expr) -> Stmt {
    Stmt::Assign {
        name: name.to_string(),
        value: Box::new(value),
        span: None,
    }
}

#[test]
fn compile_program_executes_expected_result() {
    let program = Program::new(vec![
        Box::new(Stmt::Define {
            name: "x".to_string(),
            value: Box::new(Expr::Val(Val::Int(40))),
        }),
        Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Bin(
                Box::new(Expr::Var("x".to_string())),
                BinOp::Add,
                Box::new(Expr::Val(Val::Int(2))),
            ))),
        }),
    ])
    .expect("program");

    let func = compile_program(&program);
    let mut vm_env = VmContext::new();
    let mut vm = Vm::new();
    let vm_val = vm.exec_with(&func, &mut vm_env, None).expect("vm exec");

    assert_eq!(vm_val, Val::Int(42));
}

fn compile_and_run_with_ctx(
    stmts: Vec<Stmt>,
    setup: impl FnOnce(&mut VmContext),
) -> (Function, VmContext, anyhow::Result<Val>) {
    let program = Program::new(stmts.into_iter().map(|stmt| Box::new(stmt)).collect()).expect("program");
    let function = compile_program(&program);
    let mut ctx = VmContext::new();
    setup(&mut ctx);
    let mut vm = Vm::new();
    let result = vm.exec_with(&function, &mut ctx, None);
    (function, ctx, result)
}

fn compile_and_run(stmts: Vec<Stmt>) -> (Function, VmContext, anyhow::Result<Val>) {
    compile_and_run_with_ctx(stmts, |_| {})
}

#[test]
fn const_function_call_is_evaluated() {
    let stmt_result = Stmt::Let {
        pattern: Pattern::Variable("result".to_string()),
        type_annotation: None,
        value: Box::new(Expr::Call(
            "add1".to_string(),
            vec![Box::new(Expr::Var("n".to_string()))],
        )),
        span: None,
        is_const: true,
    };
    let (function, _ctx, result) = compile_and_run(vec![
        make_add1_function(),
        make_const_let("n", Val::Int(10), true),
        stmt_result,
        Stmt::Return {
            value: Some(Box::new(Expr::Var("result".to_string()))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(11));
    assert!(function.consts.contains(&Val::Int(11)));
    assert!(
        !function
            .code
            .iter()
            .any(|op| matches!(op, Op::Call { .. } | Op::CallNamed { .. })),
        "call opcode should be eliminated for const-evaluated function call"
    );
}

#[test]
fn constant_for_loop_is_precomputed() {
    let iterable = Expr::Range {
        start: Some(Box::new(Expr::Val(Val::Int(0)))),
        end: Some(Box::new(Expr::Var("iters".to_string()))),
        inclusive: false,
        step: None,
    };
    let loop_body = Stmt::Block {
        statements: vec![Box::new(make_assign(
            "acc",
            Expr::Call("add1".to_string(), vec![Box::new(Expr::Var("base".to_string()))]),
        ))],
    };
    let loop_stmt = Stmt::For {
        pattern: ForPattern::Ignore,
        iterable: Box::new(iterable),
        body: Box::new(loop_body),
    };
    let (function, _ctx, _result) = compile_and_run(vec![
        make_add1_function(),
        make_const_let("iters", Val::Int(3), true),
        make_const_let("base", Val::Int(41), true),
        make_const_let("acc", Val::Int(0), false),
        loop_stmt,
        Stmt::Return {
            value: Some(Box::new(Expr::Var("acc".to_string()))),
        },
    ]);
    assert!(
        !function.code.iter().any(|op| matches!(
            op,
            Op::ForRangePrep { .. } | Op::ForRangeLoop { .. } | Op::ForRangeStep { .. } | Op::ToIter { .. }
        )),
        "range loop should be precomputed"
    );
    assert!(
        function.code.iter().any(|op| match op {
            Op::DefineGlobal(name_idx, _) => matches!(
                function.consts.get(*name_idx as usize),
                Some(Val::Str(s)) if s.as_ref() == "acc"
            ),
            _ => false,
        }),
        "acc should be defined via const precomputation"
    );
}

#[test]
fn mutable_let_precomputes_expression() {
    let let_stmt = Stmt::Let {
        pattern: Pattern::Variable("value".to_string()),
        type_annotation: None,
        value: Box::new(Expr::Call("add1".to_string(), vec![Box::new(Expr::Val(Val::Int(41)))])),
        span: None,
        is_const: false,
    };
    let (function, _ctx, result) = compile_and_run(vec![
        make_add1_function(),
        let_stmt,
        Stmt::Return {
            value: Some(Box::new(Expr::Var("value".to_string()))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(42));
    assert!(function.consts.contains(&Val::Int(42)));
    assert!(
        !function
            .code
            .iter()
            .any(|op| matches!(op, Op::Call { .. } | Op::CallNamed { .. })),
        "call opcode should be eliminated for constant expression"
    );
}

#[test]
fn assign_updates_const_environment_when_expression_constant() {
    let (function, _ctx, result) = compile_and_run(vec![
        make_const_let("counter", Val::Int(10), false),
        make_assign("counter", Expr::Val(Val::Int(20))),
        Stmt::Return {
            value: Some(Box::new(Expr::Var("counter".to_string()))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(20));
    assert!(function.consts.contains(&Val::Int(20)));
    assert!(
        !function
            .code
            .iter()
            .any(|op| matches!(op, Op::Call { .. } | Op::CallNamed { .. })),
        "constant assignment should not emit calls"
    );
}

#[test]
fn add_int_imm_emitted_for_small_literal() {
    let compiler = Compiler::new();
    let body = Stmt::Return {
        value: Some(Box::new(Expr::Bin(
            Box::new(Expr::Var("x".to_string())),
            BinOp::Add,
            Box::new(Expr::Val(Val::Int(1))),
        ))),
    };
    let function = compiler.compile_function(&["x".to_string()], &[], &body);
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddIntImm(_, _, imm) if *imm == 1)),
        "expected AddIntImm in compiled code"
    );
}

#[test]
fn cmp_lt_imm_emitted_for_small_literal() {
    let compiler = Compiler::new();
    let body = Stmt::Return {
        value: Some(Box::new(Expr::Bin(
            Box::new(Expr::Var("x".to_string())),
            BinOp::Lt,
            Box::new(Expr::Val(Val::Int(8))),
        ))),
    };
    let function = compiler.compile_function(&["x".to_string()], &[], &body);
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::CmpLtImm(_, _, imm) if *imm == 8)),
        "expected CmpLtImm in compiled code"
    );
}

#[test]
fn block_updates_existing_binding() {
    let block = Stmt::Block {
        statements: vec![Box::new(make_assign("value", Expr::Val(Val::Int(5))))],
    };
    let (function, _ctx, result) = compile_and_run(vec![
        make_const_let("value", Val::Int(1), false),
        block,
        Stmt::Return {
            value: Some(Box::new(Expr::Var("value".to_string()))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(5));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::StoreLocal(_, _))),
        "store opcode should persist"
    );
}

#[test]
fn loop_with_new_binding_falls_back() {
    let iterable = Expr::Range {
        start: Some(Box::new(Expr::Val(Val::Int(0)))),
        end: Some(Box::new(Expr::Var("iters".to_string()))),
        inclusive: false,
        step: None,
    };
    let body = Stmt::Block {
        statements: vec![Box::new(make_const_let("tmp", Val::Int(1), false))],
    };
    let loop_stmt = Stmt::For {
        pattern: ForPattern::Ignore,
        iterable: Box::new(iterable),
        body: Box::new(body),
    };
    let (function, _ctx, result) = compile_and_run(vec![
        make_const_let("iters", Val::Int(2), true),
        make_const_let("acc", Val::Int(0), false),
        loop_stmt,
        Stmt::Return {
            value: Some(Box::new(Expr::Var("acc".to_string()))),
        },
    ]);
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(0));
    assert!(function.code.iter().any(|op| matches!(
        op,
        Op::ForRangePrep { .. } | Op::ForRangeLoop { .. } | Op::ForRangeStep { .. } | Op::ToIter { .. }
    )));
}

#[test]
fn assign_runtime_expression_keeps_env() {
    let (function, _ctx, result) = compile_and_run_with_ctx(
        vec![
            make_const_let("value", Val::Int(1), false),
            make_assign("value", Expr::Var("unknown".to_string())),
            Stmt::Return {
                value: Some(Box::new(Expr::Var("value".to_string()))),
            },
        ],
        |ctx| {
            ctx.define("unknown", Val::Int(5));
        },
    );
    let result = result.expect("vm exec");
    assert_eq!(result, Val::Int(5));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::LoadGlobal(_, _) | Op::LoadLocal(_, _))),
        "runtime assignment should remain"
    );
}

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
        Op::ForRangePrep { .. } | Op::ForRangeLoop { .. } | Op::ForRangeStep { .. } | Op::ToIter { .. }
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
