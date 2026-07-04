#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use crate::val::RuntimeVal;
use crate::vm::CallWindow;

use super::return_values::ReturnValues;

/// A suspended LK→LK call activation, flattened onto the heap instead of the
/// Rust call stack (plan M2.5 sub-step ①): pushed by `CallDirect`/`Call`
/// (when the target is a closure) instead of recursing, popped by `Return*`
/// or by raise unwinding. Mirrors exactly what the old recursive
/// implementation saved into Rust locals across the call (see
/// `exec/call.rs`'s deleted `call_closure_stack_args`).
///
/// `Clone` is needed so a parked `CoroutineState` (plan: coroutines/`yield`)
/// can derive `Clone` itself — cheap here (an `Arc` clone plus a handful of
/// `Copy` fields), and in practice only ever exercised by `HeapValue`'s
/// derive, not a hot path.
#[derive(Debug, Clone)]
pub(super) struct CallFrame {
    /// The *caller's* function index, restored on pop so the trampoline in
    /// `run_function_inner_impl` knows which `Function`/`code` to resume.
    pub(super) function_index: u32,
    /// The caller's pc at the call instruction (not yet advanced past it).
    pub(super) pc: usize,
    pub(super) frame_base: usize,
    pub(super) register_count: u16,
    pub(super) captures: Arc<Vec<RuntimeVal>>,
    /// `handler_stack.len()` at call time, truncated back to on pop (mirrors
    /// `call_closure_stack_args`'s `saved_handler_depth`).
    pub(super) handler_depth: usize,
    /// The call site's window, replayed against the *caller* frame on pop to
    /// clear argument temporaries and write the result (mirrors
    /// `clear_call_window_temps`/`write_returns` at the old recursive call site).
    pub(super) window: CallWindow,
    /// Named-argument pair count for `CallNamed` (0 for `CallDirect`/`Call`),
    /// needed to clear the named k/v temp registers on pop, same as
    /// `clear_call_window_temps(window, named_count)` at the old call site.
    pub(super) named_count: u16,
    pub(super) stack_top: usize,
}

/// What `dispatch_within_frame` hands back to the trampoline loop in
/// `run_function_inner_impl` (or, for `Yielded`, `coroutine::run_coroutine_step`).
pub(super) enum FrameOutcome {
    /// A frame was pushed (LK call) or popped (LK return); resume dispatch
    /// at `module.functions[_]` with the executor's now-current state.
    Switch(u32),
    /// This flat run is finished (no caller frame within it) — return to
    /// whatever Rust caller invoked `run_function_inner_impl` (native
    /// re-entry, or the true top-level entry).
    Done(ReturnValues),
    /// A `Yield` instruction suspended the *entire* flat run (not just the
    /// innermost frame) inside a coroutine-driving `Executor`
    /// (`Executor::active_coroutine.is_some()` — the `Yield` opcode handler
    /// checks this and only ever produces this outcome when it holds).
    /// `dst` is the register (in the *current*, about-to-be-parked, frame)
    /// that the next resume's value must be written into.
    Yielded { value: RuntimeVal, dst: u8 },
}
