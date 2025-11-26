use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::{
    stmt::{Stmt, StmtParser},
    token::Tokenizer,
    vm::{Compiler, Function, Vm, VmContext},
};
use std::hint::black_box;

const HEAVY_CALL_NAMED_SCRIPT: &str = r#"
fn compute(base, delta, { scale: Int? = 2, offset: Int? = 0, weight: Int? = 1, bias: Int? = 0 }) {
    let s = scale ?? 2;
    let o = offset ?? 0;
    let w = weight ?? 1;
    let b = bias ?? 0;
    return (base + delta) * s + o * w + b;
}

fn run(loops) {
    let total = 0;
    let i = 0;
    while (i < loops) {
        total += compute(i, i + 1, scale: 3, offset: i, weight: 2, bias: 7);
        total += compute(i, i + 1, offset: 2, bias: i);
        total += compute(i, i + 1, weight: 4, scale: 5);
        total += compute(i, i + 1, bias: 11);
        i += 1;
    }
    return total;
}

return run(300);
"#;

const DEFAULT_ONLY_SCRIPT: &str = r#"
fn emitter(value, { alpha: Int? = 1, beta: Int? = 2, gamma: Int? = 3, delta: Int? = 4 }) {
    let a = alpha ?? 1;
    let b = beta ?? 2;
    let g = gamma ?? 3;
    let d = delta ?? 4;
    return value * (a + b + g + d);
}

fn run_defaults(loops) {
    let total = 0;
    let i = 0;
    while (i < loops) {
        total += emitter(i, gamma: i + 3);
        total += emitter(i, beta: 5, delta: 7);
        total += emitter(i, alpha: 2, gamma: 4, delta: 8);
        i += 1;
    }
    return total;
}

return run_defaults(320);
"#;

fn compile_script(source: &str) -> Function {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source).expect("tokenize call_named bench script");
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser
        .parse_program_with_enhanced_errors(source)
        .expect("parse call_named bench script");
    Compiler::new().compile_stmt(&Stmt::Block {
        statements: program.statements,
    })
}

fn bench_call_named_heavy(c: &mut Criterion) {
    let function = compile_script(HEAVY_CALL_NAMED_SCRIPT);
    let mut vm = Vm::new();
    c.bench_function("vm_call_named_heavy", |b| {
        b.iter(|| {
            let mut ctx = VmContext::new();
            let value = vm
                .exec_with(&function, &mut ctx, None)
                .expect("execute heavy call_named benchmark");
            black_box(value);
        });
    });
}

fn bench_call_named_default_mix(c: &mut Criterion) {
    let function = compile_script(DEFAULT_ONLY_SCRIPT);
    let mut vm = Vm::new();
    c.bench_function("vm_call_named_default_mix", |b| {
        b.iter(|| {
            let mut ctx = VmContext::new();
            let value = vm
                .exec_with(&function, &mut ctx, None)
                .expect("execute default-heavy call_named benchmark");
            black_box(value);
        });
    });
}

criterion_group!(call_named, bench_call_named_heavy, bench_call_named_default_mix);
criterion_main!(call_named);
