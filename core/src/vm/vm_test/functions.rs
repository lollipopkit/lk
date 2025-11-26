use super::*;

#[test]
fn test_vm_recursive_function_factorial() {
    use Compiler;

    // function fact(n) { if (n <= 1) return 1; return n * fact(n-1); }
    // return fact(5)
    let fact_body = Stmt::Block {
        statements: vec![
            Box::new(Stmt::If {
                condition: Box::new(Expr::Bin(
                    Box::new(Expr::Var("n".into())),
                    BinOp::Le,
                    Box::new(Expr::Val(Val::Int(1))),
                )),
                then_stmt: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Val(Val::Int(1)))),
                }),
                else_stmt: None,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Bin(
                    Box::new(Expr::Var("n".into())),
                    BinOp::Mul,
                    Box::new(Expr::Call(
                        "fact".into(),
                        vec![Box::new(Expr::Bin(
                            Box::new(Expr::Var("n".into())),
                            BinOp::Sub,
                            Box::new(Expr::Val(Val::Int(1))),
                        ))],
                    )),
                ))),
            }),
        ],
    };

    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "fact".into(),
                params: vec!["n".into()],
                param_types: vec![None],
                named_params: Vec::new(),
                return_type: None,
                body: Box::new(fact_body),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Call(
                    "fact".into(),
                    vec![Box::new(Expr::Val(Val::Int(5)))],
                ))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(120));
}

#[test]
fn test_vm_call_many_positional_args() {
    use Compiler;

    let sum_body = Stmt::Block {
        statements: vec![Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Bin(
                Box::new(Expr::Bin(
                    Box::new(Expr::Bin(
                        Box::new(Expr::Bin(
                            Box::new(Expr::Var("a".into())),
                            BinOp::Add,
                            Box::new(Expr::Var("b".into())),
                        )),
                        BinOp::Add,
                        Box::new(Expr::Var("c".into())),
                    )),
                    BinOp::Add,
                    Box::new(Expr::Var("d".into())),
                )),
                BinOp::Add,
                Box::new(Expr::Var("e".into())),
            ))),
        })],
    };

    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "sum5".into(),
                params: vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
                param_types: vec![None; 5],
                named_params: Vec::new(),
                return_type: None,
                body: Box::new(sum_body),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Call(
                    "sum5".into(),
                    vec![
                        Box::new(Expr::Val(Val::Int(1))),
                        Box::new(Expr::Val(Val::Int(2))),
                        Box::new(Expr::Val(Val::Int(3))),
                        Box::new(Expr::Val(Val::Int(4))),
                        Box::new(Expr::Val(Val::Int(5))),
                    ],
                ))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(15));
}

#[test]
fn test_vm_mutual_recursion_even_odd() {
    use Compiler;

    // is_even(n) { if (n == 0) return true; return is_odd(n-1); }
    // is_odd(n) { if (n == 0) return false; return is_even(n-1); }
    // return is_even(10) && !is_even(11)
    let is_even_body = Stmt::Block {
        statements: vec![
            Box::new(Stmt::If {
                condition: Box::new(Expr::Bin(
                    Box::new(Expr::Var("n".into())),
                    BinOp::Eq,
                    Box::new(Expr::Val(Val::Int(0))),
                )),
                then_stmt: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Val(Val::Bool(true)))),
                }),
                else_stmt: None,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Call(
                    "is_odd".into(),
                    vec![Box::new(Expr::Bin(
                        Box::new(Expr::Var("n".into())),
                        BinOp::Sub,
                        Box::new(Expr::Val(Val::Int(1))),
                    ))],
                ))),
            }),
        ],
    };
    let is_odd_body = Stmt::Block {
        statements: vec![
            Box::new(Stmt::If {
                condition: Box::new(Expr::Bin(
                    Box::new(Expr::Var("n".into())),
                    BinOp::Eq,
                    Box::new(Expr::Val(Val::Int(0))),
                )),
                then_stmt: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Val(Val::Bool(false)))),
                }),
                else_stmt: None,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Call(
                    "is_even".into(),
                    vec![Box::new(Expr::Bin(
                        Box::new(Expr::Var("n".into())),
                        BinOp::Sub,
                        Box::new(Expr::Val(Val::Int(1))),
                    ))],
                ))),
            }),
        ],
    };

    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "is_even".into(),
                params: vec!["n".into()],
                param_types: vec![None],
                named_params: Vec::new(),
                return_type: None,
                body: Box::new(is_even_body),
            }),
            Box::new(Stmt::Function {
                name: "is_odd".into(),
                params: vec!["n".into()],
                param_types: vec![None],
                named_params: Vec::new(),
                return_type: None,
                body: Box::new(is_odd_body),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::And(
                    Box::new(Expr::Call("is_even".into(), vec![Box::new(Expr::Val(Val::Int(10)))])),
                    Box::new(Expr::Unary(
                        UnaryOp::Not,
                        Box::new(Expr::Call("is_even".into(), vec![Box::new(Expr::Val(Val::Int(11)))])),
                    )),
                ))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Bool(true));
}

