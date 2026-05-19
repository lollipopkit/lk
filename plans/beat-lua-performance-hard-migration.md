# Beat Lua Performance Hard Migration Plan

## Goal

Preserve LK source syntax and observable behavior. Everything below that line
is negotiable: runtime layout, bytecode format, value representation, call
protocol, stdlib ABI, BC32 packing, and AOT lowering can be replaced.

The performance gate is the real workload suite:

```bash
cargo build --release -p lk-cli
RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh
```

Completion requires all of the following:

- All 15 workload checksums match Lua.
- Geometric mean `LK/Lua <= 0.95x`.
- No stable workload row is slower than `1.10x`.
- The reported winning path includes AOT; VM-only wins are useful but not the
  final bar.

## Non-Goals

- Do not keep old bytecode or runtime compatibility for its own sake.
- Do not optimize microbenchmarks that are not represented by
  `bench/workloads_business_algorithms.lk`.
- Do not accept a fast path that silently drops a function out of BC32 packed
  execution or AOT lowering.
- Do not preserve current `Val` ownership internals if they conflict with hot
  path performance.

## Current Evidence

`bench/README.md` defines the benchmark as the canonical gate and lists the
current broad bottlenecks:

- VM dispatch in loops and calls.
- Integer comparison/modulo dispatch.
- `Val` clone/refcount overhead.
- String conversion and key construction.
- Map/list memory layout and cache locality.

A low-sample local run on the current branch showed these high-priority rows:

| Workload | Current Signal | Main Hot Path |
| --- | ---: | --- |
| `cart_pricing_rules` | about `7.4x` slower | small function calls, map get, floor, starts_with |
| `route_permission_check` | about `5.8x` slower | const map lookup, const string length, branches |
| `fraud_rule_scoring` | about `5.4x` slower | small function calls, map membership, starts_with |
| `inventory_reorder` | about `2.5x` slower | template keys, map set/get, list push, join |
| `log_parse_filter` | close but still slower | split/join elimination, grouped map counters |

Treat these numbers as direction only; every claimed improvement must compare
against a fresh branch-local baseline.

## Architecture Target

### 1. Performance IR Before Runtime Execution

Compile LK source into a typed performance IR before final bytecode/AOT
lowering. This IR should represent:

- local variable lifetimes and escape state
- integer/float/bool/string/list/map facts
- exact-arity call sites
- const tables and const string keys
- loop induction variables and range bounds
- direct stdlib intrinsics such as `math.floor`, `map.get`, `map.set`,
  `starts_with`, `contains`, `len`, `push`, and `join`

The existing AST-to-bytecode compiler can remain as a fallback, but benchmark
hot paths must lower through the performance IR.

### 2. Immediate-First Value Layout

Redesign `Val` around cheap immediates:

- `Nil`, `Bool`, `Int`, `Float`, and short strings are copy-cheap.
- Heap values are used only when a value escapes the current stack/window.
- VM-owned internals use single-threaded ownership (`Rc`, arena, or explicit
  owner handles) unless a public API truly crosses threads.
- String keys store or reference cached hash/key identity.
- `Arc` is not used in the VM hot path by default.

Acceptance:

- Add a diagnostic test for `size_of::<Val>()` and clone/refcount counts on
  representative loops.
- `sliding_window_sum`, `histogram_group_count`, and `inventory_reorder`
  improve beyond noise without checksum changes.

### 3. Fixed Frame Window Call Protocol

Replace allocation-heavy call dispatch with fixed frame windows:

- Positional calls pass a register window, not `Vec<Val>`.
- The callee reads arguments directly from the caller-provided window.
- Return values are written directly into known return registers.
- Exact-arity monomorphic call sites cache callee function pointer, frame
  layout, parameter registers, and return slot.
- Small pure functions are inlined before bytecode/AOT emission.

Required hot functions:

- `cart_line_total` must be inlined or exact-call-fast.
- `fraud_score` must be inlined or exact-call-fast.
- `binary_search` helper calls must avoid generic call dispatch.

Acceptance:

- Regression tests cover recursive calls, closure captures, named parameters,
  default thunks, and error stack reporting.
- `cart_pricing_rules`, `fraud_rule_scoring`, `gcd_batch`, and
  `binary_search` improve beyond noise.

### 4. Typed Bytecode As The Default Hot Path

Typed bytecode is mandatory for hot code. Generic opcodes remain fallback only.

Required op families:

- integer arithmetic: `AddI`, `SubI`, `MulI`, `DivI`, `ModI`
- integer comparisons and fused branches: `JmpEqI`, `JmpNeI`, `JmpLtI`,
  `JmpLeI`, `JmpGtI`, `JmpGeI`
- register/immediate variants for loop constants
- list ops: `IndexListI`, `ListPushUnique`, `ListLen`
- map ops: `MapGetConstStr`, `MapGetStrReg`, `MapSetConstStr`,
  `MapSetStrReg`, `MapHasConstStr`, `MapHasStrReg`
- string ops: `StrLen`, `StrStartsWithConst`, `StrContainsConst`,
  `TemplateBuildKnownCap`
- stdlib intrinsics: `FloorI`, `FloorF`, `JoinConstSep`

Acceptance:

- Every typed op has generic fallback behavior covered by tests.
- Every typed op needed by benchmark hot paths can be represented in BC32 and
  AOT lowering.

### 5. Container And String Runtime Rewrite

Map/list/string internals should match LK semantics while optimizing benchmark
patterns.

Map:

- Small maps use inline storage before hashing.
- Const maps can lower to direct const-table/perfect-match lookup.
- Interned short keys avoid repeated allocation and hashing.
- Dynamic string keys preserve content equality.

