use crate::expr::{Expr, Pattern};
use crate::stmt::{ForPattern, Program, Stmt, stmt_parser::StmtParser};
use crate::token::Tokenizer;
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

fn make_let(name: &str, value: Expr, is_const: bool) -> Stmt {
    Stmt::Let {
        pattern: Pattern::Variable(name.to_string()),
        type_annotation: None,
        value: Box::new(value),
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
    let program = Program::new(stmts.into_iter().map(Box::new).collect()).expect("program");
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

fn parse_compile_and_run(source: &str) -> (Function, VmContext, anyhow::Result<Val>) {
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let mut ctx = VmContext::new();
    let mut vm = Vm::new();
    let result = vm.exec_with(&function, &mut ctx, None);
    (function, ctx, result)
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
                Some(v) if v.as_str() == Some("acc")
            ),
            _ => false,
        }),
        "acc should be defined via const precomputation"
    );
}

#[test]
fn constant_for_loop_precomputes_compound_assign() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        let iters = 10;
        let count = 0;
        for _ in 1..=iters {
            count += 1;
        }
        return count;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(10));
    assert!(
        !function.code.iter().any(|op| matches!(
            op,
            Op::ForRangePrep { .. } | Op::ForRangeLoop { .. } | Op::ForRangeStep { .. } | Op::ToIter { .. }
        )),
        "constant compound-assign loop should be precomputed"
    );
}

#[test]
fn runtime_ignored_range_counter_uses_count_accumulator() {
    let iterable = Expr::Range {
        start: Some(Box::new(Expr::Val(Val::Int(1)))),
        end: Some(Box::new(Expr::Var("limit".to_string()))),
        inclusive: true,
        step: None,
    };
    let loop_stmt = Stmt::For {
        pattern: ForPattern::Ignore,
        iterable: Box::new(iterable),
        body: Box::new(Stmt::Block {
            statements: vec![Box::new(Stmt::CompoundAssign {
                name: "count".to_string(),
                op: BinOp::Add,
                value: Box::new(Expr::Val(Val::Int(1))),
                span: None,
            })],
        }),
    };
    let (function, _ctx, result) = compile_and_run_with_ctx(
        vec![
            make_const_let("count", Val::Int(0), false),
            loop_stmt,
            Stmt::Return {
                value: Some(Box::new(Expr::Var("count".to_string()))),
            },
        ],
        |ctx| {
            ctx.define("limit", Val::Int(4));
        },
    );

    assert_eq!(result.expect("vm exec"), Val::Int(4));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::AddRangeCountImm { .. })),
        "ignored counter loop should compile to AddRangeCountImm"
    );
    assert!(
        !function.code.iter().any(|op| matches!(
            op,
            Op::ForRangePrep { .. } | Op::ForRangeLoop { .. } | Op::ForRangeStep { .. }
        )),
        "counter loop should not keep per-iteration range opcodes"
    );
}

#[test]
fn varying_numeric_loop_remains_packable_mod_add_range_loop() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        let limit = 10;
        let acc = 0;
        for i in 1..=limit {
            acc += i % 7;
        }
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(27));
    assert!(
        function.code32.is_some(),
        "varying numeric loop should stay on the BC32 packed path"
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::ModInt(..)))
            && function.code.iter().any(|op| matches!(op, Op::AddInt(..))),
        "varying numeric loop should keep real per-iteration mod and add work"
    );
}

