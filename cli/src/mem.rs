//! Byte-accurate process memory limit for the `lk` CLI (plan M2.6).
//!
//! A counting global allocator forwards every allocation size to
//! [`lk_core::mem`], and the VM **aborts** with a `memory limit exceeded` error
//! at a GC safepoint once the live footprint passes the budget (a hard sandbox
//! stop, like fuel — untrusted code can't `catch` past it) — the byte
//! counterpart to `LK_MAX_HEAP_OBJECTS`, matching the convention of JVM `-Xmx` /
//! V8 `--max-old-space-size` / Lua's `lua_setallocf`.
//!
//! Budget: `LK_MAX_HEAP_BYTES=<bytes>` (0 = unlimited), else **70% of system
//! RAM** by default. Undetectable RAM ⇒ unlimited.

use std::alloc::{GlobalAlloc, Layout, System};

use lk_core::mem;

/// The system allocator, wrapped to keep [`lk_core::mem`]'s live-byte counter in
/// sync. The bookkeeping is a pair of relaxed atomics per alloc/free.
pub struct CountingAllocator;

// SAFETY: every method forwards to the `System` allocator with the same layout;
// the only added work is relaxed atomic accounting, which cannot affect memory
// safety. `alloc`/`dealloc`/`realloc` all balance so the counter never underflows
// (every freed block was counted when allocated, since this is *the* allocator
// from process start).
// Accounting is gated on an active budget (`mem::accounting_enabled`): an
// unlimited run pays a single relaxed load per op and skips the read-modify-
// writes, so the perf bench (`LK_MAX_HEAP_BYTES=0`) is unaffected.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && mem::accounting_enabled() {
            mem::record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        if mem::accounting_enabled() {
            mem::record_dealloc(layout.size());
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() && mem::accounting_enabled() {
            mem::record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() && mem::accounting_enabled() {
            // Success frees the old block and returns a `new_size` one (whether
            // grown in place or moved): net delta is `new_size - old_size`.
            mem::record_dealloc(layout.size());
            mem::record_alloc(new_size);
        }
        new_ptr
    }
}

/// Install the process byte budget from `LK_MAX_HEAP_BYTES` (explicit, `0` =
/// unlimited) or default to 70% of system RAM. Call once at startup.
pub fn configure() {
    if let Some(bytes) = std::env::var("LK_MAX_HEAP_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        mem::set_limit(bytes); // 0 = unlimited (an explicit opt-out)
        return;
    }
    if let Some(total) = system_memory_bytes() {
        // 70% of RAM; `total / 10 * 7` avoids overflow on the multiply.
        mem::set_limit(total / 10 * 7);
    }
}

/// Best-effort total system RAM in bytes. `None` when it cannot be determined
/// (⇒ the limit stays unlimited).
fn system_memory_bytes() -> Option<usize> {
    #[cfg(target_os = "macos")]
    {
        // `sysctl -n hw.memsize` — avoids a libc dependency; macOS/BSD only.
        let output = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse::<usize>().ok()
    }
    #[cfg(target_os = "linux")]
    {
        // `/proc/meminfo`: `MemTotal:  16384000 kB`.
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: usize = meminfo.lines().find_map(|line| {
            line.strip_prefix("MemTotal:")?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse()
                .ok()
        })?;
        Some(kb * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}
