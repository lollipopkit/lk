//! Native channels + goroutine threads (deep-coverage plan H, user
//! adjudication: "OS threads + deep-copy channels, no tokio in lkrt").
//!
//! The VM's concurrency model is isolate semantics: a value crossing a
//! channel (or a spawn boundary) is a structural deep copy, never a shared
//! heap reference. Natively: every goroutine is an OS thread with its own
//! thread-local arena; a sent value deep-copies out of the sender's arena
//! into an owned tree ([`OwnedVal`]), and materializes into the receiver's
//! arena on delivery. Channels live in a *process-global* registry (plain
//! `Mutex`/`Condvar` — deliberately separate from the single-threaded arena).
//!
//! Blocking semantics mirror the VM's v2 model: `send` on a closed channel
//! raises; `recv` drains the buffer after close, then raises; a bounded
//! `send` blocks while full.

use alloc::ffi::CString;
use core::ffi::{CStr, c_char, c_void};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

use crate::lkdyn::{DYN_BOOL, DYN_F64, DYN_I64, DYN_LIST, DYN_MAP, DYN_NIL, DYN_STR, LkDyn, is_list_tag};
use crate::lkmap::StrDynMap;
use crate::lkstr::arena_c_string;
use crate::state::arena_handle;

/// A value that crossed an isolate boundary: fully owned, `Send`. Maps keep
/// their iteration order (entries captured in order, replayed on rebuild —
/// same keys + same insertion order = the same Fx layout on the other side).
#[derive(Clone)]
pub(crate) enum OwnedVal {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<OwnedVal>),
    Map(Vec<(String, OwnedVal)>),
    /// A closure: its code address, its visible arity, its module function
    /// index (for `display`), and its own captures owned the same way. The
    /// address is shared rather than copied — it is code.
    Closure(usize, i64, i64, Vec<OwnedVal>),
}

pub(crate) fn own(v: LkDyn) -> OwnedVal {
    match v.tag {
        DYN_NIL => OwnedVal::Nil,
        DYN_BOOL => OwnedVal::Bool(v.payload != 0),
        DYN_I64 => OwnedVal::Int(v.payload),
        DYN_F64 => OwnedVal::Float(f64::from_bits(v.payload as u64)),
        DYN_STR => {
            let ptr = v.payload as *const c_char;
            let text = if ptr.is_null() {
                String::new()
            } else {
                // SAFETY: DYN_STR payloads are NUL-terminated arena strings.
                unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
            };
            OwnedVal::Str(text)
        }
        // A channel copies by value, so every list representation deep-copies
        // the same way — the typed carriers box in place now and would
        // otherwise fall through to the unsupported arm.
        tag if is_list_tag(tag) => {
            OwnedVal::List(crate::lkdyn::dyn_list_values(v).iter().map(|&item| own(item)).collect())
        }
        DYN_MAP => {
            let handle = v.payload as *mut c_void;
            if handle.is_null() {
                return OwnedVal::Map(Vec::new());
            }
            // SAFETY: DYN_MAP payloads are live `StrDynMap` handles.
            let map = unsafe { &*(handle as *mut StrDynMap) };
            OwnedVal::Map(map.iter().map(|(k, &val)| (k.clone(), own(val))).collect())
        }
        // Channels/tasks/functions do not cross as *values* in the native
        // subset (channels travel as their i64 ids).
        // A closure copies its captures the same way and shares its code.
        crate::lkdyn::DYN_CLOSURE => crate::lkclosure::own_closure(v),
        _ => crate::panic::raise_str("value cannot cross a channel"),
    }
}

