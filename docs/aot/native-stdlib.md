# AOT Native Stdlib Architecture

## Goal

AOT binaries may link a small native runtime, but they must remain true
native executables. The runtime boundary is for typed host primitives and helper
code, not for running LK bytecode.

> Naming note: this document predates the Cranelift migration and the
> `llvm` → `lk-aot` crate rename, so "LLVM" below should be read as "the
> native AOT backend" wherever it describes current rules.

## Binary Boundary

Allowed in a native binary:

- Rust `std`, libc, libm, and platform runtime code required by linked helpers.
- `lkrt`, a small typed native runtime static library.
- Typed container, string, display, panic/error, and host intrinsic helpers.

Forbidden in a native binary:

- LK parser, type checker, compiler, resolver, or package loader.
- `ModuleArtifact` JSON payloads.
- bytecode executor, bytecode dispatcher, VM shell launcher, or `VmContext`.
- Any path that compiles to bytecode and then executes that bytecode at runtime.

## Stdlib Source Of Truth

Stdlib support has two sources:

- Pure stdlib logic lives as LK source and is compiled through the normal
  compiler, VM IR, and LLVM lowering pipeline.
- Runtime stdlib modules live in `lk-stdlib`; the AOT lowering may read the
  stdlib registry at compile time to discover module/global availability and
  display metadata without making `lk-core` depend on stdlib.
- Host-only primitives live in `lkrt` and are exposed through typed ABI
  capability mappings.

Native lowering must not reimplement full stdlib method bodies with ad hoc
string matches. It may call monomorphized LK stdlib functions or typed `lkrt`
intrinsics.

## ABI Rules

- Prefer typed ABI: `i64`, `double`, `(ptr, len)` text, typed list/map handles,
  and monomorphized container layouts.
- `lkrt_abi_version()` exposes the native runtime ABI version. Native lowering
  should treat a missing or incompatible ABI as a link/configuration error, not
  as a reason to fall back to the VM.
- Container method bodies live in `lkrt` as typed helpers rather than as
  per-shape hand-written codegen. Lists and maps are **opaque growable
  handles** (`*mut Vec<T>` / `*mut FxHashMap<K, V>` behind `ptr`), created by
  the `lkrt_lklist_*_new` / `lkrt_lkmap_*_new` entry points in
  `lkrt/src/lklist.rs` / `lkrt/src/lkmap.rs` and registered in the runtime
  arena (reclaimed by `lkrt_cleanup()` at exit). There is no fixed capacity
  and no caller-owned buffer; the raw-buffer `lkrt_list_*`/`lkrt_map_*`
  helper family described by earlier drafts was retired with the legacy text
  backend.
- The schema single source of truth is `aot/abi` (`for_each_abi_fn!`):
  `list_h.*` covers `i64`/`f64`/`str` element lists (`push`/`set`/`len`/
  `at`/`contains`/`join` plus the fn-pointer HOF entries
  `i64_{map,filter,reduce}_fn`), `map_h.*` covers `{str,i64} × {i64,f64}`
  maps (`new`/`set`/`set_ik`/`len`). Handle-typed parameters are `Ptr`.
- Reads with missing/out-of-range semantics return a by-value `Maybe`
  (`lkrt_lklist_*_get_pair` / `lkrt_lkmap_*_get_pair` → `{value, present}`
  structs); stores with fatal VM semantics (`xs[i] = v` out of range) abort
  loudly via `lkrt` instead of returning. String elements/keys are
  `*const c_char`; the map copies its key on insert, so composite-key stores
  use the zero-allocation `map_h.*_set_ik` (key assembled on the lkrt stack)
  and key temporaries are freed eagerly by the lowering.
- Strings returned by `lkrt` are owned by `lkrt` and must be released with
  `lkrt_string_free(ptr)` when generated code starts tracking native ownership.
