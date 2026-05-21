mod caches;
mod frame;
mod guards;
mod quickening;
mod runtime;

use crate::val::Val;
use crate::vm::alloc::RegionAllocator;

use caches::{ClosureFastCache, RuntimeCacheStore};
use frame::CallFrameMeta;
pub(crate) use frame::FrameInfo;
pub(crate) use guards::with_current_vm;
#[cfg(test)]
pub(crate) use guards::with_current_vm_ctx;

/// LK's register-based bytecode virtual machine.
///
/// ## Architecture
///
/// The VM executes compiled bytecode using a register-based model:
/// - `stack`: Single register stack. Each call frame owns a contiguous window.
/// - Inline caches per instruction site:
///   - `access_ic`: `.field` / `[key]` on Maps and Objects (4-entry LRU)
///   - `index_ic`: `list[idx]` / `str[idx]` (4-entry LRU)
///   - `global_ic`: Global variable lookup with generation tracking
///   - `call_ic`: Function call fast-path (closure_ptr + argc match)
///   - `for_range_ic`: Numeric for-range state (bare i64, no Val boxing)
///   - `packed_hot_ic`: BC32 hot instruction cache (switch-free dispatch)
/// - `frames`: Call frame metadata stack for return addresses and register windows.
/// - `region_alloc`: Thread-local scratch buffer for temporary allocations.
///
/// ## Execution Paths
///
/// 1. **BC32 Packed Fast Path** (preferred): When a `Function` has `code32`,
///    `run_packed_code` executes it with a switch-free loop using the
///    packed hot cache and sentinel-based skip for cold sites.
///
/// 2. **Standard Match Dispatch**: `run_opcode_code` uses a `match` on the
///    `Op` enum and supports all opcodes including peephole-fused forms.
///    Functions without BC32 encoding always use this path.
const INITIAL_STACK_CAPACITY: usize = 65_536;

pub struct Vm {
    pub(super) stack: Vec<Val>,
    stack_top: usize,
    nested_cache_pool: Vec<ClosureFastCache>,
    runtime_caches: RuntimeCacheStore,
    frames: Vec<CallFrameMeta>,
    pending_resume_pc: Option<usize>,
    region_alloc: RegionAllocator,
}

impl Vm {
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(INITIAL_STACK_CAPACITY),
            stack_top: 0,
            nested_cache_pool: Vec::new(),
            runtime_caches: RuntimeCacheStore::new(),
            frames: Vec::new(),
            pending_resume_pc: None,
            region_alloc: RegionAllocator::new(),
        }
    }

    pub fn heap_bytes(&self) -> u64 {
        self.region_alloc.heap_bytes()
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}