pub(crate) fn materialize(v: &OwnedVal) -> LkDyn {
    match v {
        OwnedVal::Nil => LkDyn::NIL,
        OwnedVal::Bool(b) => LkDyn {
            tag: DYN_BOOL,
            payload: i64::from(*b),
        },
        OwnedVal::Int(n) => LkDyn {
            tag: DYN_I64,
            payload: *n,
        },
        OwnedVal::Float(x) => LkDyn {
            tag: DYN_F64,
            payload: x.to_bits() as i64,
        },
        OwnedVal::Str(s) => {
            let ptr = arena_c_string(CString::new(s.as_str()).unwrap_or_default());
            LkDyn {
                tag: DYN_STR,
                payload: ptr as i64,
            }
        }
        OwnedVal::List(items) => {
            let list: Vec<LkDyn> = items.iter().map(materialize).collect();
            LkDyn {
                tag: DYN_LIST,
                payload: arena_handle(list) as i64,
            }
        }
        OwnedVal::Map(entries) => {
            let mut map = StrDynMap::default();
            for (k, val) in entries {
                map.insert(k.clone(), materialize(val));
            }
            LkDyn {
                tag: DYN_MAP,
                payload: arena_handle(map) as i64,
            }
        }
        OwnedVal::Closure(code, params, fn_index, env) => {
            crate::lkclosure::materialize_closure(*code, *params, *fn_index, env)
        }
    }
}

struct ChanState {
    queue: VecDeque<OwnedVal>,
    closed: bool,
    /// How many threads are inside `recv_cv.wait` / `send_cv.wait` on this
    /// channel.
    ///
    /// Kept in the state rather than in an atomic because it is only ever read
    /// and written under `state`, which makes it exact for free: a waiter
    /// increments it before `wait` releases the lock, so a notifier holding the
    /// lock and seeing zero knows nobody is waiting *and* nobody can start
    /// without going through it.
    ///
    /// The point is the syscall. `Condvar::notify_one` on Linux issues a
    /// `futex_wake` whether or not anything is parked, and a send/receive loop
    /// spent a third of its time in that syscall waking nobody.
    recv_waiters: usize,
    send_waiters: usize,
}

struct ChanInner {
    state: Mutex<ChanState>,
    /// Signals receivers (value available / closed).
    recv_cv: Condvar,
    /// Signals bounded senders (space available / closed).
    send_cv: Condvar,
    cap: Option<usize>,
    /// What the program asked for, which is not `cap`: `chan.new(0)` is
    /// unbuffered and reports `0` while the queue's bound is 1. The VM keeps
    /// the same two numbers apart (`ChannelValue::capacity`).
    requested: i64,
}

