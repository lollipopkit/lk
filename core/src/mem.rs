//! Process-wide heap-byte accounting for the sandbox memory limit (plan M2.6).
//!
//! These counters are **process-global** — the allocator is process-global, so
//! this is deliberately *not* per-VM state. A host installs a counting global
//! allocator (see the CLI's `CountingAllocator`) that forwards every alloc/
//! dealloc size to [`record_alloc`] / [`record_dealloc`], and the VM polls
//! [`over_limit`] at GC safepoints — collecting first, then **aborting** with a
//! `memory limit exceeded` error if the *reachable* footprint is still over
//! budget. Like fuel and the object cap this is a hard sandbox stop, not a
//! program-catchable error (untrusted code must not be able to `catch` past it).
//!
//! This is the byte-accurate counterpart to the coarse object-count cap
//! (`LK_MAX_HEAP_OBJECTS`): it matches the convention of JVM `-Xmx`, V8
//! `--max-old-space-size`, and Lua's `lua_setallocf` — a byte budget enforced at
//! the allocator. A `0` limit means unlimited (the default when no counting
//! allocator is installed, so a plain `lk-core` embedding pays nothing).

use core::sync::atomic::{AtomicUsize, Ordering};

/// Live bytes currently allocated process-wide, as seen by the counting
/// allocator. Stays `0` when no such allocator is installed.
pub static HEAP_BYTES_USED: AtomicUsize = AtomicUsize::new(0);

/// Byte budget; `0` = unlimited (the default).
pub static HEAP_BYTES_LIMIT: AtomicUsize = AtomicUsize::new(0);

/// Record `bytes` newly allocated (called from the global allocator).
#[inline]
pub fn record_alloc(bytes: usize) {
    HEAP_BYTES_USED.fetch_add(bytes, Ordering::Relaxed);
}

/// Record `bytes` freed (called from the global allocator). Saturates at `0`:
/// the allocator only accounts while a limit is set, so a block allocated in the
/// brief pre-`set_limit` startup window (unaccounted) but freed afterwards would
/// otherwise underflow the counter — a negligible skew, clamped here.
#[inline]
pub fn record_dealloc(bytes: usize) {
    let _ = HEAP_BYTES_USED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
        Some(used.saturating_sub(bytes))
    });
}

/// Set the byte budget (`0` = unlimited).
pub fn set_limit(bytes: usize) {
    HEAP_BYTES_LIMIT.store(bytes, Ordering::Relaxed);
}

/// The current byte budget (`0` = unlimited).
pub fn limit() -> usize {
    HEAP_BYTES_LIMIT.load(Ordering::Relaxed)
}

/// Whether a byte budget is active. The counting allocator skips its
/// bookkeeping (a single relaxed load, no read-modify-write) when this is
/// `false`, so an unlimited run — including the perf-bench's `LK_MAX_HEAP_BYTES=0`
/// — pays essentially nothing.
#[inline]
pub fn accounting_enabled() -> bool {
    HEAP_BYTES_LIMIT.load(Ordering::Relaxed) != 0
}

/// Live bytes currently allocated.
pub fn used() -> usize {
    HEAP_BYTES_USED.load(Ordering::Relaxed)
}

/// Whether the live footprint currently exceeds the budget. Short-circuits to
/// `false` (a single atomic load) when unlimited — the common case.
#[inline]
pub fn over_limit() -> bool {
    let limit = HEAP_BYTES_LIMIT.load(Ordering::Relaxed);
    limit != 0 && HEAP_BYTES_USED.load(Ordering::Relaxed) > limit
}
