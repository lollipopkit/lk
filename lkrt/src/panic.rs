//! Native protected regions (deep-coverage plan G): a setjmp/
//! longjmp handler stack plus mutable capture cells.
//!
//! The generated code executes `_setjmp` itself (declared `returns_twice` in
//! the IR — the compiler must see it); this module owns the jump buffers,
//! the raised value, and the raise entry points. `raise` with no live
//! handler flushes stdout, prints the error, and exits 1 — the same status
//! the VM gives, because an uncaught error is the program failing.
//!
//! longjmp-over-Rust-frames safety: the frames skipped between a raise and
//! its handler only hold arena-owned values and plain temporaries (the arena
//! frees at process end, leaks are the lkrt model); nothing on those frames
//! runs a load-bearing destructor. Hard rule: a raise must never happen
//! while a `with_runtime` borrow is live (the `RefCell` borrow flag would
//! stay set) — the raise paths below touch only their own `RefCell`s, and
//! every ABI entry that can raise takes care to drop runtime borrows first.

// `alloc`, not the std prelude: this module is part of the computation-only
// subset that builds without an OS.
#[allow(unused_imports)]
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use alloc::ffi::CString;
use core::cell::Cell;
#[cfg(feature = "std")]
use core::cell::RefCell;
use core::ffi::{c_char, c_void};
// `c_int` is only in the `_longjmp` declaration, which is hosted-only — the
// bare-metal raise path unwinds by other means.
#[cfg(feature = "std")]
use core::ffi::c_int;

use crate::lkdyn::LkDyn;
use crate::lkstr::arena_c_string;

// glibc's BSD-semantics pair (no signal-mask save/restore): `_setjmp` is
// what the generated IR declares (`returns_twice`), `_longjmp` is called
// from the raise path here. A glibc x86-64 `jmp_buf` is 200 bytes; the
// buffer is oversized and 16-aligned for safety across libcs.
#[cfg(feature = "std")]
unsafe extern "C" {
    fn _longjmp(env: *mut c_void, val: c_int) -> !;
}

#[repr(C, align(16))]
struct JmpBuf([u8; 512]);

/// The jump buffer of the last raise, parked instead of freed: `_longjmp`
/// still reads the buffer after the handler pop, so freeing at raise time
/// races the jump — but once the landing pad runs the buffer is dead. It is
/// reclaimed (reused or freed) at the next `try` push, the next parked
/// raise, or thread exit, so raise-heavy programs stay at one buffer per
/// thread instead of leaking 512 bytes per raise (LeakSanitizer flagged the
/// old leak, and its exit-time report also swallowed buffered stdout).
struct SpareJmpBuf(Cell<*mut JmpBuf>);

impl SpareJmpBuf {
    /// Takes the parked buffer for reuse, or allocates fresh. Safe to reuse:
    /// any longjmp through the parked buffer landed before the control flow
    /// pushing a new `try` frame could run.
    fn take_or_alloc(&self) -> Box<JmpBuf> {
        let parked = self.0.replace(core::ptr::null_mut());
        if parked.is_null() {
            Box::new(JmpBuf([0; 512]))
        } else {
            // SAFETY: parked pointers come from `Box::into_raw` in `park`
            // and are handed out exactly once (replaced with null above).
            unsafe { Box::from_raw(parked) }
        }
    }

    /// Parks `buf` for the raise in flight and returns the raw pointer the
    /// longjmp reads. A previously parked buffer is dead by now (its landing
    /// pad ran before this raise could execute) and is freed here.
    fn park(&self, buf: Box<JmpBuf>) -> *mut JmpBuf {
        let raw = Box::into_raw(buf);
        let previous = self.0.replace(raw);
        if !previous.is_null() {
            // SAFETY: `previous` came from `Box::into_raw` in an earlier
            // `park`; its longjmp completed before this code could run.
            drop(unsafe { Box::from_raw(previous) });
        }
        raw
    }
}

impl Drop for SpareJmpBuf {
    fn drop(&mut self) {
        let parked = self.0.replace(core::ptr::null_mut());
        if !parked.is_null() {
            // SAFETY: no longjmp is in flight during thread teardown.
            drop(unsafe { Box::from_raw(parked) });
        }
    }
}

