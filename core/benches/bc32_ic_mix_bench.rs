use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::{
    expr::Expr,
    stmt::Stmt,
    val::Val,
    vm::{self, VmContext},
};
use std::hint::black_box;

fn make_ic_mix_function() -> vm::Function {
    // Build program:
    // sum=0; j=0; i=0;
    // while (i < n) {
    //   sum = sum + l[j];        // dynamic Index (IC hits)
    //   sum = sum + m["a"];     // AccessK (no IC on bc32 path, but common op)
    //   sum = sum + g;           // LoadGlobal (IC hits)
    //   sum = sum + inc(sum);    // Rust call (call-site IC)
    //   i = i + 1;
    // }
    // return sum;

    // Pre-parse a few small expressions
    let cond = Expr::parse_cached_arc("i < n").unwrap();
    let sum_add_index = Expr::parse_cached_arc("sum + l[j]").unwrap();
    let sum_add_ma = Expr::parse_cached_arc("sum + m[\"a\"]").unwrap();
    let sum_add_g = Expr::parse_cached_arc("sum + g").unwrap();
    let sum_add_inc_sum = Expr::parse_cached_arc("sum + inc(sum)").unwrap();

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
                        // sum = sum + l[j]
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new((*sum_add_index).clone()),
                            span: None,
                        }),
                        // sum = sum + m["a"]
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new((*sum_add_ma).clone()),
                            span: None,
                        }),
                        // sum = sum + g
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new((*sum_add_g).clone()),
                            span: None,
                        }),
                        // sum = sum + inc(sum)
                        Box::new(Stmt::Assign {
                            name: "sum".into(),
                            value: Box::new((*sum_add_inc_sum).clone()),
                            span: None,
                        }),
                        // i = i + 1
                        Box::new(Stmt::Assign {
                            name: "i".into(),
                            value: Box::new(Expr::parse_cached_arc("i + 1").unwrap().as_ref().clone()),
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

    // Function parameters: (m, l, n)
    let params = vec!["m".to_string(), "l".to_string(), "n".to_string()];
    vm::Compiler::new().compile_function(&params, &[], &program)
}

fn bc32_ic_mix_bench(c: &mut Criterion) {
    // Prepare params
    let mut m = std::collections::HashMap::new();
    m.insert("a".to_string(), Val::Int(5));
    let m_val: Val = m.into();
    let l_val = Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)].into());
    let n_val = Val::Int(5_000);
    let args = [m_val, l_val, n_val];

    // Build function and prepare bc32 and enum variants
    let f_bc32 = make_ic_mix_function();
    let mut f_enum = f_bc32.clone();

    f_enum.code32 = None;
    f_enum.bc32_decoded = None;

    // Prepare environment: define global 'g' and Rust function 'inc'
    fn inc(args: &[Val], _env: &mut VmContext) -> anyhow::Result<Val> {
        let x = match args.get(0) {
            Some(Val::Int(i)) => *i,
            _ => 0,
        };
        Ok(Val::Int(x + 1))
    }

    c.bench_function("bc32_ic_mix_packed", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            env.define("g".to_string(), Val::Int(3));
            env.define("inc".to_string(), Val::RustFunction(inc));
            let out = vm.exec_with(&f_bc32, &mut env, Some(&args)).unwrap();
            black_box(out);
        })
    });

    c.bench_function("bc32_ic_mix_enum", |b| {
        b.iter(|| {
            let mut vm = vm::Vm::new();
            let mut env = vm::VmContext::new();
            env.define("g".to_string(), Val::Int(3));
            env.define("inc".to_string(), Val::RustFunction(inc));
            let out = vm.exec_with(&f_enum, &mut env, Some(&args)).unwrap();
            black_box(out);
        })
    });
}

criterion_group!(benches, bc32_ic_mix_bench);
criterion_main!(benches);
