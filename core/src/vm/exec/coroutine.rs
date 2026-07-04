//! Stackless coroutines / `yield` (post-M2.5 plan, plan.md 4.5): a suspended
//! coroutine is a `CoroutineState` holding exactly the pieces the flattened
//! `CallFrame` dispatch (plan M2.5) already externalizes onto the heap —
//! `frames`, a private register `stack`, and the innermost activation's
//! `pc`/`frame_base`/`register_count`/`captures` — plus its own call-depth
//! counter. `resume_coroutine_runtime` drives it via the same "take the
//! shared `RuntimeModuleState` out of the resumer, run a dedicated
//! `Executor`, put it back" pattern `runtime_callable.rs`'s
//! `call_closure_value` already uses for native re-entry.
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{CallableValue, HeapRef, HeapValue, RuntimeVal};
use crate::vm::{Function, LkRaisedValue, Module, RuntimeModuleState, VmContext};

use super::{CallFrame, Executor, FrameOutcome, ReturnValues};

/// Lifecycle of a `HeapValue::Coroutine`. Collapses Lua's four-state model
/// (suspended/running/normal/dead) to what v1 actually distinguishes: a
/// coroutine that hasn't started yet behaves like `Suspended` from the
/// outside (`coroutine_status` reports "suspended" for both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoroutineStatus {
    NotStarted,
    Suspended,
    Running,
    Done,
    Errored,
}

impl CoroutineStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted | Self::Suspended => "suspended",
            Self::Running => "running",
            Self::Done | Self::Errored => "dead",
        }
    }
}

/// Suspended coroutine state. Everything here is private to the coroutine —
/// the heap/globals/inline-caches/pending-raise-root it needs while running
/// are *shared* with whatever resumes it (see `resume_coroutine_runtime`),
/// borrowed for the duration of one resume call, never stored here.
#[derive(Debug, Clone)]
pub struct CoroutineState {
    status: CoroutineStatus,
    /// Entry function; only used to seed the very first resume's arguments.
    function_index: u32,
    captures: Arc<Vec<RuntimeVal>>,
    stack: Vec<RuntimeVal>,
    stack_top: usize,
    frames: Vec<CallFrame>,
    frame_base: usize,
    register_count: u16,
    pc: usize,
    /// The function whose code the parked `pc` belongs to — may differ from
    /// `function_index` once the coroutine has made LK calls of its own
    /// (those are flattened into `frames`, same as any other flat run).
    current_function_index: u32,
    /// The register (in the parked, innermost frame) the next resume's value
    /// must land in — remembered from `FrameOutcome::Yielded`.
    resume_dst: u8,
    /// The coroutine's own LK call-depth counter — independent of whatever
    /// resumes it, for the same reason `RuntimeModuleState::call_depth`
    /// lives in shared state rather than the executor (a runaway coroutine
    /// must not have its depth silently reset by being resumed repeatedly).
    call_depth: usize,
}

impl CoroutineState {
    fn new(function_index: u32, captures: Arc<Vec<RuntimeVal>>) -> Self {
        Self {
            status: CoroutineStatus::NotStarted,
            function_index,
            captures,
            stack: Vec::new(),
            stack_top: 0,
            frames: Vec::new(),
            frame_base: 0,
            register_count: 0,
            pc: 0,
            current_function_index: function_index,
            resume_dst: 0,
            call_depth: 0,
        }
    }

    /// A minimal, GC-safe stand-in swapped into the heap slot while the real
    /// state is checked out and running (see `resume_coroutine_runtime`):
    /// empty stack/frames, so a GC pass that finds this slot reachable (via
    /// some other lingering register/global) traces nothing stale — the
    /// coroutine's actual live values are rooted normally through the
    /// dedicated `Executor` that's running them for the duration of the call.
    fn running_placeholder(function_index: u32, captures: Arc<Vec<RuntimeVal>>) -> Self {
        Self {
            status: CoroutineStatus::Running,
            ..Self::new(function_index, captures)
        }
    }

    pub fn status(&self) -> CoroutineStatus {
        self.status
    }

    /// Every `RuntimeVal` directly reachable from this parked coroutine's own
    /// state — the GC edges `val::runtime_model`'s tracer needs for
    /// `HeapValue::Coroutine`. Mirrors `Executor::root_refs`'s "current +
    /// ancestor frame captures" chain, just over a parked coroutine's own
    /// state instead of a live `Executor`.
    pub fn gc_edges(&self) -> impl Iterator<Item = &RuntimeVal> {
        self.stack[..self.stack_top]
            .iter()
            .chain(self.captures.iter())
            .chain(self.frames.iter().flat_map(|frame| frame.captures.iter()))
    }
}

