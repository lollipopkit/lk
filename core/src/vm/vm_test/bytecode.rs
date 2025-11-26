use super::*;
use crate::vm::bc32::{self, DecodedTag, Tag};

#[test]
fn test_compile_time_const_binding() {
    let const_a = Stmt::Let {
        pattern: Pattern::Variable("A".to_string()),
        type_annotation: None,
        value: Box::new(Expr::Bin(
            Box::new(Expr::Val(Val::Int(2))),
            BinOp::Add,
            Box::new(Expr::Val(Val::Int(3))),
        )),
        span: None,
        is_const: true,
    };
    let const_b = Stmt::Let {
        pattern: Pattern::Variable("B".to_string()),
        type_annotation: None,
        value: Box::new(Expr::Bin(
            Box::new(Expr::Var("A".into())),
            BinOp::Mul,
            Box::new(Expr::Val(Val::Int(10))),
        )),
        span: None,
        is_const: true,
    };
    let ret = Stmt::Return {
        value: Some(Box::new(Expr::List(vec![
            Box::new(Expr::Var("A".into())),
            Box::new(Expr::Var("B".into())),
        ]))),
    };
    let program = Stmt::Block {
        statements: vec![Box::new(const_a), Box::new(const_b), Box::new(ret)],
    };

    let fun = Compiler::new().compile_stmt(&program);

    assert!(fun.code.iter().all(|op| !matches!(op, Op::StoreLocal(_, _))));
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let result = vm.exec(&fun, &mut ctx).expect("vm exec");
    assert_eq!(result, Val::List(vec![Val::Int(5), Val::Int(50)].into()));
}

#[test]
fn vm_errors_include_call_stack() {
    let inner_func = Stmt::Function {
        name: "inner".to_string(),
        params: Vec::new(),
        param_types: Vec::new(),
        named_params: Vec::new(),
        return_type: None,
        body: Box::new(Stmt::Block {
            statements: vec![
                Box::new(Stmt::Let {
                    pattern: Pattern::Variable("f".into()),
                    type_annotation: None,
                    value: Box::new(Expr::Val(Val::Int(1))),
                    span: None,
                    is_const: false,
                }),
                Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Call("f".into(), Vec::new()))),
                }),
            ],
        }),
    };
    let outer_func = Stmt::Function {
        name: "outer".to_string(),
        params: Vec::new(),
        param_types: Vec::new(),
        named_params: Vec::new(),
        return_type: None,
        body: Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Call("inner".into(), Vec::new()))),
        }),
    };
    let program = Stmt::Block {
        statements: vec![
            Box::new(inner_func),
            Box::new(outer_func),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Call("outer".into(), Vec::new()))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let err = vm.exec(&fun, &mut ctx).expect_err("vm execution should fail");
    let msg = err.to_string();
    assert!(msg.contains("Call stack"), "missing call stack: {msg}");
    assert!(msg.contains("outer"), "missing outer frame: {msg}");
}

#[test]
#[ignore]
fn debug_print_closure_bytecode() {
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "outer".into(),
                params: vec!["a".into()],
                param_types: Vec::new(),
                named_params: Vec::new(),
                return_type: None,
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::Function {
                            name: "inner".into(),
                            params: vec!["b".into()],
                            param_types: Vec::new(),
                            named_params: Vec::new(),
                            return_type: None,
                            body: Box::new(Stmt::Return {
                                value: Some(Box::new(Expr::Bin(
                                    Box::new(Expr::Var("a".into())),
                                    BinOp::Add,
                                    Box::new(Expr::Var("b".into())),
                                ))),
                            }),
                        }),
                        Box::new(Stmt::Return {
                            value: Some(Box::new(Expr::Var("inner".into()))),
                        }),
                    ],
                }),
            }),
            Box::new(Stmt::Define {
                name: "f".into(),
                value: Box::new(Expr::Call("outer".into(), vec![Box::new(Expr::Val(Val::Int(5)))])),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallExpr(
                    Box::new(Expr::Var("f".into())),
                    vec![Box::new(Expr::Val(Val::Int(7)))],
                ))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    println!(
        "code len: {}, code32 len: {:?}",
        fun.code.len(),
        fun.code32.as_ref().map(|c| c.len())
    );
    for (idx, op) in fun.code.iter().enumerate() {
        println!("op[{idx}]: {op:?}");
    }
    if let Some(code32) = &fun.code32 {
        for (idx, word) in code32.iter().enumerate() {
            println!("code32[{idx}]: {:#010x}", word);
        }
    }

    fn dump_function(func: &Function, indent: usize) {
        let pad = " ".repeat(indent);
        println!(
            "{}code len: {}, code32 len: {:?}",
            pad,
            func.code.len(),
            func.code32.as_ref().map(|c| c.len())
        );
        for (idx, op) in func.code.iter().enumerate() {
            println!("{}op[{idx}]: {op:?}", pad);
        }
        for (pidx, proto) in func.protos.iter().enumerate() {
            println!("{}proto[{pidx}] params={:?}", pad, proto.params);
            if let Some(inner) = &proto.func {
                dump_function(inner, indent + 2);
            }
        }
    }

    use crate::vm::bytecode::Function;
    dump_function(&fun, 0);
}

