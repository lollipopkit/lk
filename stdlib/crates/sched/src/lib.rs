// Cooperative coroutine scheduler for LK — the bridge between stackless
// coroutines (`coroutine_create`/`coroutine_resume` + `yield`) and the
// tokio-backed chan/task runtime.
//
// Natives can't yield (a structural VM restriction — see docs/coroutines.md),
// so blocking channel ops can't transparently suspend a coroutine the way Go
// does. Instead, the classic stackless design: the builders here (`sched.recv`
// & friends) only *allocate a descriptor* of the wait — a small tagged list —
// user code suspends itself explicitly with `yield sched.recv(c)`, and the
// `sched.run` driver resumes coroutines round-robin, interprets yielded
// descriptors (non-blocking try first), and only blocks the VM thread once
// every coroutine is parked — on a tokio select over all parked channel/task
// arms bounded by the earliest sleep deadline.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use lk_core::{
    rt::{AsyncRuntimeHandle, RuntimePayload},
    val::{HeapRef, HeapStore, HeapValue, RuntimeVal, ShortStr, TaskValue, TypedList},
    vm::{
        CoroutineStatus, Module, NativeArgs, NativeRuntime, RuntimeModuleState, VmContext, coroutine_status_runtime,
        create_coroutine_runtime, resume_coroutine_runtime,
    },
};
use tokio::task::JoinHandle;

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

/// Descriptor tags. All ≤7 bytes so they stay inline `ShortStr`s (zero heap
/// allocation); the "s." prefix keeps ordinary user lists from matching one
/// by accident.
const TAG_RECV: &str = "s.recv";
const TAG_SEND: &str = "s.send";
const TAG_SLEEP: &str = "s.sleep";
const TAG_PAUSE: &str = "s.pause";
const TAG_SPAWN: &str = "s.spawn";
const TAG_JOIN: &str = "s.join";
const TAG_AWAIT: &str = "s.await";

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(
    name = "sched",
    docs = "Cooperative scheduler multiplexing coroutines over channels, timers and tasks"
)]
pub struct SchedModule;

#[lk_stdlib_common::stdlib_exports(module = "sched", runtime_builtins = true)]
impl SchedModule {
    /// Wait descriptor: receive from a channel. `yield sched.recv(c)`
    /// evaluates to `[ok, value]`, `[false, nil]` once the channel closes —
    /// the same convention as the blocking global `recv(c)`.
    #[stdlib_export(name = "recv", params(channel: Channel), returns = List)]
    fn recv(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let channel = *args.get(0).expect("checked arity");
        channel_id(&channel, runtime.heap(), "sched.recv()")?;
        Ok(descriptor(runtime.heap_mut(), TAG_RECV, [channel]))
    }

    /// Wait descriptor: send a value to a channel. `yield sched.send(c, v)`
    /// evaluates to a Bool — `true` once delivered, `false` if the channel
    /// closed first — mirroring the blocking global `send(c, v)`.
    #[stdlib_export(name = "send", params(channel: Channel, value: Any), returns = List)]
    fn send(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        channel_id(&values[0], runtime.heap(), "sched.send()")?;
        let (channel, value) = (values[0], values[1]);
        Ok(descriptor(runtime.heap_mut(), TAG_SEND, [channel, value]))
    }

    /// Wait descriptor: suspend this coroutine for at least `ms` milliseconds
    /// without blocking the other coroutines. `yield sched.sleep(ms)` → Nil.
    #[stdlib_export(name = "sleep", params(ms: Int | Float), returns = List)]
    fn sleep(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let ms = match args.get(0).expect("checked arity") {
            RuntimeVal::Int(value) => *value,
            RuntimeVal::Float(value) => *value as i64,
            other => bail!("sched.sleep() expects a numeric argument, got {:?}", other.kind()),
        };
        if ms < 0 {
            bail!("sched.sleep() duration must be non-negative");
        }
        Ok(descriptor(runtime.heap_mut(), TAG_SLEEP, [RuntimeVal::Int(ms)]))
    }

    /// Wait descriptor: yield the rest of this time slice (go to the back of
    /// the run queue). `yield sched.pause()` → Nil.
    #[stdlib_export(name = "pause", params(), returns = List)]
    fn pause(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(descriptor(runtime.heap_mut(), TAG_PAUSE, []))
    }

