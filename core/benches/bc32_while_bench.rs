use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::{expr::Expr, stmt::Stmt, val::Val, vm};
use std::hint::black_box;

fn make_while_function(n: i64) -> vm::Function {
    // i = 0; while (i < n) { i = i + 1 }; return i
    let cond = Expr::parse_cached_arc(&format!("i < {}", n)).unwrap();
    let incr = Expr::parse_cached_arc("i + 1").unwrap();

    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "i".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::While {
                condition: Box::new((*cond).clone()),
                body: Box::new(Stmt::Block {
                    statements: vec![Box::new(Stmt::Assign {
                        name: "i".into(),
                        value: Box::new((*incr).clone()),
                        span: None,
                    })],
                }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("i".into()))),
            }),
        ],
    };
    vm::Compiler::new().compile_stmt(&program)
}

fn bc32_while_bench(c: &mut Criterion) {
    let f_bc32 = make_while_function(10_000);
    // Force unpacked path copy
    let mut f_enum = f_bc32.clone();

    f_enum.code32 = None;
    f_enum.bc32_decoded = None;

    c.bench_function("bc32_while_packed", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&f_bc32, &mut env, None).unwrap();
            black_box(out);
        })
    });

    c.bench_function("bc32_while_enum", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&f_enum, &mut env, None).unwrap();
            black_box(out);
        })
    });
}

criterion_group!(benches, bc32_while_bench);
criterion_main!(benches);