/// Every channel ever created, indexed by `id - 1`.
///
/// A `Vec` rather than a map because ids come from one `fetch_add` and nothing
/// is ever removed — a channel outlives the program, which is the same arena
/// model the rest of lkrt uses. Lookup is then a bounds check and an `Arc`
/// clone, and it takes a *read* lock, so two threads on two channels do not
/// serialize on each other.
///
/// It was a `Mutex<HashMap<i64, _>>` with the default hasher. Every send and
/// every receive resolves its channel through here, so each one paid a
/// process-global mutex plus a SipHash of an `i64` — 28% of a send/receive loop
/// between them, to look up a dense integer.
///
/// `Option` because ids are handed out before the insert takes the lock, so two
/// threads creating channels can arrive out of order and leave a hole for the
/// slower one to fill.
fn registry() -> &'static RwLock<Vec<Option<Arc<ChanInner>>>> {
    static REGISTRY: OnceLock<RwLock<Vec<Option<Arc<ChanInner>>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Process-global select wake-up: a generation counter bumped (and
/// broadcast) by every state-changing channel operation. A `select` with no
/// ready arm blocks here instead of spin-polling; the timeout is only a
/// missed-wakeup safety net, not the discovery mechanism. Ordering
/// discipline: notifiers take this lock *after* dropping their channel's
/// `state` guard (never nested), and no raise can fire while it is held.
struct SelectGen {
    lock: Mutex<u64>,
    cv: Condvar,
}

/// How many `select`s are parked on [`SelectGen`] right now.
///
/// Read by every send and receive so that a program with no `select` in it pays
/// one relaxed load instead of a process-global mutex and a `notify_all`. That
/// was not a rounding error: on a send/receive loop with no `select` anywhere,
/// `notify_selects` and the futex calls its broadcast made were **58% of the
/// program** — a global serialization point on the hot path of a feature the
/// program did not use.
static BLOCKED_SELECTS: AtomicUsize = AtomicUsize::new(0);

/// A `select` counted in [`BLOCKED_SELECTS`] for as long as this is alive.
struct SelectParked;

impl SelectParked {
    fn enter() -> Self {
        BLOCKED_SELECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for SelectParked {
    fn drop(&mut self) {
        BLOCKED_SELECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn select_gen() -> &'static SelectGen {
    static INSTANCE: OnceLock<SelectGen> = OnceLock::new();
    INSTANCE.get_or_init(|| SelectGen {
        lock: Mutex::new(0),
        cv: Condvar::new(),
    })
}

/// Signals every blocked `select` that some channel changed state.
///
/// Skipping the broadcast when nothing is parked is safe, and the ordering is
/// what makes it so. A notifier reaches here having *already* released its
/// channel's `state` guard, so its change is published; a `select` increments
/// [`BLOCKED_SELECTS`] *before* reading the generation and polling. So if the
/// load below sees zero, the increment that would have made it one comes later
/// in the `SeqCst` total order — and the poll that follows that increment takes
/// the channel lock, and therefore sees the change this notifier just made.
/// Either the notifier wakes the select, or the select was never going to sleep.
fn notify_selects() {
    if BLOCKED_SELECTS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let wake = select_gen();
    {
        let mut generation = wake.lock.lock().expect("select generation poisoned");
        *generation = generation.wrapping_add(1);
    }
    wake.cv.notify_all();
}

fn channel(id: i64) -> Arc<ChanInner> {
    // The registry guard must be dropped before the unknown-id raise: the
    // raise longjmps to the nearest handler, skipping Rust drops — a live
    // guard would leave the *global* registry locked forever, deadlocking
    // every later channel operation once the raise is caught.
    let found = usize::try_from(id).ok().and_then(|id| {
        registry()
            .read()
            .expect("channel registry poisoned")
            .get(id.checked_sub(1)?)
            .cloned()
            .flatten()
    });
    match found {
        Some(inner) => inner,
        None => crate::panic::raise_str("Channel not found"),
    }
}

static NEXT_CHANNEL_ID: AtomicI64 = AtomicI64::new(1);

/// `chan(capacity)`. The channel travels as its `i64` id.
///
/// `0` is **unbuffered**, not unbounded — the same ruling the VM's
/// `create_channel_value` records, and this had been left behind on the older
/// one (`capacity <= 0` meant unbounded here). It was observable: `chan.new(0)`
/// then two `try_send`s answered `true, false` on the VM and `true, true`
/// natively. As there, unbuffered takes the smallest bound the queue offers.
///
/// A negative capacity raises, likewise matching the VM instead of silently
/// handing back an unbounded channel.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_new(capacity: i64) -> i64 {
    if capacity < 0 {
        crate::panic::raise_str(&format!("chan() capacity cannot be negative, got {capacity}"));
    }
    let id = NEXT_CHANNEL_ID.fetch_add(1, Ordering::Relaxed);
    let inner = Arc::new(ChanInner {
        state: Mutex::new(ChanState {
            queue: VecDeque::new(),
            closed: false,
            recv_waiters: 0,
            send_waiters: 0,
        }),
        recv_cv: Condvar::new(),
        send_cv: Condvar::new(),
        cap: Some((capacity as usize).max(1)),
        requested: capacity,
    });
    {
        let mut table = registry().write().expect("channel registry poisoned");
        let slot = id as usize - 1;
        if table.len() <= slot {
            table.resize(slot + 1, None);
        }
        table[slot] = Some(inner);
    }
    id
}

/// Blocking `send(c, v)`: deep-copies the value out of this thread's arena,
/// waits for space on a bounded channel, raises once closed (Go's
/// panic-on-closed-send, the VM's catchable raise).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_send(id: i64, value: LkDyn) {
    let owned = own(value);
    let inner = channel(id);
    let mut state = inner.state.lock().expect("channel poisoned");
    loop {
        if state.closed {
            drop(state);
            crate::panic::raise_str("send on closed channel");
        }
        if inner.cap.is_none_or(|cap| state.queue.len() < cap) {
            state.queue.push_back(owned);
            let wake = state.recv_waiters > 0;
            drop(state);
            if wake {
                inner.recv_cv.notify_one();
            }
            notify_selects();
            return;
        }
        state.send_waiters += 1;
        state = inner.send_cv.wait(state).expect("channel poisoned");
        state.send_waiters -= 1;
    }
}

/// Blocking `recv(c)`: drains buffered values after close, then raises
/// (the VM's catchable "receive on closed channel").
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_recv(id: i64) -> LkDyn {
    let inner = channel(id);
    let mut state = inner.state.lock().expect("channel poisoned");
    loop {
        if let Some(value) = state.queue.pop_front() {
            let wake = state.send_waiters > 0;
            drop(state);
            if wake {
                inner.send_cv.notify_one();
            }
            notify_selects();
            return materialize(&value);
        }
        if state.closed {
            drop(state);
            crate::panic::raise_str("receive on closed channel");
        }
        state.recv_waiters += 1;
        state = inner.recv_cv.wait(state).expect("channel poisoned");
        state.recv_waiters -= 1;
    }
}

/// `chan.close(c)`: marks closed; buffered values stay drainable (Go
/// semantics), blocked parties wake.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_close(id: i64) {
    let inner = channel(id);
    let (wake_recv, wake_send) = {
        let mut state = inner.state.lock().expect("channel poisoned");
        state.closed = true;
        (state.recv_waiters > 0, state.send_waiters > 0)
    };
    if wake_recv {
        inner.recv_cv.notify_all();
    }
    if wake_send {
        inner.send_cv.notify_all();
    }
    notify_selects();
}

/// `time.timeout(ms)` / `time.after(ms)` — a capacity-1 channel that receives
/// one value once the duration is up.
///
/// The stdlib module builds this out of a tokio timer plus its async runtime's
/// channel; here it is a thread that sleeps and then sends, because lkrt's
/// channels are already thread-backed. What matters is the observable part, and
/// it is the same on both: capacity 1, exactly one value, and the value itself
/// — `timeout` sends nil, `after` sends the epoch milliseconds **read when the
/// timer fires**, not when it was armed.
///
/// The send is a `try_send` whose result is dropped, matching the module: if
/// nobody ever receives, the timer must not keep a thread parked forever, and a
/// closed channel is not the timer's error to report.
fn spawn_timer(duration_ms: i64, after: bool) -> i64 {
    let id = lkrt_chan_new(1);
    let delay = core::time::Duration::from_millis(duration_ms.max(0) as u64);
    register_task(std::thread::spawn(move || {
        std::thread::sleep(delay);
        let value = if after {
            crate::lkdyn::lkrt_dyn_from_i64(crate::host::lkrt_time_now_ms())
        } else {
            crate::lkdyn::lkrt_dyn_from_nil()
        };
        let inner = channel(id);
        let mut state = inner.state.lock().expect("channel poisoned");
        // Not `lkrt_chan_try_send`: that raises on a closed channel, and a
        // raise `longjmp`s — out of a spawned thread, past this lock guard,
        // with nobody to catch it. A closed channel means the receiver is gone,
        // which is the timer's cue to do nothing.
        if !state.closed && inner.cap.is_none_or(|cap| state.queue.len() < cap) {
            state.queue.push_back(own(value));
            if state.recv_waiters > 0 {
                inner.recv_cv.notify_all();
            }
        }
        drop(state);
        own(crate::lkdyn::lkrt_dyn_from_nil())
    }));
    id
}

/// `time.timeout(ms)` — fires with nil.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_time_timeout(duration_ms: i64) -> i64 {
    spawn_timer(duration_ms, false)
}