    /// Wait descriptor: add a new coroutine to the running scheduler.
    /// `yield sched.spawn(f, ...args)` evaluates to the new coroutine's
    /// handle (usable with `sched.join`); `args` seed `f`'s parameters.
    #[stdlib_export(name = "spawn", params(f: Fn, ...args: Any), returns = List)]
    fn spawn(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let Some((&callee, rest)) = values.split_first() else {
            bail!("sched.spawn() expects at least 1 argument: the function");
        };
        if !matches!(
            heap_value(&callee, runtime.heap(), "sched.spawn()")?,
            HeapValue::Callable(_)
        ) {
            bail!("sched.spawn() expects a function argument");
        }
        let mut items = Vec::with_capacity(1 + rest.len());
        items.push(callee);
        items.extend_from_slice(rest);
        Ok(descriptor(runtime.heap_mut(), TAG_SPAWN, items))
    }

    /// Wait descriptor: wait until a scheduler-managed coroutine (from
    /// `sched.spawn`) finishes. `yield sched.join(co)` → its `[ok, value]`.
    #[stdlib_export(name = "join", params(coroutine: Any), returns = List)]
    fn join(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let coroutine = *args.get(0).expect("checked arity");
        if !matches!(
            heap_value(&coroutine, runtime.heap(), "sched.join()")?,
            HeapValue::Coroutine(_)
        ) {
            bail!("sched.join() expects a coroutine argument");
        }
        Ok(descriptor(runtime.heap_mut(), TAG_JOIN, [coroutine]))
    }

    /// Wait descriptor: wait for a tokio task (from the global `spawn(f)`)
    /// without blocking the other coroutines. `yield sched.await(t)` →
    /// `[ok, value]` (`[false, message]` if the task failed).
    #[stdlib_export(name = "await", params(task: Task), returns = List)]
    fn task_await(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let task = *args.get(0).expect("checked arity");
        if !matches!(heap_value(&task, runtime.heap(), "sched.await()")?, HeapValue::Task(_)) {
            bail!("sched.await() expects a Task argument");
        }
        Ok(descriptor(runtime.heap_mut(), TAG_AWAIT, [task]))
    }

    /// Drive coroutines (functions are wrapped on entry) until all of them —
    /// including any they `sched.spawn` — finish. Returns one `[ok, value]`
    /// per argument, in order; a coroutine erroring doesn't stop the rest.
    #[stdlib_export(name = "run", params(...entries: Any), returns = List, kind = "full_state")]
    fn run(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let async_runtime = runtime.async_runtime();
        let entries = args.as_slice().to_vec();
        let Some((state, mut ctx, module)) = runtime.state_ctx_module_mut() else {
            bail!("sched.run() requires full VM state");
        };
        let Some(module) = module else {
            bail!("sched.run() requires module execution");
        };

        let mut scheduler = Scheduler::new(async_runtime);
        for entry in &entries {
            scheduler.add_root(*entry, state)?;
        }
        scheduler.drive(state, module, &mut ctx)?;
        scheduler.into_root_results(entries.len(), state)
    }
}

/// How a queued coroutine gets resumed on its next turn.
enum Seed {
    /// First resume: these values seed the entry function's parameters.
    Start(Vec<RuntimeVal>),
    /// The paused `yield` expression evaluates to this value directly.
    Value(RuntimeVal),
    /// The paused `yield` evaluates to a freshly-built `[ok, value]` list.
    Pair(bool, RuntimeVal),
}

enum ParkedOp {
    Recv {
        channel_id: u64,
    },
    Send {
        channel_id: u64,
        payload: RuntimePayload,
    },
    Sleep {
        deadline: Instant,
    },
    /// Owns the tokio `JoinHandle` (taken out of the runtime via
    /// `Runtime::take_task`); awaited by `&mut` so a select round that
    /// resolves some *other* arm doesn't lose it (JoinHandle is Unpin and
    /// cancel-safe).
    Await {
        handle: JoinHandle<Result<RuntimePayload>>,
    },
}

struct Parked {
    managed: usize,
    op: ParkedOp,
}

/// What a blocking round resolved for one parked entry.
enum Wake {
    Recv(bool, RuntimePayload),
    Sent(bool),
    Awaited(Result<RuntimePayload, String>),
}

/// One selectable arm of a blocking round: resolves to the parked entry's
/// position plus what completed there.
type WakeArm<'a> = Pin<Box<dyn Future<Output = (usize, Wake)> + 'a>>;

struct Managed {
    handle: HeapRef,
    result: Option<(bool, RuntimeVal)>,
    /// Managed indices parked on this coroutine via `sched.join`.
    joiners: Vec<usize>,
}