/// `coroutine_create(fn) -> Coroutine`: `fn` must be a plain LK closure (not
/// a native function — natives can't contain `yield`).
pub fn create_coroutine_runtime(callee: RuntimeVal, heap: &mut crate::val::HeapStore) -> Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = callee else {
        bail!("coroutine_create expects a function, got {:?}", callee.kind());
    };
    let Some(HeapValue::Callable(CallableValue::Closure {
        function_index,
        captures,
    })) = heap.get(handle)
    else {
        bail!("coroutine_create expects a plain LK function (not a native function)");
    };
    let state = CoroutineState::new(*function_index, Arc::clone(captures));
    Ok(RuntimeVal::Obj(heap.alloc(HeapValue::Coroutine(Box::new(state)))))
}

/// `coroutine_status(co) -> "suspended" | "running" | "dead"`.
pub fn coroutine_status_runtime(coroutine: RuntimeVal, heap: &crate::val::HeapStore) -> Result<&'static str> {
    let RuntimeVal::Obj(handle) = coroutine else {
        bail!("coroutine_status expects a coroutine, got {:?}", coroutine.kind());
    };
    let Some(HeapValue::Coroutine(state)) = heap.get(handle) else {
        bail!("coroutine_status expects a coroutine");
    };
    Ok(state.status().as_str())
}

/// What one call to `run_coroutine_step` (the coroutine-dedicated sibling of
/// `run_function_inner_impl`) produced.
enum CoroutineStepOutcome {
    Done(ReturnValues),
    Yielded { value: RuntimeVal, dst: u8 },
}

