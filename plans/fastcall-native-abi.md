# Fastcall and Native ABI Plan

## Reference Implementations

- Luau FASTCALL: `references/luau/Compiler/src/Compiler.cpp`,
  `references/luau/VM/src/lvmexecute.cpp`
- Rune runtime calls: `references/rune/crates/rune/src/runtime/vm_call.rs`,
  `references/rune/crates/rune/src/runtime/call.rs`
- Rhai native functions: `references/rhai/src/func/call.rs`,
  `references/rhai/src/func/native.rs`
- V8 fast API shape: `references/v8/include/v8-fast-api-calls.h`

## Borrow

- Luau's important idea is not the exact opcode: it emits a fastcall that can
  skip the generic function lookup/call path, while the normal `CALL` remains
  nearby as fallback.
- Rune/Rhai are useful for Rust API ergonomics: native functions should receive
  borrowed argument views where possible and return owned values only when
  needed.
- Cache native function identity and arity at call sites.

## Do Not Borrow

- Do not expose unsafe raw pointers in public stdlib registration APIs.
- Do not require all stdlib functions to be rewritten at once.
- Do not break named-argument semantics.

## LK Landing

- Introduce a fast native ABI alongside existing `RustFunction`:
  - input: `&mut VmContext`, `&mut [Val]` or a lightweight `ArgWindow`
  - output: direct write to a return register or `Val`
  - no fresh `Vec<Val>` for the common positional path
- Keep current `RustFunction` as compatibility bridge during migration.
- Add compiler/runtime support for known stdlib fastcalls:
  - `math.floor`
  - string predicates and length
  - list `push`, `len`, indexed `get`
  - map `get`, `set`, `has`
- Convert hot stdlib methods incrementally. Each converted function must keep
  identical user-visible behavior and errors.

## Acceptance

- Bench focus: `binary_search`, `order_score_pipeline`, `log_parse_filter`,
  `sliding_window_sum`, `histogram_group_count`.
- Add regression tests that exercise both generic and fast native paths.
- Verify no new allocation-heavy argument vector is created in the fast path.
- Full gate: `cargo test --workspace` plus the real workload benchmark.