struct Scheduler {
    rt: AsyncRuntimeHandle,
    managed: Vec<Managed>,
    by_handle: HashMap<HeapRef, usize>,
    queue: VecDeque<(usize, Seed)>,
    parked: Vec<Parked>,
}

impl Scheduler {
    fn new(rt: AsyncRuntimeHandle) -> Self {
        Self {
            rt,
            managed: Vec::new(),
            by_handle: HashMap::new(),
            queue: VecDeque::new(),
            parked: Vec::new(),
        }
    }

    fn add_root(&mut self, entry: RuntimeVal, state: &mut RuntimeModuleState) -> Result<()> {
        let coroutine = match heap_value(&entry, state.heap(), "sched.run()")? {
            HeapValue::Callable(_) => create_coroutine_runtime(entry, state.heap_mut())?,
            HeapValue::Coroutine(co) => {
                if matches!(co.status(), CoroutineStatus::Done | CoroutineStatus::Errored) {
                    bail!("sched.run() got a coroutine that is already dead");
                }
                entry
            }
            other => bail!(
                "sched.run() arguments must be functions or coroutines, got {}",
                other.type_name()
            ),
        };
        self.register(coroutine, Seed::Start(Vec::new()))?;
        Ok(())
    }

    fn register(&mut self, coroutine: RuntimeVal, seed: Seed) -> Result<usize> {
        let RuntimeVal::Obj(handle) = coroutine else {
            bail!("internal error: coroutine must be a heap value");
        };
        if self.by_handle.contains_key(&handle) {
            bail!("sched.run(): the same coroutine was scheduled twice");
        }
        let index = self.managed.len();
        self.managed.push(Managed {
            handle,
            result: None,
            joiners: Vec::new(),
        });
        self.by_handle.insert(handle, index);
        self.queue.push_back((index, seed));
        Ok(index)
    }

