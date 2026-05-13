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
| List Ops | Push + iterate list of 100 | 10,000 × 100 |
| Map Ops | Set + iterate map of 50 | 10,000 × 50 |
| String Concat | Repeated `"x" + "x"` | 10,000 × 100 |
| Closure Call | Closure capture + dispatch | 100,000 |

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
| Empty Loop | 3 | 0.3 | 10.0x |
| Fibonacci Iterative | 55 | 6.5 | 8.46x |
| Fibonacci Recursive | 1147 | 85.8 | 13.36x |
| Function Call | 12 | 0.9 | 13.33x |
| List Ops | 111 | 25.9 | 4.28x |
| Map Ops | 140 | 59.0 | 2.37x |
| String Concat | 51 | 40.7 | 1.25x |
| Closure Call | 11 | 1.1 | **10.0x** |

### Startup Overhead
- LK: ~4.5ms
- Lua: ~3.0ms
- Ratio: ~1.5x

## Analysis

**Where LK is closest:**
- **String concatenation** (~1.25x) — near Lua on this machine
- **Map operations** (~2.37x) — moderate overhead, HashMap vs Lua table

**Where LK has significant overhead:**
- **Recursive calls** (~13.36x) — function call overhead compounds in recursion
- **Function calls** (~13.33x) — general dispatch overhead per call
- **Closure calls** (~10.0x) — captured positional closures now use the VM fast path, but still pay frame setup and capture access costs
- **Empty loops** (~10.0x) — basic iteration overhead

**Key bottlenecks:**
1. **Function call overhead** — VM dispatch + frame setup per call
2. **Recursion** — repeated frame setup dominates recursive workloads
3. **Loop dispatch** — basic loop/control-flow overhead remains visible in tight loops
4. **Capture access** — captured positional closures are faster than the old call environment path, but still slower than direct Lua closures

**Optimization opportunities:**
- Reduce frame setup overhead for hot monomorphic function calls
- Stack-allocate closures when captures don't escape
- Tail-call optimization for recursive patterns
- Bytecode specialization for common arithmetic patterns

## Files

| File | Description |
|------|-------------|
| `bench_*.lk` | LK benchmark scripts |
| `bench_*.lua` | Lua benchmark scripts (equivalent logic) |
| `run_bench.sh` | Main benchmark runner (median of 7 runs) |
| `bench_startup.sh` | Startup overhead measurement |