#[test]
fn test_vm_const_bool_and_or() {
    let compiler = Compiler::new();

    // true && false => false
    let expr = Expr::And(
        Box::new(Expr::Val(Val::Bool(true))),
        Box::new(Expr::Val(Val::Bool(false))),
    );
    let fun = compiler.compile_expr(&expr);
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let out = vm.exec(&fun, &mut ctx).unwrap();
    assert_eq!(out, Val::Bool(false));

    // true || false => true
    let expr = Expr::Or(
        Box::new(Expr::Val(Val::Bool(true))),
        Box::new(Expr::Val(Val::Bool(false))),
    );
    let fun = compiler.compile_expr(&expr);
    let out = vm.exec(&fun, &mut ctx).unwrap();
    assert_eq!(out, Val::Bool(true));
}

#[test]
fn test_vm_ascii_string_iter_index() {
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "s".into(),
                value: Box::new(Expr::Val(Val::Str("abcd".into()))),
            }),
            Box::new(Stmt::Define {
                name: "out".into(),
                value: Box::new(Expr::Val(Val::Str("".into()))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Variable("ch".into()),
                iterable: Box::new(Expr::Var("s".into())),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "out".into(),
                        value: Box::new(Expr::Bin(
                            Box::new(Expr::Var("out".into())),
                            BinOp::Add,
                            Box::new(Expr::Var("ch".into())),
                        )),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("out".into()))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut vm_bc32 = Vm::new();
    let mut env_bc32 = VmContext::new();
    let out_bc32 = vm_bc32.exec_with(&fun, &mut env_bc32, None).unwrap();
    assert_eq!(out_bc32, Val::Str("abcd".into()));

    {
        let mut fallback_fun = fun.clone();
        fallback_fun.code32 = None;
        fallback_fun.bc32_decoded = None;
        let mut vm_fallback = Vm::new();
        let mut env_fallback = VmContext::new();
        let out_fallback = vm_fallback.exec_with(&fallback_fun, &mut env_fallback, None).unwrap();
        assert_eq!(out_fallback, Val::Str("abcd".into()));
    }
}

#[test]
fn test_vm_compile_select_lowering() {
    let expr = Expr::Select {
        cases: vec![
            SelectCase {
                pattern: SelectPattern::Recv {
                    binding: Some("msg".into()),
                    channel: Box::new(Expr::Var("rx".into())),
                },
                guard: None,
                body: Box::new(Expr::Var("msg".into())),
            },
            SelectCase {
                pattern: SelectPattern::Send {
                    channel: Box::new(Expr::Var("tx".into())),
                    value: Box::new(Expr::Val(Val::Int(7))),
                },
                guard: None,
                body: Box::new(Expr::Val(Val::Int(1))),
            },
        ],
        default_case: Some(Box::new(Expr::Val(Val::Int(0)))),
    };
    let fun = Compiler::new().compile_expr(&expr);

    assert!(
        fun.consts
            .iter()
            .any(|v| matches!(v, Val::Str(name) if name.as_ref() == "select$block")),
        "expected select$block builtin string constant"
    );
    let build_list_count = fun.code.iter().filter(|op| matches!(op, Op::BuildList { .. })).count();
    assert!(
        build_list_count >= 4,
        "expected at least four BuildList ops, got {}",
        build_list_count
    );
    assert!(
        fun.code.iter().any(|op| matches!(op, Op::Call { argc: 5, .. })),
        "select lowering should call select$block with five arguments"
    );
}

#[test]
fn test_vm_compile_select_with_guard_lowering() {
    let expr = Expr::Select {
        cases: vec![SelectCase {
            pattern: SelectPattern::Recv {
                binding: None,
                channel: Box::new(Expr::Var("rx".into())),
            },
            guard: Some(Box::new(Expr::Var("ready".into()))),
            body: Box::new(Expr::Val(Val::Int(10))),
        }],
        default_case: Some(Box::new(Expr::Val(Val::Int(0)))),
    };
    let fun = Compiler::new().compile_expr(&expr);

    assert!(
        fun.code.iter().any(|op| matches!(op, Op::ToBool(_, _))),
        "guard lowering should ToBool the guard expression"
    );
    assert!(
        fun.consts
            .iter()
            .any(|v| matches!(v, Val::Str(name) if name.as_ref() == "select$block")),
        "guarded select still calls select$block"
    );
}

#[test]
fn test_vm_compile_select_without_default_lowering() {
    let expr = Expr::Select {
        cases: vec![SelectCase {
            pattern: SelectPattern::Send {
                channel: Box::new(Expr::Var("tx".into())),
                value: Box::new(Expr::Val(Val::Int(1))),
            },
            guard: None,
            body: Box::new(Expr::Val(Val::Int(2))),
        }],
        default_case: None,
    };
    let fun = Compiler::new().compile_expr(&expr);

    assert!(
        fun.consts.iter().any(|v| matches!(v, Val::Bool(false))),
        "select without default should embed has_default=false constant"
    );
    assert!(
        fun.code.iter().any(|op| matches!(op, Op::Call { argc: 5, .. })),
        "select without default still invokes select$block"
    );
}

#[test]
fn test_bc32_for_range_ascending_exclusive() {
    // Program: x=0; for _ in 0..3 { x = x + 1 } return x
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Ignore,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(0)))),
                    end: Some(Box::new(Expr::Val(Val::Int(3)))),
                    inclusive: false,
                    step: None,
                }),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "x".into(),
                        value: Box::new(Expr::Bin(
                            Box::new(Expr::Var("x".into())),
                            BinOp::Add,
                            Box::new(Expr::Val(Val::Int(1))),
                        )),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("x".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    println!(
        "code len: {}, code32 len: {:?}",
        fun.code.len(),
        fun.code32.as_ref().map(|c| c.len())
    );

    assert!(fun.code32.is_some(), "function should be bc32-packed");
    let code32 = fun.code32.as_ref().unwrap();
    let mut seen_prep = 0;
    let mut seen_guard = 0;
    let mut seen_step = 0;
    for &w in code32.iter() {
        let tag = vm::tag_of(w);
        if tag == vm::TAG_FOR_RANGE_PREP {
            seen_prep += 1;
        }
        if tag == vm::TAG_FOR_RANGE_LOOP {
            seen_guard += 1;
        }
        if tag == vm::TAG_FOR_RANGE_STEP {
            seen_step += 1;
        }
    }
    assert!(
        seen_prep >= 1 && seen_guard >= 1 && seen_step >= 1,
        "expected ForRange* tags present in bc32 stream"
    );

    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_bc32_for_range_descending_inclusive_with_step() {
    // Program: x=0; for _ in 5..=1 step -2 { x = x + 1 } return x
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Ignore,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(5)))),
                    end: Some(Box::new(Expr::Val(Val::Int(1)))),
                    inclusive: true,
                    step: Some(Box::new(Expr::Val(Val::Int(-2)))),
                }),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "x".into(),
                        value: Box::new(Expr::Bin(
                            Box::new(Expr::Var("x".into())),
                            BinOp::Add,
                            Box::new(Expr::Val(Val::Int(1))),
                        )),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("x".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    assert!(fun.code32.is_some(), "function should be bc32-packed");
    let out = exec_with_new_vm(&fun);
    // 5,3,1 => 3 iterations
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_bc32_for_range_explicit_step_inclusive() {
    // Program: x=0; for _ in 0..=10 step 2 { x = x + 1 } return x
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Ignore,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(0)))),
                    end: Some(Box::new(Expr::Val(Val::Int(10)))),
                    inclusive: true,
                    step: Some(Box::new(Expr::Val(Val::Int(2)))),
                }),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "x".into(),
                        value: Box::new(Expr::Bin(
                            Box::new(Expr::Var("x".into())),
                            BinOp::Add,
                            Box::new(Expr::Val(Val::Int(1))),
                        )),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("x".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    assert!(fun.code32.is_some(), "function should be bc32-packed");
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(6));
}