    fn drive(
        &mut self,
        state: &mut RuntimeModuleState,
        module: &Module,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<()> {
        loop {
            while let Some((index, seed)) = self.queue.pop_front() {
                self.step(index, seed, state, module, ctx)?;
            }
            if self.parked.is_empty() {
                let waiting: usize = self.managed.iter().map(|managed| managed.joiners.len()).sum();
                if waiting == 0 {
                    return Ok(());
                }
                // Joins can only ever be satisfied by coroutines this
                // scheduler drives — nothing external can un-stick them, so
                // (unlike channel/task waits) this is a provable deadlock.
                bail!("sched.run() deadlocked: {waiting} coroutine(s) waiting on sched.join with no runnable work");
            }
            self.block_until_ready(state)?;
        }
    }

    fn step(
        &mut self,
        index: usize,
        seed: Seed,
        state: &mut RuntimeModuleState,
        module: &Module,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<()> {
        let resume_args: Vec<RuntimeVal> = match seed {
            Seed::Start(args) => args,
            Seed::Value(value) => vec![value],
            Seed::Pair(ok, value) => vec![pair_list(state.heap_mut(), ok, value)],
        };
        // Everything the scheduler holds in Rust locals — coroutine handles,
        // stored results, queued resume values — must ride along as GC roots:
        // it is invisible to the safepoints hit while the coroutine runs.
        let mut roots = self.gc_roots();
        roots.extend_from_slice(&resume_args);
        let coroutine = RuntimeVal::Obj(self.managed[index].handle);
        let outcome =
            resume_coroutine_runtime(coroutine, &resume_args, &roots, state, Some(module), ctx.as_deref_mut())?;
        let (ok, value) = read_pair(state.heap(), outcome)?;
        if coroutine_status_runtime(coroutine, state.heap())? == "dead" {
            self.finish(index, ok, value);
            return Ok(());
        }
        self.interpret(index, value, state)
    }

    fn finish(&mut self, index: usize, ok: bool, value: RuntimeVal) {
        self.managed[index].result = Some((ok, value));
        let joiners = std::mem::take(&mut self.managed[index].joiners);
        for joiner in joiners {
            self.queue.push_back((joiner, Seed::Pair(ok, value)));
        }
    }

    fn interpret(&mut self, index: usize, yielded: RuntimeVal, state: &mut RuntimeModuleState) -> Result<()> {
        let Some(parts) = descriptor_parts(state.heap(), yielded) else {
            bail!(
                "sched.run(): a coroutine yielded a value that is not a scheduler operation; \
                 yield one built with sched.recv/send/sleep/pause/spawn/join/await"
            );
        };
        let RuntimeVal::ShortStr(tag) = parts[0] else {
            unreachable!("descriptor_parts checked the tag");
        };
        match tag.as_str() {
            TAG_RECV => {
                let id = channel_id(&parts[1], state.heap(), "sched.recv()")?;
                match self.rt.with(|rt| rt.try_recv(id)) {
                    Ok(Some((ok, payload))) => {
                        let value = payload.into_value(state.heap_mut())?;
                        self.queue.push_back((index, Seed::Pair(ok, value)));
                    }
                    Ok(None) => self.parked.push(Parked {
                        managed: index,
                        op: ParkedOp::Recv { channel_id: id },
                    }),
                    // Closed/removed channel: same `[false, nil]` a blocking
                    // recv reports when the channel closes under it.
                    Err(_) => self.queue.push_back((index, Seed::Pair(false, RuntimeVal::Nil))),
                }
            }
            TAG_SEND => {
                let id = channel_id(&parts[1], state.heap(), "sched.send()")?;
                let payload = RuntimePayload::copy_from_value(&parts[2], state.heap())?;
                match self.rt.with(|rt| rt.try_send(id, payload.clone())) {
                    Ok(true) => self.queue.push_back((index, Seed::Value(RuntimeVal::Bool(true)))),
                    Ok(false) => self.parked.push(Parked {
                        managed: index,
                        op: ParkedOp::Send {
                            channel_id: id,
                            payload,
                        },
                    }),
                    Err(_) => self.queue.push_back((index, Seed::Value(RuntimeVal::Bool(false)))),
                }
            }
            TAG_SLEEP => {
                let RuntimeVal::Int(ms) = parts[1] else {
                    bail!("sched.sleep(): malformed descriptor");
                };
                if ms <= 0 {
                    self.queue.push_back((index, Seed::Value(RuntimeVal::Nil)));
                } else {
                    self.parked.push(Parked {
                        managed: index,
                        op: ParkedOp::Sleep {
                            deadline: Instant::now() + Duration::from_millis(ms as u64),
                        },
                    });
                }
            }
            TAG_PAUSE => self.queue.push_back((index, Seed::Value(RuntimeVal::Nil))),
            TAG_SPAWN => {
                let coroutine = create_coroutine_runtime(parts[1], state.heap_mut())?;
                self.register(coroutine, Seed::Start(parts[2..].to_vec()))?;
                self.queue.push_back((index, Seed::Value(coroutine)));
            }
            TAG_JOIN => {
                let RuntimeVal::Obj(target) = parts[1] else {
                    bail!("sched.join(): malformed descriptor");
                };
                let Some(&target_index) = self.by_handle.get(&target) else {
                    bail!("sched.join(): that coroutine is not managed by this scheduler");
                };
                if target_index == index {
                    bail!("sched.join(): a coroutine cannot join itself");
                }
                match self.managed[target_index].result {
                    Some((ok, value)) => self.queue.push_back((index, Seed::Pair(ok, value))),
                    None => self.managed[target_index].joiners.push(index),
                }
            }
            TAG_AWAIT => {
                let task = task_value(&parts[1], state.heap(), "sched.await()")?;
                if let Some(payload) = &task.value {
                    let value = payload.clone_value_into(state.heap_mut())?;
                    self.queue.push_back((index, Seed::Pair(true, value)));
                    return Ok(());
                }
                let taken = self.rt.with(|rt| Ok(rt.take_task(task.id)))?;
                let Some(task) = taken else {
                    bail!("sched.await(): unknown task (already awaited or cancelled)");
                };
                self.parked.push(Parked {
                    managed: index,
                    op: ParkedOp::Await { handle: task.handle },
                });
            }
            _ => bail!(
                "sched.run(): a coroutine yielded a value that is not a scheduler operation; \
                 yield one built with sched.recv/send/sleep/pause/spawn/join/await"
            ),
        }
        Ok(())
    }

    /// Every coroutine is parked: wake expired sleepers, otherwise block on a
    /// tokio select over all channel/task arms, bounded by the earliest sleep
    /// deadline. Blocking here is correct, not a deadlock — a parked channel
    /// or task arm can be completed by an external tokio task at any time
    /// (same stance as Go: only join-cycles are provably stuck, and those are
    /// rejected in `drive`).
    fn block_until_ready(&mut self, state: &mut RuntimeModuleState) -> Result<()> {
        let now = Instant::now();
        if self.wake_expired_sleepers(now) {
            return Ok(());
        }
        let earliest = self
            .parked
            .iter()
            .filter_map(|parked| match parked.op {
                ParkedOp::Sleep { deadline } => Some(deadline),
                _ => None,
            })
            .min();

        let has_io_arms = self
            .parked
            .iter()
            .any(|parked| !matches!(parked.op, ParkedOp::Sleep { .. }));
        let rt = self.rt.clone();
        if !has_io_arms {
            let deadline = earliest.expect("non-empty parked list with no IO arms must hold a sleeper");
            rt.with(|rt| {
                // The timer must be created inside `block_on` — `tokio::time::
                // sleep` captures the runtime handle at construction time.
                rt.block_on(async { tokio::time::sleep(deadline.saturating_duration_since(now)).await });
                Ok(())
            })?;
            self.wake_expired_sleepers(Instant::now());
            return Ok(());
        }

        let parked = &mut self.parked;
        let winner: Option<(usize, Wake)> = rt.with(|rt| {
            rt.block_on(async {
                let mut arms: Vec<WakeArm<'_>> = Vec::new();
                for (position, entry) in parked.iter_mut().enumerate() {
                    match &mut entry.op {
                        ParkedOp::Recv { channel_id } => {
                            let id = *channel_id;
                            arms.push(Box::pin(async move {
                                match rt.recv_async(id).await {
                                    Ok((ok, payload)) => (position, Wake::Recv(ok, payload)),
                                    Err(_) => (position, Wake::Recv(false, RuntimePayload::nil())),
                                }
                            }));
                        }
                        ParkedOp::Send { channel_id, payload } => {
                            let id = *channel_id;
                            let payload = payload.clone();
                            arms.push(Box::pin(async move {
                                match rt.send_async(id, payload).await {
                                    Ok(sent) => (position, Wake::Sent(sent)),
                                    Err(_) => (position, Wake::Sent(false)),
                                }
                            }));
                        }
                        ParkedOp::Await { handle } => {
                            arms.push(Box::pin(async move {
                                let wake = match handle.await {
                                    Ok(Ok(payload)) => Wake::Awaited(Ok(payload)),
                                    Ok(Err(error)) => Wake::Awaited(Err(error.to_string())),
                                    Err(join_error) => Wake::Awaited(Err(join_error.to_string())),
                                };
                                (position, wake)
                            }));
                        }
                        ParkedOp::Sleep { .. } => {}
                    }
                }
                let select = futures::future::select_all(arms);
                let outcome = match earliest {
                    Some(deadline) => {
                        match tokio::time::timeout(deadline.saturating_duration_since(now), select).await {
                            Ok((first, _, _)) => Some(first),
                            Err(_elapsed) => None,
                        }
                    }
                    None => Some(select.await.0),
                };
                Ok(outcome)
            })
        })?;

        match winner {
            Some((position, wake)) => self.apply_wake(position, wake, state),
            None => {
                self.wake_expired_sleepers(Instant::now());
                Ok(())
            }
        }
    }

    fn apply_wake(&mut self, position: usize, wake: Wake, state: &mut RuntimeModuleState) -> Result<()> {
        let entry = self.parked.swap_remove(position);
        let index = entry.managed;
        match wake {
            Wake::Recv(ok, payload) => {
                let value = payload.into_value(state.heap_mut())?;
                self.queue.push_back((index, Seed::Pair(ok, value)));
            }
            Wake::Sent(sent) => self.queue.push_back((index, Seed::Value(RuntimeVal::Bool(sent)))),
            Wake::Awaited(Ok(payload)) => {
                let value = payload.into_value(state.heap_mut())?;
                self.queue.push_back((index, Seed::Pair(true, value)));
            }
            Wake::Awaited(Err(message)) => {
                let value = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::String(Arc::from(message.as_str()))));
                self.queue.push_back((index, Seed::Pair(false, value)));
            }
        }
        Ok(())
    }

