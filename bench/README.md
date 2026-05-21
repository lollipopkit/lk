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
is not a replacement for the documented baseline above.

Command:

```bash
RUNS=10 EXTRA_RUNS=20 PROFILE_WORKLOADS=1 bench/run_workload_bench.sh
```

Date: 2026-05-22

| Workload | LK VM (ms) | LK AOT (ms) | Lua (ms) | VM/Lua | AOT/Lua | AOT/VM | Conf. | Status |
|----------|------------|-------------|----------|--------|---------|--------|-------|--------|
| gcd_batch | 8.912 | 8.280 | 8.380 | 1.063x | 0.988x | 0.929x | low | close |
| prime_trial_division | 0.427 | 2.005 | 0.586 | 0.729x | 3.422x | 4.696x | low | ahead |
| binary_search | 14.195 | 11.827 | 54.009 | 0.263x | 0.219x | 0.833x | low | ahead |
| two_sum_map | 54.648 | 124.643 | 47.089 | 1.161x | 2.647x | 2.281x | low | behind |
| sliding_window_sum | 56.300 | 97.006 | 24.190 | 2.327x | 4.010x | 1.723x | low | behind |
| matrix_3x3_multiply | 4.064 | 5.755 | 1.544 | 2.632x | 3.727x | 1.416x | low | behind |
| stock_max_profit | 36.575 | 23.449 | 9.946 | 3.677x | 2.358x | 0.641x | low | behind |
| histogram_group_count | 69.632 | 116.644 | 48.280 | 1.442x | 2.416x | 1.675x | low | behind |
| string_key_hash | 19.291 | 31.004 | 7.833 | 2.463x | 3.958x | 1.607x | low | behind |
| order_score_pipeline | 9.368 | 8.558 | 3.606 | 2.598x | 2.373x | 0.914x | low | behind |
| log_parse_filter | 77.747 | 101.803 | 230.651 | 0.337x | 0.441x | 1.309x | low | ahead |
| cart_pricing_rules | 5.396 | 5.810 | 2.289 | 2.357x | 2.538x | 1.077x | low | behind |
| route_permission_check | 10.545 | 11.880 | 3.334 | 3.163x | 3.563x | 1.127x | low | behind |
| inventory_reorder | 70.204 | 86.894 | 30.482 | 2.303x | 2.851x | 1.238x | low | behind |
| fraud_rule_scoring | 29.222 | 34.193 | 12.524 | 2.333x | 2.730x | 1.170x | low | behind |

Samples reported: 30 per engine.
Geometric mean VM/Lua ratio: **1.542x**.
AOT geometric mean ratio: **2.053x** vs Lua.
AOT/VM geometric mean ratio: **1.332x**.

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