/// `time.after(ms)` — fires with the epoch milliseconds at that moment.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_time_after(duration_ms: i64) -> i64 {
    spawn_timer(duration_ms, true)
}

/// Non-blocking send: 1 delivered, 0 full; closed raises.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_try_send(id: i64, value: LkDyn) -> i64 {
    let owned = own(value);
    let inner = channel(id);
    let mut state = inner.state.lock().expect("channel poisoned");
    if state.closed {
        drop(state);
        crate::panic::raise_str("send on closed channel");
    }
    if inner.cap.is_none_or(|cap| state.queue.len() < cap) {
        state.queue.push_back(owned);
        let wake = state.recv_waiters > 0;
        drop(state);
        if wake {
            inner.recv_cv.notify_one();
        }
        notify_selects();
        1
    } else {
        0
    }
}

/// Non-blocking receive: the value, or nil when empty; closed-and-drained
/// raises (the VM's rule).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_try_recv(id: i64) -> LkDyn {
    let inner = channel(id);
    let mut state = inner.state.lock().expect("channel poisoned");
    if let Some(value) = state.queue.pop_front() {
        let wake = state.send_waiters > 0;
        drop(state);
        if wake {
            inner.send_cv.notify_one();
        }
        notify_selects();
        return materialize(&value);
    }
    if state.closed {
        drop(state);
        crate::panic::raise_str("receive on closed channel");
    }
    LkDyn::NIL
}

