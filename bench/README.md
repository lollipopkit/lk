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

By default the runner tries to compile and measure native AOT as an additional
engine. If LLVM is disabled or the current workload artifact is not native
lowerable yet, AOT is reported as skipped and the VM/Lua benchmark still runs.
Use `RUN_AOT=0` to skip the compile attempt entirely.

The runner executes one workload at a time and prints progress to stderr, so a
slow or stuck workload can be identified directly. Each workload has a timeout
controlled by `BENCH_TIMEOUT` (default `30` seconds, `0` disables it). Set
`BENCH_PROGRESS=0` to keep progress output quiet.

For a higher-confidence baseline refresh:

```bash
RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh
```

For VM-side diagnostics, enable one extra filtered LK run per workload. This
prints VM opcode, call, branch, container, copy-policy, heap-value movement,
dynamic top-opcode, register-write source, and index-key counters after the
timing table:

```bash
PROFILE_WORKLOADS=1 bench/run_workload_bench.sh
```

To run only one LK workload directly, set `LK_WORKLOAD_FILTER`:

```bash
LK_WORKLOAD_FILTER=two_sum_map target/release/lk bench/workloads_business_algorithms.lk
```

The Lua comparison script supports the same filter:

```bash
LK_WORKLOAD_FILTER=two_sum_map lua bench/workloads_business_algorithms.lua
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
RUN_AOT=0 RUNS=3 EXTRA_RUNS=3 bash bench/run_workload_bench.sh
```

Date: 2026-05-23

| Workload | LK VM (ms) | Lua (ms) | VM/Lua | Conf. | Status |
|----------|------------|----------|--------|-------|--------|
| gcd_batch | 145.162 | 8.033 | 18.071x | medium | behind |
| prime_trial_division | 9.599 | 0.593 | 16.187x | low | behind |
| binary_search | 1226.532 | 49.207 | 24.926x | medium | behind |
| two_sum_map | 858.071 | 41.795 | 20.530x | high | behind |
| sliding_window_sum | 699.602 | 22.045 | 31.735x | high | behind |
| matrix_3x3_multiply | 20.675 | 1.471 | 14.055x | low | behind |
| stock_max_profit | 202.318 | 9.773 | 20.702x | low | behind |
| histogram_group_count | 1056.039 | 44.648 | 23.653x | medium | behind |
| string_key_hash | 63.804 | 7.137 | 8.940x | high | behind |
| order_score_pipeline | 56.584 | 3.352 | 16.881x | low | behind |
| log_parse_filter | 964.967 | 224.930 | 4.290x | medium | behind |
| cart_pricing_rules | 57.223 | 2.280 | 25.098x | high | behind |
| route_permission_check | 82.670 | 3.221 | 25.666x | medium | behind |
| inventory_reorder | 591.418 | 29.356 | 20.146x | high | behind |
| fraud_rule_scoring | 213.840 | 11.705 | 18.269x | high | behind |

Samples reported: 6 per engine.
Geometric mean VM/Lua ratio: **17.648x**.
This validation run measures the VM artifact path, not native AOT.

## Current Bottlenecks

The real workload suite is the completion gate for claiming broad VM
performance improvements. LK is now ahead or close on several loop-heavy
workloads, but the geometric mean is still behind Lua, so microbenchmark wins
are not considered sufficient evidence.

Primary bottlenecks:
- General VM overhead in realistic while loops and function calls
- Integer comparison/modulo dispatch in branch-heavy loops
- Runtime heap-value copy pressure in list/map mutation and iteration
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

## Latest Validation (2026-06-04)

Command:

```bash
RUN_AOT=0 RUNS=6 EXTRA_RUNS=6 bash bench/run_workload_bench.sh
```

Date: 2026-06-04

