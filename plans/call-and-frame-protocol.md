# Call and Frame Protocol Plan

## Reference Implementations

- Luau call path: `references/luau/VM/src/lvmexecute.cpp`
- JavaScriptCore call frames: `references/webkit/Source/JavaScriptCore/interpreter/CallFrame.h`,
  `references/webkit/Source/JavaScriptCore/interpreter/CachedCall.h`
- Rune VM calls: `references/rune/crates/rune/src/runtime/vm.rs`,
  `references/rune/crates/rune/src/runtime/vm_call.rs`
- LK current frames: `core/src/vm/vm/runtime/exec.rs`,
  `core/src/vm/vm/frame.rs`

## Borrow

- Keep call frames as compact metadata over a contiguous value stack.
- Cache exact-arity call sites.
- Separate closure calls, native calls, named calls, and AOT calls early so hot
  paths do not repeatedly branch through every case.
- Use frame reuse where correctness allows it.

## Do Not Borrow

- Do not depend on GC stack scanning.
- Do not remove LK named parameters or lazy defaults.
- Do not hide call-frame invariants behind broad unsafe pointer mutation.

## LK Landing

- Current LK already has a flat stack. Remove remaining avoidable argument
  copies such as `args.to_vec()` in hot call paths.
- Make `CallArgs` a borrowed window for positional calls and a compact named
  layout for named calls.
- Cache function pointer, arity, parameter register map, and frame info per
  monomorphic call site.
- Keep default-argument thunk execution isolated so previous bugs around
  pending resume PC and frame visibility do not return.
- Define exact return-slot rules for:
  - normal closure call
  - tiny call plan
  - Rust/native fastcall
  - named call with defaults

## Acceptance

- Bench focus: `gcd_batch`, `binary_search`, `order_score_pipeline`,
  `route_permission_check`.
- Regression tests for recursive calls, default named parameters, closure
  captures, and error call-stack reporting.
- Debug assertions must prove frame depth and stack top are restored after
  success and error paths.