List:

- Unique-owner list mutation is in-place.
- Shared list mutation clones only when required by observable behavior.
- Push/index/len have typed bytecode and AOT lowering.

String/template:

- Template strings pre-compute capacity when segment count is known.
- Short generated keys can stay immediate or interned.
- `split(sep).join(sep)` remains a compile-time identity optimization.

Acceptance:

- `route_permission_check` must become close/ahead through const map and const
  string lowering.
- `inventory_reorder` must improve through template key and unique list/map
  mutation.
- `log_parse_filter` must not regress while string and map paths change.

### 6. BC32/Packed Coverage Is A Gate

BC32 packed execution is not optional for optimized VM performance. If the
current tag space blocks hot op coverage, replace the format.

Required design:

- A versioned BC32/BC64 format or extension-word scheme.
- No overloading that makes decode ambiguous.
- Roundtrip tests for every hot typed op.
- Coverage diagnostics that report unsupported opcodes by function/workload.

Acceptance:

- A workload function containing optimized hot ops still has packed code.
- CI/test output can show unsupported opcode counts.
- Adding a new hot opcode without BC32 support fails a targeted coverage test.

### 7. AOT Is The Final Winning Path

The VM can be improved for iteration, but final Lua-beating performance should
come from AOT lowering.

Required AOT lowering:

- typed integer arithmetic and fused integer branches
- range loops and induction variables
- exact/inlined function calls
- const map lookups
- interned/dynamic map lookups where static lowering is safe
- string length and starts/contains predicates
- list push/index/len for unique-owner local lists

Fallback:

- Dynamic behavior that cannot be proven stays on VM/runtime calls.
- Fallback must preserve LK behavior and checksum parity.

Acceptance:

- `bench/run_workload_bench.sh` reports VM, AOT, and Lua.
- The final pass condition is based on AOT rows.
- AOT checksum mismatch is a correctness failure, not a performance result.

## Implementation Order

### Phase 0: Coverage And Baseline

Deliverables:

- Add a benchmark coverage report that lists, per workload/function:
  - packed or unpacked VM execution
  - unsupported BC32 opcodes
  - AOT-native lowered or VM trampoline
  - fallback reasons
- Refresh current baseline with:

```bash
cargo build --release -p lk-cli
RUNS=10 EXTRA_RUNS=20 RUN_AOT=1 bench/run_workload_bench.sh
```

Exit criteria:

- We know exactly why each slow workload misses packed/AOT coverage.

### Phase 1: Frame Window Calls And Inlining

Deliverables:

- Replace positional hot calls with borrowed frame windows.
- Add exact-arity call site cache.
- Inline small pure functions from performance IR.
- Prove `cart_line_total` and `fraud_score` no longer use generic call dispatch
  in benchmark hot loops.

Target rows:

- `cart_pricing_rules`
- `fraud_rule_scoring`
- `binary_search`
- `gcd_batch`

### Phase 2: Immediate Values And Container Ownership

Deliverables:

- Immediate-first `Val` layout.
- VM-local heap ownership strategy.
- Unique-owner list/map mutation APIs.
- Clone/refcount diagnostic probes.

Target rows:

- `sliding_window_sum`
- `histogram_group_count`
- `inventory_reorder`
- `two_sum_map`

### Phase 3: Typed Bytecode And Packed Format

Deliverables:

- Typed arithmetic, comparison, branch, map, list, and string op families.
- New or extended packed format with roundtrip tests.
- Coverage tests that reject unsupported benchmark hot ops.

Target rows:

- `gcd_batch`
- `stock_max_profit`
- `matrix_3x3_multiply`
- `route_permission_check`

### Phase 4: Const Tables And String Key Lowering

Deliverables:

- Const map literal lowering for small maps.
- Perfect/direct lookup for const string keys.
- Template key capacity planning and short-key interning.
- String predicate intrinsics.

Target rows:

- `route_permission_check`
- `fraud_rule_scoring`
- `inventory_reorder`
- `log_parse_filter`

### Phase 5: AOT Native Lowering

Deliverables:

- AOT lowering for every typed hot op.
- VM fallback calls for dynamic cases only.
- Runner and docs report AOT as the final performance gate.

Target:

- Geometric mean `AOT/Lua <= 0.95x`.
- No stable workload row `> 1.10x`.

## Required Tests And Gates

Correctness:

```bash
cargo test -p lk-core
cargo test --workspace
cargo test --all-features --all-targets
```

Performance:

```bash
cargo build --release -p lk-cli
RUNS=10 EXTRA_RUNS=20 RUN_AOT=1 bench/run_workload_bench.sh
```

Coverage:

- A benchmark coverage command or test must show that all 15 workloads hit the
  intended optimized path.
- Unsupported packed/AOT opcode counts must be zero for benchmark hot functions,
  or explicitly explained as cold fallback paths.

## Completion Audit Checklist

Before claiming this plan is implemented:

- [ ] All 15 checksums match Lua in the canonical benchmark command.
- [ ] Geometric mean `AOT/Lua <= 0.95x`.
- [ ] No stable row is `> 1.10x`.
- [ ] VM packed coverage is complete for benchmark hot functions.
- [ ] AOT lowering, not VM trampoline, is responsible for the final winning
      rows.
- [ ] `cart_line_total` and `fraud_score` avoid generic call dispatch.
- [ ] Const map lookups in `route_permission_check` avoid generic map hashing.
- [ ] `inventory_reorder` avoids avoidable template/list/map allocation churn.
- [ ] Correctness tests pass, or any blocker is documented with exact logs.
