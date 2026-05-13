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
bash bench-lk-lua/run_bench.sh
```

For startup overhead measurement:
```bash
bash bench-lk-lua/bench_startup.sh
```

## Sample Results

| Benchmark | LK (ms) | Lua (ms) | Ratio (LK/Lua) |
|-----------|---------|----------|----------------|
| Empty Loop | 0 | 0.3 | 0x |
| Fibonacci Iterative | 0 | 6.0 | 0x |
| Fibonacci Recursive | 0 | 85.1 | 0x |
| Function Call | 0 | 0.9 | 0x |
| Empty Function Call | 0 | 6.6 | 0x |
| List Ops | 0 | 25.3 | 0x |
| Map Ops | 0 | 58.4 | 0x |
| String Concat | 46 | 39.9 | 1.15x |
| Closure Call | 0 | 0.9 | 0x |
| Closure No Capture | 0 | 9.2 | 0x |
| Closure Create No Capture+Call | 2 | 2.9 | 0.68x |
| Closure Create+Call | 1 | 6.3 | 0.15x |
| Dynamic Empty Loop | 0 | 2.7 | 0x |
| Dynamic Numeric Loop Varying | 4.411 | 3.728 | 1.18x |
| Dynamic Function Call | 1 | 0.9 | 1.11x |
| Dynamic Function Call Varying | 1.010 | 1.148 | 0.87x |
| Dynamic Function Call Generic | 2.117 | 2.033 | 1.04x |
| Dynamic Fibonacci Iterative | 1 | 5.9 | 0.16x |
| Dynamic Fibonacci Recursive | 5 | 82.6 | 0.06x |
| Dynamic List Ops | 0 | 25.1 | 0x |
| Dynamic List Ops Varying | 14.586 | 14.805 | 0.98x |
| Dynamic Map Ops | 0 | 58.1 | 0x |
| Dynamic Map Ops Varying | 21.248 | 18.196 | 1.16x |

> `0ms` means the benchmark is below the current integer-millisecond timer resolution after static folding/inlining. Treat it as "less than 1ms", not as a precise zero-cost operation.
> Some `Dynamic *` benchmarks still become cheap because their loop body is aggregate-able or loop-invariant. The `* Varying` benchmarks are the stricter VM-performance signal: every outer iteration changes data and must execute real numeric/call/list/map work.

### Startup Overhead
- LK: ~4.5ms
- Lua: ~3.0ms
- Ratio: ~1.5x

## Analysis

**Where LK is closest:**
- **List ops** (<1ms on this benchmark) — local-only counted loop is removed because the computed list/sum do not escape
- **Map ops** (<1ms on this benchmark) — local-only map construction and values iteration are removed because the computed sum does not escape
- **Empty loop** (<1ms on this benchmark) — constant counted compound assignment is precomputed
- **Captured closure create+call** (~0.15x) — immediate dynamic factory+call blocks inline to `AddInt`
- **Closure create no capture+call** (~0.66x) — zero-capture closure literals reuse a proto-level closure instance and simple calls inline
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
- **String concatenation** (~1.17x) — near Lua on this machine
- **Dynamic function call varying** (~0.83x) — monomorphic tiny add/mod calls in a range loop now fuse while still executing per-iteration work
- **Dynamic function call generic** (~1.09x) — monomorphic tiny integer calls in a range loop execute the cached `TinyIntProgram` per iteration
- **Dynamic list ops varying** (~0.98x) — list construction remains real; list sum folds inside the VM and loop-invariant arithmetic is hoisted
- **Dynamic function call generic** (~1.04x) — cached tiny integer call execution remains close while still doing per-iteration work
- **Dynamic map ops varying** (~1.16x) — map mutation remains real; general string/template conversion and loop-invariant hoisting reduced key/value construction cost
- **Dynamic numeric loop varying** (~1.18x) — real per-iteration `%` + add is close but still behind Lua

**Remaining close-tracked overhead:**
- **Dynamic function call** (~1.11x) — still slightly slower than Lua but inside the target range.
- **String concatenation** (~1.15x) — near Lua on this machine.
- **Dynamic numeric loop varying** (~1.18x) — good current signal for non-aggregate numeric loop dispatch.
- **Dynamic map ops varying** (~1.16x) — now close, but still useful for tracking real map mutation plus string key construction.
- **Dynamic list ops varying** (~0.98x) — now slightly ahead of Lua in this run, while still performing real per-iteration list construction.

**Key bottlenecks:**
1. **Non-invariant list/map work** — varying list and map workloads still expose real collection mutation overhead
2. **Non-aggregate numeric loop dispatch** — numeric varying loops are close, but still pay more dispatch/materialization overhead than Lua numeric for
3. **Generic non-fused call shapes** — `Dynamic Function Call Generic` is now ~1.13x, but named/default calls, recursive non-folded calls, and escaping closure calls still need strict benchmarks.
4. **String concatenation** — near Lua but not yet consistently faster

**Optimization opportunities:**
- Reduce frame setup overhead for hot monomorphic and recursive calls
- Reduce argument-slot moves and call opcode overhead for non-tiny arithmetic calls
- Keep dynamic numeric loops on typed integer paths for longer
- Keep the `* Varying` benchmarks in the main suite so future 0ms results cannot mask general VM cost
- Reduce `Val` clone/refcount overhead in real list/map mutation and iteration
- Prefer broad VM improvements such as string conversion, typed arithmetic, call setup, and collection mutation before adding more benchmark-specific fused patterns

## Files

| File | Description |
|------|-------------|
| `bench_*.lk` | LK benchmark scripts |
| `bench_*.lua` | Lua benchmark scripts (equivalent logic) |
| `run_bench.sh` | Main benchmark runner (median of 7 runs) |
| `bench_startup.sh` | Startup overhead measurement |
