mod caches;
mod frame;
mod guards;
mod quickening;
mod runtime;

use crate::val::Val;
use crate::vm::alloc::RegionAllocator;

use caches::{AccessIc, CallIc, ClosureFastCache, ForRangeState, GlobalEntry, IndexIc, PackedHotEntry};
use frame::CallFrameMeta;
pub(crate) use frame::FrameInfo;
pub(crate) use guards::with_current_vm;
#[cfg(test)]
pub(crate) use guards::with_current_vm_ctx;
use quickening::QuickeningSite;

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
    access_ic: Vec<Option<AccessIc>>,
    index_ic: Vec<Option<IndexIc>>,
    global_ic: Vec<Option<GlobalEntry>>,
    call_ic: Vec<Option<CallIc>>,
    for_range_ic: Vec<Option<ForRangeState>>,
    packed_hot_ic: Vec<Option<PackedHotEntry>>,
    packed_hot_ic_key: usize,
    quickening_ic: Vec<QuickeningSite>,
    quickening_ic_key: usize,
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
            access_ic: Vec::new(),
            index_ic: Vec::new(),
            global_ic: Vec::new(),
            call_ic: Vec::new(),
            for_range_ic: Vec::new(),
            packed_hot_ic: Vec::new(),
            packed_hot_ic_key: 0,
            quickening_ic: Vec::new(),
            quickening_ic_key: 0,
            frames: Vec::new(),
            pending_resume_pc: None,
            region_alloc: RegionAllocator::new(),
        }
    }

    pub(super) fn read_reg(&self, window: frame::RegisterWindowRef, idx: usize) -> Option<&Val> {
        match window {
            frame::RegisterWindowRef::Current => self.stack.get(idx),
            frame::RegisterWindowRef::Base(base) => self.stack.get(base + idx),
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
