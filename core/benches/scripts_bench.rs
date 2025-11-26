use criterion::{Criterion, criterion_group, criterion_main};
use lkr_core::perf::scenarios::prepare_script_scenarios;
use lkr_core::vm::Vm;
use std::hint::black_box;

fn bench_script_scenarios(c: &mut Criterion) {
    let scenarios = prepare_script_scenarios().expect("prepare script scenarios for benchmarking");
    for scenario in scenarios {
        let vm_case = scenario.clone();
        c.bench_function(&vm_case.bench_case_name(), move |b| {
            let mut vm = Vm::new();
            b.iter(|| {
                let value = vm_case
                    .run_with_vm(&mut vm)
                    .expect("vm execution failed for benchmarking scenario");
                black_box(value);
            });
        });
    }
}

criterion_group!(scripts, bench_script_scenarios);
criterion_main!(scripts);
