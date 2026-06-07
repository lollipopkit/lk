# LLVM Native Stdlib Plan

## Phase 1: Link Boundary

- Add `lkrt` as a workspace crate with `rlib` and `staticlib` outputs.
- Make `lk-cli` depend on `lkrt` behind the `llvm` feature.
- Link `liblkrt.a` when building `lk compile exe` output.
- Keep `lkrt` independent from `lk-core` and `lk-stdlib`.

Acceptance:

- `cargo build -p lk-cli --features llvm`
- `lk compile exe` still emits a native executable.
- `strings` on the executable does not show `ModuleArtifact`,
  `bytecode VM`, `VmContext`, `compile_program`, or `execute_module`.

## Phase 2: Intrinsic Registry

- Add a single LLVM intrinsic registry that describes module/name, arity,
  typed signature, effects, and runtime symbol.
- Use the registry as the only path for host primitives that cannot be written
  as LK source.
- Reject unsupported stdlib shapes with a precise reason.

Acceptance:

- New host primitives require a registry entry.
- No new scattered stdlib method-name matches in LLVM lowering.

## Phase 3: Pure LK Stdlib

- Add `stdlib/lk/` for pure LK implementations.
- Lower meta-method calls to canonical stdlib function calls, for example
  `s.substring(a, b)` to `string.substring(s, a, b)`.
- Monomorphize stdlib functions by call shape.

Acceptance:

- String/list/map/iter pure methods compile through the normal LLVM pipeline.
- Existing hand-written static method folds are removed or reduced to registry
  calls and compile-time constant evaluation.

## Phase 4: Coverage Closure

- Build a language-shape coverage matrix for spec methods, VM IR/runtime
  shapes, LLVM lowering, and `lkrt` calls.
- Run the full examples VM/native diff sweep.
- Update `docs/llvm/backend.md` and `plan-progress.md` with verified facts.

Acceptance:

- `cargo test -p lk-core --features llvm llvm::tests:: -- --nocapture`
- `cargo build -p lk-cli --features llvm`
- Full `examples/**/*.lk` native/VM diff sweep.
- Unsupported examples have concrete, non-generic reasons.
