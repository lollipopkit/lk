# LK vs Lua Real Workload Benchmark Suite

This directory intentionally keeps only real workload benchmarks. Synthetic
microbenchmarks were removed because they can produce misleading `0ms` results
after static folding, inlining, or other compile-time optimizations. The
remaining suite measures common CPU/memory-heavy scripting workloads that must
execute real runtime work.

## Test Environment

- **LK**: Rust VM, `--release` build
- **Lua**: Lua 5.5.0 (PUC-Rio reference interpreter)
- **Methodology**: Each workload measures internal elapsed time, excluding
  process startup overhead.
- **Correctness gate**: LK and Lua checksums must match for every workload.
- **Default sampling**: 5 base samples, with adaptive reruns when results are
  noisy or appear regressed against the documented baseline.

## How to Run

```bash
cargo build --release -p lk-cli
bench/run_workload_bench.sh
```

For a higher-confidence baseline refresh:

```bash
RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh
```

For VM-side diagnostics, enable one extra filtered LK run per workload. This
prints opcode, call, branch, container, BC32 fallback-reason, clone counters,
and copy-policy heap-clone source counters after the timing table:

```bash
PROFILE_WORKLOADS=1 bench/run_workload_bench.sh
```

To run only one LK workload directly, set `LK_WORKLOAD_FILTER`:

```bash
LK_WORKLOAD_FILTER=two_sum_map target/release/lk bench/workloads_business_algorithms.lk
```

## Workloads

`run_workload_bench.sh` runs one LK script and one equivalent Lua script, each
containing 15 common business/interview-style algorithm workloads.

| Workload | What it stresses |
|----------|------------------|
| `gcd_batch` | tight while loops, function calls, modulo |
| `prime_trial_division` | nested numeric loops, branch-heavy modulo |
| `binary_search` | repeated function calls and integer comparisons |
| `two_sum_map` | string keys, map set/get, template strings |
| `sliding_window_sum` | list push, indexed access, rolling arithmetic |
| `matrix_3x3_multiply` | dense scalar arithmetic and register pressure |
| `stock_max_profit` | branch-heavy single-pass scan |
| `histogram_group_count` | map mutation, map lookup, string-key construction |
| `string_key_hash` | template strings, string iteration, hashing loop |
| `order_score_pipeline` | small business function pipeline |
| `log_parse_filter` | log line construction, split, field extraction, grouped counters |
| `cart_pricing_rules` | cart pricing, map lookups, discounts, tax rules |
| `route_permission_check` | permission checks, path prefix matching, branch-heavy routing |
| `inventory_reorder` | inventory aggregation, list building, map update, join |
| `fraud_rule_scoring` | rule scoring, map membership, string prefix checks |

## Adaptive Rerun Policy

The runner starts with `RUNS` samples. If any workload is more than 3% slower
than the documented baseline, or if sample spread exceeds 8%, it runs
`EXTRA_RUNS` additional full-suite samples before reporting medians.

Status thresholds:
- `ahead`: LK/Lua <= 0.95x
- `close`: LK/Lua <= 1.10x
- `behind`: LK/Lua > 1.10x

Confidence uses `max((p80 - p20) / median)` across LK and Lua samples:
- `high`: <= 3%
- `medium`: <= 8%
- `low`: > 8%

Low-confidence rows should be rerun on a quieter machine before making
fine-grained claims.

## Current Baseline

The documented baseline below used `RUNS=10 EXTRA_RUNS=20` and covers the
original 10 workloads. The newer mixed real-world workloads are included in the
runner and correctness gate, but their regression baselines should be recorded
after a dedicated quiet-machine refresh.

