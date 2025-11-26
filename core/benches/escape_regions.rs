use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::{
    stmt::{Stmt, StmtParser},
    token::Tokenizer,
    vm::{AllocationRegion, Compiler, Function, Op, RegionAllocator, Vm, VmContext},
};
use std::{
    hint::black_box,
    sync::{Arc, OnceLock},
};

fn bench_thread_local_allocator(c: &mut Criterion) {
    let allocator = RegionAllocator::new();
    c.bench_function("region/thread_local", |b| {
        b.iter(|| {
            allocator.with_thread_local(64, |slice| {
                slice[0] = 42;
                black_box(slice);
            })
        });
    });
}

fn bench_heap_allocator(c: &mut Criterion) {
    let allocator = RegionAllocator::new();
    c.bench_function("region/heap", |b| {
        b.iter(|| {
            let buf = allocator.allocate_heap(64);
            black_box(buf)
        });
    });
}

fn bench_vm_thread_local_plan(c: &mut Criterion) {
    let function = tls_region_function();
    c.bench_function("region/vm_tls_plan", |b| {
        b.iter(|| {
            let mut vm = Vm::new();
            let mut ctx = VmContext::new();
            let value = vm.exec(function, &mut ctx).expect("vm exec");
            black_box(value);
            assert_eq!(vm.heap_bytes(), 0, "thread-local plan should avoid heap fallback");
        });
    });
}

fn bench_vm_heap_plan(c: &mut Criterion) {
    let function = heap_region_function();
    c.bench_function("region/vm_heap_plan", |b| {
        b.iter(|| {
            let mut vm = Vm::new();
            let mut ctx = VmContext::new();
            let value = vm.exec(function, &mut ctx).expect("vm exec");
            black_box(value);
            assert!(vm.heap_bytes() > 0, "heap plan should record fallback allocations");
        });
    });
}

fn tls_region_function() -> &'static Function {
    static TLS_FN: OnceLock<Function> = OnceLock::new();
    TLS_FN.get_or_init(|| {
        let mut func = compile_region_fixture();
        enforce_thread_local_plan(&mut func);
        func
    })
}

fn heap_region_function() -> &'static Function {
    static HEAP_FN: OnceLock<Function> = OnceLock::new();
    HEAP_FN.get_or_init(|| {
        let mut func = tls_region_function().clone();
        if let Some(analysis) = func.analysis.as_mut() {
            let plan = Arc::make_mut(&mut analysis.region_plan);
            plan.values.resize(func.n_regs as usize, AllocationRegion::Heap);
            for region in plan.values.iter_mut() {
                *region = AllocationRegion::Heap;
            }
            plan.return_region = AllocationRegion::Heap;
        }
        func
    })
}

fn compile_region_fixture() -> Function {
    const SOURCE: &str = r#"
let acc = 0;
let idx = 0;
while (idx < 32) {
    let row = [idx, idx + 1, idx + 2, idx + 3];
    let mapping = {"x": idx, "y": idx + 1};
    acc = acc + row[0] + row[1] + mapping["x"];
    idx = idx + 2;
}
return acc;
"#;
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(SOURCE).expect("tokenize fixture script");
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser
        .parse_program_with_enhanced_errors(SOURCE)
        .expect("parse fixture script");
    let block = Stmt::Block {
        statements: program.statements,
    };
    Compiler::new().compile_stmt(&block)
}

fn enforce_thread_local_plan(func: &mut Function) {
    if let Some(analysis) = func.analysis.as_mut() {
        let plan = Arc::make_mut(&mut analysis.region_plan);
        plan.values.resize(func.n_regs as usize, AllocationRegion::ThreadLocal);
        for op in &func.code {
            if let Some(dst) = match op {
                Op::BuildList { dst, .. } => Some(*dst),
                Op::BuildMap { dst, .. } => Some(*dst),
                _ => None,
            } {
                if let Some(region) = plan.values.get_mut(dst as usize) {
                    *region = AllocationRegion::ThreadLocal;
                }
            }
        }
        // Function returns an Int, keep return heap classification consistent.
        plan.return_region = AllocationRegion::Heap;
    }
}

criterion_group!(
    region_benches,
    bench_thread_local_allocator,
    bench_heap_allocator,
    bench_vm_thread_local_plan,
    bench_vm_heap_plan
);
criterion_main!(region_benches);
