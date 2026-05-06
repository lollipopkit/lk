use anyhow::Result;
use lkr_core::perf::scenarios::prepare_script_scenarios;
use lkr_core::vm::Vm;
use std::hint::black_box;
use std::time::Instant;

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

fn main() -> Result<()> {
    let scenarios = prepare_script_scenarios()?;
    let samples: usize = std::env::var("LKR_AR_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(25);
    let iters: usize = std::env::var("LKR_AR_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(250);

    let mut total_ns = 0.0;
    let mut total_heap_bytes = 0u64;

    for scenario in scenarios {
        let mut times = Vec::with_capacity(samples);
        let mut heap_bytes = 0u64;
        // Warm reused VM/caches before measuring.
        let mut warm_vm = Vm::new();
        for _ in 0..50 {
            let out = scenario.run_with_vm(&mut warm_vm)?;
            heap_bytes = out.heap_bytes;
            black_box(out.value);
        }

        for _ in 0..samples {
            let mut vm = Vm::new();
            // Warm each sample VM so inline/cache effects match steady state while
            // still charging per-execution frame/context work.
            for _ in 0..10 {
                let out = scenario.run_with_vm(&mut vm)?;
                heap_bytes = out.heap_bytes;
                black_box(out.value);
            }
            let start = Instant::now();
            for _ in 0..iters {
                let out = scenario.run_with_vm(&mut vm)?;
                heap_bytes = out.heap_bytes;
                black_box(out.value);
            }
            let elapsed = start.elapsed().as_nanos() as f64 / iters as f64;
            times.push(elapsed);
        }

        let ns = median(times);
        total_ns += ns;
        total_heap_bytes = total_heap_bytes.saturating_add(heap_bytes);
        println!("METRIC {}_ns={:.3}", scenario.key(), ns);
        println!("METRIC {}_heap_bytes={}", scenario.key(), heap_bytes);
    }

    println!("METRIC total_ns={:.3}", total_ns);
    println!("METRIC total_heap_bytes={}", total_heap_bytes);
    Ok(())
}
