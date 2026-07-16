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
- **Default sampling**: 3 base samples, with 5 adaptive reruns when results are
  noisy or appear regressed against the documented baseline.

## How to Run

```bash
cargo build --profile dist -p lk-cli
bench/run_workload_bench.sh
```

By default the runner measures the same bytecode VM path used by direct
`lk file.lk` execution. LK samples set `LK_FORCE_VM=1` unless
`LK_NATIVE_RUN=1` is explicitly provided, so an exported shell native-cache flag
does not accidentally change the default VM baseline. Use `RUN_AOT=1` to compile
and measure native AOT as an additional explicit engine. If the native backend
is disabled (build without `--features llvm`) or the current workload artifact
is not native lowerable yet, AOT is reported as skipped and the VM/Lua benchmark
still runs. Since the legacy text backend retired, AOT coverage equals the MIR
pipeline's subset.

> **Codegen backend note:** the native codegen was migrated from the string-IR
> renderer (MIR → LLVM text → clang `-O2`) to **Cranelift** (MIR → object;
> `clang` is now only the linker driver). A same-machine, back-to-back run of
> this suite measured **AOT/VM geomean ≈0.288x** on the string-IR path vs
> **≈0.336x** on Cranelift — i.e. Cranelift is **≈17% slower** (0.336/0.288 ≈
> 1.17; `speed` opt level vs clang `-O2`), traded for a typed builder + verifier
> and faster compiles. Checksums still match the VM. (The `≈0.26x` figure in the
> paragraph below is an earlier, separately-dated dist-build measurement under
> different conditions — a historical data point, not the baseline for the ≈17%
> comparison above.)

As of 2026-07-02 the
MIR pipeline compiles the **full 20-workload suite** (module builtins, mutable
globals, range-for, fused arithmetic, string-keyed maps, method dispatch), all
20 checksums match the VM, and a min-of-3 dist-build comparison measured
**AOT/VM geomean ≈0.26x** (string-IR era; previously 0.329x). The former lkrt string-map
bottleneck is fixed: the runtime arena is thread-local (no per-operation
global lock), arena registry and map handles use FxHash, `str.concat_i64`
builds composite/template int-suffix strings in one allocation, and the
`set_ik` map ABI stores composite keys with no key allocation at all. The five
dynamic string-key map workloads (two_sum_map, histogram_group_count,
log_parse_filter, inventory_reorder, event_join_by_id) went from 2.0–3.5x
*slower* than the VM to **0.79–1.04x** (geomean ≈0.89x); scalar/control-flow
workloads stay at 0.02x–0.24x.

The runner executes one workload at a time and prints progress to stderr, so a
slow or stuck workload can be identified directly. Each workload has a timeout
controlled by `BENCH_TIMEOUT` (default `30` seconds, `0` disables it). Set
`BENCH_PROGRESS=0` to keep progress output quiet.

For a higher-confidence baseline refresh:

```bash
RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh
```

To emit machine-readable results for CI or local comparison, set
`BENCH_RESULT_TSV`:

```bash
LUA_BIN=$PWD/lua-5.5.0/src/lua \
  RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 \
  BENCH_RESULT_TSV=/tmp/lk-bench.tsv \
  bash bench/run_workload_bench.sh
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
containing 20 common business/interview-style algorithm workloads. The suite is
intended to prepare opcode design with broad runtime evidence: each workload
mixes language features that naturally occur together instead of targeting one
specific opcode in isolation.

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
| `customer_ltv_segments` | list of map records, field reads, segmentation branches, numeric scoring |
| `event_join_by_id` | dynamic string IDs, map-of-records lookup, nested field reads |
| `config_defaults_merge` | sparse maps, nil/default handling, mixed scalar/string branches |
| `template_render_mix` | template construction, string length/prefix checks, branch-heavy formatting |
| `state_machine_transitions` | state transition branches, repeated comparisons, string state updates |

The suite is intentionally broad: it covers numeric loops, maps, lists, strings,
business-rule branches, config/default handling, joins, templates, and
state-machine style control flow. It should be used to guide generic VM/opcode
work such as operand-shape specialization and branch/materialization reduction,
not to justify workload-specific fused opcodes.

Current opcode evidence: `AddIntI`, `MulIntI`, and `ModIntI` cover real
small-int literal RHS arithmetic paths in this suite. Typed compare-test now
uses compiler control-flow facts to jump directly to the patched target pc and
keeps non-Int fallback code cold. `ConcatN` is enabled for 3+ part template
strings as a generic multi-register string concatenation opcode; it is not tied
to a specific workload. `Return0` and `Return1` are enabled for common
zero-value and single-value return paths while the generic `Return` remains for
multi-return/old bytecode. `Move2` is enabled for adjacent local assignment
chains, including direct-call inline blocks; it reduced `gcd_batch` dynamic
`Move` counts substantially but did not materially change whole-suite geomean.
`TestEqIntI2` is enabled for facts-confirmed `a == K && b == L` small-int
condition pairs; it reduces state-machine compare dispatch but is still a small
interpreter-side improvement rather than a path to the target by itself.
`MinInt` and `MaxInt` are enabled for facts-confirmed integer min/max update
branches such as `if candidate < current { current = candidate }`; they are
general register-write opcodes, not workload-specific fused operations.
`BrEqZeroInt` and `BrNeZeroInt` are enabled for facts-confirmed zero compare
false edges such as `while (x != 0)` and `if (x == 0)`. They replace the
immediate compare-test plus following `Jmp` with one direct `AsBx` branch while
keeping normal comparison expressions as bool-producing expressions.
`BrEqIntI4` and `BrNeIntI4` extend the same idea to facts-confirmed `0..15`
small-int equality branches. They pack the immediate and relative offset into
one `ABx` payload, reducing branch-chain dispatch without adding
workload-specific transition opcodes.
`BrEqInt8` / `BrNeInt8` / `BrLtInt8` / `BrLeInt8` / `BrGtInt8` / `BrGeInt8`
were tested as a register-vs-register short-branch candidate and reverted.
They covered general `Int/Int` compare branches in `binary_search`,
`sliding_window_sum`, and `prime_trial_division`, but the default VM geomean
only moved to `0.887x` and the AOT smoke exposed a separate native dynamic-map
`GetIndex` lowering gap, so the candidate is not part of the default benchmark
path.
The compiler also lowers selected calls directly into their destination local:
`math.floor(Int-like)` writes the proven-integer argument expression into the
target register, and external `map.get(map, key)` writes `GetFieldK`/`GetIndex`
into the target register. This reduces temporary-register `Move` instructions
without adding workload-specific opcodes.
Plain indexed access now uses the same direct-to-destination path, and
facts-confirmed `List<Int> + Int key` access lowers to `GetList` instead of the
fully generic `GetIndex`.
Facts-confirmed typed int-list accumulator access also lowers
`total += values[i]` / `total -= values[j]` to `AddListInt` / `SubListInt` when
the accumulator is an `Int`, the list local is known as `List<Int>`, and the key
is Int-like. This is a generic list-index accumulator shape; it is not a
workload-specific fold opcode.
Facts-confirmed integer midpoint expressions lower
`math.floor((lhs + rhs) / 2)` to `MidInt` when both operands are Int-like. This
covers a common interval/binary-search operand shape without naming any
workload and removes the paired `AddInt + DivInt` dispatch for that expression.
Map writes with direct template keys such as `"n${i}"` now lower to
`SetIndexStrI` when the target is known to be a map and the suffix is proven
`Int`. This avoids materializing the short string key before `.set(...)` while
keeping ordinary dynamic keys on `SetIndex`. A profile-enabled single-sample
run reduced `two_sum_map` opcode steps from about `2.04M` to `1.84M` and
`event_join_by_id` from about `3.78M` to `3.55M`; the default checksum-clean
VM/Lua geomean is now `0.783x`.
Direct `lk file.lk` execution defaults to the bytecode VM. Set
`LK_NATIVE_RUN=1` to opt into the cached native executable fast path when LLVM
lowering succeeds; keep `LK_FORCE_VM=1` for interpreter-only benchmark/profile
runs. A historical opt-in native sample used
`RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30` and was
checksum-clean: cached-native LK/Lua geomean `0.353x` with a cold native cache
dir and prewarm. A historical full run with `RUN_AOT=1` reported cached-native
LK/Lua `0.349x`, AOT/Lua `0.350x`, and AOT/LK `1.001x`. The current full-suite
AOT smoke skips native compilation until loop-after dynamic-map `GetIndex`
lowering is implemented. A
precomputed absolute-target table for every `Jmp` was tested and rejected after
it made `gcd_batch` hit the 30s timeout. `DivIntI` was also tested and rejected
because it covered static division literals but regressed interpreter geomean.
An interpreter peephole that executed a following `ForLoopI` inside hot
`AddInt` / `AddIntI` / `AddMulInt` / `ListPush` handlers was also tested and
reverted: checksum stayed clean, but the default VM geomean regressed to
`0.856x` and `state_machine_transitions` regressed to `1.329x`.
The next higher-leverage interpreter opcode work should target generic
control-flow reduction, loop-carried register writes, or existing index
helper/layout costs rather than workload-specific fused opcodes.

Latest default VM validation:

```bash
RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh
```

This is the direct-execution baseline: `lk file.lk` defaults to the bytecode VM,
and the runner defaults to `RUN_AOT=0` plus `LK_FORCE_VM=1` for LK samples unless
`LK_NATIVE_RUN=1` is explicitly provided, so this command intentionally does not
use cached native execution or AOT. The latest checksum-clean rerun after opcode
renumbering reported LK/Lua geomean `0.783x`; the prior ordering compare-test
dispatch-arm rerun was `0.785x`, the `MidInt`
rerun was `0.792x`, the `AddListInt` / `SubListInt` rerun was `0.797x`, the
previous `Add2Int` rerun was `0.796x`, the previous profile-metric split rerun
was `0.800x`, and the retained `ConcatN` register-window optimization
previously measured `0.798x` / `0.793x`. The
largest remaining VM regressions still include `sliding_window_sum`,
`state_machine_transitions`, `prime_trial_division`, `gcd_batch`, and
`config_defaults_merge`. Small-int branch lowering reduced
`state_machine_transitions` from the previous `2.073x` result while keeping the
optimization generic; `AddMulInt` folds facts-confirmed compound-add multiply
terms into one accumulator write, and `Add2Int` folds adjacent plain Int terms
in compound-add accumulator chains; `AddIntI` / `MulIntI` also cover commuted
small-int immediate arithmetic (`literal + x` / `literal * x`) when facts
confirm the register operand is `Int`; facts-confirmed typed int-list `GetIndex`
reads now try the `TypedList::Int` backing before the generic typed-list element
matcher, `GetList` covers the narrower facts-confirmed `List<Int> + Int key`
shape, `AddListInt` / `SubListInt` cover typed int-list accumulator reads,
known string key lookup on empty `TypedMap::Mixed` now returns `nil`
directly for sparse/default maps, and `ListPush` propagates pushed element kind into list facts without
preserving unsafe static length assumptions.
Continuous `Move` dispatch now uses a tight next-op bounds check instead of
`code.get(...).map(...)` on each adjacent move. Loop scalar const caching now
also covers short template literal parts, so `"prefix${i}"` style keys do not
reload the literal prefix on every iteration.
`ConcatN` now lowers literal, arithmetic, `map.get`, and indexed-access template
parts directly into its contiguous register window instead of first writing a
temporary register and moving it into the window.
`ModInt` / `ModIntI` followed by a same-register `BrEqZeroInt` / `BrNeZeroInt`
now applies that branch inside the modulo arm and skips the next dispatch; this
is a VM peephole over existing bytecode, not a new opcode.
`BrModEqZeroIntI4` / `BrModNeZeroIntI4` additionally cover facts-confirmed
small-divisor divisibility guards like `(x % K) == 0` and `(x % K) != 0` for
`K` in `1..15`, folding `ModIntI + zero branch` into one packed branch without
creating workload-specific fused opcodes.
`MidInt` covers facts-confirmed midpoint expressions like
`math.floor((lo + hi) / 2)` and keeps the current integer division semantics.
`TestLtInt`, `TestLeInt`, `TestGtInt`, and `TestGeInt` now have dedicated VM
dispatch arms for facts-confirmed Int/Int ordering compare-tests, avoiding the
generic compare-test opcode match on hot condition paths without adding new
bytecode.
Template string assignment now lowers `ConcatString` / `ConcatN` directly into
the destination register when possible, avoiding `ConcatString temp; Move dst
temp` on dynamic string-key construction without adding a new opcode.
Nil branches, zero branches, small-int branch/test, and ordinary compare-test
fallthrough bodies shaped as `Move + Jmp` or a single `Move` also execute inside
the current branch handler, reducing branch-chain/default dispatch without
changing bytecode or LLVM lowering.
`GetFieldK` also applies a following same-register `BrNil` / `BrNotNil` inside
the field-read arm, and directly executes a fallthrough default `Move` when
present. Profile validation reduced `config_defaults_merge` `opcode_steps` from
`2386193` to `1772050`, with `BrNotNil` and `Move` dropping out of the top
dynamic opcodes.
The compiler also sinks safe `let/assign x = default; if-chain { x = value }`
patterns into a synthetic final `else` default. This is a branch-chain/register
write optimization rather than a new opcode: it avoids writing the default value
before a branch that immediately overwrites it.

The same VM line also includes a correctness fix for immediate arithmetic:
`AddIntI`, `MulIntI`, and `ModIntI` now index through the current call
`frame_base`, so direct-call callees no longer read or write the entry frame
when executing immediate arithmetic. `AddIntI` / `MulIntI` additionally lower
commuted small-int literal forms without adding new opcodes.
Profile-enabled validation showed `gcd_batch` executing `BrEqZeroInt:737372`,
`config_defaults_merge` executing `BrNeZeroInt:360000`, and
`stock_max_profit` executing `MaxInt:540000` and `MinInt:540000`. The latest
profile-enabled run showed `sliding_window_sum AddListInt:480000`, with opcode
steps dropping from about `7.06M` to `6.15M`. Static coverage now reports
`instructions=1902`, `Jmp=166`, `LoadInt=313`, `AddInt=99`, `SubInt=43`,
`DivInt=16`, `AddIntI=58`, `AddMulInt:10`, `Add2Int:2`, `AddListInt:1`,
`SubListInt:1`, `MidInt:2`, `BrModNeZeroIntI4:21`, `BrNeZeroInt:9`,
`BrNeIntI4:10`, `SetIndexStrI:3`, `GetIndex=62`, `GetList=4`, and `Move=300`.
Typed int sidecar/register cache was tested and rejected after a low-sample
VM-only run regressed to `1.210x`. `ConcatKInt` was also tested as a general
`"prefix${int}"` string construction opcode, but was reverted after the default VM
geomean regressed to `1.073x`.

## Adaptive Rerun Policy

The runner starts with `RUNS` samples. If any workload is more than 3% slower
than the documented baseline, or if sample spread exceeds 8%, it runs
`EXTRA_RUNS` additional full-suite samples before reporting medians.

When `LK_NATIVE_RUN=1` enables cached native execution, the runner prewarms the
LK native cache once before timed samples. This mirrors the AOT compile step
being outside timed workload runs and prevents the first timed sample from
including clang/native build time. Leave `LK_NATIVE_RUN` unset, or set
`LK_FORCE_VM=1`, to measure the pure interpreter path instead. `LK_PREWARM_TIMEOUT` controls the
prewarm timeout and defaults to 120 seconds.

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

## PR Performance Gate

Pull requests run the `Performance Gate / workload-bench` GitHub Actions job.
The job builds the base and head `lk` binaries on the same runner, then uses the
head checkout's benchmark runner to execute the same workload suite against both
binaries. This keeps the comparison independent of older base-branch runner
features while still measuring the base executable.

The hard gate is the geometric mean of per-workload `head_ratio / base_ratio`,
where each ratio is LK time divided by Lua time for that same run. The job fails
when the geomean is more than 10% slower than base. Individual workloads more
than 10% slower are listed in the GitHub step summary, but they do not fail the
job by themselves unless the whole-suite geomean also crosses the threshold.

The workflow downloads Lua 5.5.0 from the official Lua release tarball, builds
it under `head/lua-5.5.0/src/lua`, and uses that binary for both base and head
measurements. It builds each LK checkout with `dist` when that checkout defines
`[profile.dist]`, otherwise it falls back to `release` so older base commits can
still be benchmarked. This matches the benchmark methodology while avoiding
differences from runner-provided Lua packages.

To reproduce the comparator locally after collecting two TSV files:

```bash
python3 bench/compare_workload_bench.py \
  --base /tmp/lk-perf-base.tsv \
  --head /tmp/lk-perf-head.tsv \
  --max-geomean-regression 0.10 \
  --warn-workload-regression 0.10