/// `chan.len(c)` — buffered value count.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_len(id: i64) -> i64 {
    channel(id).state.lock().expect("channel poisoned").queue.len() as i64
}

/// `chan.capacity(c)` — the capacity as asked for, so `chan.new(0)` reports 0.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_capacity(id: i64) -> i64 {
    channel(id).requested
}

/// `chan.is_closed(c)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_is_closed(id: i64) -> i64 {
    i64::from(channel(id).state.lock().expect("channel poisoned").closed)
}

/// `select { … }` (the desugared `select$block`): four parallel dyn lists
/// (types 0=recv/1=send, channel ids, send values, guards) plus the default
/// flag. Enabled arms poll in order; with none ready and no default, a
/// short parked spin retries (timing is not part of the observable
/// contract). Returns the VM's exact result shape:
/// `[is_default, index, payload]` — payload is `[ok, value]` for a recv arm
/// (`ok = false` + nil once closed-and-drained, Go's zero-value model), nil
/// for a send arm; a closed-channel *send* arm raises.
///
/// # Safety
/// The four list handles must be live dyn-list handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_chan_select(
    types: *mut c_void,
    channels: *mut c_void,
    values: *mut c_void,
    guards: *mut c_void,
    has_default: i64,
) -> *mut c_void {
    fn dyn_items<'a>(handle: *mut c_void) -> &'a [LkDyn] {
        if handle.is_null() {
            &[]
        } else {
            // SAFETY: callers pass live dyn-list handles.
            unsafe { &*(handle as *mut Vec<LkDyn>) }
        }
    }
    let types = dyn_items(types);
    let channels = dyn_items(channels);
    let values = dyn_items(values);
    let guards = dyn_items(guards);
    let len = types.len();
    if channels.len() != len || values.len() != len || guards.len() != len {
        crate::panic::raise_str("select$block: all lists must have equal length");
    }
    let result = |is_default: bool, index: i64, payload: LkDyn| -> *mut c_void {
        let list = vec![
            LkDyn {
                tag: DYN_BOOL,
                payload: i64::from(is_default),
            },
            LkDyn {
                tag: DYN_I64,
                payload: index,
            },
            payload,
        ];
        arena_handle(list)
    };
    // Everything that can raise, and everything that does not change between
    // polls, happens here — before any channel lock is taken and before this
    // call registers itself as a parked select.
    //
    // Raising is the original reason: `own` and the shape guards raise, and a
    // longjmp past a live `MutexGuard` would leave that channel locked forever
    // (the blocking send/recv paths follow the same drop-before-raise
    // discipline). A send arm's copy is taken exactly once, up front; the retry
    // loop consumes it on delivery.
    //
    // Resolving the channels here as well does two more things. It keeps the
    // *global registry* mutex out of the poll loop, which used to take it once
    // per armed arm per round. And it leaves the loop below with only one raise
    // in it, which matters because a raise skips the parked-select bookkeeping
    // (see `SelectParked`) — one site is a thing that can be got right by
    // reading, a site per arm is not.
    //
    // A *disarmed* arm is not resolved and not shape-checked, because it was
    // not before: `select { c1 <- v if false, … }` naming a channel that does
    // not exist is a program the VM runs.
    let mut arms: Vec<Option<(i64, Arc<ChanInner>)>> = Vec::with_capacity(len);
    let mut owned_sends: Vec<Option<OwnedVal>> = Vec::with_capacity(len);
    for index in 0..len {
        let kind = match types[index].tag {
            DYN_I64 if matches!(types[index].payload, 0 | 1) => types[index].payload,
            _ => crate::panic::raise_str("select$block: invalid arm entry types"),
        };
        // Guard must be exactly `true` (the VM normalizes to Bool).
        let armed = guards[index].tag == DYN_BOOL && guards[index].payload != 0;
        owned_sends.push((kind == 1 && armed).then(|| own(values[index])));
        arms.push(armed.then(|| {
            let id = match channels[index].tag {
                DYN_I64 => channels[index].payload,
                _ => crate::panic::raise_str("select$block: invalid channel arm"),
            };
            (kind, channel(id))
        }));
    }
    let any_armed = arms.iter().any(Option::is_some);
    loop {
        // Registered *before* the poll, which is what lets a notifier skip its
        // broadcast when this counter reads zero — see `notify_selects` for the
        // ordering argument. Dropped on every way out of this loop body,
        // including the `return`s inside the poll.
        let parked = SelectParked::enter();
        // Read the wake-up generation *before* polling: a channel op that
        // lands mid-poll bumps it, so the wait below returns immediately
        // instead of missing the change.
        let round_gen = *select_gen().lock.lock().expect("select generation poisoned");
        for index in 0..len {
            let Some((kind, inner)) = &arms[index] else {
                continue;
            };
            let (kind, inner) = (*kind, inner.clone());
            let mut state = inner.state.lock().expect("channel poisoned");
            match kind {
                0 => {
                    if let Some(value) = state.queue.pop_front() {
                        let wake = state.send_waiters > 0;
                        drop(state);
                        if wake {
                            inner.send_cv.notify_one();
                        }
                        notify_selects();
                        let payload = arena_handle(vec![
                            LkDyn {
                                tag: DYN_BOOL,
                                payload: 1,
                            },
                            materialize(&value),
                        ]);
                        return result(
                            false,
                            index as i64,
                            LkDyn {
                                tag: DYN_LIST,
                                payload: payload as i64,
                            },
                        );
                    }
                    if state.closed {
                        // Closed recv arm: always-ready with a nil binding.
                        drop(state);
                        let payload = arena_handle(vec![
                            LkDyn {
                                tag: DYN_BOOL,
                                payload: 0,
                            },
                            LkDyn::NIL,
                        ]);
                        return result(
                            false,
                            index as i64,
                            LkDyn {
                                tag: DYN_LIST,
                                payload: payload as i64,
                            },
                        );
                    }
                }
                1 => {
                    if state.closed {
                        drop(state);
                        // The one raise left inside the poll. A raise longjmps
                        // past Rust drops, so the registration has to come off
                        // by hand — otherwise this select stays counted as
                        // parked forever and every channel operation in the
                        // process goes back to broadcasting.
                        drop(parked);
                        crate::panic::raise_str("send on closed channel");
                    }
                    if inner.cap.is_none_or(|cap| state.queue.len() < cap) {
                        let owned = owned_sends[index].take().expect("armed send payload pre-owned");
                        state.queue.push_back(owned);
                        let wake = state.recv_waiters > 0;
                        drop(state);
                        if wake {
                            inner.recv_cv.notify_one();
                        }
                        notify_selects();
                        return result(false, index as i64, LkDyn::NIL);
                    }
                }
                // Kinds are pre-validated above; reaching this is an
                // internal invariant break, not a user error.
                _ => unreachable!("select$block arm kinds pre-validated"),
            }
        }
        if has_default != 0 {
            return result(true, -1, LkDyn::NIL);
        }
        if !any_armed {
            // Every arm disabled and no default: the VM yields nil-ish;
            // mirror its documented "all guards off → nil" rule by
            // reporting the default shape.
            return result(true, -1, LkDyn::NIL);
        }
        // Block until some channel op bumps the generation (or the safety-
        // net timeout fires — never the discovery mechanism, only insurance
        // against a lost wakeup).
        let wake = select_gen();
        let mut generation = wake.lock.lock().expect("select generation poisoned");
        while *generation == round_gen {
            let (next, timeout) = wake
                .cv
                .wait_timeout(generation, std::time::Duration::from_millis(1))
                .expect("select generation poisoned");
            generation = next;
            if timeout.timed_out() {
                break;
            }
        }
    }
}

