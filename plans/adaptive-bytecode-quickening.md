# Adaptive Bytecode and Quickening Plan

## Reference Implementations

- CPython: `references/cpython/Python/bytecodes.c`
- CPython metadata: `references/cpython/Include/internal/pycore_opcode_metadata.h`
- Luau VM: `references/luau/VM/src/lvmexecute.cpp`
- Luau bytecode/compiler: `references/luau/VM/src/lbytecode.h`, `references/luau/Compiler/src/Compiler.cpp`
- V8 Ignition: `references/v8/src/interpreter/bytecodes.h`, `references/v8/src/interpreter/bytecode-generator.cc`

## Borrow

- Start with generic bytecode and specialize hot instruction sites after stable
  runtime types are observed.
- Store small cache metadata per instruction site, close to the opcode.
- Make specializations explicit opcodes, not hidden side effects:
  `Add -> AddInt`, `Index -> IndexListInt`, `Call -> CallRustFast`.
- Keep every specialized opcode paired with a deopt/fallback path back to the
  generic opcode.

## Do Not Borrow

- Do not copy CPython's stack VM model; LK should stay register-oriented.
- Do not add a multi-tier JIT in this phase.
- Do not make specialization depend on a VM-owned GC object model.

## LK Landing

- Current LK already has `Op`, BC32, packed hot slots, and per-site ICs. Build
  quickening on top of those rather than adding another interpreter.
- Add a small per-function quickening state:
  - execution counters per `pc`
  - observed operand tags for selected op families
  - original opcode for deopt restoration
- First specialization families:
  - numeric arithmetic: `Add/Sub/Mul/Mod` with Int/Float variants
  - comparisons and branch fusion: `Cmp* + JmpFalse`
  - indexing: `IndexListInt`, `IndexStrInt`, `IndexMapInternedStr`
  - calls: `CallRustFast`, `CallClosureExact`
- BC32 should either encode the specialized op directly or reuse an extension
  word. Avoid maintaining independent semantics between `opcode.rs` and
  `packed.rs`.

## Acceptance

- Correctness: all current VM/compiler tests pass with quickening enabled and
  disabled.
- Bench focus: `gcd_batch`, `binary_search`, `matrix_3x3_multiply`,
  `stock_max_profit`, `cart_pricing_rules`.
- Required command: `RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh`.
- A specialization is accepted only if checksum parity holds and the target
  workload improves beyond sample noise.

