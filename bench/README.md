# LK vs Lua Performance Benchmark Suite

Comparing the LK VM (Rust-based bytecode interpreter) against the standard Lua 5.5.0 interpreter.

## Test Environment

- **LK**: Rust VM, `--release` build (optimized, no debug info)
- **Lua**: Lua 5.5.0 (PUC-Rio reference interpreter)
- **Methodology**: Each benchmark measures internal elapsed time (excluding process startup overhead). Median of 7 runs reported.
- **Total iterations**: Each benchmark runs ≥10,000 iterations (inner loops), ensuring statistical significance.

## Benchmarks

| Benchmark | Description | Total iterations |
|-----------|-------------|-----------------|
| Empty Loop | Pure loop + counter increment | 100,000 |
| Fibonacci Iterative | Iterative fib(n=30) | 50,000 × 30 loops |
| Fibonacci Recursive | Recursive fib(n=15) | 5,000 × 610 calls |
| Function Call | Simple `add(a,b)` dispatch | 100,000 |
| Empty Function Call | Minimal `one()` dispatch | 1,000,000 |
| List Ops | Push + iterate list of 100 | 10,000 × 100 |
| Map Ops | Set + iterate map of 50 | 10,000 × 50 |
| String Concat | Repeated `"x" + "x"` | 10,000 × 100 |
| Closure Call | Closure capture + dispatch | 100,000 |
| Closure No Capture | Closure dispatch without captured values | 1,000,000 |
| Closure Create No Capture+Call | Create no-capture closure and call it once per loop | 100,000 |
| Closure Create+Call | Create one closure and call it once per loop | 100,000 |
| Dynamic Empty Loop | Runtime-known loop count + escaped result | 1,000,000 |
| Dynamic Numeric Loop Varying | Runtime-known loop count, per-iteration `%` + add | 1,000,000 |
| Dynamic Function Call | Runtime-known arguments + escaped result | 100,000 |
| Dynamic Function Call Varying | Runtime-known arguments, per-iteration changing call result | 100,000 |
| Dynamic Function Call Generic | Runtime-known arguments, non-tiny arithmetic call per iteration | 100,000 |
| Dynamic Fibonacci Iterative | Runtime-known fib input + escaped result | 50,000 × 30 loops |
| Dynamic Fibonacci Recursive | Runtime-known recursive fib input + escaped result | 5,000 × 610 calls |
| Dynamic List Ops | Push + iterate list with escaped result | 10,000 × 100 |
| Dynamic List Ops Varying | Push + iterate list with per-outer-iteration varying values | 5,000 × 100 |
| Dynamic Map Ops | Set + iterate map with escaped result | 10,000 × 50 |
| Dynamic Map Ops Varying | Set + iterate map with per-outer-iteration varying values | 3,000 × 50 |

## How to Run

```bash
cd /Users/lk/proj/lk
cargo build --release -p lk-cli
bash bench/run_bench.sh
bash bench/run_workload_bench.sh
```

For startup overhead measurement:
```bash
bash bench/bench_startup.sh
```

## Sample Results

| Benchmark | LK (ms) | Lua (ms) | Ratio (LK/Lua) |
|-----------|---------|----------|----------------|
| Empty Loop | 0 | 0.3 | 0x |
| Fibonacci Iterative | 0 | 6.4 | 0x |
| Fibonacci Recursive | 0 | 84.3 | 0x |
| Function Call | 0 | 0.9 | 0x |
| Empty Function Call | 0 | 6.6 | 0x |
| List Ops | 0 | 25.3 | 0x |
| Map Ops | 0 | 59.9 | 0x |
| String Concat | 48 | 40.3 | 1.19x |
| Closure Call | 0 | 1.0 | 0x |
| Closure No Capture | 0 | 9.0 | 0x |
| Closure Create No Capture+Call | 2 | 2.8 | 0.71x |
| Closure Create+Call | 1 | 6.1 | 0.16x |
| Dynamic Empty Loop | 0 | 2.7 | 0x |
| Dynamic Numeric Loop Varying | 4.378 | 3.783 | 1.15x |
| Dynamic Function Call | 1 | 0.9 | 1.11x |
| Dynamic Function Call Varying | 1.014 | 1.295 | 0.78x |
| Dynamic Function Call Generic | 2.111 | 1.983 | 1.06x |
| Dynamic Fibonacci Iterative | 0 | 6.0 | 0x |
| Dynamic Fibonacci Recursive | 4 | 84.7 | 0.04x |
| Dynamic List Ops | 0 | 25.8 | 0x |
| Dynamic List Ops Varying | 12.737 | 14.433 | 0.88x |
| Dynamic Map Ops | 0 | 58.9 | 0x |
| Dynamic Map Ops Varying | 14.588 | 18.252 | 0.79x |