// ── Goroutine threads + task registry (H2) ─────────────────────────────

struct TaskSlot {
    handle: Option<std::thread::JoinHandle<OwnedVal>>,
}

fn tasks() -> &'static Mutex<HashMap<i64, TaskSlot>> {
    static TASKS: OnceLock<Mutex<HashMap<i64, TaskSlot>>> = OnceLock::new();
    TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_TASK_ID: AtomicI64 = AtomicI64::new(1);

/// Builds the argument block for a spawn: boxed values deep-copy at once
/// (isolate semantics — the goroutine sees a snapshot).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_spawn_args_new() -> *mut c_void {
    Box::into_raw(Box::new(Vec::<OwnedVal>::new())) as *mut c_void
}

/// Appends one captured value to the argument block.
///
/// # Safety
/// `block` must be a live handle from [`lkrt_spawn_args_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_spawn_args_push(block: *mut c_void, value: LkDyn) {
    // SAFETY: `block` is the Vec allocated by `lkrt_spawn_args_new`.
    let args = unsafe { &mut *(block as *mut Vec<OwnedVal>) };
    args.push(own(value));
}

/// Reads one snapshot argument inside the goroutine (materialized into the
/// *goroutine's* arena).
///
/// # Safety
/// `block` must be the argument block the wrapper received.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_spawn_arg(block: *mut c_void, index: i64) -> LkDyn {
    // SAFETY: as above; the wrapper owns the block for its lifetime.
    let args = unsafe { &*(block as *mut Vec<OwnedVal>) };
    match args.get(index as usize) {
        Some(value) => materialize(value),
        None => crate::panic::raise_str("spawn argument out of range"),
    }
}

