mod caches;
mod frame;
mod guards;
mod runtime;

use crate::val::Val;
use crate::vm::alloc::RegionAllocator;

use caches::{AccessIc, CallIc, ForRangeState, GlobalEntry, IndexIc, PackedHotEntry};
use frame::CallFrameMeta;
pub(crate) use frame::FrameInfo;
pub(crate) use guards::with_current_vm;
#[cfg(test)]
pub(crate) use guards::with_current_vm_ctx;

/// Minimal VM loop that can execute the placeholder Function produced by the stub compiler.
/// Reuses an internal register vector across executions to reduce allocations.
pub struct Vm {
    regs: Vec<Val>,
    reg_pool: Vec<Vec<Val>>,
    reg_stack: Vec<Vec<Val>>,
    access_ic: Vec<Option<AccessIc>>,
    index_ic: Vec<Option<IndexIc>>,
    global_ic: Vec<Option<GlobalEntry>>,
    call_ic: Vec<Option<CallIc>>,
    for_range_ic: Vec<Option<ForRangeState>>,
    packed_hot_ic: Vec<Option<PackedHotEntry>>,
    packed_hot_ic_key: usize,
    frames: Vec<CallFrameMeta>,
    pending_resume_pc: Option<usize>,
    region_alloc: RegionAllocator,
}

impl Vm {
    pub fn new() -> Self {
        Self {
            regs: Vec::new(),
            reg_pool: Vec::new(),
            reg_stack: Vec::new(),
            access_ic: Vec::new(),
            index_ic: Vec::new(),
            global_ic: Vec::new(),
            call_ic: Vec::new(),
            for_range_ic: Vec::new(),
            packed_hot_ic: Vec::new(),
            packed_hot_ic_key: 0,
            frames: Vec::new(),
            pending_resume_pc: None,
            region_alloc: RegionAllocator::new(),
        }
    }

    pub(in crate::vm::vm) fn update_top_caller_window(&mut self, window: frame::RegisterWindowRef) {
        if let Some(frame) = self.frames.last_mut() {
            frame.caller_window = window;
        }
    }

    pub(super) fn read_reg(&self, window: frame::RegisterWindowRef, idx: usize) -> Option<&Val> {
        match window {
            frame::RegisterWindowRef::Current => self.regs.get(idx),
            frame::RegisterWindowRef::StackIndex(stack_idx) => {
                self.reg_stack.get(stack_idx).and_then(|regs| regs.get(idx))
            }
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