> `0ms` means the benchmark is below the current integer-millisecond timer resolution after static folding/inlining. Treat it as "less than 1ms", not as a precise zero-cost operation.
> Some `Dynamic *` benchmarks still become cheap because their loop body is aggregate-able or loop-invariant. The `* Varying` benchmarks are the stricter VM-performance signal: every outer iteration changes data and must execute real numeric/call/list/map work.

### Startup Overhead
- LK: ~4.5ms
- Lua: ~3.0ms
- Ratio: ~1.5x

## Business Algorithm Workloads

`run_workload_bench.sh` runs one LK script and one equivalent Lua script, each containing 10 common business/interview-style algorithm workloads. Each workload prints a checksum and elapsed time. The runner starts with 5 samples; if a workload looks regressed by more than 3% against the documented baseline, or if sample spread is above 8%, it automatically runs 10 more full-suite samples before reporting medians. Checksum mismatches fail the script. The result below used `RUNS=10 EXTRA_RUNS=20` because several workloads were noisy enough to need higher confidence before updating the baseline.

Status thresholds:
- `ahead`: LK/Lua <= 0.95x
- `close`: LK/Lua <= 1.10x
- `behind`: LK/Lua > 1.10x

Confidence uses `max((p80 - p20) / median)` across LK and Lua samples. Treat <=3% as high confidence, <=8% as medium confidence, and anything above 8% as low confidence that needs more samples or a quieter machine before making fine-grained claims.

| Workload | LK (ms) | Lua (ms) | Ratio (LK/Lua) | Conf. | Status | What it stresses |
|----------|---------|----------|----------------|-------|--------|------------------|
| gcd_batch | 38.844 | 5.314 | 7.310x | medium | behind | tight while loops, function calls, modulo |
| prime_trial_division | 2.176 | 0.394 | 5.523x | low | behind | nested numeric loops, branch-heavy modulo |
| binary_search | 108.927 | 32.374 | 3.365x | medium | behind | repeated function calls and integer comparisons |
| two_sum_map | 36.614 | 29.406 | 1.245x | medium | behind | string keys, map set/get, template strings |
| sliding_window_sum | 61.456 | 14.466 | 4.248x | medium | behind | list push, indexed access, rolling arithmetic |
| matrix_3x3_multiply | 6.520 | 1.004 | 6.494x | low | behind | dense scalar arithmetic and register pressure |
| stock_max_profit | 32.758 | 6.566 | 4.989x | low | behind | branch-heavy single-pass scan |
| histogram_group_count | 63.045 | 30.078 | 2.096x | medium | behind | map mutation, map lookup, string-key construction |
| string_key_hash | 12.338 | 5.368 | 2.298x | medium | behind | template strings, string iteration, hashing loop |
| order_score_pipeline | 10.271 | 2.228 | 4.610x | low | behind | small business function pipeline |

Geometric mean ratio for this run: **3.727x**.

These workloads are intentionally harder to optimize away than the microbenchmarks. They show that LK's current static folding and packed hot paths can beat Lua on selected synthetic cases, but the general VM is still behind Lua on realistic CPU/memory-heavy code.

## Analysis