fn register_task(handle: std::thread::JoinHandle<OwnedVal>) -> i64 {
    let id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
    tasks()
        .lock()
        .expect("task registry poisoned")
        .insert(id, TaskSlot { handle: Some(handle) });
    id
}

/// `spawn(closure)` / `go f(x)` — per-arity trampolines: the spawned
/// function's captures all cross boxed (`fn(LkDyn, …) -> LkDyn`, the
/// lowering joins its signature to Dyn), so a fixed set of entry shapes
/// covers every closure. The argument block's snapshot values materialize
/// *inside* the goroutine (its own arena); the block's ownership moves.
macro_rules! spawn_arity {
    ($name:ident, $($idx:literal),*) => {
        /// # Safety
        /// `f` must be a compiled function of the matching boxed arity;
        /// `block` a live handle from [`lkrt_spawn_args_new`] with at least
        /// that many entries (ownership moves).
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(f: extern "C" fn($(spawn_arity!(@ty $idx)),*) -> LkDyn, block: *mut c_void) -> i64 {
            let block_addr = block as usize;
            register_task(std::thread::spawn(move || {
                let block = block_addr as *mut c_void;
                // SAFETY: ownership of the block moved into this thread.
                let args = unsafe { Box::from_raw(block as *mut Vec<OwnedVal>) };
                let result = f($(materialize(&args[$idx])),*);
                own(result)
            }))
        }
    };
    (@ty $idx:literal) => { LkDyn };
}

