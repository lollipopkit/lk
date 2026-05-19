# Value and Ownership Layout Plan

## Reference Implementations

- QuickJS value tags/refcount: `references/quickjs/quickjs.h`,
  `references/quickjs/quickjs.c`
- Rhai dynamic values: `references/rhai/src/types/dynamic.rs`,
  `references/rhai/src/lib.rs`
- Rune values: `references/rune/crates/rune/src/runtime/value.rs`,
  `references/rune/crates/rune/src/runtime/shared.rs`
- LK current value: `core/src/val/values/mod.rs`

## Borrow

- QuickJS separates immediate values from refcounted heap objects. LK can keep
  that conceptual boundary without adopting QuickJS's C object system.
- Rhai/Rune show Rust-friendly conversion and embedding boundaries.
- Use copyable/immediate representations for Int/Float/Bool/Nil/ShortStr.
- Use shared ownership only where values truly escape the current stack window.

## Do Not Borrow

- Do not add VM-owned GC or a tracing collector.
- Do not make every heap value an opaque engine-owned pointer.
- Do not globally replace Rust lifetimes with manual lifetime management.
- Do not add NaN-boxing unless a later benchmark proves the complexity is worth it.

## LK Landing

- Audit `Val::clone()` in VM hot paths and classify clones:
  - removable by move
  - replaceable by borrowed read
  - required because value escapes
- Add helper APIs that express intent:
  - `read_reg`
  - `move_reg`
  - `write_reg`
  - `take_reg_or_clone`
- Prefer `Rc` for single-threaded VM-owned internals if the value cannot cross
  threads. Keep `Arc` at public/concurrency boundaries.
- Keep `ShortStr` and `ArcStr` as current wins. Add cached hash only after map
  lookup profiling identifies hashing as a real hotspot.
- Define an ownership rule for stdlib methods: mutate in place when unique,
  clone-on-write only when shared.

## Acceptance

- Add a small `sizeof(Val)` and clone-count diagnostic test or debug-only probe.
- Bench focus: all workloads, especially `sliding_window_sum`,
  `histogram_group_count`, `inventory_reorder`.
- No new public unsafe API.
- No semantic change for `Val` conversion, serde, CLI output, or LSP-facing data.

