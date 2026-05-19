# Runtime Reference Implementations

These checkouts are local references for LK runtime and performance work. They
are not vendored dependencies. Use them for design comparison before changing
the VM, bytecode, value representation, call protocol, string handling, or
container hot paths.

## References

| Directory | Use for |
| --- | --- |
| `luau/` | Register VM, bytecode specialization, fast calls, inline caches, typed hot paths. |
| `quickjs/` | Compact value representation, reference-counted object ownership, bytecode and closure handling. |
| `cpython/` | Adaptive bytecode, quickening, inline cache layout near instructions. |
| `ruby/` | Method dispatch caches, VM call protocol, YJIT/VM layering ideas. |
| `v8/` | Ignition bytecode, feedback vectors, IC design, baseline-tier architecture. Sparse checkout. |
| `webkit/` | JavaScriptCore LLInt/baseline/DFG ideas and property access caches. Sparse checkout. |
| `rhai/` | Rust-native embedded scripting API boundaries and safe ownership style. |
| `rune/` | Rust VM, module/native function boundaries, bytecode and stack design. |

## LK Design Boundary

- Borrow implementation ideas, not full runtime models.
- Do not introduce a VM-owned GC just because Lua/JS engines use one.
- Do not merge LK `List` and `Map` into a Lua-style unified table.
- Prefer Rust ownership, lifetimes, move semantics, `Box`/`Rc`/`Arc` only where
  they fit LK's actual execution model.
- Keep `bench/` real workloads as the performance gate.
