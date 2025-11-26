#[cfg(test)]
mod tests {
    use crate::expr::Expr;
    use crate::stmt::{ForPattern, Program, Stmt};
    use crate::val::Val;
    use crate::vm::{Function, Op, Vm, compile_program};
    use crate::{expr::Pattern, op::BinOp, vm::context::VmContext};

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

    fn make_const_let(name: &str, value: Val) -> Stmt {
        Stmt::Let {
            pattern: Pattern::Variable(name.to_string()),
            type_annotation: None,
            value: Box::new(Expr::Val(value)),
            span: None,
            is_const: true,
        }
    }

    fn make_mut_let(name: &str, value: Val) -> Stmt {
        Stmt::Let {
            pattern: Pattern::Variable(name.to_string()),
            type_annotation: None,
            value: Box::new(Expr::Val(value)),
            span: None,
            is_const: false,
        }
    }

    fn make_assign(name: &str, value: Expr) -> Stmt {
        Stmt::Assign {
            name: name.to_string(),
            value: Box::new(value),
            span: None,
        }
    }

    fn compile_and_run(stmts: Vec<Stmt>) -> (Function, VmContext, anyhow::Result<Val>) {
        let program = Program::new(stmts.into_iter().map(|stmt| Box::new(stmt)).collect()).expect("program");
        let function = compile_program(&program);
        let mut ctx = VmContext::new();
        let mut vm = Vm::new();
        let result = vm.exec_with(&function, &mut ctx, None);
        (function, ctx, result)
    }

    #[test]
    fn const_function_call_is_evaluated() {
        let const_result = Stmt::Let {
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
            make_const_let("n", Val::Int(10)),
            const_result,
            Stmt::Return {
                value: Some(Box::new(Expr::Var("result".to_string()))),
            },
        ]);
        let result = result.expect("vm exec");
        assert_eq!(result, Val::Int(11));
        assert!(function.consts.contains(&Val::Int(11)));
        assert!(
            !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
            "call opcode should be eliminated for constant evaluation"
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
            statements: vec![Box::new(Stmt::Assign {
                name: "acc".to_string(),
                value: Box::new(Expr::Call(
                    "add1".to_string(),
                    vec![Box::new(Expr::Var("base".to_string()))],
                )),
                span: None,
            })],
        };
        let loop_stmt = Stmt::For {
            pattern: ForPattern::Ignore,
            iterable: Box::new(iterable),
            body: Box::new(loop_body),
        };
        let (function, _ctx, _) = compile_and_run(vec![
            make_add1_function(),
            make_const_let("iters", Val::Int(3)),
            make_const_let("base", Val::Int(41)),
            make_mut_let("acc", Val::Int(0)),
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
            !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
            "call opcode should be eliminated for constant expression"
        );
    }

    #[test]
    fn assign_updates_const_environment_when_expression_constant() {
        let (function, _ctx, result) = compile_and_run(vec![
            make_mut_let("counter", Val::Int(10)),
            make_assign("counter", Expr::Val(Val::Int(20))),
            Stmt::Return {
                value: Some(Box::new(Expr::Var("counter".to_string()))),
            },
        ]);
        let result = result.expect("vm exec");
        assert_eq!(result, Val::Int(20));
        assert!(function.consts.contains(&Val::Int(20)));
        assert!(
            !function.code.iter().any(|op| matches!(op, Op::Call { .. })),
            "constant assignment should not emit calls"
        );
    }
}