    fn wake_expired_sleepers(&mut self, now: Instant) -> bool {
        let mut woke = false;
        let mut position = 0;
        while position < self.parked.len() {
            if matches!(&self.parked[position].op, ParkedOp::Sleep { deadline } if *deadline <= now) {
                let entry = self.parked.swap_remove(position);
                self.queue.push_back((entry.managed, Seed::Value(RuntimeVal::Nil)));
                woke = true;
            } else {
                position += 1;
            }
        }
        woke
    }

    /// The scheduler's whole shared-heap working set, for pinning across a
    /// resume. Parked entries hold no shared-heap values (channel ids are
    /// plain integers, send payloads live in private `RuntimePayload` heaps,
    /// task handles are tokio-side).
    fn gc_roots(&self) -> Vec<RuntimeVal> {
        let mut roots = Vec::with_capacity(self.managed.len() + self.queue.len());
        for managed in &self.managed {
            roots.push(RuntimeVal::Obj(managed.handle));
            if let Some((_, value)) = managed.result {
                roots.push(value);
            }
        }
        for (_, seed) in &self.queue {
            match seed {
                Seed::Start(args) => roots.extend_from_slice(args),
                Seed::Value(value) => roots.push(*value),
                Seed::Pair(_, value) => roots.push(*value),
            }
        }
        roots
    }