// Thread-local under std, spin-locked globals on bare metal, which has no TLS.
// The lock guards against an interrupt handler reaching the runtime, not
// against threads — on a single core there are none.
#[cfg(feature = "std")]
thread_local! {
    /// Live `try` frames, innermost last. The boxing is load-bearing (not a
    /// `vec_box` accident): `_setjmp` captured the buffer's address, which
    /// must survive the vector growing/reallocating.
    #[allow(clippy::vec_box)]
    static HANDLERS: RefCell<Vec<Box<JmpBuf>>> = const { RefCell::new(Vec::new()) };
    /// The value carried by the in-flight (or just-caught) raise.
    static CURRENT_ERROR: Cell<LkDyn> = const { Cell::new(LkDyn::NIL) };
    /// See [`SpareJmpBuf`].
    static SPARE_BUF: SpareJmpBuf = const { SpareJmpBuf(Cell::new(core::ptr::null_mut())) };
}

#[cfg(not(feature = "std"))]
#[allow(clippy::vec_box)]
static HANDLERS_CELL: spin::Mutex<Vec<Box<JmpBuf>>> = spin::Mutex::new(Vec::new());
#[cfg(not(feature = "std"))]
static CURRENT_ERROR_CELL: spin::Mutex<LkDyn> = spin::Mutex::new(LkDyn::NIL);
#[cfg(not(feature = "std"))]
/// SAFETY: the pointer is only ever handed back to the runtime that allocated
/// it, and the mutex serialises every access to the slot.
#[cfg(not(feature = "std"))]
struct SpareSlot(*mut JmpBuf);
#[cfg(not(feature = "std"))]
unsafe impl Send for SpareSlot {}
#[cfg(not(feature = "std"))]
static SPARE_BUF_CELL: spin::Mutex<SpareSlot> = spin::Mutex::new(SpareSlot(core::ptr::null_mut()));

/// Runs `f` with the handler stack, however it is stored.
#[cfg(feature = "std")]
fn with_handlers<R>(f: impl FnOnce(&mut Vec<Box<JmpBuf>>) -> R) -> R {
    HANDLERS.with(|handlers| f(&mut handlers.borrow_mut()))
}

#[cfg(not(feature = "std"))]
fn with_handlers<R>(f: impl FnOnce(&mut Vec<Box<JmpBuf>>) -> R) -> R {
    f(&mut HANDLERS_CELL.lock())
}

#[cfg(feature = "std")]
fn with_current_error<R>(f: impl FnOnce(&Cell<LkDyn>) -> R) -> R {
    CURRENT_ERROR.with(f)
}

#[cfg(not(feature = "std"))]
fn with_current_error<R>(f: impl FnOnce(&Cell<LkDyn>) -> R) -> R {
    let mut slot = CURRENT_ERROR_CELL.lock();
    let cell = Cell::new(*slot);
    let result = f(&cell);
    *slot = cell.get();
    result
}

#[cfg(feature = "std")]
fn with_spare_buf<R>(f: impl FnOnce(&SpareJmpBuf) -> R) -> R {
    SPARE_BUF.with(f)
}

#[cfg(not(feature = "std"))]
fn with_spare_buf<R>(f: impl FnOnce(&SpareJmpBuf) -> R) -> R {
    let mut slot = SPARE_BUF_CELL.lock();
    let spare = SpareJmpBuf(Cell::new(slot.0));
    let result = f(&spare);
    slot.0 = spare.0.get();
    result
}

/// Enters a `try` frame: pushes a fresh jump buffer and returns its address
/// (the generated code passes it to `_setjmp`).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_try_push() -> *mut c_void {
    let fresh = with_spare_buf(SpareJmpBuf::take_or_alloc);
    with_handlers(|handlers| {
        handlers.push(fresh);
        let buf: &mut JmpBuf = handlers.last_mut().expect("just pushed");
        buf as *mut JmpBuf as *mut c_void
    })
}

/// Leaves a `try` frame on the success path (the failure path's pop happens
/// inside [`raise_current`] before the jump).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_try_pop() {
    with_handlers(|handlers| {
        handlers.pop();
    });
}

/// The value of the raise that just landed (read in the catch arm).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_current_error() -> LkDyn {
    with_current_error(|slot| slot.get())
}

