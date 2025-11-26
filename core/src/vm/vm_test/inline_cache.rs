use super::*;

#[test]
fn test_vm_global_ic_multiple_redefines() {
    use Compiler;
    // Program:
    //   x = 1; _ = x; x = 2; _ = x; x = 3; return x
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(1))),
            }),
            Box::new(Stmt::Expr(Box::new(Expr::Var("x".into())))),
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(2))),
            }),
            Box::new(Stmt::Expr(Box::new(Expr::Var("x".into())))),
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(3))),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("x".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_vm_global_ic_invalidation_on_define() {
    use Compiler;

    // Program: warm LoadGlobal(x); Define x=2; return x
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Expr(Box::new(Expr::Var("x".into())))),
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(2))),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("x".into()))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    env.define("x", Val::Int(1));

    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(2));
}

#[test]
fn test_vm_access_ic_identity_change_miss() {
    use Compiler;

    // Program:
    //   m = {"a":1}; m.a; m = {"a":2}; return m.a
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "m".into(),
                value: Box::new(Expr::Map(vec![(
                    Box::new(Expr::Val(Val::Str("a".into()))),
                    Box::new(Expr::Val(Val::Int(1))),
                )])),
            }),
            // Warm IC
            Box::new(Stmt::Expr(Box::new(Expr::Access(
                Box::new(Expr::Var("m".into())),
                Box::new(Expr::Val(Val::Str("a".into()))),
            )))),
            // Replace m with a new map (different Arc identity)
            Box::new(Stmt::Define {
                name: "m".into(),
                value: Box::new(Expr::Map(vec![(
                    Box::new(Expr::Val(Val::Str("a".into()))),
                    Box::new(Expr::Val(Val::Int(2))),
                )])),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Access(
                    Box::new(Expr::Var("m".into())),
                    Box::new(Expr::Val(Val::Str("a".into()))),
                ))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(2));
}

#[test]
fn test_vm_index_ic_list_identity_replacement() {
    // Program:
    //   sum = 0; lst = [];
    //   for i in 0..3 { lst = [i, 99]; sum = sum + lst[0]; }
    //   return sum
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::Define {
                name: "lst".into(),
                value: Box::new(Expr::List(vec![])),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Variable("i".into()),
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(0)))),
                    end: Some(Box::new(Expr::Val(Val::Int(3)))),
                    inclusive: false,
                    step: None,
                }),
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::Assign {
                            name: "lst".into(),
                            value: Box::new(Expr::List(vec![
                                Box::new(Expr::Var("i".into())),
                                Box::new(Expr::Val(Val::Int(99))),
                            ])),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("sum".into())),
                                BinOp::Add,
                                Box::new(Expr::Access(
                                    Box::new(Expr::Var("lst".into())),
                                    Box::new(Expr::Val(Val::Int(0))),
                                )),
                            )),
                            span: None,
                        }),
                    ],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("sum".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let out = exec_with_new_vm(&fun);
    // Expect 0 + 1 + 2 = 3 if cache invalidates on identity change correctly
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_vm_index_ic_str_identity_replacement_multibyte() {
    // Program:
    //   acc = "";
    //   for i in 0..3 {
    //     if i == 0 { s = "αβ" } else if i == 1 { s = "XY" } else { s = "Z!" }
    //     acc = acc + s[0]
    //   }
    //   return acc
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "acc".into(),
                value: Box::new(Expr::Val(Val::Str("".into()))),
            }),
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::While {
                condition: Box::new(Expr::Bin(
                    Box::new(Expr::Var("i".into())),
                    BinOp::Lt,
                    Box::new(Expr::Val(Val::Int(3))),
                )),
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::If {
                            condition: Box::new(Expr::Bin(
                                Box::new(Expr::Var("i".into())),
                                BinOp::Eq,
                                Box::new(Expr::Val(Val::Int(0))),
                            )),
                            then_stmt: Box::new(Stmt::Define {
                                name: "s".into(),
                                value: Box::new(Expr::Val(Val::Str("αβ".into()))),
                            }),
                            else_stmt: Some(Box::new(Stmt::If {
                                condition: Box::new(Expr::Bin(
                                    Box::new(Expr::Var("i".into())),
                                    BinOp::Eq,
                                    Box::new(Expr::Val(Val::Int(1))),
                                )),
                                then_stmt: Box::new(Stmt::Define {
                                    name: "s".into(),
                                    value: Box::new(Expr::Val(Val::Str("XY".into()))),
                                }),
                                else_stmt: Some(Box::new(Stmt::Define {
                                    name: "s".into(),
                                    value: Box::new(Expr::Val(Val::Str("Z!".into()))),
                                })),
                            })),
                        }),
                        Box::new(Stmt::Assign {
                            name: "acc".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("acc".into())),
                                BinOp::Add,
                                Box::new(Expr::Access(
                                    Box::new(Expr::Var("s".into())),
                                    Box::new(Expr::Val(Val::Int(0))),
                                )),
                            )),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "i".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("i".into())),
                                BinOp::Add,
                                Box::new(Expr::Val(Val::Int(1))),
                            )),
                            span: None,
                        }),
                    ],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("acc".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let out = exec_with_new_vm(&fun);
    // Expect "αXZ" if caching invalidates on identity change and multibyte index works
    assert_eq!(out, Val::Str("αXZ".into()));
}