```

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

The real workload suite is the completion gate for claiming broad performance
improvements. The VM path is now ahead or close on many loop-heavy workloads,
but the current `<0.5x` target is met by the native AOT path, not by the
interpreter VM path.

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

## Historical AOT Validation (2026-06-07)

Command:

```bash
RUN_AOT=1 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

Date: 2026-06-07

This run covers the current 20-workload suite. It validates the architecture-level
native performance path. The first column is the pure interpreter VM from that
run, which matches direct `lk file.lk` default execution. Use `LK_NATIVE_RUN=1`
only when measuring the cached native opt-in path.

| Workload | LK VM (ms) | LK AOT (ms) | Lua (ms) | VM/Lua | AOT/Lua | AOT/VM | Conf. | Checksum |
|----------|------------|-------------|----------|--------|---------|--------|-------|----------|
| gcd_batch | 9.335 | 2.295 | 5.606 | 1.665x | 0.409x | 0.246x | medium | 312000 |
| prime_trial_division | 0.876 | 0.153 | 0.397 | 2.207x | 0.385x | 0.175x | low | 2935471 |
| binary_search | 37.866 | 9.350 | 33.110 | 1.144x | 0.282x | 0.247x | medium | 243950176 |
| two_sum_map | 18.782 | 55.423 | 28.451 | 0.660x | 1.948x | 2.951x | medium | 200000 |
| sliding_window_sum | 18.137 | 2.406 | 14.771 | 1.228x | 0.163x | 0.133x | medium | 653998251 |
| matrix_3x3_multiply | 1.487 | 0.212 | 0.965 | 1.541x | 0.220x | 0.143x | medium | 7973557 |
| stock_max_profit | 11.464 | 1.666 | 6.210 | 1.846x | 0.268x | 0.145x | low | 2974296 |
| histogram_group_count | 25.182 | 55.738 | 30.073 | 0.837x | 1.853x | 2.213x | high | 903000 |
| string_key_hash | 2.330 | 1.736 | 4.921 | 0.473x | 0.353x | 0.745x | medium | 3495227553454 |
| order_score_pipeline | 1.742 | 0.547 | 2.267 | 0.768x | 0.241x | 0.314x | low | 18815414 |
| log_parse_filter | 38.760 | 40.677 | 152.313 | 0.254x | 0.267x | 1.049x | high | 916180 |
| cart_pricing_rules | 1.400 | 0.158 | 1.506 | 0.930x | 0.105x | 0.113x | medium | 2221125 |
| route_permission_check | 3.413 | 0.281 | 2.248 | 1.518x | 0.125x | 0.082x | medium | 6208494 |
| inventory_reorder | 17.859 | 29.097 | 20.006 | 0.893x | 1.454x | 1.629x | medium | 1915398 |
| fraud_rule_scoring | 8.470 | 1.192 | 7.619 | 1.112x | 0.156x | 0.141x | medium | 3242465 |
| customer_ltv_segments | 10.279 | 1.098 | 9.821 | 1.047x | 0.112x | 0.107x | medium | 15510171 |
| event_join_by_id | 29.591 | 69.248 | 32.708 | 0.905x | 2.117x | 2.340x | high | 3855449 |
| config_defaults_merge | 20.506 | 2.447 | 12.234 | 1.676x | 0.200x | 0.119x | medium | 8313856 |
| template_render_mix | 8.350 | 10.661 | 10.146 | 0.823x | 1.051x | 1.277x | medium | 2489053 |
| state_machine_transitions | 8.924 | 0.426 | 2.941 | 3.034x | 0.145x | 0.048x | low | 2108535 |

