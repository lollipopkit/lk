# LLVM Backend

## Architecture: the typed MIR pipeline (only backend)

Since the AOT redesign ([`aot-redesign.md`](./aot-redesign.md)) — and the
retirement of the legacy text backend that followed it — every native compile
runs one pipeline:

```
ModuleArtifact → lk-aot-lower → lk_aot_mir::validate → lk-aot-codegen → clang + liblkrt.a
```

- **`lk-aot-lower`** is the *total capability predicate*: `lower() ->
  Result<MirModule, Unsupported>`. Shapes outside the subset return a precise
  `Unsupported` reason. The MIR/LLVM path itself embeds no VM shell — but at the
  CLI level `lk compile FILE` no longer fails outright on `Unsupported`
  (plan M4.2, 问题 2): it **falls back to the Tier 0 VM bundle** (`lk bundle`,
  which embeds the interpreter and runs any valid program), emitting a warning.
  Genuine source errors (syntax/type) still surface up front — they are detected
  before the native attempt, so the fallback never masks them. Set
  `LK_AOT_NO_FALLBACK=1` to disable the fallback and require native-only lowering
  (used by tooling/tests that verify the lowering in isolation).
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
- Capturing closures with statically tracked environments: the compiler's
  upvalue cells (`LoadHeapConst UpvalCell` / `StoreCellVal` / `LoadCellVal`)
  live in virtual SSA slots appended after the registers, under the same
  Braun construction — cross-block cell state (mutation in a branch arm,
  loop-carried updates) gets phis, and each closure call resolves its cells
  to their *current* content, passed as hidden trailing parameters (the VM's
  shared-mutable-cell semantics). Closure refs propagate across blocks
  through `Move`/`Move2` when every predecessor path agrees; ref consistency
  dies at loop headers whose entry edge lacks the ref, which is exactly what
  keeps per-iteration cells isolated (a loop-created closure cannot leak into
  the next iteration — such escapes reject loudly). Still rejected: lambdas
  that mutate their captures, closures stored in containers, and string
  captures flowing into `+` dispatch.
- List structural equality (`xs == [1, 2, 3]`, `!=`): same-typed
  `List<i64/f64/str>` pairs compare via lkrt helpers (length + element-wise
  `==`, so a NaN element breaks equality); Int/Float pairs coerce
  numerically (`[1] == [1.0]` is true); other cross-typed pairs fold to
  false only when both sides are provably non-empty at materialization
  (two empty lists are equal in the VM regardless of type — unproven
  emptiness rejects instead of guessing).
- Growable handle containers (`List<i64/f64/str>`, `Map<{str,i64} ×
  {i64,f64}>`) with VM-exact indexing: the `Maybe` present-bit model makes
  out-of-range/missing reads return-print `nil`, abort on arithmetic (like the
  VM halt), and answer `== nil`.
- `push`/`set`/`len`/iteration/`in`/`join`, string
  equality/concat/interpolation, long-string literals.
- `str.split(sep)` → `List<str>`: `str::split` in `lkrt` (VM-exact, so empty
  parts on consecutive/leading/trailing separators match), with each part
  copied into an arena-owned C string.
- `key in map` for string-keyed maps: the `get_pair` present bit (no value
  materialization), so `IsMap` + `in` let `if let {"a": x} = m { … }` map-shape
  destructuring lower natively (as long as the branch return types agree —
  returning the `Maybe` map value alongside a plain scalar still rejects).
- Runtime builtins recognized from `GetGlobal`: `println`/`print` (constant
  format strings expand at lower time with exact `format_variadic_runtime`
  semantics — `{}` substitution, leftover `{}` kept literal, extra args
  space-appended), `assert`/`assert(cond, message)` (false aborts loudly),
  `assert_eq`/`assert_ne` (scalar equality with Int/Float coercion and string
  bytes, VM-format failure message), `panic(args…)` (space-joined display,
  always fatal), and `typeof` (static scalar names; Maybe carriers select
  `Nil` vs the value name at runtime). `IsNil` lowers likewise (scalars are
  never nil, Maybe tests its present bit). `IsList` const-folds the same way
  (typed `List<…>` handles are lists, every other lowerable type is not), and
  `SliceFrom` (the rest-pattern tail slice) calls `list_h::{i64,f64,str}_slice_from`
  for a fresh tail handle (negative start aborts, like the VM). Together they
  lower list-shape and rest destructuring — `if let [a, b, c] = xs { … }` and
  `if let [head, ..tail] = xs { … }` — natively.
- `Raise` (the shape-mismatch guard of an irrefutable `let [a, b, c] = xs`)
  aborts via `rt::panic` with its message constant. This is sound because
  `TryBegin` is itself unsupported: any module with try/catch takes the Tier 0
  fallback, so a natively-lowered module has no handler and every `Raise` is
  uncaught — an abort matches the VM's uncaught-raise exit.
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
- `os.hostname`/`arch`/`os`, `process.cwd`, `fs.temp_dir` (owned strings),
  `fs.read_dir` (sorted UTF-8 entry names as `List<str>`), and `time.since`
  (inline `end - start`). `== nil`/`!= nil` folds for concrete-typed operands
  and tests the present bit for Maybe carriers; `Bool == Bool` widens to i64.
