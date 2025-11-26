use super::*;

#[test]
fn test_vm_stmt_block_if_while_return() {
    // {
    //   x = 0;
    //   i = 0;
    //   while (i < 3) {
    //     x = x + 2;
    //     i = i + 1;
    //   }
    //   if (x == 6) { return x; } else { return 0; }
    // }
    let block = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
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
                        Box::new(Stmt::Assign {
                            name: "x".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("x".into())),
                                BinOp::Add,
                                Box::new(Expr::Val(Val::Int(2))),
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
            Box::new(Stmt::If {
                condition: Box::new(Expr::Bin(
                    Box::new(Expr::Var("x".into())),
                    BinOp::Eq,
                    Box::new(Expr::Val(Val::Int(6))),
                )),
                then_stmt: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Var("x".into()))),
                }),
                else_stmt: Some(Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Val(Val::Int(0)))),
                })),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&block);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(6));
}

#[test]
fn test_vm_for_range_numeric() {
    // x = 0; for i in 0..3 { x = x + 1; } return x;
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
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
    let mut vm = Vm::new();
    let mut env = VmContext::new();
    let out = vm.exec_with(&fun, &mut env, None).unwrap();
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_vm_for_range_descending_exclusive() {
    // x = 0; for _ in 3..0 { x = x + 1; } return x;  // visits 3,2,1
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Ignore,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(3)))),
                    end: Some(Box::new(Expr::Val(Val::Int(0)))),
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
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_vm_for_range_inclusive_and_edges() {
    // Inclusive ascending: 0..=3 -> 4 iterations
    let prog_inc = Stmt::Block {
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
                    inclusive: true,
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
    let fun = Compiler::new().compile_stmt(&prog_inc);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(4));

    // start == end exclusive -> 0 iterations
    let prog_zero = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Ignore,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(2)))),
                    end: Some(Box::new(Expr::Val(Val::Int(2)))),
                    inclusive: false,
                    step: None,
                }),
                body: Box::new(Stmt::Block { statements: vec![] }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("x".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&prog_zero);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(0));

    // start == end inclusive -> 1 iteration
    let prog_one = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Ignore,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(2)))),
                    end: Some(Box::new(Expr::Val(Val::Int(2)))),
                    inclusive: true,
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
    let fun = Compiler::new().compile_stmt(&prog_one);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(1));
}

#[test]
fn test_vm_for_range_with_explicit_step() {
    // Ascending exclusive with step 2: 0..10..2 -> 0,2,4,6,8 => 5 iterations
    let prog_step2 = Stmt::Block {
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
                    inclusive: false,
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
    let fun = Compiler::new().compile_stmt(&prog_step2);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(5));

    // Ascending inclusive with step 2: 0..=10..2 -> includes 10 => 6 iterations
    let prog_step2_inc = Stmt::Block {
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
    let fun = Compiler::new().compile_stmt(&prog_step2_inc);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(6));

    // Descending with negative step: 5..0..-2 -> 5,3,1 => 3 iterations
    let prog_desc = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "x".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Ignore,
                iterable: Box::new(Expr::Range {
                    start: Some(Box::new(Expr::Val(Val::Int(5)))),
                    end: Some(Box::new(Expr::Val(Val::Int(0)))),
                    inclusive: false,
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
    let fun = Compiler::new().compile_stmt(&prog_desc);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(3));
}

#[test]
fn test_vm_for_list_tuple_pattern() {
    // sum = 0; for (a,b) in [[1,2],[3,4]] { sum = sum + a; sum = sum + b; } return sum;
    let iter = Expr::List(vec![
        Box::new(Expr::List(vec![
            Box::new(Expr::Val(Val::Int(1))),
            Box::new(Expr::Val(Val::Int(2))),
        ])),
        Box::new(Expr::List(vec![
            Box::new(Expr::Val(Val::Int(3))),
            Box::new(Expr::Val(Val::Int(4))),
        ])),
    ]);
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Tuple(vec![ForPattern::Variable("a".into()), ForPattern::Variable("b".into())]),
                iterable: Box::new(iter),
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("sum".into())),
                                BinOp::Add,
                                Box::new(Expr::Var("a".into())),
                            )),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("sum".into())),
                                BinOp::Add,
                                Box::new(Expr::Var("b".into())),
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
    assert_eq!(out, Val::Int(10));
}