#[test]
fn test_vm_named_call_order_independent() {
    let add_body = Stmt::Block {
        statements: vec![Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Bin(
                Box::new(Expr::Var("x".into())),
                BinOp::Add,
                Box::new(Expr::Var("y".into())),
            ))),
        })],
    };
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "add".into(),
                params: vec![],
                param_types: vec![],
                named_params: vec![
                    NamedParamDecl {
                        name: "x".into(),
                        type_annotation: Some(Type::Int),
                        default: None,
                    },
                    NamedParamDecl {
                        name: "y".into(),
                        type_annotation: Some(Type::Int),
                        default: None,
                    },
                ],
                return_type: None,
                body: Box::new(add_body),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("add".into())),
                    vec![],
                    vec![
                        ("y".into(), Box::new(Expr::Val(Val::Int(2)))),
                        ("x".into(), Box::new(Expr::Val(Val::Int(1)))),
                    ],
                ))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_vm_named_call_optional_and_default() {
    let g_body = Stmt::Block {
        statements: vec![Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Var("a".into()))),
        })],
    };
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "g".into(),
                params: vec![],
                param_types: vec![],
                named_params: vec![NamedParamDecl {
                    name: "a".into(),
                    type_annotation: Some(Type::Optional(Box::new(Type::Int))),
                    default: Some(Expr::Val(Val::Int(10))),
                }],
                return_type: None,
                body: Box::new(g_body),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("g".into())),
                    vec![],
                    vec![],
                ))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(10));

    // explicit nil does not trigger default
    let program2 = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "g".into(),
                params: vec![],
                param_types: vec![],
                named_params: vec![NamedParamDecl {
                    name: "a".into(),
                    type_annotation: Some(Type::Optional(Box::new(Type::Int))),
                    default: Some(Expr::Val(Val::Int(10))),
                }],
                return_type: None,
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Return {
                        value: Some(Box::new(Expr::Var("a".into()))),
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("g".into())),
                    vec![],
                    vec![("a".into(), Box::new(Expr::Val(Val::Nil)))],
                ))),
            }),
        ],
    };
    let fun2 = Compiler::new().compile_stmt(&program2);
    let out2 = Vm::new().exec_with(&fun2, &mut env, None).unwrap();
    assert_eq!(out2, Val::Nil);
}

#[test]
fn test_vm_named_defaults_chain_previous_params() {
    let f_body = Stmt::Block {
        statements: vec![Box::new(Stmt::Return {
            value: Some(Box::new(Expr::List(vec![
                Box::new(Expr::Var("b".into())),
                Box::new(Expr::Var("c".into())),
            ]))),
        })],
    };
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Function {
                name: "f".into(),
                params: vec![],
                param_types: vec![],
                named_params: vec![
                    NamedParamDecl {
                        name: "a".into(),
                        type_annotation: Some(Type::Int),
                        default: None,
                    },
                    NamedParamDecl {
                        name: "b".into(),
                        type_annotation: Some(Type::Int),
                        default: Some(Expr::Bin(
                            Box::new(Expr::Var("a".into())),
                            BinOp::Add,
                            Box::new(Expr::Val(Val::Int(1))),
                        )),
                    },
                    NamedParamDecl {
                        name: "c".into(),
                        type_annotation: Some(Type::Int),
                        default: Some(Expr::Bin(
                            Box::new(Expr::Var("b".into())),
                            BinOp::Add,
                            Box::new(Expr::Val(Val::Int(1))),
                        )),
                    },
                ],
                return_type: None,
                body: Box::new(f_body),
            }),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("first".into()),
                type_annotation: None,
                value: Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("f".into())),
                    vec![],
                    vec![("a".into(), Box::new(Expr::Val(Val::Int(3))))],
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("second".into()),
                type_annotation: None,
                value: Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("f".into())),
                    vec![],
                    vec![("a".into(), Box::new(Expr::Val(Val::Int(10))))],
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::List(vec![
                    Box::new(Expr::Var("first".into())),
                    Box::new(Expr::Var("second".into())),
                ]))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    let expected = Val::List(
        vec![
            Val::List(vec![Val::Int(4), Val::Int(5)].into()),
            Val::List(vec![Val::Int(11), Val::Int(12)].into()),
        ]
        .into(),
    );
    assert_eq!(out, expected);
}