- Container handles are arena-owned too, but a handle proven dead at the end of
  its block is released early via `lkrt_rt_handle_release(ptr)` (the scope-drop
  pass in `lk_aot_mir::opt`); without it a loop — or a repeatedly called
  function, such as a `try` body — retains every temporary container until exit.
  The arena is **per thread**: `spawn`/`go` are real OS threads, each owning its
  own arena and reclaiming it on exit, which is sound only because channels
  deep-copy (values never cross a thread). Never pass a handle or arena string
  between threads. Whether a call may retain a handle passed as
  its receiver is answered by `lk_aot_abi::receiver_escapes`, which defaults to
  "yes" — a new ABI entry is non-releasable until someone audits it.
- `lkrt_last_error()` returns an owned string for diagnostics. Existing aborting
  helpers still abort on failure, but new status/out-param helpers should record
  actionable errors through the same error channel.
- TCP native stdlib helpers use typed `lkrt` intrinsics: strings are passed as
  `ptr`, and TCP streams/byte buffers are opaque `i64` handles owned by `lkrt`.
  `tcp.read` returns a bytes handle, and `bytes.to_string_utf8` validates that
  handle before returning a string pointer.
- Opaque handles are typed resources managed by `lkrt`. A handle must not be
  accepted as the wrong resource kind, and every resource kind needs an explicit
  close/free path such as `lkrt_tcp_close`, `lkrt_bytes_free`, or
  `lkrt_handle_close`.
- Standard IO native helpers use small opaque `i64` resource handles
  (`0 = stdin`, `1 = stdout`, `2 = stderr`) and typed `lkrt` calls for
  `io.std.write`, `io.std.writeln`, `io.std.flush`, and
  `io.std.read_to_string`.
- Environment, filesystem, and process helpers that require host state lower to
  `lkrt` calls instead of compile-time constants. Current scalar/native
  lowering covers `env.get`, `env.get_or`, `env.has`, `fs.exists`, `fs.read`,
  `fs.read_to_string`, `fs.write`, `fs.read_dir`, `fs.canonicalize`,
  `fs.temp_dir`, and `process.cwd` where the value can be represented as
  scalar/string/bytes handles. `fs.metadata()` remains outside scalar lowering
  until native map/object ABI support exists.
- Do not use `RuntimeVal`, `HeapStore`, `RuntimeExport`, or `NativeRuntime`
  as the default native ABI.
- Generic runtime-value ABI is not allowed as a silent fallback. If a shape is
  not native-lowerable, the compiler must report a concrete unsupported reason.
- Any future exported C ABI in `lkrt` must be isolated there and audited; code
  outside `lkrt` must not introduce unsafe code.
- Host-effect intrinsic metadata lives in the `aot/abi` schema
  (`for_each_abi_fn!`), not in the AOT driver — the old `llvm` crate's `intrinsics.rs`
  registry retired with the legacy text backend. The schema
  records each intrinsic's typed signature and effect (`Pure`, `ReadsHost`, or
  `WritesHost`) and is the single source for the codegen-side declarations and
  the lkrt conformance test. `Pure` is **load-bearing**: `lk_aot_mir::opt`
  collapses redundant `Pure` calls (Cranelift cannot — an opaque `lkrt` symbol
  is a black box to it). Mislabeling a stateful helper `Pure` is therefore a
  miscompile, not a missed optimization: `bytes.to_string_utf8` *consumes* its
  handle and was mislabeled until that pass landed.

## Implementation Shape

The native stdlib path is:

```text
LK user code
  -> Compiler / ModuleArtifact compile-time boundary
  -> aot/lower: shape analysis, stdlib discovery, monomorphization -> MIR
  -> aot/codegen (clif.rs): MIR -> Cranelift IR -> native object
  -> clang (link driver only) links the object with liblkrt.a
```

`lkrt` is linked at final executable build time. It must not depend on `lk-core`
or `lk-stdlib`; that keeps parser/compiler/VM code out of the final binary.
`lk-aot-lower` is a compile-time crate and may depend on both `lk-core` and
`lk-stdlib`; the CLI only connects the AOT path when the `aot` feature is
enabled. A Tier 1 hybrid binary additionally links `liblk_api.a` for the
bridge (see [`tier1-hybrid.md`](./tier1-hybrid.md)).