#[test]
fn test_vm_global_ic_invalidation_on_redefine() {
    // program:
    // define z = 1;
    // fn g() { return z }
    // let a = g();
    // redefine z = 2; (Define emits DefineGlobal in VM)
    // let b = g();
    // return [a, b]
    let decl_g = Stmt::Function {
        name: "g".into(),
        params: vec![],
        param_types: vec![],
        named_params: vec![],
        return_type: None,
        body: Box::new(Stmt::Block {
            statements: vec![Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("z".into()))),
            })],
        }),
    };
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "z".into(),
                value: Box::new(Expr::Val(Val::Int(1))),
            }),
            Box::new(decl_g),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("a".into()),
                type_annotation: None,
                value: Box::new(Expr::Call("g".into(), vec![])),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Define {
                name: "z".into(),
                value: Box::new(Expr::Val(Val::Int(2))),
            }),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("b".into()),
                type_annotation: None,
                value: Box::new(Expr::Call("g".into(), vec![])),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::List(vec![
                    Box::new(Expr::Var("a".into())),
                    Box::new(Expr::Var("b".into())),
                ]))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    match out {
        Val::List(list) => {
            assert_eq!(list.len(), 2);
            assert_eq!(list[0], Val::Int(1));
            assert_eq!(list[1], Val::Int(2));
        }
        other => panic!("expected list, got {:?}", other),
    }
}

#[test]
fn test_vm_global_ic_local_then_global_toggle() {
    // program:
    // fn g() { return t }
    // let a = g(); // t is undefined -> nil
    // define t = 42;
    // let b = g(); // now 42
    // return [a, b]
    let decl_g = Stmt::Function {
        name: "g".into(),
        params: vec![],
        param_types: vec![],
        named_params: vec![],
        return_type: None,
        body: Box::new(Stmt::Block {
            statements: vec![Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("t".into()))),
            })],
        }),
    };
    let program = Stmt::Block {
        statements: vec![
            Box::new(decl_g),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("a".into()),
                type_annotation: None,
                value: Box::new(Expr::Call("g".into(), vec![])),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Define {
                name: "t".into(),
                value: Box::new(Expr::Val(Val::Int(42))),
            }),
            Box::new(Stmt::Let {
                pattern: Pattern::Variable("b".into()),
                type_annotation: None,
                value: Box::new(Expr::Call("g".into(), vec![])),
                span: None,
                is_const: false,
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::List(vec![
                    Box::new(Expr::Var("a".into())),
                    Box::new(Expr::Var("b".into())),
                ]))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    match out {
        Val::List(list) => {
            assert_eq!(list.len(), 2);
            assert_eq!(list[0], Val::Nil);
            assert_eq!(list[1], Val::Int(42));
        }
        other => panic!("expected list, got {:?}", other),
    }
}

#[test]
fn test_vm_local_shadow_overrides_global_and_ic() {
    // program: let y = 100; fn f(x, {y = x + 1}) { return y } ; return [f(1), f(2, y=7)]
    let decl_f = Stmt::Function {
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
    let program = Stmt::Block {
        statements: vec![
            // global y = 100
            Box::new(Stmt::Define {
                name: "y".into(),
                value: Box::new(Expr::Val(Val::Int(100))),
            }),
            // fn f
            Box::new(decl_f),
            // return [f(1), f(2, y=7)]
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::List(vec![
                    Box::new(Expr::CallNamed(
                        Box::new(Expr::Var("f".into())),
                        vec![Box::new(Expr::Val(Val::Int(1)))],
                        vec![],
                    )),
                    Box::new(Expr::CallNamed(
                        Box::new(Expr::Var("f".into())),
                        vec![Box::new(Expr::Val(Val::Int(2)))],
                        vec![("y".into(), Box::new(Expr::Val(Val::Int(7))))],
                    )),
                ]))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    match out {
        Val::List(list) => {
            assert_eq!(list.len(), 2);
            assert_eq!(list[0], Val::Int(2));
            assert_eq!(list[1], Val::Int(7));
        }
        other => panic!("expected list, got {:?}", other),
    }
}
