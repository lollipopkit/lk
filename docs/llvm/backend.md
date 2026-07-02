# LLVM Backend

## Architecture: the typed MIR pipeline (only backend)

Since the AOT redesign ([`aot-redesign.md`](./aot-redesign.md)) — and the
retirement of the legacy text backend that followed it — every native compile
runs one pipeline:

```
ModuleArtifact → lk-aot-lower → lk_aot_mir::validate → lk-aot-codegen → clang + liblkrt.a
```

- **`lk-aot-lower`** is the *total capability predicate*: `lower() ->
  Result<MirModule, Unsupported>`. Shapes outside the subset fail the compile
  with a precise, user-facing `Unsupported` reason. There is no fallback
  backend and no VM shell embedding.
- **`lk_aot_mir::validate`** runs unconditionally on the production path (a
  failure is a lowering bug, never a user error).
- **`lk-aot-codegen`** is a total `MirModule` → LLVM-text rendering; the CLI
  compiles the `.ll` with `clang` and links the typed `lkrt` static runtime
  (`--whole-archive`, `-force_load` on macOS).

## Covered subset

- Straight-line and branching/looping scalar code (i64/f64/bool/str, guarded
  div/mod, VM-exact float display) with on-demand SSA construction (block
  params as phis).
- Direct function calls with per-callsite-monomorphized i64/f64/bool
  params/returns, recursion included; dead (uncalled) functions are skipped.
- Zero-capture closures (`let f = |x| …`) as statically known function
  references: indirect calls devirtualize to direct calls, both for
  register-local lambdas and for top-level lambdas stored in a module global
  (single assignment in the entry prefix, readable from any function).
  Capturing closures and closures used as first-class values (passed as
  arguments, stored in containers) reject.
- Growable handle containers (`List<i64/f64/str>`, `Map<{str,i64} ×
  {i64,f64}>`) with VM-exact indexing: the `Maybe` present-bit model makes
  out-of-range/missing reads return-print `nil`, abort on arithmetic (like the
  VM halt), and answer `== nil`.
- `push`/`set`/`len`/iteration/`in`/`join`, string
  equality/concat/interpolation, long-string literals.
- Runtime builtins recognized from `GetGlobal`: `println`/`print` (constant
  format strings expand at lower time with exact `format_variadic_runtime`
  semantics — `{}` substitution, leftover `{}` kept literal, extra args
  space-appended), `assert`/`assert(cond, message)` (false aborts loudly),
  `assert_eq`/`assert_ne` (scalar equality with Int/Float coercion and string
  bytes, VM-format failure message), `panic(args…)` (space-joined display,
  always fatal), and `typeof` (static scalar names; Maybe carriers select
  `Nil` vs the value name at runtime). `IsNil` lowers likewise (scalars are
  never nil, Maybe tests its present bit).
- Composite string-int keys (`m["n${i}"]`): stores call the zero-allocation
  `set_ik` map ABI (key built on the lkrt stack); loads build the key with the
  single-allocation `str.concat_i64` fusion, which also fuses every int
  operand of template-string concatenation.
- `math` module: constants resolve at lower time (`pi`/`e`/`inf`/`nan`/
  `max_int`/`min_int`/`max_float`/`epsilon`); `floor`/`ceil`/`round` dispatch
  on the static type (Int passthrough, Float → lkrt `as i64` cast); `abs` and
  `min`/`max` lower to selects preserving the argument type; `sqrt` (aborts on
  a negative argument like the VM) / `sin`/`cos`/`exp`/`pow` call lkrt with
  Number→Float promotion. Native links `-lm` on Linux.
- Fused compare-branch opcodes (`TestXxxInt(I)`+`Jmp`, `TestEqIntI2`,
  `BrEqIntI4` family, `BrMod*ZeroIntI4`, nil branches).

Ownership is arena-based by default: strings and container handles register in
the `lkrt` arena and are reclaimed by `lkrt_cleanup()` at entry exit; the
lowering eagerly frees provably dead display/concat temporaries. Every fatal
guard (`div/0`, missing-key arithmetic, failed `assert`) flushes C stdio via
`lkrt_abort` before aborting so already-printed output is never discarded.

## Semantics and testing

Native behaviour is pinned to the VM by three differential corpora
(`cli/tests/aot_differential_test.rs` hand-written cases,
`cli/tests/examples_differential_test.rs` over `examples/`, and the seeded
generative fuzz in `cli/tests/aot_fuzz_differential_test.rs`), MIR snapshots
(`aot/lower/tests/mir_snapshots.rs`), and the golden semantics vectors in
[`docs/semantics.md`](../semantics.md). The ABI schema (`aot/abi`) is the
single source of truth shared by codegen declarations and `lkrt`'s
compile-time conformance checks.

Unsupported shapes fail before executable emission; LLVM output must not embed
a serialized `.lkm` payload, `ModuleArtifact`, the bytecode executor,
`VmContext`, parser, type checker, or compiler. The final executable may link
Rust `std`, libc/libm, and `lkrt`. See `docs/llvm/native-stdlib.md` for the
native stdlib boundary.

## Usage

```sh
lk compile FILE.lk          # native executable (default)
lk compile llvm FILE.lk     # emit FILE.ll
lk compile bytecode FILE.lk # emit the .lkm module artifact instead
```

Direct `lk file.lk` execution defaults to the bytecode VM; `LK_NATIVE_RUN=1`
opts into the cached native fast path when the program lowers.
`LK_NATIVE_SANITIZE=address,undefined` forwards `-fsanitize=` to clang for
sanitized differential runs.

## Future work

Expanding this lowering surface (module builtins such as `os.clock`/`math.*`,
closures/indirect calls/mutable globals — RFC §7 phase 4, mixed-element
constant containers) must go through the MIR pipeline; adding a second lowering
path or a VM runtime bridge is out of bounds.