#[test]
fn range_count_accumulator_does_not_touch_target_on_zero_iterations() {
    let iterable = Expr::Range {
        start: Some(Box::new(Expr::Var("limit".to_string()))),
        end: Some(Box::new(Expr::Val(Val::Int(5)))),
        inclusive: false,
        step: None,
    };
    let loop_stmt = Stmt::For {
        pattern: ForPattern::Ignore,
        iterable: Box::new(iterable),
        body: Box::new(Stmt::CompoundAssign {
            name: "value".to_string(),
            op: BinOp::Add,
            value: Box::new(Expr::Val(Val::Int(1))),
            span: None,
        }),
    };
    let (function, _ctx, result) = compile_and_run_with_ctx(
        vec![
            make_const_let("value", Val::from_str("unchanged"), false),
            loop_stmt,
            Stmt::Return {
                value: Some(Box::new(Expr::Var("value".to_string()))),
            },
        ],
        |ctx| {
            ctx.define("limit", Val::Int(5));
        },
    );

    assert_eq!(result.expect("vm exec"), Val::from_str("unchanged"));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::AddRangeCountImm { .. })),
        "zero-iteration counter loop should still use aggregate opcode"
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
fn compound_assign_folds_const_function_rhs_to_add_int_imm() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn one() { return 1; }
        let acc = 0;
        acc += one();
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(1));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "const function RHS should be folded before compound assignment lowering"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddIntImm(_, _, imm) if *imm == 1)),
        "expected compound assignment to use AddIntImm"
    );
}

#[test]
fn self_assign_folds_const_function_rhs_to_add_int_imm() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn one() { return 1; }
        let acc = 0;
        acc = acc + one();
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(1));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "const function RHS should be folded before self-assignment lowering"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddIntImm(_, _, imm) if *imm == 1)),
        "expected self assignment to use AddIntImm"
    );
}

#[test]
fn self_assign_inlines_simple_function_call_to_add_int_imm() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn add(a, b) { return a + b; }
        let acc = 0;
        acc = add(acc, 1);
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(1));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "simple pure self-assignment call should be inlined"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddIntImm(_, _, imm) if *imm == 1)),
        "expected inlined function call to use AddIntImm"
    );
}

#[test]
fn self_assign_inlines_simple_function_call_with_dynamic_rhs() {
    let (function, _ctx, result) = compile_and_run_with_ctx(
        vec![
            Stmt::Function {
                name: "add".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                param_types: Vec::new(),
                return_type: None,
                named_params: vec![],
                body: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Bin(
                        Box::new(Expr::Var("a".to_string())),
                        BinOp::Add,
                        Box::new(Expr::Var("b".to_string())),
                    ))),
                }),
            },
            make_const_let("acc", Val::Int(0), false),
            make_let("step", Expr::Var("unknown_step".to_string()), false),
            make_assign(
                "acc",
                Expr::Call(
                    "add".to_string(),
                    vec![
                        Box::new(Expr::Var("acc".to_string())),
                        Box::new(Expr::Var("step".to_string())),
                    ],
                ),
            ),
            Stmt::Return {
                value: Some(Box::new(Expr::Var("acc".to_string()))),
            },
        ],
        |ctx| {
            ctx.define("unknown_step", Val::Int(2));
        },
    );

    assert_eq!(result.expect("vm exec"), Val::Int(2));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "simple self-assignment call with dynamic RHS should inline without runtime Call"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::Add(_, _, _) | Op::AddInt(_, _, _))),
        "expected inlined dynamic call to emit direct addition"
    );
}

#[test]
fn dynamic_self_call_inline_keeps_generic_add_when_type_is_unknown() {
    let (function, _ctx, result) = compile_and_run_with_ctx(
        vec![
            Stmt::Function {
                name: "add".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                param_types: Vec::new(),
                return_type: None,
                named_params: vec![],
                body: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Bin(
                        Box::new(Expr::Var("a".to_string())),
                        BinOp::Add,
                        Box::new(Expr::Var("b".to_string())),
                    ))),
                }),
            },
            make_let("acc", Expr::Var("prefix".to_string()), false),
            make_let("step", Expr::Var("suffix".to_string()), false),
            make_assign(
                "acc",
                Expr::Call(
                    "add".to_string(),
                    vec![
                        Box::new(Expr::Var("acc".to_string())),
                        Box::new(Expr::Var("step".to_string())),
                    ],
                ),
            ),
            Stmt::Return {
                value: Some(Box::new(Expr::Var("acc".to_string()))),
            },
        ],
        |ctx| {
            ctx.define("prefix", Val::from_str("a"));
            ctx.define("suffix", Val::from_str("b"));
        },
    );

    assert_eq!(result.expect("vm exec"), Val::from_str("ab"));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "generic dynamic self-call should still inline the call"
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::Add(_, _, _))),
        "unknown dynamic operand types must use generic Add, not AddInt"
    );
}