impl Executor {
    /// Coroutine-dedicated counterpart of `run_function_inner_impl`: same
    /// push/pop-flattened trampoline over `dispatch_within_frame`, except
    /// `base_frame_depth` is unconditionally `0` (every frame in `self.frames`
    /// belongs to *this* coroutine — there is no outer flat run sharing the
    /// `Vec`, unlike native re-entry nested inside an ordinary call), and
    /// `FrameOutcome::Yielded` is a real, expected outcome instead of
    /// `unreachable!`.
    fn run_coroutine_step(
        &mut self,
        function: &Function,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<CoroutineStepOutcome> {
        const BASE_FRAME_DEPTH: usize = 0;
        let mut function = function;
        loop {
            match self.dispatch_within_frame::<false>(function, module, ctx, BASE_FRAME_DEPTH) {
                Ok(FrameOutcome::Switch(idx)) => {
                    function = module
                        .and_then(|module| module.functions.get(idx as usize))
                        .ok_or_else(|| anyhow!("function index {} out of bounds", idx))?;
                }
                Ok(FrameOutcome::Done(values)) => return Ok(CoroutineStepOutcome::Done(values)),
                Ok(FrameOutcome::Yielded { value, dst }) => return Ok(CoroutineStepOutcome::Yielded { value, dst }),
                Err(error) => {
                    let idx = self.unwind_flat_run(error, function, module, ctx, BASE_FRAME_DEPTH)?;
                    function = module
                        .and_then(|module| module.functions.get(idx as usize))
                        .ok_or_else(|| anyhow!("function index {} out of bounds", idx))?;
                }
            }
        }
    }
}

/// `coroutine_resume(co, ...args) -> [ok, value]` — mirrors `pcall`'s
/// `[ok, value]` convention. `args` seed the entry function's parameters on
/// the *first* resume (arity-checked like a normal call); on later resumes
/// only `args[0]` (or `Nil`) is delivered as the paused `yield` expression's
/// result.
pub fn resume_coroutine_runtime(
    coroutine: RuntimeVal,
    resume_args: &[RuntimeVal],
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = coroutine else {
        bail!("coroutine_resume expects a coroutine, got {:?}", coroutine.kind());
    };
    let module = module.ok_or_else(|| anyhow!("coroutine_resume requires Module execution"))?;

    let mut coroutine_state = check_out_coroutine(handle, state)?;
    let is_first_resume = matches!(coroutine_state.status, CoroutineStatus::NotStarted);
    let starting_function_index = if is_first_resume {
        coroutine_state.function_index
    } else {
        coroutine_state.current_function_index
    };
    let function = module
        .functions
        .get(starting_function_index as usize)
        .ok_or_else(|| anyhow!("function index {} out of bounds", starting_function_index))?;

    if is_first_resume {
        if function.param_count as usize != resume_args.len() {
            // Nothing was mutated in `state` yet — restore the coroutine
            // (still `NotStarted`) before bailing so it stays resumable.
            check_in_coroutine(handle, state, coroutine_state);
            bail!(
                "coroutine expects {} arguments, got {}",
                function.param_count,
                resume_args.len()
            );
        }
        let mut stack = vec![RuntimeVal::Nil; function.register_count as usize];
        stack[..resume_args.len()].copy_from_slice(resume_args);
        coroutine_state.stack = stack;
        coroutine_state.stack_top = function.register_count as usize;
        coroutine_state.frame_base = 0;
        coroutine_state.register_count = function.register_count;
        coroutine_state.pc = 0;
        coroutine_state.current_function_index = coroutine_state.function_index;
    } else {
        let resumed_value = resume_args.first().copied().unwrap_or(RuntimeVal::Nil);
        let dst_index = coroutine_state.frame_base + coroutine_state.resume_dst as usize;
        let Some(slot) = coroutine_state.stack.get_mut(dst_index) else {
            check_in_coroutine(handle, state, coroutine_state);
            bail!("internal error: coroutine resume register out of bounds");
        };
        *slot = resumed_value;
    }

    let mut executor = Executor::new(coroutine_state.register_count);
    executor.captures = Arc::clone(&coroutine_state.captures);
    executor.active_coroutine = Some(handle);
    executor.frames = core::mem::take(&mut coroutine_state.frames);
    executor.frame_base = coroutine_state.frame_base;
    executor.register_count = coroutine_state.register_count;
    executor.pc = coroutine_state.pc;
    executor.current_function_index = coroutine_state.current_function_index;

    // Share heap/globals/inline-caches/pending-raise-root with the resumer
    // for the run's duration; use the coroutine's *own* stack/stack_top/
    // call_depth instead of the resumer's (same swap-and-restore shape as
    // `runtime_callable.rs`'s `call_closure_value`, just field-by-field
    // since only *some* of `RuntimeModuleState` is coroutine-private).
    let mut inner_state = core::mem::take(state);
    let resumer_stack = core::mem::replace(&mut inner_state.stack, core::mem::take(&mut coroutine_state.stack));
    let resumer_stack_top = core::mem::replace(&mut inner_state.stack_top, coroutine_state.stack_top);
    let resumer_call_depth = core::mem::replace(&mut inner_state.call_depth, coroutine_state.call_depth);
    executor.state = inner_state;
    // The resumer's own register window is no longer part of `self.state.
    // stack` for the duration of this run (the coroutine has its own,
    // swapped in above) — without this, anything the resumer still needs
    // after the call returns (not least the register holding `coroutine`
    // itself) would be invisible to GC safepoints hit *while* the coroutine
    // runs. `LK_GC_STRESS=1` catches a missed root like this deterministically.
    executor.extra_gc_roots = resumer_stack[..resumer_stack_top].to_vec();

    let mut ctx = ctx;
    let step_result = executor.run_coroutine_step(function, Some(module), &mut ctx);

    coroutine_state.stack = core::mem::replace(&mut executor.state.stack, resumer_stack);
    coroutine_state.stack_top = core::mem::replace(&mut executor.state.stack_top, resumer_stack_top);
    coroutine_state.call_depth = core::mem::replace(&mut executor.state.call_depth, resumer_call_depth);
    *state = executor.state;

    let (ok, value) = match step_result {
        Ok(CoroutineStepOutcome::Done(values)) => {
            coroutine_state.status = CoroutineStatus::Done;
            coroutine_state.frames = Vec::new();
            coroutine_state.stack = Vec::new();
            (true, values.into_first())
        }
        Ok(CoroutineStepOutcome::Yielded { value, dst }) => {
            coroutine_state.status = CoroutineStatus::Suspended;
            coroutine_state.frames = core::mem::take(&mut executor.frames);
            coroutine_state.frame_base = executor.frame_base;
            coroutine_state.register_count = executor.register_count;
            coroutine_state.pc = executor.pc;
            coroutine_state.current_function_index = executor.current_function_index;
            coroutine_state.resume_dst = dst;
            (true, value)
        }
        Err(error) => {
            coroutine_state.status = CoroutineStatus::Errored;
            coroutine_state.frames = Vec::new();
            coroutine_state.stack = Vec::new();
            let root = error.root_cause();
            if let Some(raised) = root.downcast_ref::<LkRaisedValue>() {
                (false, raised.value)
            } else {
                let message = root.to_string();
                (
                    false,
                    RuntimeVal::Obj(state.heap.alloc(HeapValue::String(Arc::from(message.as_str())))),
                )
            }
        }
    };

    check_in_coroutine(handle, state, coroutine_state);
    let list = state.heap.alloc(HeapValue::List(crate::val::TypedList::Mixed(vec![
        RuntimeVal::Bool(ok),
        value,
    ])));
    Ok(RuntimeVal::Obj(list))
}

fn check_out_coroutine(handle: HeapRef, state: &mut RuntimeModuleState) -> Result<CoroutineState> {
    let Some(HeapValue::Coroutine(boxed)) = state.heap.get_mut(handle) else {
        bail!("coroutine_resume expects a coroutine");
    };
    match boxed.status {
        CoroutineStatus::Running => bail!("cannot resume a coroutine that is already running"),
        CoroutineStatus::Done => bail!("cannot resume a dead coroutine"),
        CoroutineStatus::Errored => bail!("cannot resume a coroutine that errored"),
        CoroutineStatus::NotStarted | CoroutineStatus::Suspended => {}
    }
    let function_index = boxed.function_index;
    let captures = Arc::clone(&boxed.captures);
    Ok(core::mem::replace(
        &mut **boxed,
        CoroutineState::running_placeholder(function_index, captures),
    ))
}

fn check_in_coroutine(handle: HeapRef, state: &mut RuntimeModuleState, coroutine_state: CoroutineState) {
    if let Some(HeapValue::Coroutine(boxed)) = state.heap.get_mut(handle) {
        **boxed = coroutine_state;
    }
}