/// Zero-capture spawn (no argument block).
///
/// # Safety
/// `f` must be a compiled zero-argument function returning a boxed value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_spawn0(f: extern "C" fn() -> LkDyn) -> i64 {
    register_task(std::thread::spawn(move || own(f())))
}

spawn_arity!(lkrt_spawn1, 0);
spawn_arity!(lkrt_spawn2, 0, 1);
spawn_arity!(lkrt_spawn3, 0, 1, 2);
spawn_arity!(lkrt_spawn4, 0, 1, 2, 3);

/// `task.await(t)`: joins the goroutine and materializes its result into
/// this thread's arena. A second await on the same task raises (the VM's
/// take-once semantics); a panicked goroutine raises too.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_task_await(id: i64) -> LkDyn {
    let handle = {
        let mut tasks = tasks().lock().expect("task registry poisoned");
        tasks.get_mut(&id).and_then(|slot| slot.handle.take())
    };
    match handle {
        Some(handle) => match handle.join() {
            Ok(owned) => materialize(&owned),
            Err(_) => crate::panic::raise_str("task failed"),
        },
        // Same wording as the VM: awaiting takes the task, so a second
        // await finds nothing.
        None => {
            crate::panic::raise_str("this task has already been awaited — its result was handed to the first `await`")
        }
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::lkdyn::lkrt_dyn_from_i64;

    #[test]
    fn bounded_channel_round_trip_and_close_drain() {
        let id = lkrt_chan_new(2);
        lkrt_chan_send(id, lkrt_dyn_from_i64(1));
        lkrt_chan_send(id, lkrt_dyn_from_i64(2));
        assert_eq!(lkrt_chan_try_send(id, lkrt_dyn_from_i64(3)), 0, "full");
        assert_eq!(lkrt_chan_len(id), 2);
        lkrt_chan_close(id);
        // Buffered values drain after close (Go semantics).
        assert_eq!(lkrt_chan_recv(id).payload, 1);
        assert_eq!(lkrt_chan_recv(id).payload, 2);
        assert_eq!(lkrt_chan_is_closed(id), 1);
    }

    #[test]
    fn cross_thread_blocking_send_recv() {
        let id = lkrt_chan_new(0);
        let sender = std::thread::spawn(move || {
            for i in 0..10 {
                lkrt_chan_send(id, lkrt_dyn_from_i64(i));
            }
        });
        let mut sum = 0;
        for _ in 0..10 {
            sum += lkrt_chan_recv(id).payload;
        }
        sender.join().expect("sender");
        assert_eq!(sum, 45);
    }

    /// A blocked select (no ready arm, no default) is woken by another
    /// thread's send — the generation Condvar, not the safety-net timeout,
    /// is the discovery mechanism. Functional only: asserts completion and
    /// the delivered payload, never timing.
    #[test]
    fn blocked_select_woken_by_cross_thread_send() {
        let id = lkrt_chan_new(1);
        let sender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            lkrt_chan_send(id, lkrt_dyn_from_i64(77));
        });
        let types = arena_handle(vec![lkrt_dyn_from_i64(0)]);
        let channels = arena_handle(vec![lkrt_dyn_from_i64(id)]);
        let values = arena_handle(vec![LkDyn::NIL]);
        let guards = arena_handle(vec![LkDyn {
            tag: DYN_BOOL,
            payload: 1,
        }]);
        // SAFETY: all four handles are live dyn-list handles built above.
        let out = unsafe { lkrt_chan_select(types, channels, values, guards, 0) };
        // SAFETY: select returns a live [is_default, index, payload] list.
        let triple = unsafe { &*(out as *mut Vec<LkDyn>) };
        assert_eq!(triple[0].payload, 0, "not the default arm");
        assert_eq!(triple[1].payload, 0, "arm index");
        // Recv payload is the [present, value] pair.
        // SAFETY: recv arm payload is a live dyn-list handle.
        let pair = unsafe { &*(triple[2].payload as *mut Vec<LkDyn>) };
        assert_eq!(pair[1].payload, 77);
        sender.join().expect("sender");
    }
}