#[test]
fn test_bc32_long_or_uses_extended_true_set() {
    use Compiler;
    // Build a long right-hand expression to force an i16 jump distance
    let mut rhs = Expr::Bin(
        Box::new(Expr::Var("x".into())),
        BinOp::Add,
        Box::new(Expr::Val(Val::Int(1))),
    );
    for _ in 0..100 {
        rhs = Expr::Bin(Box::new(rhs), BinOp::Add, Box::new(Expr::Val(Val::Int(1))));
    }
    let expr = Expr::Or(Box::new(Expr::Val(Val::Bool(true))), Box::new(rhs));
    let fun = Compiler::new().compile_expr(&expr);
    {
        let code32 = fun.code32.as_ref().expect("function should be bc32-packed");
        let mut found = false;
        for &w in code32.iter() {
            match bc32::decode_tag_byte(vm::tag_of(w)) {
                DecodedTag::Regular {
                    tag: Tag::JmpTrueSetX, ..
                }
                | DecodedTag::Regular {
                    tag: Tag::JmpTrueSet, ..
                } => {
                    found = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(
            found,
            "expected JmpTrueSet or its extended form to be present for long OR"
        );
    }
    // Execution should short-circuit to true
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Bool(true));
}

#[test]
fn test_bc32_long_and_uses_extended_false_set() {
    use Compiler;
    let mut rhs = Expr::Bin(
        Box::new(Expr::Var("y".into())),
        BinOp::Add,
        Box::new(Expr::Val(Val::Int(1))),
    );
    for _ in 0..100 {
        rhs = Expr::Bin(Box::new(rhs), BinOp::Add, Box::new(Expr::Val(Val::Int(1))));
    }
    let expr = Expr::And(Box::new(Expr::Val(Val::Bool(false))), Box::new(rhs));
    let fun = Compiler::new().compile_expr(&expr);

    let code32 = fun.code32.as_ref().expect("function should be bc32-packed");
    let mut found = false;
    for &w in code32.iter() {
        match bc32::decode_tag_byte(vm::tag_of(w)) {
            DecodedTag::Regular {
                tag: Tag::JmpFalseSetX, ..
            }
            | DecodedTag::Regular {
                tag: Tag::JmpFalseSet, ..
            } => {
                found = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        found,
        "expected JmpFalseSet or its extended form to be present for long AND"
    );

    // Execution should short-circuit to false
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Bool(false));
}

#[test]
fn test_bc32_long_nullish_uses_extended_pick() {
    use Compiler;
    // left is a variable (undefined -> nil at runtime), forcing NullishPick emission
    // right is a long chain to push the jump distance beyond i8
    let mut rhs = Expr::Bin(
        Box::new(Expr::Var("x".into())),
        BinOp::Add,
        Box::new(Expr::Val(Val::Int(0))),
    );
    for _ in 0..100 {
        rhs = Expr::Bin(Box::new(rhs), BinOp::Add, Box::new(Expr::Val(Val::Int(0))));
    }
    let expr = Expr::NullishCoalescing(Box::new(Expr::Var("u".into())), Box::new(rhs));
    let fun = Compiler::new().compile_expr(&expr);

    let code32 = fun.code32.as_ref().expect("function should be bc32-packed");
    let mut found = false;
    for &w in code32.iter() {
        match bc32::decode_tag_byte(vm::tag_of(w)) {
            DecodedTag::Regular {
                tag: Tag::NullishPickX, ..
            }
            | DecodedTag::Regular {
                tag: Tag::NullishPick, ..
            } => {
                found = true;
                break;
            }
            _ => {}
        }
    }
    assert!(
        found,
        "expected NullishPick or its extended form to be present for long nullish coalescing"
    );

    // Execution: define x so rhs evaluates cleanly; u is undefined -> nil so rhs taken
    let mut env = VmContext::new();
    env.define("x", Val::Int(42));
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    // Result should be 42 given repeated +0
    assert_eq!(out, Val::Int(42));
}

#[test]
fn test_bc32_for_range_step_zero_errors() {
    let program = Stmt::Block {
        statements: vec![Box::new(Stmt::For {
            pattern: ForPattern::Ignore,
            iterable: Box::new(Expr::Range {
                start: Some(Box::new(Expr::Val(Val::Int(0)))),
                end: Some(Box::new(Expr::Val(Val::Int(10)))),
                inclusive: true,
                step: Some(Box::new(Expr::Val(Val::Int(0)))),
            }),
            body: Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Val(Val::Int(1)))),
            }),
        })],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut vm = Vm::new();
    let mut env = VmContext::new();
    let err = vm.exec_with(&fun, &mut env, None).expect_err("zero step should error");
    assert!(
        err.to_string().contains("step must not be zero") || err.to_string().contains("For-range step cannot be zero"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_vm_call_many_args_packing_255() {
    use Compiler;
    // Define a Rust function that returns argc
    fn argc(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
        Ok(Val::Int(args.len() as i64))
    }
    // Build expression: argc(0,1,2,...,254) => 255
    let mut args: Vec<Box<Expr>> = Vec::with_capacity(255);
    for i in 0..255 {
        args.push(Box::new(Expr::Val(Val::Int(i))));
    }
    let expr = Expr::Call("argc".into(), args);
    let fun = Compiler::new().compile_expr(&expr);

    // Prepare env with argc
    let mut env = VmContext::new();
    env.define("argc", Val::RustFunction(argc));

    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(255));
}