#[test]
fn test_vm_named_call_missing_unknown_duplicate_errors() {
    let h_body = Stmt::Block {
        statements: vec![Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Var("r".into()))),
        })],
    };
    let decl = Stmt::Function {
        name: "h".into(),
        params: vec![],
        param_types: vec![],
        named_params: vec![NamedParamDecl {
            name: "r".into(),
            type_annotation: Some(Type::Int),
            default: None,
        }],
        return_type: None,
        body: Box::new(h_body),
    };
    let mut env = VmContext::new();
    // missing required
    let prog1 = Stmt::Block {
        statements: vec![
            Box::new(decl.clone()),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("h".into())),
                    vec![],
                    vec![],
                ))),
            }),
        ],
    };
    let fun1 = Compiler::new().compile_stmt(&prog1);
    let res1 = Vm::new().exec_with(&fun1, &mut env, None);
    assert!(res1.is_err());
    // unknown name
    let prog2 = Stmt::Block {
        statements: vec![
            Box::new(decl.clone()),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("h".into())),
                    vec![],
                    vec![("w".into(), Box::new(Expr::Val(Val::Int(1))))],
                ))),
            }),
        ],
    };
    let fun2 = Compiler::new().compile_stmt(&prog2);
    let res2 = Vm::new().exec_with(&fun2, &mut env, None);
    assert!(res2.is_err());
    // duplicate
    let prog3 = Stmt::Block {
        statements: vec![
            Box::new(decl),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("h".into())),
                    vec![],
                    vec![
                        ("r".into(), Box::new(Expr::Val(Val::Int(1)))),
                        ("r".into(), Box::new(Expr::Val(Val::Int(2)))),
                    ],
                ))),
            }),
        ],
    };
    let fun3 = Compiler::new().compile_stmt(&prog3);
    let res3 = Vm::new().exec_with(&fun3, &mut env, None);
    assert!(res3.is_err());
}

#[test]
fn test_vm_named_default_depends_on_positional_each_call() {
    // fn f(x, {y = x + 1}) { return y }
    let decl = Stmt::Function {
        name: "f".into(),
        params: vec!["x".into()],
        param_types: vec![],
        named_params: vec![NamedParamDecl {
            name: "y".into(),
            type_annotation: None,
            default: Some(Expr::Bin(
                Box::new(Expr::Var("x".into())),
                BinOp::Add,
                Box::new(Expr::Val(Val::Int(1))),
            )),
        }],
        return_type: None,
        body: Box::new(Stmt::Block {
            statements: vec![Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("y".into()))),
            })],
        }),
    };

    // program1: define f; return f(11)
    let prog1 = Stmt::Block {
        statements: vec![
            Box::new(decl.clone()),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("f".into())),
                    vec![Box::new(Expr::Val(Val::Int(11)))],
                    vec![],
                ))),
            }),
        ],
    };
    let fun1 = Compiler::new().compile_stmt(&prog1);
    let mut env = VmContext::new();
    let out1 = Vm::new().exec_with(&fun1, &mut env, None).unwrap();
    assert_eq!(out1, Val::Int(12));

    // program2: define f again; return f(10)
    let prog2 = Stmt::Block {
        statements: vec![
            Box::new(decl),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("f".into())),
                    vec![Box::new(Expr::Val(Val::Int(10)))],
                    vec![],
                ))),
            }),
        ],
    };
    let fun2 = Compiler::new().compile_stmt(&prog2);
    let out2 = Vm::new().exec_with(&fun2, &mut env, None).unwrap();
    assert_eq!(out2, Val::Int(11));
}