    fn into_root_results(self, root_count: usize, state: &mut RuntimeModuleState) -> Result<RuntimeVal> {
        let mut items = Vec::with_capacity(root_count);
        for managed in self.managed.iter().take(root_count) {
            let (ok, value) = managed
                .result
                .ok_or_else(|| anyhow!("internal error: a root coroutine finished without a result"))?;
            items.push(pair_list(state.heap_mut(), ok, value));
        }
        Ok(RuntimeVal::Obj(
            state.heap_mut().alloc(HeapValue::List(TypedList::Mixed(items))),
        ))
    }
}

fn descriptor(heap: &mut HeapStore, tag: &str, rest: impl IntoIterator<Item = RuntimeVal>) -> RuntimeVal {
    let mut items = vec![RuntimeVal::ShortStr(
        ShortStr::new(tag).expect("descriptor tags fit 7 bytes"),
    )];
    items.extend(rest);
    RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(items))))
}

/// A yielded value is a scheduler descriptor iff it is a Mixed list whose
/// first element is a `ShortStr` tag starting with `"s."`. Returns the items
/// (cloned out so the heap borrow ends) — arity per tag was fixed by the
/// builders, but re-checked by the callers where it matters.
fn descriptor_parts(heap: &HeapStore, value: RuntimeVal) -> Option<Vec<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = value else {
        return None;
    };
    let Some(HeapValue::List(TypedList::Mixed(items))) = heap.get(handle) else {
        return None;
    };
    let Some(RuntimeVal::ShortStr(tag)) = items.first() else {
        return None;
    };
    tag.as_str().starts_with("s.").then(|| items.clone())
}

fn pair_list(heap: &mut HeapStore, ok: bool, value: RuntimeVal) -> RuntimeVal {
    RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Bool(ok), value]))))
}

fn read_pair(heap: &HeapStore, value: RuntimeVal) -> Result<(bool, RuntimeVal)> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("internal error: coroutine_resume result must be a list");
    };
    let Some(HeapValue::List(TypedList::Mixed(items))) = heap.get(handle) else {
        bail!("internal error: coroutine_resume result must be a Mixed list");
    };
    match items.as_slice() {
        [RuntimeVal::Bool(ok), value] => Ok((*ok, *value)),
        _ => bail!("internal error: coroutine_resume result must be [ok, value]"),
    }
}

fn heap_value<'h>(value: &RuntimeVal, heap: &'h HeapStore, name: &str) -> Result<&'h HeapValue> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{name} expects a heap value argument, got {:?}", value.kind());
    };
    heap.get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))
}

