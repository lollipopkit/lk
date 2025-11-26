# M1 Prototype Benchmark Summary

This directory houses the checkpoint for **M1 – 原型验证**. Run the helper below to
refresh the register-based VM baseline numbers:

```bash
cargo run -p lkr-core --bin m1_bench_report
```

The command will invoke `cargo bench -p lkr-core --bench scripts_bench` and emit an
aggregated CSV at `docs/perf/m1/latest.csv`. Use `--skip-run` if you only want to
re-summarise existing Criterion outputs.

## 2025-10-24 snapshot

| Scenario        | Mean (ns) | Median (ns) | Std Dev (ns) |
| --------------- | --------: | ----------: | -----------: |
| script_fib      | 9372.877  | 9305.937    | 468.974      |
| repl_sequence   | 6639.221  | 6414.652    | 809.945      |

The register VM path more than doubles throughput on the Fibonacci
macro-benchmark relative to the retired interpreter numbers and delivers ~1.86x
improvement on the REPL-style loop workload. Follow-up work should expand
coverage to additional scripts and correlate with the micro-benches under
`core/benches/`.