- `datetime` module via chrono in lkrt (the stdlib module's exact crate, so
  formatting/weekday output is byte-identical): `now`/`format`/`parse`/
  `day_of_week`/`day_of_year`/`is_weekend`; `add`/`sub` inline as Int
  arithmetic. `io.std` (the `std` global from `use { std } from io`):
  stdin/stdout/stderr are fixed handles, `write`/`writeln` return the VM's
  byte counts, `flush` → `true`; the lkrt writers flush C stdio before and
  their own stream after, so `printf` output and Rust-side writes keep
  program order. `json` stays out of the subset (dynamic nested values).
- List HOF over compiled zero-capture lambdas (fn-pointer ABI,
  `Const::FnAddr` → `ptr @lk_fn_N`): `map`/`filter`/`reduce` over `List<i64>`
  call the lambda per element inside lkrt; the callback signature goes through
  the same monomorphization lattice as direct calls.
- Lambdas as user-function arguments (`apply(f, x)`): the lambda parameter is
  *erased* and the call retargets to a per-identity **clone** of the callee
  (identity = target function + capture count, capped at 8 clones per
  original; a function called with both lambdas and plain values is
  polymorphic over functions vs values and rejects loudly). A *capturing*
  closure argument additionally appends its environment — resolved to current
  cell contents at the call site — as hidden trailing arguments, so mutation
  between calls stays visible and the identity (hence the clone) is shared
  across environments; forwarding through further helpers nests naturally.
- Returned closures via static summaries: a non-entry function whose *single*
  return is a closure with every capture mapping to one of its own parameters,
  with an effect-free body (constant loads, moves, cell setup, the
  `MakeClosure` itself — anything that can abort or observe state
  disqualifies), is summarized; call sites seed the result register with the
  closure ref built from their argument values and no call is emitted (the
  body is skipped entirely). Factory results feed straight into the
  closure-as-argument path. Mutating returned closures (counters), multiple
  returns, and effectful factories reject loudly.
- Container display through the print family: `println(xs)` / `println("{}",
  xs)` render `List<i64/f64/str>` with VM-exact separators and `{:?}` string
  quoting via lkrt display helpers; the scalar-only contexts
  (`ToString`/template interpolation/`+` concat) keep rejecting containers
  exactly like the VM's runtime error.
- `CallMethodK` (the boxing-free method-call opcode) lowers through the same
  per-(receiver type, method) dispatch as the legacy `__lk_call_method` shape;
  builtin/global refs resolve across blocks when all predecessor paths agree
  (`assert(a || b)`).
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

The LLVM path itself stays pure: unsupported shapes produce no `.ll`, and the
LLVM output must not embed a serialized `.lkm` payload, `ModuleArtifact`, the
bytecode executor, `VmContext`, parser, type checker, or compiler. The final
native executable may link Rust `std`, libc/libm, and `lkrt`. See
`docs/llvm/native-stdlib.md` for the native stdlib boundary. (The CLI's Tier 0
fallback — used when the LLVM path reports `Unsupported` — is a *separate*
bundle path that embeds the VM via the C ABI; it does not compromise this LLVM
path's no-VM invariant.)

## Usage

```sh
lk compile FILE.lk          # native executable (default); falls back to the
                            #   Tier 0 VM bundle if not natively lowerable
lk compile llvm FILE.lk     # emit FILE.ll (no fallback: fails on Unsupported)
lk compile bytecode FILE.lk # emit the .lkm module artifact instead
lk bundle FILE.lk           # Tier 0: self-contained exe that embeds the VM
```

`LK_AOT_NO_FALLBACK=1` makes `lk compile FILE.lk` require native lowering (no
Tier 0 fallback).

Direct `lk file.lk` execution defaults to the bytecode VM; `LK_NATIVE_RUN=1`
opts into the cached native fast path when the program lowers.
`LK_NATIVE_SANITIZE=address,undefined` forwards `-fsanitize=` to clang for
sanitized differential runs.

## Future work

Expanding this lowering surface (map structural equality, `json`/dynamic
tagged values, mixed-element constant containers, closures stored in
containers) must go through the MIR pipeline; a second lowering path, or any
VM/dynamic-value representation *inside lowered code*, stays out of bounds.
The planned per-function Tier 1 hybrid (plan.md M4.2.2) does not breach this:
VM-executed functions appear in the IR only as extern `lk_hybrid_*` calls and
the VM enters at link time — see
[`docs/llvm/tier1-hybrid.md`](tier1-hybrid.md) for the accepted design. The
RFC §7 phase-4 first-class-function arc (indirect calls, lambdas
and capturing closures as arguments, returned closures) landed via erasure,
clone specialization, and static summaries — a runtime `{fn_ptr, env}`
representation was never needed for the observed corpus.