| Workload | LK (ms) | Lua (ms) | Ratio (LK/Lua) | Conf. | Status |
|----------|---------|----------|----------------|-------|--------|
| gcd_batch | 38.844 | 5.314 | 7.310x | medium | behind |
| prime_trial_division | 2.176 | 0.394 | 5.523x | low | behind |
| binary_search | 108.927 | 32.374 | 3.365x | medium | behind |
| two_sum_map | 36.614 | 29.406 | 1.245x | medium | behind |
| sliding_window_sum | 61.456 | 14.466 | 4.248x | medium | behind |
| matrix_3x3_multiply | 6.520 | 1.004 | 6.494x | low | behind |
| stock_max_profit | 32.758 | 6.566 | 4.989x | low | behind |
| histogram_group_count | 63.045 | 30.078 | 2.096x | medium | behind |
| string_key_hash | 12.338 | 5.368 | 2.298x | medium | behind |
| order_score_pipeline | 10.271 | 2.228 | 4.610x | low | behind |

Geometric mean ratio for this run: **3.727x**.

## Latest Quick Comparison

This table records the latest per-iteration validation run after the current
implementation round. It is useful for spotting large direction changes, but it
is not a replacement for the documented baseline above because it uses only one
sample per engine.

Command:

```bash
RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh
```

Date: 2026-05-21

| Workload | LK VM (ms) | LK AOT (ms) | Lua (ms) | VM/Lua | AOT/Lua | AOT/VM | Conf. | Status |
|----------|------------|-------------|----------|--------|---------|--------|-------|--------|
| gcd_batch | 8.044 | 7.846 | 8.108 | 0.992x | 0.968x | 0.975x | high | close |
| prime_trial_division | 0.413 | 1.794 | 0.539 | 0.766x | 3.328x | 4.344x | high | ahead |
| binary_search | 13.914 | 10.846 | 50.464 | 0.276x | 0.215x | 0.780x | high | ahead |
| two_sum_map | 61.061 | 115.144 | 41.891 | 1.458x | 2.749x | 1.886x | high | behind |
| sliding_window_sum | 54.439 | 71.670 | 22.108 | 2.462x | 3.242x | 1.317x | high | behind |
| matrix_3x3_multiply | 6.109 | 2.262 | 1.467 | 4.164x | 1.542x | 0.370x | high | behind |
| stock_max_profit | 32.279 | 34.253 | 9.934 | 3.249x | 3.448x | 1.061x | high | behind |
| histogram_group_count | 93.568 | 107.289 | 44.218 | 2.116x | 2.426x | 1.147x | high | behind |
| string_key_hash | 24.111 | 28.266 | 7.232 | 3.334x | 3.908x | 1.172x | high | behind |
| order_score_pipeline | 8.497 | 7.561 | 3.345 | 2.540x | 2.260x | 0.890x | high | behind |
| log_parse_filter | 68.583 | 100.385 | 210.904 | 0.325x | 0.476x | 1.464x | high | ahead |
| cart_pricing_rules | 4.777 | 5.583 | 2.354 | 2.029x | 2.372x | 1.169x | high | behind |
| route_permission_check | 13.913 | 15.176 | 3.431 | 4.055x | 4.423x | 1.091x | high | behind |
| inventory_reorder | 67.513 | 81.054 | 29.956 | 2.254x | 2.706x | 1.201x | high | behind |
| fraud_rule_scoring | 26.855 | 32.576 | 11.547 | 2.326x | 2.821x | 1.213x | high | behind |

Geometric mean VM/Lua ratio: **1.687x**.
AOT geometric mean ratio: **1.973x** vs Lua.
AOT/VM geometric mean ratio: **1.170x**.

## Current Bottlenecks

The real workload suite is the completion gate for claiming broad VM
performance improvements. LK is now ahead or close on several loop-heavy
workloads, but the geometric mean is still behind Lua, so microbenchmark wins
are not considered sufficient evidence.

Primary bottlenecks:
- General VM overhead in realistic while loops and function calls
- Integer comparison/modulo dispatch in branch-heavy loops
- `Val` clone/refcount overhead in list/map mutation and iteration
- Local-slot copies are now measured separately (`LocalHeap`) so alias-safe
  ownership work can target them without hiding them inside generic register
  copies
- String conversion and string-key construction
- Map/list memory layout and cache locality

## Files

| File | Description |
|------|-------------|
| `workloads_business_algorithms.lk` | LK real workload suite |
| `workloads_business_algorithms.lua` | Lua equivalent workload suite |
| `run_workload_bench.sh` | Adaptive median runner for the workload suite |