fn raise_current(value: LkDyn) -> ! {
    // The rule at the top of this module, asked rather than trusted. A raise
    // taken with a runtime borrow live does not fail here — it fails at the next
    // runtime operation, which is somewhere else entirely and reads as a bug in
    // whatever code happened to be next. Saying it at the raise is the
    // difference between a name and a puzzle.
    #[cfg(feature = "std")]
    if crate::state::runtime_borrow_is_live() {
        crate::rt_eprintln!(
            "lkrt: a raise was taken while a runtime borrow was live; the borrow would never be \
             released. This is an lkrt bug — the entry that raised must drop its runtime borrow \
             first (see `raising` in abi.rs)."
        );
        crate::abi::flush_and_abort()
    }
    with_current_error(|slot| slot.set(value));
    let target = with_handlers(|handlers| handlers.pop());
    match target {
        // Park (not free) the buffer: the longjmp still reads it. Bounded at
        // one parked buffer per thread; reclaimed at the next push/park/
        // thread end (see `SpareJmpBuf`).
        Some(_buf) => {
            #[cfg(feature = "std")]
            {
                let raw = with_spare_buf(|spare| spare.park(_buf));
                unsafe { _longjmp(raw as *mut c_void, 1) }
            }
            // Bare metal has no libc `setjmp`/`longjmp`, and the native
            // lowering does not support `try`/`catch` there either — the
            // trampoline that would make it work is skipped for those targets
            // (see lkrt/build.rs). A handler cannot have been pushed, so this
            // is unreachable in practice; aborting is the honest answer if it
            // somehow is not.
            #[cfg(not(feature = "std"))]
            {
                crate::abi::flush_and_abort()
            }
        }
        // Uncaught: surface the error before dying — a silent abort loses it.
        // Exit 1 rather than abort: the program failed, the runtime did not.
        //
        // `Error: ` is the label the whole language reports with — parse errors,
        // type errors, and every `diagnostic::error` in the CLI. This said `lk:
        // uncaught error: ` for as long as the divergence was written off as
        // "only the stderr text differs, and the differential compares stdout +
        // success only" — which says what the gate looked at, not what a reader
        // gets: the same failing program read two different ways depending on
        // which backend ran it, and `lk:` named a program that a compiled binary
        // is not. `an_uncaught_error_exits_and_reads_the_same_on_both_backends` now compares
        // the two byte for byte.
        None => {
            crate::rt_eprintln!("Error: {}", crate::lkdyn::display_for_diagnostics(value));
            crate::abi::flush_and_exit_failure()
        }
    }
}

/// Internal guard entry: raises a message string to the nearest `try` frame
/// (arena-owned), or reports it and exits 1 — every lkrt guard that mirrors a
/// *catchable* VM error routes through here (G3). `panic` stays fatal.
pub(crate) fn raise_str(message: &str) -> ! {
    let owned = arena_c_string(CString::new(message).unwrap_or_default());
    raise_current(crate::lkdyn::lkrt_dyn_from_str(owned))
}

/// `error(v)` and every runtime guard: raises a boxed value to the nearest
/// `try` frame, or aborts loudly (the uncaught behaviour, byte-unchanged).
/// Diverges (longjmp or abort); the `()` signature keeps it inside the ABI
/// vocabulary — the lowering emits `unreachable` after the call.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_raise_dyn(value: LkDyn) {
    raise_current(value)
}

/// A message-carrying raise (runtime guards whose VM counterpart raises a
/// string): the text is arena-owned.
///
/// # Safety
/// `message` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_raise_msg(message: *const c_char) {
    let owned = if message.is_null() {
        arena_c_string(CString::default())
    } else {
        // SAFETY: caller passes a NUL-terminated string; copy it into the
        // arena so the raised value outlives the raising frame.
        let text = unsafe { core::ffi::CStr::from_ptr(message) }.to_owned();
        arena_c_string(text)
    };
    raise_current(crate::lkdyn::lkrt_dyn_from_str(owned))
}

// ── Mutable capture cells ───────────────────────────────────────────────
// The VM promotes a local assigned inside a closure to an `UpvalCell` (a
// shared mutable box). Natively a cell is an arena-owned `LkDyn` slot passed
// by pointer: the caller and the closure body write through the same slot.

/// Allocates a cell holding `value`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_cell_new(value: LkDyn) -> *mut c_void {
    crate::state::arena_handle(value)
}

/// Reads a cell.
///
/// # Safety
/// `cell` must be a live handle from [`lkrt_rt_cell_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_cell_get(cell: *mut c_void) -> LkDyn {
    // SAFETY: `cell` addresses an `LkDyn` from `lkrt_rt_cell_new`.
    unsafe { *(cell as *mut LkDyn) }
}

/// Allocates a cell parking a **raw handle** — a typed container, which cannot
/// survive being boxed (see [`crate::lkdyn::DYN_RAW`]).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_cell_new_raw(handle: i64) -> *mut c_void {
    crate::state::arena_handle(LkDyn {
        tag: crate::lkdyn::DYN_RAW,
        payload: handle,
    })
}

/// Reads a raw-handle cell. Raises if the cell holds a boxed value instead —
/// the two families must not be crossed, and this is where that is caught.
///
/// # Safety
/// `cell` must be a live handle from [`lkrt_rt_cell_new_raw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_cell_get_raw(cell: *mut c_void) -> i64 {
    // SAFETY: `cell` addresses an `LkDyn` from one of the cell constructors.
    let value = unsafe { *(cell as *mut LkDyn) };
    if value.tag != crate::lkdyn::DYN_RAW {
        crate::panic::raise_str("runtime error");
    }
    value.payload
}

