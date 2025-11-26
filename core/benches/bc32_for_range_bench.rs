use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::{
    expr::Expr,
    stmt::{self, Stmt},
    val::Val,
    vm,
};
use std::hint::black_box;

fn make_for_range_function(n: i64, inclusive: bool) -> vm::Function {
    // Build: sum = 0; for i in 0..n { sum = sum + 1 } ; return sum
    let for_iter = Expr::Range {
        start: Some(Box::new(Expr::Val(Val::Int(0)))),
        end: Some(Box::new(Expr::Val(Val::Int(n)))),
        inclusive,
        step: None,
    };
    let program = Stmt::Block {
        statements: vec![
            Box::new(Stmt::Define {
                name: "sum".into(),
                value: Box::new(Expr::Val(Val::Int(0))),
            }),
            Box::new(Stmt::For {
                pattern: stmt::ForPattern::Ignore,
                iterable: Box::new(for_iter),
                body: Box::new(Stmt::Block { statements: vec![] }),
            }),
            Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Var("sum".into()))),
            }),
        ],
    };
    vm::Compiler::new().compile_stmt(&program)
}

fn bc32_for_range_bench(c: &mut Criterion) {
    let fun = make_for_range_function(10_000, false);

    // Clone and force normal path by clearing code32
    let mut f_normal = fun.clone();

    f_normal.code32 = None;
    f_normal.bc32_decoded = None;

    // Run bc32 packed path
    c.bench_function("bc32_for_range_packed", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&fun, &mut env, None).unwrap();
            black_box(out);
        })
    });

    // Run enum (unpacked) path
    c.bench_function("bc32_for_range_enum", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            let out = vm.exec_with(&f_normal, &mut env, None).unwrap();
            black_box(out);
        })
    });
}

criterion_group!(benches, bc32_for_range_bench);
criterion_main!(benches);