#[test]
fn self_assign_inlines_no_capture_closure_call_to_add_int_imm() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        let inc = |x| x + 1;
        let acc = 0;
        acc = inc(acc);
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(1));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "no-capture closure self-assignment call should be inlined"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddIntImm(_, _, imm) if *imm == 1)),
        "expected inlined closure call to use AddIntImm"
    );
}

#[test]
fn self_assign_inlines_const_captured_closure_call_to_add_int_imm() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn make_adder(n) { return |x| x + n; }
        let adder = make_adder(1);
        let acc = 0;
        acc = adder(acc);
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(1));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "const-captured closure self-assignment call should be inlined"
    );
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::AddIntImm(_, _, imm) if *imm == 1)),
        "expected inlined captured closure call to use AddIntImm"
    );
}

#[test]
fn block_inlines_immediate_dynamic_captured_closure_call() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn make_adder(n) { return |x| x + n; }
        let acc = 0;
        let i = 7;
        {
            let adder = make_adder(i);
            acc = adder(acc);
        }
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(7));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "immediate dynamic captured closure creation+call should be inlined"
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::AddInt(_, _, _))),
        "expected dynamic captured closure call to use AddInt"
    );
}

#[test]
fn assignment_uses_simple_const_function_result_without_runtime_call() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn forty_two() { return 42; }
        let acc = 0;
        acc = forty_two();
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "assignment from a safe constant function call should not emit runtime Call"
    );
    assert!(
        function.consts.contains(&Val::Int(42)),
        "expected folded function result in constant pool"
    );
}

#[test]
fn mutable_known_arg_allows_safe_function_call_fold() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn add1(x) { return x + 1; }
        let n = 41;
        let result = add1(n);
        return result;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "safe call with currently-known mutable argument should fold"
    );
    assert!(
        function.consts.contains(&Val::Int(42)),
        "expected folded function result in constant pool"
    );
}

#[test]
fn runtime_assignment_invalidates_known_mutable_binding() {
    let (function, _ctx, result) = compile_and_run_with_ctx(
        vec![
            make_add1_function(),
            make_const_let("n", Val::Int(41), false),
            make_assign("n", Expr::Var("unknown".to_string())),
            Stmt::Let {
                pattern: Pattern::Variable("result".to_string()),
                type_annotation: None,
                value: Box::new(Expr::Call(
                    "add1".to_string(),
                    vec![Box::new(Expr::Var("n".to_string()))],
                )),
                span: None,
                is_const: false,
            },
            Stmt::Return {
                value: Some(Box::new(Expr::Var("result".to_string()))),
            },
        ],
        |ctx| {
            ctx.define("unknown", Val::Int(100));
        },
    );

    assert_eq!(result.expect("vm exec"), Val::Int(101));
    assert!(
        !function.consts.contains(&Val::Int(42)),
        "runtime assignment must not fold through a stale known value"
    );
}

#[test]
fn straight_line_known_call_inlines_without_runtime_call() {
    let source = r#"
        fn score(price, qty, discount) {
            let subtotal = price * qty;
            let fee = (subtotal % 17) + 3;
            return subtotal + fee - discount;
        }
        let p = unknown_price;
        let result = score(p, 4, 2);
        return result;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let mut ctx = VmContext::new();
    ctx.define("unknown_price", Val::Int(20));
    let mut vm = Vm::new();
    let result = vm.exec_with(&function, &mut ctx, None);

    assert_eq!(result.expect("vm exec"), Val::Int(93));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "straight-line pure helper call should inline into arithmetic opcodes"
    );
}

#[test]
fn recursive_known_call_folds_with_fuel() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        fn fib(n) {
            if n <= 1 { return n; }
            return fib(n - 1) + fib(n - 2);
        }
        let n = 10;
        let result = fib(n);
        return result;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(55));
    assert!(
        !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "known recursive call should fold before runtime"
    );
}

