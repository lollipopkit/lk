use std::{
    cell::RefCell,
    sync::atomic::{AtomicU64, Ordering},
};

use tracing::trace;

/// Allocation region selected by escape analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AllocationRegion {
    #[default]
    ThreadLocal,
    Heap,
}

/// Plan produced for a function describing how SSA values should be allocated.
#[derive(Debug, Clone, Default)]
pub struct RegionPlan {
    /// Allocation class per SSA value index.
    pub values: Vec<AllocationRegion>,
    /// Allocation class for the function return value (by convention index = `values.len()`).
    pub return_region: AllocationRegion,
}

impl RegionPlan {
    pub fn region_for(&self, value_index: usize) -> AllocationRegion {
        self.values
            .get(value_index)
            .copied()
            .unwrap_or(AllocationRegion::ThreadLocal)
    }
}

thread_local! {
    static TLS_ARENA: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(32 * 1024));
}

/// Thread-safe allocator that prefers thread-local arenas and falls back to heap.
#[derive(Debug, Default)]
pub struct RegionAllocator {
    heap_fallback_bytes: AtomicU64,
}

impl RegionAllocator {
    pub const fn new() -> Self {
        Self {
            heap_fallback_bytes: AtomicU64::new(0),
        }
    }

    /// Borrow a zeroed thread-local buffer of at least `len` bytes and return the closure result.
    pub fn with_thread_local<F, R>(&self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        TLS_ARENA.with(|cell| {
            let mut buffer = cell.borrow_mut();
            if buffer.len() < len {
                buffer.resize(len, 0);
            } else {
                buffer[..len].fill(0);
            }
            let result = f(&mut buffer[..len]);
            buffer[..len].fill(0);
            result
        })
    }

    /// Allocate a heap buffer for escaping values and zero it for determinism.
    pub fn allocate_heap(&self, len: usize) -> Box<[u8]> {
        let prev = self.heap_fallback_bytes.fetch_add(len as u64, Ordering::Relaxed);
        let total = prev + len as u64;
        trace!(
            target: "lk::vm::alloc",
            bytes = len,
            total_bytes = total,
            "region_allocator.heap_alloc"
        );
        vec![0u8; len].into_boxed_slice()
    }

    pub fn heap_bytes(&self) -> u64 {
        self.heap_fallback_bytes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn thread_local_reuse_is_deterministic() {
        let allocator = RegionAllocator::new();

        allocator.with_thread_local(16, |slice| {
            for (idx, byte) in slice.iter_mut().enumerate() {
                *byte = idx as u8;
            }
        });

        allocator.with_thread_local(16, |slice| {
            assert!(slice.iter().all(|b| *b == 0), "buffer should be zeroed between uses");
            slice.copy_from_slice(&[1; 16]);
        });
    }

    #[test]
    fn thread_local_buffers_are_isolated_per_thread() {
        let allocator = RegionAllocator::new();
        let ptr_main = allocator.with_thread_local(8, |slice| slice.as_ptr() as usize);

        let handle = thread::spawn(move || {
            let allocator = RegionAllocator::new();
            allocator.with_thread_local(8, |slice| slice.as_ptr() as usize)
        })
        .join()
        .expect("thread join");

        assert_ne!(ptr_main, handle, "different threads should receive independent buffers");
    }

    #[test]
    fn heap_allocation_tracks_bytes() {
        let allocator = RegionAllocator::new();
        let buf = allocator.allocate_heap(32);
        assert_eq!(buf.len(), 32);
        assert_eq!(allocator.heap_bytes(), 32);
    }
}