fn channel_id(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<u64> {
    match heap_value(value, heap, name)? {
        HeapValue::Channel(channel) => Ok(channel.id),
        other => bail!("{name} expects a Channel argument, got {}", other.type_name()),
    }
}

fn task_value(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<Arc<TaskValue>> {
    match heap_value(value, heap, name)? {
        HeapValue::Task(task) => Ok(task.clone()),
        other => bail!("{name} expects a Task argument, got {}", other.type_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::{
        val::{CallableValue, ChannelValue, Type},
        vm::{Function, Instr, NativeFunction, Opcode},
    };

    fn sched_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&SchedModule::new(), name)
    }

    fn call_plain(name: &str, args: &[RuntimeVal], state: &mut RuntimeModuleState) -> Result<RuntimeVal> {
        let (_, function) = sched_native(name)?;
        let NativeFunction::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, None, None);
        function(NativeArgs::new(args), &mut runtime)
    }

    fn expect_descriptor(state: &RuntimeModuleState, value: RuntimeVal, tag: &str, len: usize) {
        let parts = descriptor_parts(state.heap(), value).expect("expected a descriptor list");
        assert_eq!(parts.len(), len);
        let RuntimeVal::ShortStr(actual) = parts[0] else {
            panic!("expected a ShortStr tag");
        };
        assert_eq!(actual.as_str(), tag);
    }

    #[test]
    fn descriptor_tags_fit_inline_short_strings() {
        for tag in [TAG_RECV, TAG_SEND, TAG_SLEEP, TAG_PAUSE, TAG_SPAWN, TAG_JOIN, TAG_AWAIT] {
            assert!(ShortStr::new(tag).is_some(), "{tag} must fit a ShortStr");
        }
    }

    #[test]
    fn sched_exports_have_expected_kinds() -> Result<()> {
        for name in ["recv", "send", "sleep", "pause", "spawn", "join", "await"] {
            let (_, function) = sched_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)), "{name} should be Plain");
        }
        let (arity, function) = sched_native("run")?;
        assert!(matches!(function, NativeFunction::FullState(_)));
        assert_eq!(arity, lk_core::vm::NativeEntry::VARIADIC);
        Ok(())
    }

    #[test]
    fn sleep_and_pause_build_descriptors() -> Result<()> {
        let mut state = RuntimeModuleState::default();
        let sleep = call_plain("sleep", &[RuntimeVal::Int(5)], &mut state)?;
        expect_descriptor(&state, sleep, TAG_SLEEP, 2);
        let pause = call_plain("pause", &[], &mut state)?;
        expect_descriptor(&state, pause, TAG_PAUSE, 1);
        Ok(())
    }

    #[test]
    fn sleep_rejects_negative_durations() {
        let mut state = RuntimeModuleState::default();
        let err = call_plain("sleep", &[RuntimeVal::Int(-1)], &mut state).expect_err("negative must fail");
        assert!(err.to_string().contains("non-negative"));
    }

    #[test]
    fn recv_send_join_await_validate_their_argument_types() {
        let mut state = RuntimeModuleState::default();
        for (name, args) in [
            ("recv", vec![RuntimeVal::Int(1)]),
            ("send", vec![RuntimeVal::Int(1), RuntimeVal::Nil]),
            ("spawn", vec![RuntimeVal::Int(1)]),
            ("join", vec![RuntimeVal::Int(1)]),
            ("await", vec![RuntimeVal::Int(1)]),
        ] {
            let err = call_plain(name, &args, &mut state).expect_err("type mismatch must fail");
            assert!(err.to_string().contains("expects"), "{name}: {err}");
        }
    }

    #[test]
    fn user_lists_do_not_parse_as_descriptors() {
        let mut state = RuntimeModuleState::default();
        let plain = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::List(TypedList::Mixed(vec![
            RuntimeVal::ShortStr(ShortStr::new("recv").expect("fits")),
            RuntimeVal::Int(1),
        ]))));
        assert!(descriptor_parts(state.heap(), plain).is_none());
        assert!(descriptor_parts(state.heap(), RuntimeVal::Int(3)).is_none());
    }

    // --- driver tests over hand-built bytecode ------------------------------
    //
    // These pin the park/wake internals that pure-LK tests can't reach today:
    // `sched.await` (tasks are only produced by natives — the global `spawn`
    // doesn't accept plain closures yet), the blocking-select wakeup, and the
    // join-cycle deadlock report. The coroutine bodies just yield a descriptor
    // read from a global slot, so the descriptors themselves are built here.

    /// `GetGlobal r0, slot` → `Yield r0` → `Return r0`: yields the descriptor
    /// parked in `state.globals[slot]`, then returns whatever the scheduler
    /// resumed it with. `pad` shifts the pcs so two functions in one module
    /// don't collide in the pc-keyed global inline cache (real compiled code
    /// carries per-function slot facts instead).
    fn yield_global_function(slot: u16, pad: usize) -> Function {
        let mut code = vec![Instr::abc(Opcode::LoadNil, 0, 0, 0); pad];
        code.push(Instr::abx(Opcode::GetGlobal, 0, slot));
        code.push(Instr::abc(Opcode::Yield, 0, 0, 0));
        code.push(Instr::abc(Opcode::Return, 0, 1, 0));
        Function {
            code,
            register_count: 1,
            param_count: 0,
            positional_param_count: 0,
            param_names: Vec::new(),
            capture_count: 0,
            ..Function::default()
        }
    }

    fn coroutine_for(state: &mut RuntimeModuleState, function_index: u32) -> Result<RuntimeVal> {
        let closure = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures: std::sync::Arc::new(Vec::new()),
        })));
        create_coroutine_runtime(closure, state.heap_mut())
    }

    fn call_run(
        args: &[RuntimeVal],
        state: &mut RuntimeModuleState,
        ctx: &mut VmContext,
        module: &Module,
    ) -> Result<RuntimeVal> {
        let (_, function) = sched_native("run")?;
        let NativeFunction::FullState(function) = function else {
            bail!("run must use full-state RuntimeNative");
        };
        let mut runtime = NativeRuntime::new(state, Some(ctx), Some(module));
        function(NativeArgs::new(args), &mut runtime)
    }

    fn nth_pair(state: &RuntimeModuleState, results: RuntimeVal, index: usize) -> (bool, RuntimeVal) {
        let RuntimeVal::Obj(handle) = results else {
            panic!("expected a results list");
        };
        let Some(HeapValue::List(TypedList::Mixed(items))) = state.heap().get(handle) else {
            panic!("expected a Mixed results list");
        };
        read_pair(state.heap(), items[index]).expect("result entry must be [ok, value]")
    }

    #[test]
    fn await_parks_until_the_task_completes() -> Result<()> {
        let module = Module {
            functions: vec![yield_global_function(0, 0)],
            natives: Vec::new(),
            globals: Vec::new(),
            entry: 0,
        };
        let mut state = RuntimeModuleState::default();
        let mut ctx = VmContext::new_without_core_vm_builtins();
        // Slow enough that the scheduler must park on the JoinHandle and go
        // through the blocking select instead of the fast path.
        let task_id = ctx.async_runtime().with(|rt| {
            rt.spawn(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(RuntimePayload::new(RuntimeVal::Int(7), HeapStore::new()))
            })
        })?;
        let task = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Task(Arc::new(TaskValue {
            id: task_id,
            value: None,
        }))));
        let waiter = descriptor(state.heap_mut(), TAG_AWAIT, [task]);
        state.globals_mut().push(waiter);
        let co = coroutine_for(&mut state, 0)?;

        let results = call_run(&[co], &mut state, &mut ctx, &module)?;
        let (ok, value) = nth_pair(&state, results, 0);
        assert!(ok);
        let (task_ok, task_value) = read_pair(state.heap(), value)?;
        assert!(task_ok);
        assert_eq!(task_value, RuntimeVal::Int(7));
        Ok(())
    }

    #[test]
    fn parked_recv_wakes_when_an_external_task_sends() -> Result<()> {
        let module = Module {
            functions: vec![yield_global_function(0, 0)],
            natives: Vec::new(),
            globals: Vec::new(),
            entry: 0,
        };
        let mut state = RuntimeModuleState::default();
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let channel_id = ctx.async_runtime().with(|rt| rt.create_channel(Some(1)))?;
        let channel = RuntimeVal::Obj(state.heap_mut().alloc(HeapValue::Channel(Arc::new(ChannelValue {
            id: channel_id,
            capacity: Some(1),
            inner_type: Type::Nil,
        }))));
        let waiter = descriptor(state.heap_mut(), TAG_RECV, [channel]);
        state.globals_mut().push(waiter);
        let co = coroutine_for(&mut state, 0)?;

        // An external tokio task delivers while the scheduler blocks — the
        // "channels bridge coroutines and real parallelism" integration.
        let sender = ctx.async_runtime().clone();
        ctx.async_runtime().with(|rt| {
            rt.spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                sender.with(|rt| rt.try_send(channel_id, RuntimePayload::new(RuntimeVal::Int(9), HeapStore::new())))?;
                Ok(RuntimePayload::nil())
            })
        })?;

        let results = call_run(&[co], &mut state, &mut ctx, &module)?;
        let (ok, value) = nth_pair(&state, results, 0);
        assert!(ok);
        let (recv_ok, received) = read_pair(state.heap(), value)?;
        assert!(recv_ok);
        assert_eq!(received, RuntimeVal::Int(9));
        Ok(())
    }

    #[test]
    fn join_cycle_between_two_coroutines_is_a_deadlock_error() -> Result<()> {
        let module = Module {
            functions: vec![yield_global_function(0, 0), yield_global_function(1, 1)],
            natives: Vec::new(),
            globals: Vec::new(),
            entry: 0,
        };
        let mut state = RuntimeModuleState::default();
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let co_a = coroutine_for(&mut state, 0)?;
        let co_b = coroutine_for(&mut state, 1)?;
        let join_b = descriptor(state.heap_mut(), TAG_JOIN, [co_b]);
        let join_a = descriptor(state.heap_mut(), TAG_JOIN, [co_a]);
        state.globals_mut().push(join_b); // slot 0: A waits on B
        state.globals_mut().push(join_a); // slot 1: B waits on A

        let err = call_run(&[co_a, co_b], &mut state, &mut ctx, &module).expect_err("join cycle must be reported");
        assert!(err.to_string().contains("deadlock"), "unexpected error: {err}");
        Ok(())
    }
}
