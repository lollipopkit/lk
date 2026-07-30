// `alloc`, not the std prelude: this module is part of the computation-only
// subset that builds without an OS.
#[allow(unused_imports)]
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use alloc::ffi::CString;
#[cfg(feature = "std")]
use core::cell::RefCell;
use core::ffi::{CStr, c_char};

use crate::state::with_runtime;

#[cfg(feature = "std")]
unsafe extern "C" {
    fn fflush(stream: *mut core::ffi::c_void) -> i32;
}

/// Fatal-guard abort. Native output goes through C stdio (`printf`), which is
/// block-buffered when stdout is not a TTY and does **not** flush on `abort()`;
/// a guard firing after user output must not silently discard what the program
/// already printed (the VM keeps it), so every abort path flushes all C streams
/// first (`fflush(NULL)` flushes every open stream).
pub(crate) fn flush_and_abort() -> ! {
    flush_c_stdio();
    #[cfg(feature = "std")]
    {
        std::process::abort()
    }
    // Bare metal has no process to abort. Panicking is the portable stop:
    // the binary supplies a panic handler, and `panic = "abort"` makes it one.
    #[cfg(not(feature = "std"))]
    {
        panic!("lkrt: unrecoverable runtime failure")
    }
}

/// The exit of a program whose own error nobody caught.
///
/// Distinct from [`flush_and_abort`] on purpose: an uncaught raise is the
/// *program* failing, not the runtime, and the VM reports it as exit status 1.
/// Aborting instead made the same program die with SIGABRT (status 134), print
/// `Aborted` from the shell, and — where core dumps are enabled — write one for
/// a script that merely forgot a `catch`.
pub(crate) fn flush_and_exit_failure() -> ! {
    flush_c_stdio();
    #[cfg(feature = "std")]
    {
        std::process::exit(1)
    }
    // Bare metal has no process to exit; the panic handler is the stop.
    #[cfg(not(feature = "std"))]
    {
        panic!("Error: uncaught")
    }
}

/// Flushes every C stdio stream (`fflush(NULL)`). Rust-side writers that share
/// a stream with generated `printf` output call this first so the two buffers
/// cannot interleave out of order.
pub(crate) fn flush_c_stdio() {
    // Bare metal has no C stdio to flush — no libc, and whatever the board
    // prints through goes out synchronously anyway.
    #[cfg(feature = "std")]
    // SAFETY: fflush(NULL) is defined by C99 to flush all open output streams.
    unsafe {
        fflush(core::ptr::null_mut());
    }
}

/// The generated-code guard exit (`Term::Abort`), kept under its ABI name.
/// It does not abort: those guards mirror *catchable* VM errors, so this
/// raises to the nearest `try` frame and, uncaught, exits 1 like the VM.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_abort() {
    crate::panic::raise_str("runtime error");
}

pub(crate) use lk_aot_abi::ABI_VERSION;
pub(crate) const LKRT_STATUS_OK: i64 = 0;
pub(crate) const LKRT_STATUS_ERR: i64 = -1;

// Thread-local under std, a spin-locked global on bare metal.
//
// Bare metal has no TLS. The lock is not guarding against threads — there are
// none — but against an interrupt handler reaching the runtime. Uncontended on
// a single core it is one atomic operation.
#[cfg(feature = "std")]
thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[cfg(not(feature = "std"))]
static LAST_ERROR: spin::Mutex<Option<String>> = spin::Mutex::new(None);

/// Runs `f` with the last-error slot, however it is stored.
#[cfg(feature = "std")]
fn with_last_error<R>(f: impl FnOnce(&mut Option<String>) -> R) -> R {
    LAST_ERROR.with(|slot| f(&mut slot.borrow_mut()))
}

#[cfg(not(feature = "std"))]
fn with_last_error<R>(f: impl FnOnce(&mut Option<String>) -> R) -> R {
    f(&mut LAST_ERROR.lock())
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_abi_version() -> i64 {
    ABI_VERSION
}

// `sigaltstack`/`sigaction` in C, because lkrt has no `libc` dependency to spell
// their platform structs with — the same reason `try_trampoline.c` exists. Absent
// on bare metal, where `build.rs` skips the C files and there are no signals.
#[cfg(feature = "std")]
unsafe extern "C" {
    fn lk_install_stack_guard();
}

/// The program's start, called from a native binary's `main` before any user
/// code, with the ABI version the code was generated against.
///
/// Two things happen here, which is why this is `rt_begin` and not `abi_check`
/// (its name until the second one arrived):
///
/// * The ABI version is checked. A linked `lkrt` reporting a different version
///   disagrees with the binary about the calling/representation contract, so this
///   aborts with a clear message rather than executing under a mismatched ABI —
///   a link/configuration error, never a reason to fall back to the VM.
/// * The stack-exhaustion handler is installed (`stack_guard.c`). Runaway
///   recursion used to die on SIGSEGV with exit 139 and no output at all, while
///   the VM raised a catchable `call depth limit exceeded`. It costs nothing on
///   the hot path: this runs once, and the handler only ever runs on a fault.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_rt_begin(expected: i64) {
    if expected != ABI_VERSION {
        crate::rt_eprintln!("lkrt ABI mismatch: binary built for ABI v{expected}, linked lkrt is v{ABI_VERSION}");
        flush_and_abort();
    }
    #[cfg(feature = "std")]
    // SAFETY: installs a signal handler and an alternate stack; both are
    // process-wide, idempotent, and this runs once before any user code.
    unsafe {
        lk_install_stack_guard()
    };
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_last_error() -> *mut c_char {
    let error = with_last_error(|slot| slot.clone().unwrap_or_default());
    owned_c_string_lossy(error)
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_error_clear() {
    with_last_error(|slot| *slot = None);
}

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cleanup() {
    with_runtime(|rt| rt.cleanup());
}

/// Frees an arena-registered string returned by an lkrt function. Unregistered
/// or null pointers are ignored, so double-frees through this entry point are
/// harmless.
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by an lkrt function
/// (`CString::into_raw`-based) that has not been freed by other means.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    if !with_runtime(|rt| rt.unregister_string(ptr)) {
        return;
    }
    // SAFETY: The pointer must come from an lkrt function that returned a
    // CString through CString::into_raw. Null was handled above.
    unsafe {
        drop(CString::from_raw(ptr));
    }
}