| Workload | LK VM (ms) | Lua (ms) | VM/Lua | Noise | Conf. | Status |
|----------|------------|----------|--------|-------|-------|--------|
| gcd_batch | 14.397 | 5.439 | 2.647x | 0.153 | low | behind |
| prime_trial_division | 0.799 | 0.395 | 2.023x | 0.161 | low | behind |
| binary_search | 38.311 | 32.751 | 1.170x | 0.142 | low | behind |
| two_sum_map | 91.223 | 28.028 | 3.255x | 0.019 | high | behind |
| sliding_window_sum | 29.211 | 14.561 | 2.006x | 0.065 | medium | behind |
| matrix_3x3_multiply | 2.509 | 0.992 | 2.529x | 0.153 | low | behind |
| stock_max_profit | 13.566 | 6.548 | 2.072x | 0.095 | low | behind |
| histogram_group_count | 117.940 | 29.347 | 4.019x | 0.029 | high | behind |
| string_key_hash | 8.220 | 4.881 | 1.684x | 0.067 | medium | behind |
| order_score_pipeline | 3.871 | 2.246 | 1.724x | 0.195 | low | behind |
| log_parse_filter | 223.026 | 149.307 | 1.494x | 0.017 | high | behind |
| cart_pricing_rules | 2.415 | 1.504 | 1.606x | 0.088 | low | behind |
| route_permission_check | 8.813 | 2.214 | 3.981x | 0.085 | low | behind |
| inventory_reorder | 69.728 | 19.703 | 3.539x | 0.057 | medium | behind |
| fraud_rule_scoring | 15.994 | 7.777 | 2.057x | 0.051 | medium | behind |

Samples: 12 per engine.
**Geometric mean LK/Lua: 2.235x**.

This is a substantial improvement from the 17.648x baseline on 2026-05-23, driven by
the bytecode VM rewrite, shared-stack call ABI, typed containers, and inline caching.
The remaining gap (target ≤1.10x) is primarily due to:
- Per-opcode GC check overhead
- Typed container fast path not yet reducing branch count
- String interning not yet implemented
- Dispatch loop overhead (measurement-enabled checks)

## Latest Low-Sample Direction Check (2026-06-06)

Command:

```bash
RUN_AOT=0 RUNS=1 EXTRA_RUNS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

This single-sample run is only for direction finding after runner instrumentation;
it is not a replacement for the 2026-06-04 baseline.

- Samples: 1 per engine
- Geometric mean LK/Lua: **1.330x**
- Ahead/close workloads: `binary_search`, `two_sum_map`, `histogram_group_count`, `inventory_reorder`
- Largest remaining ratios: `route_permission_check`, `stock_max_profit`, `fraud_rule_scoring`, `prime_trial_division`, `sliding_window_sum`

Profile-enabled direction check:

```bash
cargo build --release -p lk-cli --features vm-profile
RUN_AOT=0 RUNS=1 EXTRA_RUNS=0 PROFILE_WORKLOADS=1 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

The latest profile run reports meaningful list/map/string buckets for
`GetIndex` / `SetIndex`: `sliding_window_sum` has about `1.39M` list ops,
`histogram_group_count` about `742K` map ops, `route_permission_check` about
`90K` map ops, and `string_key_hash` about `144K` string ops.

## Latest Low-Sample Direction Check (2026-06-07)

Command:

```bash
RUN_AOT=0 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

This low-sample run is only for direction finding. It used a normal release
build, not a `vm-profile` feature build.

- Samples: 3 per engine
- Geometric mean LK/Lua: **0.971x**
- Checksums: all matched
- Ahead workloads: `binary_search`, `two_sum_map`, `histogram_group_count`,
  `inventory_reorder`, `order_score_pipeline`, `cart_pricing_rules`
- Close workloads: `stock_max_profit`, `string_key_hash`,
  `route_permission_check`
- Largest remaining ratios: `matrix_3x3_multiply`, `prime_trial_division`,
  `sliding_window_sum`, `log_parse_filter`, `fraud_rule_scoring`,
  `gcd_batch`

Profile-enabled direction check:

```bash
cargo build --release -p lk-cli --features vm-profile
RUN_AOT=0 RUNS=1 EXTRA_RUNS=0 PROFILE_WORKLOADS=1 BENCH_PROGRESS=0 BENCH_TIMEOUT=10 bash bench/run_workload_bench.sh
```

The profile table now includes `VM Dynamic Opcode Top-6 by Workload`,
`VM Register Write Source Top-6 by Workload`, and `VM Index Key Top-6 by
Workload`. The latest coverage profile shows aggregate dynamic `LoadInt`
at about `1.30M`; the remaining top dispatch pressure is now `AddInt`
(`~8.10M`), `Move` (`~6.06M`), `ModInt` (`~4.99M`), `MulInt` (`~4.48M`),
`ForLoopI` (`~3.25M`), `Jmp` (`~3.19M`), and typed compare opcodes.
Map/list/string-heavy workloads still show the expected `GetIndex`, `SetIndex`,
`ConcatString`, and `LoadString` pressure, but readonly string-int const map
lookups can now fold before `GetIndex`.

The latest write-source counters show aggregate `arithmetic` writes at about
`21.85M`, `move` at about `6.06M`, `const_load` at about `3.37M`, `string` at
about `2.55M`, and `index` at about `2.21M`. `binary_search` and `gcd_batch`
now have only trivial `const_load` pressure, while `fraud_rule_scoring` still
has about `97K` `const_load` writes plus significant arithmetic/string/index
pressure. This moves the next optimization target toward arithmetic immediates,
compare branch lowering, `Move` elimination, and string/index-result
consumption rather than workload-specific opcodes.

The VM currently has one temporary opcode exception: `Opcode::ForLoopI` reuses
the old `Extra = 62` slot to compress static positive/negative-step range loops.
It is not the long-term opcode encoding migration described in `OPCODE.md`.
Continuous `Move` dispatch batching is also enabled; it preserves per-instruction
profile accounting, so `Move` remains visible in dynamic opcode counters even
when adjacent moves are consumed inside one dispatch arm.

An attempted follow-up that stored static range `step_value` in
`PerfForLoopFact` regressed low-sample geomean and was reverted; `ForLoopI`
continues to read the step register.

The normal release VM now uses a zero-sized no-op runtime profile frame when
`vm-profile` is not enabled; opcode histogram, register write source, and
index-key arrays are only allocated in tests or profile builds. Fused branch
helpers also use the current frame's local metrics flag on the hot compare path.
An attempted absolute jump-target representation for fused branch facts kept
checksums correct but regressed release wall-clock timing, so the compact jump
offset representation is retained.

Heap string `GetIndex` now has a static-fact fast path for integer keys, so
string indexing no longer falls through the generic heap-index slow path when
the compiler already knows the target is a string. A subsequent attempt to force
inline `Test` / `Jmp`, and another attempt to collapse fused-branch fact lookup
to one direct slot read, both kept checksums correct but did not improve release
geomean, so those control-flow micro-optimizations were reverted.

The current compiler lowering skips scalar constant loads at the front of both
normal `while` conditions and direct-inline `while` conditions on loop-back, and
now also keeps loop body scalar literals in a loop-local cache for normal
`while`, direct-inline `while`, and range `for`. Cached literal registers are
kept live and treated as non-consumable sources so container writes cannot turn
them into `nil`.

The compiler also tracks readonly local const maps outside loop bodies and folds
`map.get(local, "literal")` to a scalar load when the key is a string literal and
the value is an int. Mutation paths (`.set`, rewritten set-index, assignment)
clear the fact, and loop-local maps are not recorded. This removes the hot
`role_levels` lookup from `route_permission_check`; the latest low-sample ratio
for that workload is about `1.18x`, down from the previous `~1.58x` direction
check.

Straight-line simple local assignment now rebinds `a = b` without emitting a
`Move`, with copy-on-write when either alias is written later. This is currently
disabled inside control-flow and loop bodies because those paths need explicit
phi/loop-carried slot lowering; current workload `Move` pressure therefore
remains dominated by loop/control-flow copies rather than straight-line copies.
An attempted tail-condition lowering for block-body `while` loops reduced
dynamic `Jmp` counts but regressed release wall-clock timing, so that path was
rolled back; future loop work should use phi/native hot-loop lowering instead
of rearranging interpreter branch shape alone.

The latest index-key counters show that string-map direct lookup now covers most
hot map accesses: aggregate `generic_map_lookup` remains about `19.5K`, while
`typed_map_direct` is about `1.99M` after const map folding removes some
lookups before runtime. Per workload,
`two_sum_map` is down to about `2.5K` generic lookups, `histogram_group_count`
about `7K`, `log_parse_filter` about `4.4K`, and `inventory_reorder` about
`5.6K`. The next optimization target should move to general loop
materialization, arithmetic temporaries, `Move` elimination, and branch lowering.