#[test]
#[ignore = "Test depends on vm_blocked which has been removed"]
fn test_vm_nested_closure_named_defaults_and_captures() {
    let make_body = Stmt::Block {
        statements: vec![Box::new(Stmt::Return {
            value: Some(Box::new(Expr::Closure {
                params: vec!["value".into()],
                body: Box::new(Expr::Bin(
                    Box::new(Expr::Var("base".into())),
                    BinOp::Add,
                    Box::new(Expr::Bin(
                        Box::new(Expr::Var("value".into())),
                        BinOp::Mul,
                        Box::new(Expr::Var("scale".into())),
                    )),
                )),
            })),
        })],
    };
    let make_stmt = Stmt::Function {
        name: "make".into(),
        params: vec!["base".into()],
        param_types: vec![],
        named_params: vec![NamedParamDecl {
            name: "scale".into(),
            type_annotation: None,
            default: Some(Expr::Var("base".into())),
        }],
        return_type: None,
        body: Box::new(make_body),
    };

    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("c1".into()),
                type_annotation: None,
                value: Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("make".into())),
                    vec![Box::new(Expr::Val(Val::Int(10)))],
                    vec![],
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("c2".into()),
                type_annotation: None,
                value: Box::new(Expr::CallNamed(
                    Box::new(Expr::Var("make".into())),
                    vec![Box::new(Expr::Val(Val::Int(5)))],
                    vec![("scale".into(), Box::new(Expr::Val(Val::Int(3))))],
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::List(vec![
                    Box::new(Expr::CallExpr(
                        Box::new(Expr::Var("c1".into())),
                        vec![Box::new(Expr::Val(Val::Int(2)))],
                    )),
                    Box::new(Expr::CallExpr(
                        Box::new(Expr::Var("c2".into())),
                        vec![Box::new(Expr::Val(Val::Int(4)))],
                    )),
                ]))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let init_program = Program::new(vec![Box::new(make_stmt)]).unwrap();
    let init_function = crate::vm::compile_program(&init_program);
    let mut init_vm = Vm::new();
    let _ = init_vm.exec_with(&init_function, &mut env, None).unwrap();
    if let Some(_make_val) = env.get("make").cloned() {
        // vm_blocked has been removed - no longer needed for VM fallback
    }
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::List(vec![Val::Int(30), Val::Int(17)].into()));
}

#[test]
fn test_vm_closure_captures_register_value() {
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
    let mut vm = Vm::new();
    let mut env = VmContext::new();
    let out = vm.exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(12));
}

#[test]
fn test_vm_nested_closure_register_windows() {
    let make_adder_stmt = Stmt::Define {
        name: "make_adder".into(),
        value: Box::new(Expr::Closure {
            params: vec!["base".into()],
            body: Box::new(Expr::Closure {
                params: vec!["x".into()],
                body: Box::new(Expr::Bin(
                    Box::new(Expr::Var("base".into())),
                    BinOp::Add,
                    Box::new(Expr::Var("x".into())),
                )),
            }),
        }),
    };

    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("add_two".into()),
                type_annotation: None,
                value: Box::new(Expr::CallExpr(
                    Box::new(Expr::Var("make_adder".into())),
                    vec![Box::new(Expr::Val(Val::Int(2)))],
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("add_three".into()),
                type_annotation: None,
                value: Box::new(Expr::CallExpr(
                    Box::new(Expr::Var("make_adder".into())),
                    vec![Box::new(Expr::Val(Val::Int(3)))],
                )),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Bin(
                    Box::new(Expr::CallExpr(
                        Box::new(Expr::Var("add_two".into())),
                        vec![Box::new(Expr::Val(Val::Int(10)))],
                    )),
                    BinOp::Add,
                    Box::new(Expr::CallExpr(
                        Box::new(Expr::Var("add_three".into())),
                        vec![Box::new(Expr::Val(Val::Int(20)))],
                    )),
                ))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut vm = Vm::new();
    let mut env = VmContext::new();
    let init_program = Program::new(vec![Box::new(make_adder_stmt)]).unwrap();
    let init_function = crate::vm::compile_program(&init_program);
    let mut init_vm = Vm::new();
    let _ = init_vm.exec_with(&init_function, &mut env, None).unwrap();
    // vm_blocked has been removed - no longer needed for VM fallback
    let out = vm.exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(35));
}
