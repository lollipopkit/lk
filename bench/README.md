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
prints opcode, call, branch, container, BC32 fallback-reason, and clone counters
after the timing table:

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

Date: 2026-05-20

| Workload | LK VM (ms) | LK AOT (ms) | Lua (ms) | VM/Lua | AOT/Lua | AOT/VM | Conf. | Status |
|----------|------------|-------------|----------|--------|---------|--------|-------|--------|
| gcd_batch | 7.891 | 7.650 | 8.522 | 0.926x | 0.898x | 0.969x | high | ahead |
| prime_trial_division | 0.343 | 2.000 | 0.601 | 0.571x | 3.328x | 5.831x | high | ahead |
| binary_search | 13.932 | 11.071 | 48.644 | 0.286x | 0.228x | 0.795x | high | ahead |
| two_sum_map | 63.177 | 115.275 | 42.591 | 1.483x | 2.707x | 1.825x | high | behind |
| sliding_window_sum | 59.579 | 72.040 | 22.197 | 2.684x | 3.245x | 1.209x | high | behind |
| matrix_3x3_multiply | 8.483 | 2.289 | 1.508 | 5.625x | 1.518x | 0.270x | high | behind |
| stock_max_profit | 36.042 | 34.632 | 10.164 | 3.546x | 3.407x | 0.961x | high | behind |
| histogram_group_count | 106.413 | 108.847 | 45.061 | 2.362x | 2.416x | 1.023x | high | behind |
| string_key_hash | 25.580 | 28.273 | 7.050 | 3.628x | 4.010x | 1.105x | high | behind |
| order_score_pipeline | 11.107 | 7.635 | 3.358 | 3.308x | 2.274x | 0.687x | high | behind |
| log_parse_filter | 76.768 | 99.585 | 212.319 | 0.362x | 0.469x | 1.297x | high | ahead |
| cart_pricing_rules | 5.987 | 5.729 | 2.256 | 2.654x | 2.539x | 0.957x | high | behind |
| route_permission_check | 15.815 | 15.334 | 3.212 | 4.924x | 4.774x | 0.970x | high | behind |
| inventory_reorder | 73.059 | 81.492 | 29.465 | 2.480x | 2.766x | 1.115x | high | behind |
| fraud_rule_scoring | 36.332 | 32.469 | 11.840 | 3.069x | 2.742x | 0.894x | high | behind |

Geometric mean VM/Lua ratio: **1.873x**.
AOT geometric mean ratio: **1.986x** vs Lua.
AOT/VM geometric mean ratio: **1.060x**.

## Current Bottlenecks

The real workload suite is the completion gate for claiming broad VM
performance improvements. LK remains behind Lua on every current workload, so
microbenchmark wins are not considered sufficient evidence.

Primary bottlenecks:
- General VM overhead in realistic while loops and function calls
- Integer comparison/modulo dispatch in branch-heavy loops
- `Val` clone/refcount overhead in list/map mutation and iteration
- String conversion and string-key construction
- Map/list memory layout and cache locality

## Files

| File | Description |
|------|-------------|
| `workloads_business_algorithms.lk` | LK real workload suite |
| `workloads_business_algorithms.lua` | Lua equivalent workload suite |
| `run_workload_bench.sh` | Adaptive median runner for the workload suite |
