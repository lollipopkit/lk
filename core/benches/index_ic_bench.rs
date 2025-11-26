use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::{expr::Expr, stmt::Stmt, val::Val, vm};
use std::hint::black_box;

fn make_hits_function() -> vm::Function {
    // sum=0; j=0; i=0;
    // while (i < n) { sum = sum + l[j]; i = i + 1; }
    // return sum;
    let cond = Expr::parse_cached_arc("i < n").unwrap();
    let body_sum = Expr::parse_cached_arc("sum + l[j]").unwrap();
    let inc_i = Expr::parse_cached_arc("i + 1").unwrap();
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::Define {
                name: "j".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::While {
                condition: Box::new((*cond).clone()),
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new((*body_sum).clone()),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "i".into(),
                            value: Box::new((*inc_i).clone()),
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
    let params = vec!["l".to_string(), "n".to_string()];
    vm::Compiler::new().compile_function(&params, &[], &program)
}

fn make_misses_function() -> vm::Function {
    // sum=0; j=0; i=0;
    // while (i < n) { sum = sum + l[j]; j = (j + 1) % 1024; i = i + 1; }
    // return sum;
    let cond = Expr::parse_cached_arc("i < n").unwrap();
    let body_sum = Expr::parse_cached_arc("sum + l[j]").unwrap();
    let inc_i = Expr::parse_cached_arc("i + 1").unwrap();
    let inc_j = Expr::parse_cached_arc("(j + 1) % 1024").unwrap();
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::Define {
                name: "j".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::While {
                condition: Box::new((*cond).clone()),
                body: Box::new(Stmt::Block {
                    statements: vec![
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new((*body_sum).clone()),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "j".into(),
                            value: Box::new((*inc_j).clone()),
                            span: None,
                        }),
                        Box::new(Stmt::Assign {
                            name: "i".into(),
                            value: Box::new((*inc_i).clone()),
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
    let params = vec!["l".to_string(), "n".to_string()];
    vm::Compiler::new().compile_function(&params, &[], &program)
}

fn index_ic_bench(c: &mut Criterion) {
    // Prepare a large list to avoid bounds checks impacting misses too early
    let mut list_elems: Vec<Val> = (0..1024).map(|i| Val::Int((i % 10) as i64)).collect();
    // ensure some variety
    list_elems[0] = Val::Int(7);
    let l_val = Val::List(list_elems.into());
    let n_val = Val::Int(20_000);
    let args = [l_val, n_val];

    let f_hits = make_hits_function();
    let f_misses = make_misses_function();

    let mut f_hits_enum = f_hits.clone();
    let mut f_misses_enum = f_misses.clone();
    {
        f_hits_enum.code32 = None;
        f_hits_enum.bc32_decoded = None;
        f_misses_enum.code32 = None;
        f_misses_enum.bc32_decoded = None;
    }

    c.bench_function("index_ic_hits_packed", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&f_hits, &mut env, Some(&args)).unwrap();
            black_box(out);
        })
    });

    c.bench_function("index_ic_misses_packed", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&f_misses, &mut env, Some(&args)).unwrap();
            black_box(out);
        })
    });

    c.bench_function("index_ic_hits_enum", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&f_hits_enum, &mut env, Some(&args)).unwrap();
            black_box(out);
        })
    });

    c.bench_function("index_ic_misses_enum", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&f_misses_enum, &mut env, Some(&args)).unwrap();
            black_box(out);
        })
    });
}

criterion_group!(benches, index_ic_bench);
criterion_main!(benches);