/// Runtime `panic(message)` lowered from AOT builtin calls: always fatal and
/// uncatchable, matching the VM's loud panic halt down to the exit status
/// (the message goes to stderr; the VM additionally prints a backtrace,
/// which stderr comparisons don't cover).
///
/// # Safety
/// `message` must be null or a NUL-terminated string pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_panic(message: *const c_char) {
    let text = if message.is_null() {
        String::new()
    } else {
        // SAFETY: non-null message pointers are NUL-terminated per the ABI.
        unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned()
    };
    crate::rt_eprintln!("{text}");
    // Uncatchable in both backends, but the *status* has to agree: the VM's
    // panic halt exits 1, so aborting here made the same program die with
    // SIGABRT (134) once compiled.
    flush_and_exit_failure();
}

/// Runtime `assert(cond)` lowered from AOT builtin calls: a false (zero)
/// condition is a fatal error, matching the VM's loud `assertion failed` halt.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_assert(cond: i64) {
    if cond == 0 {
        // Catchable in the VM (a try around a failing assert recovers):
        // raise to the nearest frame, exit 1 when uncaught.
        crate::panic::raise_str("assertion failed");
    }
}

/// `assert(cond, message)` variant: the message is display-converted by the
/// lowering, so it arrives as a C string.
///
/// # Safety
/// `message` must be null or a NUL-terminated string pointer (an LK string
/// constant or an lkrt-owned string).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_assert_msg(cond: i64, message: *const c_char) {
    if cond == 0 {
        let text = if message.is_null() {
            String::new()
        } else {
            // SAFETY: non-null message pointers are NUL-terminated per the ABI.
            unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned()
        };
        crate::panic::raise_str(&format!("assertion failed: {text}"));
    }
}

pub(crate) fn c_str(ptr: *const c_char, context: &str) -> Result<String, String> {
    if ptr.is_null() {
        return Err(format!("{context} is null"));
    }
    // SAFETY: LLVM generated code passes NUL-terminated pointers produced by
    // LK string constants or lkrt-owned CString values. Null is checked above.
    let value = unsafe { CStr::from_ptr(ptr) };
    value
        .to_str()
        .map(alloc::borrow::ToOwned::to_owned)
        .map_err(|err| format!("{context} is not valid UTF-8: {err}"))
}

pub(crate) fn owned_c_string(value: impl AsRef<str>) -> Result<*mut c_char, String> {
    let ptr = CString::new(value.as_ref())
        .map(CString::into_raw)
        .map_err(|_| "string contains interior NUL byte".to_string())?;
    with_runtime(|rt| rt.register_string(ptr));
    Ok(ptr)
}

/// Runs a host operation whose failure is a *language* error — a missing file,
/// an unreadable directory, a bad address — and raises it to the nearest `try`
/// frame, exactly as the VM does.
///
/// It used to abort the process. That made the same `fs.read_dir("/nope")`
/// catchable in the VM and fatal natively, with SIGABRT (status 134) instead of
/// the VM's exit 1 — a backend disagreement about whether a program can handle
/// its own IO failure. `set_last_error` still records the text for the ABI
/// entries that report a status instead of raising.
pub(crate) fn raising<T>(f: impl FnOnce() -> Result<T, String>) -> T {
    match f() {
        Ok(value) => value,
        Err(error) => {
            // Both borrows are dropped before the raise: `raise_str` longjmps
            // past Rust drops, so a live `RefCell` borrow would stay flagged.
            set_last_error(error.clone());
            crate::panic::raise_str(&error)
        }
    }
}

pub(crate) fn set_last_error(error: impl Into<String>) {
    with_last_error(|slot| *slot = Some(error.into()));
}

pub(crate) fn status(f: impl FnOnce() -> Result<(), String>) -> i64 {
    match f() {
        Ok(()) => {
            lkrt_error_clear();
            LKRT_STATUS_OK
        }
        Err(error) => {
            set_last_error(error);
            LKRT_STATUS_ERR
        }
    }
}

pub(crate) fn write_out<T>(out: *mut T, value: T, context: &str) -> Result<(), String> {
    if out.is_null() {
        return Err(format!("{context} out pointer is null"));
    }
    // SAFETY: The caller provides a valid out pointer for the C ABI result.
    unsafe {
        *out = value;
    }
    Ok(())
}

fn owned_c_string_lossy(value: impl AsRef<str>) -> *mut c_char {
    let sanitized = value.as_ref().replace('\0', "\\0");
    let ptr = CString::new(sanitized)
        .expect("sanitized lkrt error string has no interior NUL")
        .into_raw();
    with_runtime(|rt| rt.register_string(ptr));
    ptr
}