/// Writes a raw-handle cell.
///
/// # Safety
/// `cell` must be a live handle from [`lkrt_rt_cell_new_raw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_cell_set_raw(cell: *mut c_void, handle: i64) {
    // SAFETY: as above.
    unsafe {
        *(cell as *mut LkDyn) = LkDyn {
            tag: crate::lkdyn::DYN_RAW,
            payload: handle,
        }
    };
}

/// Writes a cell.
///
/// # Safety
/// `cell` must be a live handle from [`lkrt_rt_cell_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_cell_set(cell: *mut c_void, value: LkDyn) {
    // SAFETY: as above.
    unsafe { *(cell as *mut LkDyn) = value };
}

/// Releases an arena-owned container handle early, before `lkrt_cleanup`.
///
/// Emitted by the scope-drop pass for a container proven dead at the end of
/// its block (`lk_aot_mir::opt`), so a loop that builds a temporary list per
/// iteration does not grow the arena without bound. An unknown or already
/// released pointer is a no-op rather than a double free, which keeps a
/// lowering bug from turning into memory corruption.
///
/// # Safety
/// `handle` must be a pointer previously returned by an arena container
/// constructor, and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_handle_release(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let drop_fn = crate::state::with_runtime(|rt| rt.unregister_container(handle));
    if let Some(drop_fn) = drop_fn {
        // SAFETY: the arena stored this drop function alongside the pointer
        // when the handle was registered, so the type matches. Removing it
        // from the table first makes a second release a no-op.
        unsafe { drop_fn(handle) };
    }
}

/// [`lkrt_rt_handle_release`] plus the arena strings the container created
/// itself (`str.split`, `str.chars` — see
/// `lkrt::state::arena_handle_owning_strings`).
///
/// Emitted only where the scope-drop pass proved no element ever left the
/// container, which is what makes freeing the elements sound. On a container
/// with no registered element strings this is exactly the shallow release, so a
/// mis-emitted deep release degrades to the safe one rather than freeing
/// something it should not.
///
/// # Safety
/// As [`lkrt_rt_handle_release`], and no element read out of `handle` may still
/// be in use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_handle_release_deep(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    // The strings come out while the container is still alive — reading them is
    // what needs it — and the arena registration is already gone, so a
    // concurrent release cannot see the same entry.
    let released = crate::state::with_runtime(|rt| rt.unregister_container_deep(handle));
    let Some((drop_fn, strings)) = released else {
        return;
    };
    for string in strings {
        // SAFETY: each pointer came from `CString::into_raw` and was registered
        // in the arena, which `unregister_container_deep` just removed it from,
        // so this is the only owner left.
        drop(unsafe { CString::from_raw(string) });
    }
    // SAFETY: as in the shallow release — the drop function was stored with the
    // pointer and matches its concrete type.
    unsafe { drop_fn(handle) };
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::lkdyn::lkrt_dyn_from_i64;

    #[test]
    fn cells_share_mutations() {
        let cell = lkrt_rt_cell_new(lkrt_dyn_from_i64(1));
        unsafe {
            assert_eq!(lkrt_rt_cell_get(cell).payload, 1);
            lkrt_rt_cell_set(cell, lkrt_dyn_from_i64(7));
            assert_eq!(lkrt_rt_cell_get(cell).payload, 7);
        }
    }

    // The setjmp/longjmp round trip itself is exercised end-to-end by the
    // native differential corpus (Rust tests cannot call `_setjmp` safely);
    // the no-handler path is the existing abort, covered there too.
}

// No `_setjmp` involved — runs under Miri too (Stacked Borrows over the
// park/reuse/free raw-pointer choreography).
#[cfg(test)]
mod spare_buf_tests {
    use super::*;

    #[test]
    fn spare_jmp_buf_parks_reuses_and_frees() {
        let spare = SpareJmpBuf(Cell::new(core::ptr::null_mut()));
        // Nothing parked: allocates fresh.
        let first = spare.take_or_alloc();
        let first_addr = &*first as *const JmpBuf;
        // Parking hands back the same address for the longjmp.
        assert_eq!(spare.park(first), first_addr as *mut JmpBuf);
        // Reuse: the parked buffer comes back instead of a fresh allocation.
        let reused = spare.take_or_alloc();
        assert_eq!(&*reused as *const JmpBuf, first_addr);
        // Parking twice frees the previous buffer (no growth) — Miri/ASan
        // validate the frees; Drop reclaims whatever stays parked.
        spare.park(reused);
        spare.park(Box::new(JmpBuf([0; 512])));
    }
}
