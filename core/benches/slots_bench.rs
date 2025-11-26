use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::{
    stmt::{Program, Stmt, stmt_parser::StmtParser},
    token,
    val::Val,
    vm,
    vm::VmContext,
};
use std::hint::black_box;

// Build a simple function with many params and locals to exercise slot prebinding
fn build_program(n_params: usize, n_locals: usize) -> Program {
    // fn f(p0, p1, ..., pN) { let a0 = p0; let a1 = p1; ...; return p0; }
    let params: Vec<String> = (0..n_params).map(|i| format!("p{}", i)).collect();
    let mut stmts: Vec<Box<Stmt>> = Vec::new();
    // Build local let statements
    for i in 0..n_locals {
        let pi = format!("p{}", i % n_params.max(1));
        let code = format!("let a{} = {};", i, pi);
        let tokens = token::Tokenizer::tokenize(&code).unwrap();
        let mut p = StmtParser::new(&tokens);
        let s = p.parse_statement().unwrap();
        stmts.push(Box::new(s));
    }
    // return p0;
    if n_params > 0 {
        let code = format!("return {};", params[0]);
        let tokens = token::Tokenizer::tokenize(&code).unwrap();
        let mut p = StmtParser::new(&tokens);
        let s = p.parse_statement().unwrap();
        stmts.push(Box::new(s));
    }
    let body = Stmt::Block { statements: stmts };
    let fun = Stmt::Function {
        name: "f".into(),
        params: params.clone(),
        param_types: Vec::new(),
        named_params: Vec::new(),
        return_type: None,
        body: Box::new(body),
    };
    Program::new(vec![Box::new(fun)]).unwrap()
}

fn bench_call_slots(c: &mut Criterion) {
    // Build a program with a moderately large number of params and locals
    let prog = build_program(8, 64);
    let mut env = vm::VmContext::new();
    // Execute the function definition to bind it in the environment
    let function = vm::compile_program(&prog);
    let mut machine = vm::Vm::new();
    machine.exec_with(&function, &mut env, None).unwrap();
    // Prepare arguments
    let args: Vec<Val> = (0..8).map(|i| Val::Int(i as i64)).collect();

    // Helper to call f(args)
    let call_once = |env: &mut VmContext| {
        if let Some(f) = env.get_value("f") {
            let _ = f.call(&args, env).unwrap();
        }
    };

    // First-call benchmark (includes layout computation and preloading)
    c.bench_function("slots_first_call", |b| {
        b.iter(|| {
            let mut e = env.clone();
            call_once(&mut e);
            black_box(())
        })
    });

    // Subsequent-call benchmark (layout cached via OnceCell)
    c.bench_function("slots_repeated_call", |b| {
        b.iter(|| {
            call_once(&mut env);
            black_box(())
        })
    });
}

criterion_group!(benches, bench_call_slots);
criterion_main!(benches);
