# Tier 1 Hybrid: Per-Function VM Fallback (Design)

Status: **design, not yet implemented** (plan.md M4.2.2). This document fixes
the architecture decisions so implementation can proceed in bounded,
independently committable sub-steps. Mapped against the tree at the time of
writing: `aot/lower/src/lib.rs` (`lower()` final pass fails the whole module),
`aot/abi` (scalar ABI table), `aot/codegen` (IR text rendering),
`llvm/src/native_executable.rs` (clang + `liblkrt.a` link),
`cli/src/main.rs::compile_executable` (program-granularity Tier 0 fallback).

## Problem

M4.2 fallback is program-granular: one unsupported construct anywhere flips
the *entire* program to the Tier 0 VM bundle, discarding native compilation
for every function that would have lowered. Tier 1 keeps supported functions
native and executes only the unsupported ones on the VM, inside one binary.

## Decisions

1. **Asymmetric, one-way bridge: native calls VM, never the reverse.** A
   VM-executed function and everything it transitively calls runs on the VM
   (the embedded artifact contains all functions' bytecode, so the VM can
   always continue downward). No VM→native re-entry means no reentrancy
   hazards and no native function table exposed across the boundary. A
   VM-side callee that *would* have lowered natively simply runs interpreted —
   an acceptable v1 performance loss, not a correctness issue.

2. **The bridge lives in `lk-api`, not `lkrt`.** The iron rule stands: `lkrt`
   must not depend on the parser, compiler, `ModuleArtifact`, `VmContext`, or
   the bytecode executor. The bridge is a new `lk_hybrid_*` C-ABI surface in
   `lk-api` (`ffi` feature), the same staticlib the Tier 0 bundle links.

3. **The LLVM path's no-VM invariant is preserved at the `.ll` level and
   amended at the executable level.** `backend.md` declares a VM runtime
   bridge "out of bounds" — that rule was written to keep *lowered code*
   free of dynamic boxing/VM types, and it survives intact: MIR and the
   generated IR never represent a dynamic value; VM-executed functions appear
   only as `declare`d extern `lk_hybrid_*` symbols called with statically
   typed scalar arguments. The serialized artifact is embedded by the *link
   wrapper* (exactly like Tier 0 embeds source today), never by the IR. The
   VM enters at link time: a hybrid executable links `liblkrt.a` *and*
   `liblk_api.a`.

4. **Eligibility (v1) — a reachable non-entry function `f` may be marked
   VM-executed instead of failing the module when:**
   - every `CallDirect` site passes subset-typed arguments (`SigInfer`
     already infers these from call sites — parameter types need no new
     analysis);
   - the call result is **discarded at every site** (the `dead_writes` fact
     for the result register's write, re-checked in the lowering). This
     sidesteps the hard problem: a body that does not lower has no inferable
     return type, and guessing one would miscompile the caller;
   - `f` and its transitive bytecode callees touch **no user global slots**
     (`GetGlobal`/`SetGlobal` of user slots; runtime-builtin `GetGlobal`s are
     fine). Native keeps mutable globals in native storage while the bridge
     VM has its own nil-initialized copies — unsynced access would fork
     state. Global sync is a later slice, not v1;
   - `f` has no captures (`capture_count == 0`) and is not lambda-specialized
     (clone machinery stays out of v1);
   - argument marshaling is scalar-only: `I64`/`F64`/`Bool`/`Str`. Container
     handles (`list_h`/`map_h`) are **not** bridgeable in v1 — a native
     handle and a VM heap object are different worlds; copying is a later,
     explicit decision.
   A function that fails eligibility infects its *callers* (they become
   VM-executed candidates in turn); infection reaching the entry means the
   program is not hybrid-lowerable and falls back to Tier 0 — the existing
   M4.2 behavior, unchanged.

5. **The VM side executes the same `ModuleArtifact`, not re-parsed source.**
   The wrapper embeds the artifact JSON; `lk_hybrid_init` decodes it through
   `from_json_str` → `into_module` → `verify_module` (the surface hardened by
   the M2.7 decoder/verifier fuzz) and builds one lazy `VmContext` on first
   bridge call. Same bytecode identity = same semantics by construction, and
   startup pays nothing when no bridge call ever fires.

6. **C stdio must be flushed before every bridge call.** lkrt prints through
   C stdio (block-buffered on pipes); the VM prints through Rust's
   line-buffered stdout. Without a flush, native output written *before* a
   bridge call can appear *after* the VM's output — a differential failure on
   exactly the corpora that gate this work. Precedent: `lkrt_abort` already
   flushes C stdio for the same reason. The generated call sequence is
   `flush → lk_hybrid_call → (native continues)`; anything native prints
   afterwards lands behind the VM's already-flushed lines, preserving order.

7. **Uncaught VM errors abort the process** with the VM's rendered message
   and a nonzero exit, matching what the VM would have done (`pcall`-caught
   errors never cross the boundary — they resolve inside the VM). Exit-code
   and stderr parity are part of the differential contract.

## Bridge ABI sketch (v1)

```c
// Wrapper-provided, called once lazily from the first bridge call.
void lk_hybrid_init(const char *module_artifact_json);

typedef struct { uint8_t tag; /* 0=i64 1=f64 2=bool 3=str */
                 union { int64_t i; double f; const char *s; } v; } LkHybridArg;

// v1: results are proven-discarded, so the bridge returns nothing.
void lk_hybrid_call_v(uint32_t func_index, const LkHybridArg *args, size_t argc);
```

These symbols are declared directly by codegen (like the `get_pair` helpers),
*not* added to the `aot/abi` table — that table is the lkrt conformance
contract, and lkrt neither implements nor knows about the bridge.

Later slices, in order of unlock value: typed returns once a return-type
proof exists (a conservative bytecode return scan, or the VM type checker's
facts once they reach the artifact); opaque `Any` value handles (native code
moves them between bridge calls without inspecting them — the stepping stone
toward the mixed/dynamic type system every remaining AOT blocker shares);
mutable-global snapshot/sync; container copying.

## Staged sub-steps (each independently committable and gated)

1. **`lk-api` hybrid runtime**: `lk_hybrid_init`/`lk_hybrid_call_v` + a
   Rust-level `call_module_function(fidx, args)` on a decoded artifact, with
   unit tests (no lower/codegen changes; nothing links it yet).
2. **Lower marking**: replace the final-pass `?` in `lower()` with the
   eligibility analysis + caller infection; `MirModule` grows a
   `vm_functions` table (index, symbol-facing signature). MIR snapshot tests
   pin the marking.
3. **Codegen**: render `declare`s + flush/bridge call sequences for
   VM-executed callees; `.ll` snapshot tests (still nothing links).
4. **CLI hybrid link**: when the lowered module has `vm_functions`, emit the
   wrapper (artifact JSON + `lk_hybrid_init` registration), link
   `liblkrt.a` + `liblk_api.a`; end-to-end demo + hand-written differential
   cases (native-with-bridge == VM, stdout + exit code).
5. **Gate hardening**: teach the generative fuzz to emit eligible-but-
   unsupported callees so hybrid binaries join the seeded differential and
   ASan/UBSan corpora.

## Explicitly out of scope (v1)

Bridged return values (needs return-type proof), container/closure
marshaling, global sync, VM→native calls, fuel/heap sandboxing inside hybrid
binaries (Tier 0 has none either), and any change to `lkrt`.
