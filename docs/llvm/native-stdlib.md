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
- TCP native stdlib helpers use typed `lkrt` intrinsics: strings are passed as
  `ptr`, and TCP streams/byte buffers are opaque `i64` handles owned by `lkrt`.
  `tcp.read` returns a bytes handle, and `bytes.to_string_utf8` validates that
  handle before returning a string pointer.
- Standard IO native helpers use small opaque `i64` resource handles
  (`0 = stdin`, `1 = stdout`, `2 = stderr`) and typed `lkrt` calls for
  `io.std.write`, `io.std.writeln`, `io.std.flush`, and
  `io.std.read_to_string`.
- Environment, filesystem, and process helpers that require host state lower to
  `lkrt` calls instead of compile-time constants. `env.get_or`,
  `fs.exists`, `fs.read_dir`, and `process.cwd` read the host state at native
  executable runtime.
- Do not use `RuntimeVal`, `HeapStore`, `RuntimeExport`, or `NativeRuntime`
  as the default native ABI.
- Generic runtime-value ABI is not allowed as a silent fallback. If a shape is
  not native-lowerable, the compiler must report a concrete unsupported reason.
- Any future exported C ABI in `lkrt` must be isolated there and audited; LLVM
  outside `lkrt` must not introduce unsafe code.

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
