# Runtime References Overview

This directory turns `references/*` into concrete LK runtime planning material.
These notes are for design and implementation guidance only; none of the
reference projects are dependencies.

## Design Boundary

- Borrow implementation techniques, not complete runtime models.
- Do not introduce VM-owned GC or a tracing collector as LK's object lifetime model.
- Do not merge LK `List` and `Map` into a Lua-style unified table.
- Prefer Rust ownership, lifetimes, move semantics, and RAII-friendly APIs.
- Keep unsafe narrow, local, documented, and unnecessary unless a measured hot
  path justifies it.
- Treat `bench/run_workload_bench.sh` as the broad performance gate.

## Reference Map

| Reference | Primary files | Borrow |
| --- | --- | --- |
| Luau | `references/luau/VM/src/lvmexecute.cpp`, `references/luau/VM/src/lbytecode.h`, `references/luau/Compiler/src/Compiler.cpp` | Register VM layout, specialized bytecode, FASTCALL fallback shape, constant-key access ops. |
| CPython | `references/cpython/Python/bytecodes.c`, `references/cpython/Include/internal/pycore_opcode_metadata.h`, `references/cpython/InternalDocs/interpreter.md` | Adaptive bytecode, inline cache entries near instructions, specialization/deopt metadata. |
| QuickJS | `references/quickjs/quickjs.h`, `references/quickjs/quickjs.c`, `references/quickjs/quickjs-opcode.h` | Compact value tags, explicit refcount boundaries, atom/property key handling. |
| Ruby | `references/ruby/vm*`, `references/ruby/yjit*`, `references/ruby/benchmark/vm_*` | Method/cache benchmarking categories, call protocol and YJIT layering ideas. |
| V8 | `references/v8/src/interpreter`, `references/v8/src/ic`, `references/v8/src/runtime` | Ignition bytecode builder, feedback vector ideas, IC tiers. |
| JavaScriptCore | `references/webkit/Source/JavaScriptCore/interpreter`, `references/webkit/Source/JavaScriptCore/dfg` | Call frame layout, cached calls, property/cache watchpoint ideas. |
| Rhai | `references/rhai/src/types/dynamic.rs`, `references/rhai/src/func`, `references/rhai/src/engine.rs` | Rust-native embedding surface, shared ownership toggled by features, SmartString usage. |
| Rune | `references/rune/crates/rune/src/runtime`, `references/rune/crates/rune/src/module` | Rust VM/module/native boundary, value conversions, stack/call APIs. |

## Recommended Reading Order

1. Start from the LK hotspot category in `bench/README.md`.
2. Read the matching plan in `plans/`.
3. Open the listed reference files and inspect only the relevant implementation.
4. Compare against LK's current `core/src/vm/` and `core/src/val/` code before
   designing code changes.

## Cross-Cutting Lessons

- Dispatch wins come from fewer unpredictable branches and denser instruction
  data, not from copying a specific C macro style.
- Fast native calls need a low-allocation ABI. Most references avoid building a
  fresh owned argument vector for every call.
- Specialization should be reversible. CPython and Luau both keep a fallback
  path for generic semantics.
- Container fast paths should match LK semantics. Use dedicated `List` and
  `Map` optimizations rather than a unified table.
- String/key optimization should focus on interned keys, cached hashes, and
  fewer temporary allocations.

