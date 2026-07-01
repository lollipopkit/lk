# LLVM Native Stdlib Architecture

## Goal

LLVM AOT binaries may link a small native runtime, but they must remain true
native executables. The runtime boundary is for typed host primitives and helper
code, not for running LK bytecode.

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
- Runtime stdlib modules live in `lk-stdlib`; `lk-llvm` may read the stdlib
  registry at compile time to discover module/global availability and display
  metadata without making `lk-core` depend on stdlib.
- Host-only primitives live in `lkrt` and are exposed through typed LLVM
  capability mappings.

LLVM lowering must not reimplement full stdlib method bodies with ad hoc string
matches. It may call monomorphized LK stdlib functions or typed `lkrt`
intrinsics.

## ABI Rules

- Prefer typed ABI: `i64`, `double`, `(ptr, len)` text, typed list/map handles,
  and monomorphized container layouts.
- `lkrt_abi_version()` exposes the native runtime ABI version. LLVM lowering
  should treat a missing or incompatible ABI as a link/configuration error, not
  as a reason to fall back to the VM.
- Monomorphized container method bodies live in `lkrt` as typed helpers rather
  than as per-shape hand-written LLVM IR. The `DynamicList<i64>`, `DynamicList<f64>`,
  and `DynamicList<str>` layouts are fully sunk to `lkrt_list_{i64,f64,str}_*`
  helpers, declared through the native intrinsic registry (modules `list.i64`,
  `list.f64`, `list.str`). They take a raw element `ptr` + `i64` length (plus
  `dst`/`dst_len` out-pointers for producing helpers). They are `Pure`: they only
  read the source buffer and write the caller-owned destination buffer, never
  touching host state.
- Because LLVM lowers in-place shapes (`xs.slice(..)`, `xs.sort()`) onto the same
  storage, every helper must tolerate `src == dst` aliasing — use raw pointers and
  `ptr::copy` (memmove) for range moves, never overlapping `&`/`&mut` slices or
  `copy_nonoverlapping`.
- Element semantics must match the old IR exactly: `f64` compares via `fcmp`
  (Rust `PartialEq`/`PartialOrd`, NaN-aware); `str` elements are `*const c_char`
  compared with `strcmp` (byte-wise `CStr`). Structural `str` ops only move
  pointers; only `push`/`insert`/`set` take ownership of the injected value by
  duplicating it (leaked, matching `strdup`'s never-freed contract). Missing/empty
  `str` results return a stable empty C string.
- `DynamicMap<i64, V>` lookup/set helpers are also sunk to
  `lkrt_map_i64_{int,f64,ptr}_{lookup,set}` (module `map.i64`): `lookup` writes the
  found value through an `out` pointer and returns 1/0 (the caller stores that into
  its `present` slot, preserving `nil`), and `set` updates in place or appends.
  The `i64`-keyed map's `has`/`delete`/`iter`/`values`/`keys` shapes remain inline
  IR in the emitters (they are not helper-pool functions).
- `DynamicMap<str, V>` uses a composite short-string key (`prefix` string + integer
  suffix, so `"k12"` stores `prefix="k"`, `number=12`). Its `split_key`, `lookup`,
  `set`, `contains` (for `has`), and compaction `delete` are sunk to
  `lkrt_map_str_{split_key,contains}` + `lkrt_map_str_{int,f64,ptr}_{lookup,set,delete}`
  (module `map.str`). Keys compare with `strcmp(prefix) && number ==`; `split_key`
  scans trailing ASCII digits and leaks a truncated prefix copy (a "raw" key — empty,
  all-digits, or no trailing digit — keeps the original pointer with `number=0`).
  `lookup` follows the same `out`/return present-bit convention as `map.i64`; `delete`
  copies the surviving entries into the destination arrays (tolerating `src == dst`),
  writes the removed value through `out_value`, sets `out_present`, and returns the
  destination length. Only the `str`-keyed map's `iter`/`values`/`keys` remain inline.
- `lkrt_i64_decimal_len(i64)` (module `fmt`, `Pure`) returns the decimal-spelling
  width of an `i64` (counting a leading `-`); it sizes dynamic template/text buffers,
  replacing the old hand-written `@lk_i64_decimal_len` IR.
- Strings returned by `lkrt` are owned by `lkrt` and must be released with
  `lkrt_string_free(ptr)` when generated code starts tracking native ownership.
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
- Any future exported C ABI in `lkrt` must be isolated there and audited; LLVM
  outside `lkrt` must not introduce unsafe code.
- Host-effect intrinsic metadata lives in `lk-llvm`'s native intrinsic registry.
  The registry is the source for `lkrt_*` LLVM declarations and records each
  intrinsic's typed signature and effect (`Pure`, `ReadsHost`, or `WritesHost`).

## Implementation Shape

The native stdlib path is:

```text
LK user code
  -> Compiler / ModuleArtifact compile-time boundary
  -> lk-llvm shape analysis, stdlib discovery, and monomorphization
  -> direct LLVM IR + typed calls to lkrt
  -> clang links IR with liblkrt.a
```

`lkrt` is linked at final executable build time. It must not depend on `lk-core`
or `lk-stdlib`; that keeps parser/compiler/VM code out of the final binary.
`lk-llvm` is a compile-time crate and may depend on both `lk-core` and
`lk-stdlib`; the CLI only connects it when the `llvm` feature is enabled.
