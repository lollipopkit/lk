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
- Host-only primitives live in `lkrt` and are exposed through a typed intrinsic
  registry.

LLVM lowering must not reimplement full stdlib method bodies with ad hoc string
matches. It may call monomorphized LK stdlib functions or typed `lkrt`
intrinsics.

## ABI Rules

- Prefer typed ABI: `i64`, `double`, `(ptr, len)` text, typed list/map handles,
  and monomorphized container layouts.
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
  -> LLVM shape analysis and monomorphization
  -> direct LLVM IR + typed calls to lkrt
  -> clang links IR with liblkrt.a
```

`lkrt` is linked at final executable build time. It must not depend on `lk-core`
or `lk-stdlib`; that keeps parser/compiler/VM code out of the final binary.