#[test]
fn recursive_known_call_falls_back_when_fuel_exhausts() {
    let source = r#"
        fn spin(n) { return spin(n + 1); }
        let result = spin(0);
        return result;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);

    assert!(
        function.code.iter().any(|op| matches!(op, Op::Call { .. })),
        "non-terminating recursive const eval should fall back to runtime call"
    );
}

#[test]
fn loop_invariant_safe_call_is_cached_inside_range_loop() {
    let source = r#"
        fn fib(n) {
            if n <= 1 { return n; }
            return fib(n - 1) + fib(n - 2);
        }
        let n = unknown;
        let acc = 0;
        for _ in 1..=5 {
            acc = fib(n);
        }
        return acc;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let mut ctx = VmContext::new();
    ctx.define("unknown", Val::Int(8));
    let mut vm = Vm::new();
    let result = vm.exec_with(&function, &mut ctx, None);

    assert_eq!(result.expect("vm exec"), Val::Int(21));
    let top_level_calls = function.code.iter().filter(|op| matches!(op, Op::Call { .. })).count();
    assert_eq!(
        top_level_calls, 1,
        "loop-invariant safe call should be emitted once and cached across iterations"
    );
}

#[test]
fn loop_call_cache_does_not_cache_target_dependent_args() {
    let (_function, _ctx, result) = parse_compile_and_run(
        r#"
        fn inc(n) { return n + 1; }
        let acc = 0;
        for _ in 1..=3 {
            acc = inc(acc);
        }
        return acc;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(3));
}

#[test]
fn loop_invariant_local_delta_is_cached_inside_range_loop() {
    let source = r#"
        let width = unknown;
        let total = 0;
        for _ in 1..=5 {
            let sum = 0;
            for i in 1..=width {
                sum += i;
            }
            total += sum;
        }
        return total;
        "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let mut ctx = VmContext::new();
    ctx.define("unknown", Val::Int(4));
    let mut vm = Vm::new();
    let result = vm.exec_with(&function, &mut ctx, None);

    assert_eq!(result.expect("vm exec"), Val::Int(50));
    let for_range_loops = function
        .code
        .iter()
        .filter(|op| matches!(op, Op::ForRangeLoop { .. }))
        .count();
    assert!(
        for_range_loops >= 2,
        "outer loop plus cached one-time local computation should both remain in bytecode"
    );
}

#[test]
fn loop_delta_cache_does_not_cache_target_dependent_prefix() {
    let (_function, _ctx, result) = parse_compile_and_run(
        r#"
        let total = 0;
        for _ in 1..=3 {
            let delta = total + 1;
            total += delta;
        }
        return total;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(7));
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
fn loop_with_only_local_bindings_is_elided() {
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
    assert!(
        !function.code.iter().any(|op| matches!(
            op,
            Op::ForRangePrep { .. } | Op::ForRangeLoop { .. } | Op::ForRangeStep { .. } | Op::ToIter { .. }
        )),
        "local-only ignored loop should be elided"
    );
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
fn stdlib_map_get_on_known_local_map_lowers_to_access() {
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
        function.code.iter().any(|op| matches!(op, Op::Access(_, _, _))),
        "expected stdlib map.get(data, key) to lower to Access in {:?}",
        function.code
    );
}

#[test]
fn stdlib_map_get_literal_key_lowers_to_accessk() {
    let source = r#"
        import map;
        let data = {};
        data.set("answer", 42);
        return map.get(data, "answer");
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::AccessK(_, _, _))),
        "expected stdlib map.get(data, \"answer\") to lower to AccessK in {:?}",
        function.code
    );
}

#[test]
fn stdlib_map_set_on_known_local_map_lowers_to_mapset() {
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
            .any(|op| matches!(op, Op::MapSet { .. } | Op::MapSetMove { .. })),
        "expected stdlib map.set(data, \"answer\", 42) to lower to MapSet in {:?}",
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
        .filter_map(|(idx, op)| matches!(op, Op::ForRangeLoop { .. }).then_some(idx))
        .nth(1)
        .expect("expected nested range loop");
    assert!(
        mod_pos < inner_loop_pos,
        "expected invariant modulo before inner loop guard in {:?}",
        function.code
    );
}
