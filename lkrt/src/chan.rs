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

use core::ffi::{CStr, c_char, c_void};
use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use crate::lkdyn::{DYN_BOOL, DYN_F64, DYN_I64, DYN_LIST, DYN_MAP, DYN_NIL, DYN_STR, LkDyn};
use crate::lkmap::StrDynMap;
use crate::lkstr::arena_c_string;
use crate::state::arena_handle;

/// A value that crossed an isolate boundary: fully owned, `Send`. Maps keep
/// their iteration order (entries captured in order, replayed on rebuild —
/// same keys + same insertion order = the same Fx layout on the other side).
enum OwnedVal {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<OwnedVal>),
    Map(Vec<(String, OwnedVal)>),
}

fn own(v: LkDyn) -> OwnedVal {
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
        DYN_LIST => {
            let handle = v.payload as *mut c_void;
            let items: &[LkDyn] = if handle.is_null() {
                &[]
            } else {
                // SAFETY: DYN_LIST payloads are live dyn-list handles.
                unsafe { &*(handle as *mut Vec<LkDyn>) }
            };
            OwnedVal::List(items.iter().map(|&item| own(item)).collect())
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
        _ => crate::panic::raise_str("value cannot cross a channel"),
    }
}

fn materialize(v: &OwnedVal) -> LkDyn {
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
    }
}

struct ChanState {
    queue: VecDeque<OwnedVal>,
    closed: bool,
}

struct ChanInner {
    state: Mutex<ChanState>,
    /// Signals receivers (value available / closed).
    recv_cv: Condvar,
    /// Signals bounded senders (space available / closed).
    send_cv: Condvar,
    cap: Option<usize>,
}

fn registry() -> &'static Mutex<HashMap<i64, Arc<ChanInner>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<i64, Arc<ChanInner>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn channel(id: i64) -> Arc<ChanInner> {
    match registry().lock().expect("channel registry poisoned").get(&id) {
        Some(inner) => Arc::clone(inner),
        None => crate::panic::raise_str("Channel not found"),
    }
}

static NEXT_CHANNEL_ID: AtomicI64 = AtomicI64::new(1);

/// `chan(capacity)` — `capacity <= 0` is unbounded (the VM's rule). The
/// channel travels as its `i64` id.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_new(capacity: i64) -> i64 {
    let id = NEXT_CHANNEL_ID.fetch_add(1, Ordering::Relaxed);
    let inner = Arc::new(ChanInner {
        state: Mutex::new(ChanState {
            queue: VecDeque::new(),
            closed: false,
        }),
        recv_cv: Condvar::new(),
        send_cv: Condvar::new(),
        cap: if capacity <= 0 { None } else { Some(capacity as usize) },
    });
    registry().lock().expect("channel registry poisoned").insert(id, inner);
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
            drop(state);
            inner.recv_cv.notify_one();
            return;
        }
        state = inner.send_cv.wait(state).expect("channel poisoned");
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
            drop(state);
            inner.send_cv.notify_one();
            return materialize(&value);
        }
        if state.closed {
            drop(state);
            crate::panic::raise_str("receive on closed channel");
        }
        state = inner.recv_cv.wait(state).expect("channel poisoned");
    }
}

/// `chan.close(c)`: marks closed; buffered values stay drainable (Go
/// semantics), blocked parties wake.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_close(id: i64) {
    let inner = channel(id);
    inner.state.lock().expect("channel poisoned").closed = true;
    inner.recv_cv.notify_all();
    inner.send_cv.notify_all();
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
        drop(state);
        inner.recv_cv.notify_one();
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
        drop(state);
        inner.send_cv.notify_one();
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

/// `chan.is_closed(c)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_chan_is_closed(id: i64) -> i64 {
    i64::from(channel(id).state.lock().expect("channel poisoned").closed)
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

/// `spawn(closure)` / `go f(x)`: launches the compiled wrapper on a fresh OS
/// thread (its own arena) and returns the task id. The wrapper takes the
/// argument block and returns its boxed result.
///
/// # Safety
/// `wrapper` must be a compiled `extern "C" fn(*mut c_void) -> LkDyn`;
/// `block` a live handle from [`lkrt_spawn_args_new`] (ownership moves).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_spawn(wrapper: extern "C" fn(*mut c_void) -> LkDyn, block: *mut c_void) -> i64 {
    let id = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
    let block_addr = block as usize;
    let handle = std::thread::spawn(move || {
        let block = block_addr as *mut c_void;
        let result = wrapper(block);
        let owned = own(result);
        // SAFETY: ownership of the block moved into this thread; nothing
        // else references it.
        drop(unsafe { Box::from_raw(block as *mut Vec<OwnedVal>) });
        owned
    });
    tasks()
        .lock()
        .expect("task registry poisoned")
        .insert(id, TaskSlot { handle: Some(handle) });
    id
}

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
        None => crate::panic::raise_str("Task not found"),
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
}
