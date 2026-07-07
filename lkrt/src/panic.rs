//! Native protected calls (deep-coverage plan G: `try$call`): a setjmp/
//! longjmp handler stack plus mutable capture cells.
//!
//! The generated code executes `_setjmp` itself (declared `returns_twice` in
//! the IR — the compiler must see it); this module owns the jump buffers,
//! the raised value, and the raise entry points. `raise` with no live
//! handler stays `flush_and_abort()` — an uncaught error's observable
//! behaviour (flushed stdout + abnormal exit) is unchanged.
//!
//! longjmp-over-Rust-frames safety: the frames skipped between a raise and
//! its handler only hold arena-owned values and plain temporaries (the arena
//! frees at process end, leaks are the lkrt model); nothing on those frames
//! runs a load-bearing destructor. Hard rule: a raise must never happen
//! while a `with_runtime` borrow is live (the `RefCell` borrow flag would
//! stay set) — the raise paths below touch only their own `RefCell`s, and
//! every ABI entry that can raise takes care to drop runtime borrows first.

use core::ffi::{c_char, c_int, c_void};
use std::cell::{Cell, RefCell};
use std::ffi::CString;

use crate::lkdyn::LkDyn;
use crate::lkstr::arena_c_string;

// glibc's BSD-semantics pair (no signal-mask save/restore): `_setjmp` is
// what the generated IR declares (`returns_twice`), `_longjmp` is called
// from the raise path here. A glibc x86-64 `jmp_buf` is 200 bytes; the
// buffer is oversized and 16-aligned for safety across libcs.
unsafe extern "C" {
    fn _longjmp(env: *mut c_void, val: c_int) -> !;
}

#[repr(C, align(16))]
struct JmpBuf([u8; 512]);

thread_local! {
    /// Live `try` frames, innermost last. The boxing is load-bearing (not a
    /// `vec_box` accident): `_setjmp` captured the buffer's address, which
    /// must survive the vector growing/reallocating.
    #[allow(clippy::vec_box)]
    static HANDLERS: RefCell<Vec<Box<JmpBuf>>> = const { RefCell::new(Vec::new()) };
    /// The value carried by the in-flight (or just-caught) raise.
    static CURRENT_ERROR: Cell<LkDyn> = const { Cell::new(LkDyn::NIL) };
}

/// Enters a `try` frame: pushes a fresh jump buffer and returns its address
/// (the generated code passes it to `_setjmp`).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_try_push() -> *mut c_void {
    HANDLERS.with(|handlers| {
        let mut handlers = handlers.borrow_mut();
        handlers.push(Box::new(JmpBuf([0; 512])));
        let buf: &mut JmpBuf = handlers.last_mut().expect("just pushed");
        buf as *mut JmpBuf as *mut c_void
    })
}

/// Leaves a `try` frame on the success path (the failure path's pop happens
/// inside [`raise_current`] before the jump).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_try_pop() {
    HANDLERS.with(|handlers| {
        handlers.borrow_mut().pop();
    });
}

/// The value of the raise that just landed (read in the catch arm).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_current_error() -> LkDyn {
    CURRENT_ERROR.with(|slot| slot.get())
}

fn raise_current(value: LkDyn) -> ! {
    CURRENT_ERROR.with(|slot| slot.set(value));
    let target = HANDLERS.with(|handlers| {
        let mut handlers = handlers.borrow_mut();
        handlers.pop().map(|buf| Box::into_raw(buf) as *mut c_void)
    });
    match target {
        // The buffer is intentionally leaked (arena model): the landing pad
        // may still be on the stack that owns it, freeing here would race
        // the longjmp.
        Some(buf) => unsafe { _longjmp(buf, 1) },
        None => crate::abi::flush_and_abort(),
    }
}

/// Internal guard entry: raises a message string to the nearest `try` frame
/// (arena-owned), or aborts loudly — every lkrt guard that mirrors a
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

/// Writes a cell.
///
/// # Safety
/// `cell` must be a live handle from [`lkrt_rt_cell_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_rt_cell_set(cell: *mut c_void, value: LkDyn) {
    // SAFETY: as above.
    unsafe { *(cell as *mut LkDyn) = value };
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
