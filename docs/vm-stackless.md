# M2.5 Stackless VM: Design & Staging (plan.md 4.5)

Status: **design, not yet implemented.** Mapped against the tree at the time
of writing (`core/src/vm/exec.rs`, `exec/call.rs`, `exec/handler.rs`).

## Current state (mapped)

- The dispatch loop (`run_function_inner_impl`, exec.rs) runs **one function's
  code**; every LK→LK call (`call_closure_stack_args`,
  `call_closure_named_stack_args`, call.rs) saves six pieces of caller state
  into Rust locals (`frame_base`, `stack_top`, `pc`, `captures`,
  `register_count`, `handler_stack` depth) and **recurses** into
  `run_function_inner`. The Rust stack is the VM's control stack.
- The **value stack is already stackless-ready**: one contiguous
  `RuntimeModuleState.stack` with `[frame_base, frame_base+register_count)`
  windows per activation; callee frames are appended at `stack_top`.
- Errors propagate as Rust `Err` through `?`; each recursion boundary
  intercepts `LanguageRaise` (`handle_language_raise`) to find a matching
  `ErrorHandler { catch_reg, catch_pc, frame_base, stack_top }` and resume, and
  pushes traceback frames on propagation. This is the subtlest machinery to
  port — read `handler.rs` + both call sites in `call.rs` before touching it.
- Native→VM re-entry (`call_runtime_value_runtime`: pcall, stdlib iter/stream
  HOFs, the Tier 1 bridge) puts a *native Rust frame in the middle* of VM
  execution by design.
- There is **no recursion depth guard**: deep LK recursion overflows the Rust
  stack and aborts the process today.

## Decisions

1. **Scope v1: flatten LK→LK calls only.** Call opcodes push an explicit
   `Frame` onto a `Vec<Frame>` owned by the executor and `continue` the
   dispatch loop; `Return` pops. Native→VM re-entry **stays recursive**: each
   re-entry starts a new flat run bounded by its `run_function_inner` call
   (frames pushed within it pop within it), and errors cross the native
   boundary as Rust `Err` exactly as today. Rewriting stdlib callbacks as
   piccolo-style `Sequence` state machines is explicitly out of scope — LK
   recursion depth then only grows the frame Vec, not the Rust stack.
2. **Frame contents** = today's six saved items plus what the recursive
   return path carried implicitly: `{ function_index, pc, frame_base,
   register_count, captures, handler_depth, ret_dst }` where `ret_dst` is the
   caller register receiving the call result (today: `returns.into_first()`
   written by the opcode handler after recursion).
3. **The loop rekeys on frame switch, not per instruction**: it holds
   `code: &[Instr]`/`function: &Function` and refreshes both on push/pop.
   Module-less execution (`execute(function)` test entry) keeps a single
   implicit frame.
4. **Raise unwinding walks the frame Vec**: pop frames (truncating
   `handler_stack` to each frame's `handler_depth`, pushing traceback names)
   until a handler within the current frame's depth matches, then restore
   `pc = catch_pc`/`frame_base`/`stack_top` from the handler. Uncaught at the
   flat run's bottom → return `Err` to the (possibly native) caller as today.
5. **Depth guard**: with control state on the heap, add a configurable frame
   limit (default generous, e.g. 1M frames) so runaway recursion raises a
   catchable error instead of aborting — closes the stack-overflow hole.
6. **The perf gate is the exit criterion for every sub-step**: dist build +
   `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 bash bench/run_workload_bench.sh`, geomean
   LK/Lua regression >10% blocks. The frame push/pop replaces six local saves
   plus a Rust call — expected roughly neutral, but *measured, not assumed*.

## Sub-steps (each committable, full suite + GC-stress + bench green)

1. **①** `Frame` struct + flatten the positional paths
   (`CallDirect`/`call_closure_stack_args` — the hot family). Named/method
   calls stay recursive; mixing is safe because each recursive boundary still
   saves/restores absolute state, and flat unwinding stops at the current flat
   run's bottom frame.
2. **②** Flatten `CallNamed`/`CallMethodK` (named-arg marshaling moves into
   the frame push).
3. **③** Port raise unwinding to the frame walk (removes the per-boundary
   `downcast_ref` on the error path); traceback and `pcall`/`try` semantics
   pinned by the existing conformance corpus (`error_model_edges.lk`,
   traceback tests).
4. **④** Frame-depth guard + bench validation write-up; coroutines/`yield`
   (plan 4.5's real payoff) become a follow-up building on the frame Vec —
   not part of M2.5.

## Risks

- `handle_language_raise` resume semantics (catch across inlined scopes,
  pending-error rooting) are the highest-risk port — study before ①.
- Dispatch-loop shape changes can shift optimizer behavior even when the
  frame ops are cheap; if the bench gate rejects ①, the fallback is keeping
  recursion for calls but adding only the depth guard (bounded loss, plan
  M2.5 deferred with data — the "measured rejection" outcome plan.md allows).