#[test]
fn test_vm_for_list_array_rest_pattern() {
    // sum = 0; for [a, ..rest] in [[1,2,3],[4],[5,6]] { sum += a; sum += rest.len } return sum;
    let iter = Expr::List(vec![
        Box::new(Expr::List(vec![
            Box::new(Expr::Val(Val::Int(1))),
            Box::new(Expr::Val(Val::Int(2))),
            Box::new(Expr::Val(Val::Int(3))),
        ])),
        Box::new(Expr::List(vec![Box::new(Expr::Val(Val::Int(4)))])),
        Box::new(Expr::List(vec![
            Box::new(Expr::Val(Val::Int(5))),
            Box::new(Expr::Val(Val::Int(6))),
        ])),
    ]);
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Array {
                    patterns: vec![ForPattern::Variable("a".into())],
                    rest: Some("rest".into()),
                },
                iterable: Box::new(iter),
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("sum".into())),
                                BinOp::Add,
                                Box::new(Expr::Var("a".into())),
                            )),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("sum".into())),
                                BinOp::Add,
                                Box::new(Expr::Access(
                                    Box::new(Expr::Var("rest".into())),
                                    Box::new(Expr::Val(Val::Str("len".into()))),
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
    assert_eq!(out, Val::Int(13));
}

#[test]
fn test_vm_for_list_object_pattern() {
    // sum = 0; for {x:a, y:b} in [{"x":1,"y":2},{"x":3,"y":4}] { sum += a; sum += b } return sum;
    let iter = Expr::List(vec![
        Box::new(Expr::Map(vec![
            (
                Box::new(Expr::Val(Val::Str("x".into()))),
                Box::new(Expr::Val(Val::Int(1))),
            ),
            (
                Box::new(Expr::Val(Val::Str("y".into()))),
                Box::new(Expr::Val(Val::Int(2))),
            ),
        ])),
        Box::new(Expr::Map(vec![
            (
                Box::new(Expr::Val(Val::Str("x".into()))),
                Box::new(Expr::Val(Val::Int(3))),
            ),
            (
                Box::new(Expr::Val(Val::Str("y".into()))),
                Box::new(Expr::Val(Val::Int(4))),
            ),
        ])),
    ]);
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Object(vec![
                    ("x".into(), ForPattern::Variable("a".into())),
                    ("y".into(), ForPattern::Variable("b".into())),
                ]),
                iterable: Box::new(iter),
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("sum".into())),
                                BinOp::Add,
                                Box::new(Expr::Var("a".into())),
                            )),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new(Expr::Bin(
                                Box::new(Expr::Var("sum".into())),
                                BinOp::Add,
                                Box::new(Expr::Var("b".into())),
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
    assert_eq!(out, Val::Int(10));
}

#[test]
fn test_vm_for_string_iter_count() {
    // n = 0; for ch in "abcd" { n = n + 1 } return n; => 4
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "n".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Variable("ch".into()),
                iterable: Box::new(Expr::Val(Val::Str("abcd".into()))),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "n".into(),
                        value: Box::new(Expr::Bin(
                            Box::new(Expr::Var("n".into())),
                            BinOp::Add,
                            Box::new(Expr::Val(Val::Int(1))),
                        )),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("n".into()))),
            }),
        ],
    };
    let fun = Compiler::new().compile_stmt(&program);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(4));
}

#[test]
fn test_vm_for_map_pairs_sum_values() {
    // sum = 0; for (k,v) in {"a":1, "b":2, "c":3} { sum += v } return sum; => 6
    let map = Expr::Map(vec![
        (
            Box::new(Expr::Val(Val::Str("a".into()))),
            Box::new(Expr::Val(Val::Int(1))),
        ),
        (
            Box::new(Expr::Val(Val::Str("b".into()))),
            Box::new(Expr::Val(Val::Int(2))),
        ),
        (
            Box::new(Expr::Val(Val::Str("c".into()))),
            Box::new(Expr::Val(Val::Int(3))),
        ),
    ]);
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Tuple(vec![ForPattern::Variable("k".into()), ForPattern::Variable("v".into())]),
                iterable: Box::new(map),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "sum".into(),
                        value: Box::new(Expr::Bin(
                            Box::new(Expr::Var("sum".into())),
                            BinOp::Add,
                            Box::new(Expr::Var("v".into())),
                        )),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("sum".into()))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(6));
}

#[test]
fn test_vm_map_iteration_order_stable() {
    use Compiler;
    // Define a Rust function update(hash:Int, key:String) -> Int accumulating key order as digits
    fn update(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
        use anyhow::anyhow;
        if args.len() != 2 {
            return Err(anyhow!("update expects 2 args"));
        }
        let h = match &args[0] {
            Val::Int(i) => *i,
            other => return Err(anyhow!("hash must be Int, got {:?}", other)),
        };
        let k = match &args[1] {
            Val::Str(s) => s.as_ref(),
            other => return Err(anyhow!("key must be String, got {:?}", other)),
        };
        let digit = match k {
            "a" => 1,
            "b" => 2,
            "c" => 3,
            _ => 9,
        };
        Ok(Val::Int(h * 10 + digit))
    }
    // Program: hash = 0; for (k, _v) in {b:2, a:1, c:3} { hash = update(hash, k) } return hash
    let map = Expr::Map(vec![
        (
            Box::new(Expr::Val(Val::Str("b".into()))),
            Box::new(Expr::Val(Val::Int(2))),
        ),
        (
            Box::new(Expr::Val(Val::Str("a".into()))),
            Box::new(Expr::Val(Val::Int(1))),
        ),
        (
            Box::new(Expr::Val(Val::Str("c".into()))),
            Box::new(Expr::Val(Val::Int(3))),
        ),
    ]);
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "hash".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: ForPattern::Tuple(vec![ForPattern::Variable("k".into()), ForPattern::Ignore]),
                iterable: Box::new(map),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "hash".into(),
                        value: Box::new(Expr::CallExpr(
                            Box::new(Expr::Var("update".into())),
                            vec![Box::new(Expr::Var("hash".into())), Box::new(Expr::Var("k".into()))],
                        )),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("hash".into()))),
            }),
        ],
    };

    let fun = Compiler::new().compile_stmt(&program);
    let mut env = VmContext::new();
    env.define("update", Val::RustFunction(update));
    let out = Vm::new().exec_with(&fun, &mut env, None).unwrap();
    // Keys should be iterated in sorted order: a,b,c -> hash 123
    assert_eq!(out, Val::Int(123));
}

#[test]
fn test_vm_for_range_explicit_negative_step_ascending_zero_iters() {
    // x = 0; for _ in 0..5 step -1 { x += 1 } return x;  // zero iterations
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
                    end: Some(Box::new(Expr::Val(Val::Int(5)))),
                    inclusive: false,
                    step: Some(Box::new(Expr::Val(Val::Int(-1)))),
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
    let out = exec_with_new_vm(&fun);
    assert_eq!(out, Val::Int(0));
}