Samples: 8 per engine.
**Geometric mean VM/Lua: 1.063x**.
**Geometric mean AOT/Lua: 0.351x**.
**Geometric mean AOT/VM: 0.331x**.

The remaining AOT regressions are concentrated in dynamic string-key map
workloads and template-heavy formatting. These should be optimized before
raising confidence with a larger quiet-machine run.

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
cargo build --profile dist -p lk-cli --features vm-profile
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
cargo build --profile dist -p lk-cli --features vm-profile
RUN_AOT=0 RUNS=1 EXTRA_RUNS=0 PROFILE_WORKLOADS=1 BENCH_PROGRESS=0 BENCH_TIMEOUT=10 bash bench/run_workload_bench.sh
```

The profile table now includes `VM Dynamic Opcode Top-6 by Workload`,
`VM Register Write Source Top-6 by Workload`, and `VM Index Key Top-6 by
Workload`. Index-key profiling keeps `dynamic_register_key` as the total and
also breaks it down into `dynamic_int_key`, `dynamic_short_string_key`,
`dynamic_object_key`, and `dynamic_other_key`; the latest profile shows the hot
dynamic map keys are overwhelmingly short strings, so `GetI` / `SetI` is not the
next default opcode direction. The latest coverage profile shows aggregate dynamic immediate
arithmetic coverage in the multi-million range: `MulIntI` covers hot multiply
literal RHS paths and `ModIntI` covers hot modulo literal RHS paths. The
remaining top dispatch pressure is now `AddInt`, `Move`, `ForLoopI`, `Jmp`,
dynamic `ModInt` where the divisor is not a small literal, and typed compare
opcodes.
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

The VM previously had one temporary opcode exception: `Opcode::ForLoopI` reused
the old `Extra = 62` slot to compress static positive/negative-step range loops.
Opcodes are now renumbered into semantic groups within the 7-bit encoding, so
`ForLoopI` no longer occupies that historical slot. This is an encoding and
maintenance refactor; benchmark claims still come from checksum-clean wall-clock
runs. Module artifacts were bumped to version `2` so old raw instruction words
are rejected instead of being decoded under the new numbering.
Continuous `Move` dispatch batching is also enabled; it preserves per-instruction
profile accounting, so `Move` remains visible in dynamic opcode counters even
when adjacent moves are consumed inside one dispatch arm. On the current opcode
baseline, the next-op check is a direct bounds check plus opcode read.

An attempted follow-up that stored static range `step_value` in
`PerfForLoopFact` regressed low-sample geomean and was reverted; `ForLoopI`
continues to read the step register.
Another attempted follow-up split `ForLoopI` into four positive/negative and
inclusive/exclusive opcode shapes. With `LK_FORCE_VM=1 RUN_AOT=0 RUNS=3
EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30`, interpreter geomean regressed
to `1.075x`, so the split was reverted. An earlier continuous `Move` next-op
check experiment regressed on the then-current baseline, but the current
small-int branch baseline validates the direct bounds-check form at `0.905x`.
Moving `DivInt` / `ModInt` zero-divisor construction into a cold helper
regressed to `1.079x` and is not kept as a default optimization.

Template literal parts inside loops are now collected into the same scalar
literal cache as ordinary string literals. Profile-enabled single-sample runs
showed opcode steps dropping from about `2.63M` to `2.24M` in `two_sum_map`,
`4.41M` to `3.99M` in `histogram_group_count`, and `1.08M` to `0.92M` in
`template_render_mix`; this removes dynamic `LoadString` prefix materialization
without adding a workload-specific opcode.

`ConcatN` register-window direct lowering further reduces template/string-heavy
temporary moves without adding a new opcode. In a profile-enabled single-sample
run, `log_parse_filter` dropped from about `4.26M` to `3.62M` opcode steps and
from about `1.39M` to `757K` dynamic `Move`; `template_render_mix` dropped from
about `860K` to `730K` opcode steps and from about `255K` to `125K` dynamic
`Move`.

`Add2Int` is a new general accumulator operand shape for `A += B + C` after
facts prove both RHS terms are Int and the accumulator can be updated in place.
It is paired with the older `AddMulInt` multiply-term shape and is not a
workload-specific transition opcode. Static coverage is only `Add2Int:2`, but
profile-enabled validation shows `state_machine_transitions Add2Int:120000` and
opcode steps dropping from about `1.663M` to `1.543M`; the default VM
checksum-clean benchmark reported `0.796x` vs Lua at that step. The later
`AddListInt` / `SubListInt` typed-list accumulator lowering kept checksums clean
and reported `0.797x` vs Lua while reducing `sliding_window_sum` opcode steps
from about `7.06M` to `6.15M`.

Two dispatch-local loop candidates were also measured and rejected. Fusing
`Move2` / `AddIntI` into a following `Jmp` reduced profile opcode steps in
`gcd_batch` and `binary_search`, but the default release VM regressed to
`0.823x`. Recording static range `step_value` in `ForLoopI` facts removed one
hot-path step-register read, but the default release VM regressed to `0.821x`.

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
hot map accesses: aggregate `generic_map_lookup` is about `84K`, while
`typed_map_direct` is about `2.97M` after const map folding removes some
lookups before runtime. The increase in aggregate generic lookups comes from
`config_defaults_merge`, where sparse/default maps still leave about `69K`
generic lookups after the empty `TypedMap::Mixed` fast path converts many misses
to direct `nil`. Per workload, `two_sum_map` is down to about `2.5K` generic
lookups, `histogram_group_count` about `3.5K`, `log_parse_filter` about `2.2K`,
and `inventory_reorder` about `2.8K`. The next optimization target should move
to general loop materialization, arithmetic temporaries, `Move` elimination, and
branch lowering.