**Where LK is closest:**
- **List ops** (<1ms on this benchmark) — local-only counted loop is removed because the computed list/sum do not escape
- **Map ops** (<1ms on this benchmark) — local-only map construction and values iteration are removed because the computed sum does not escape
- **Empty loop** (<1ms on this benchmark) — constant counted compound assignment is precomputed
- **Captured closure create+call** (~0.15x) — immediate dynamic factory+call blocks inline to `AddInt`
- **Closure create no capture+call** (~0.71x) — zero-capture closure literals reuse a proto-level closure instance and simple calls inline
- **Closure no capture** (<1ms on this benchmark) — no-capture closure variables can be registered as compile-time closures for self-assignment inlining
- **Function call** (<1ms on this benchmark) — simple pure self-assignment calls such as `acc = add(acc, 1)` inline to `AddIntImm`
- **Fibonacci iterative** (<1ms on this benchmark) — constant-argument safe function calls fold before runtime timing starts
- **Fibonacci recursive** (<1ms on this benchmark) — bounded compile-time recursive folding handles pure constant-argument recursion
- **Closure call** (<1ms on this benchmark) — const-captured closures such as `make_adder(1)` can inline to `AddIntImm`
- **Empty function call** (<1ms on this benchmark) — safe constant zero-arg calls in arithmetic updates fold to `AddIntImm`
- **Dynamic Fibonacci iterative** (<1ms on this benchmark) — loop-invariant pure calls are computed once and cached across range iterations
- **Dynamic Fibonacci recursive** (~0.05x) — the same loop-invariant call cache avoids repeated recursive frame setup
- **Dynamic list ops** (<1ms on this benchmark) — loop-invariant local delta is computed once and added to the escaped accumulator on later iterations
- **Dynamic map ops** (<1ms on this benchmark) — the same local-delta cache avoids rebuilding the same per-iteration map
- **Dynamic empty loop** (<1ms on this benchmark) — ignored range loops with `target += imm` compile to one runtime range-count add
- **Dynamic function call** (~1.11x) — simple `return a + b` self-assignment calls inline even with runtime-known RHS values
- **String concatenation** (~1.19x) — near Lua but not faster on this machine
- **Dynamic function call varying** (~0.78x) — monomorphic tiny add/mod calls in a range loop now fuse while still executing per-iteration work
- **Dynamic function call generic** (~1.06x) — monomorphic tiny integer calls in a range loop execute the cached `TinyIntProgram` per iteration, while generic nested-call setup reuses VM-level nested call caches
- **Dynamic list ops varying** (~0.88x) — list construction remains real; list sum folds inside the VM, loop-invariant arithmetic is hoisted, and first mutation reserves list capacity to reduce realloc/copy churn
- **Dynamic map ops varying** (~0.79x) — map mutation remains real; first-write reserve plus packed `ToStr + Add` fusion reduce allocation/dispatch cost
- **Dynamic numeric loop varying** (~1.15x) — real per-iteration `%` + add is close but still behind Lua

**Remaining close-tracked overhead:**
- **Dynamic function call** (~1.11x) — still slightly slower than Lua but inside the target range.
- **String concatenation** (~1.19x) — near Lua but still slower.
- **Dynamic numeric loop varying** (~1.15x) — good current signal for non-aggregate numeric loop dispatch.
- **Dynamic map ops varying** (~0.79x) — now ahead on this run, but still useful for tracking real map mutation plus string key construction.
- **Dynamic function call generic** (~1.06x) — close, but still exercises the non-fused generic VM call path.
- **Business algorithm workloads** (~1.25x to ~7.31x, geometric mean ~3.73x) — current best signal for general VM performance; these must improve before claiming LK broadly exceeds Lua.

**Key bottlenecks:**
1. **General VM overhead on realistic workloads** — the 10 algorithm workloads remain ~1.2x-7.3x slower than Lua, so microbenchmark wins are not enough
2. **Non-invariant collection work** — varying list is now ahead on this run, but list/map workloads still expose real allocation, refcount, lookup, and mutation overhead
3. **Non-aggregate numeric loop dispatch** — numeric varying loops are close, but still pay more dispatch/materialization overhead than Lua numeric loops
4. **Generic non-fused call shapes** — straight-line helper calls now inline, but loops with control flow, recursion, named/default calls, and escaping closures still pay high setup cost.
5. **String concatenation and string-key construction** — near Lua in the microbenchmark, but still expensive in map/string-heavy workloads

**Optimization opportunities:**
- Reduce frame setup overhead for hot monomorphic and recursive calls
- Reduce argument-slot moves and call opcode overhead for non-tiny arithmetic calls
- Keep dynamic numeric loops on typed integer paths for longer
- Continue CPU/cache work on collection layout, especially dynamic map/list growth and string-key construction
- Keep the `* Varying` benchmarks in the main suite so future 0ms results cannot mask general VM cost
- Keep the business algorithm workload suite as a completion gate before claiming LK broadly exceeds Lua
- Reduce `Val` clone/refcount overhead in real list/map mutation and iteration
- Prefer broad VM improvements such as string conversion, typed arithmetic, call setup, and collection mutation before adding more benchmark-specific fused patterns

## Files

| File | Description |
|------|-------------|
| `bench_*.lk` | LK benchmark scripts |
| `bench_*.lua` | Lua benchmark scripts (equivalent logic) |
| `run_bench.sh` | Main benchmark runner (median of 7 runs) |
| `workloads_business_algorithms.lk` | LK business/algorithm workload suite |
| `workloads_business_algorithms.lua` | Lua business/algorithm workload suite |
| `run_workload_bench.sh` | Median runner for 10 business algorithm workloads |
| `bench_startup.sh` | Startup overhead measurement |
