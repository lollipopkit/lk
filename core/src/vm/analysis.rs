use std::sync::Arc;
#[cfg(all(not(test), feature = "vm-profile"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::val::{LiteralVal, Type};
use crate::vm::alloc::RegionPlan;
use crate::vm::ssa::SsaFunction;

/// Classification of how a value escapes during execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EscapeClass {
    /// Value is compile-time constant or otherwise trivially confined.
    #[default]
    Trivial,
    /// Value remains within the current stack frame but may outlive temporaries.
    Local,
    /// Value may escape the current frame (stored into heap, captured, or returned).
    Escapes,
}

impl EscapeClass {
    pub fn is_escaping(self) -> bool {
        matches!(self, EscapeClass::Escapes)
    }

    pub fn join(self, other: EscapeClass) -> EscapeClass {
        use EscapeClass::*;
        match (self, other) {
            (Escapes, _) | (_, Escapes) => Escapes,
            (Local, _) | (_, Local) => Local,
            _ => Trivial,
        }
    }
}

/// Summary of escape behaviour for the current SSA function.
#[derive(Debug, Clone, Default)]
pub struct EscapeSummary {
    pub return_class: EscapeClass,
    /// SSA values that were classified as escaping.
    pub escaping_values: Vec<usize>,
}

impl EscapeSummary {
    pub fn mark_escaping(&mut self, value: usize) {
        if !self.escaping_values.contains(&value) {
            self.escaping_values.push(value);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PerfValueKind {
    #[default]
    Unknown,
    Nil,
    Bool,
    Int,
    Float,
    String,
    List,
    Map,
    Object,
}

impl PerfValueKind {
    pub fn join(self, other: Self) -> Self {
        if self == other { self } else { Self::Unknown }
    }

    pub fn from_type(ty: &Type) -> Self {
        match ty {
            Type::Nil => Self::Nil,
            Type::Bool => Self::Bool,
            Type::Int => Self::Int,
            Type::Float => Self::Float,
            Type::String => Self::String,
            Type::List(_) => Self::List,
            Type::Map(_, _) => Self::Map,
            Type::Named(_) => Self::Object,
            Type::Optional(inner) => Self::from_type(inner).join(Self::Nil),
            _ => Self::Unknown,
        }
    }

    pub fn from_literal(value: &LiteralVal) -> Self {
        match value {
            LiteralVal::Nil => Self::Nil,
            LiteralVal::Bool(_) => Self::Bool,
            LiteralVal::Int(_) => Self::Int,
            LiteralVal::Float(_) => Self::Float,
            value if value.as_str().is_some() => Self::String,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfContainerFact {
    pub value_kind: PerfValueKind,
    pub known_len: Option<usize>,
    pub adoptable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerfValueFact {
    pub kind: PerfValueKind,
    pub escape: EscapeClass,
    pub move_preferred: bool,
    pub must_clone: bool,
}

impl Default for PerfValueFact {
    fn default() -> Self {
        Self {
            kind: PerfValueKind::Unknown,
            escape: EscapeClass::Trivial,
            move_preferred: false,
            must_clone: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfRegisterFact {
    pub value: PerfValueFact,
    pub list: Option<PerfContainerFact>,
    pub map: Option<PerfContainerFact>,
    pub callable: PerfCallTargetKind,
    pub live_after: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PerformanceFacts {
    pub values: Vec<PerfValueFact>,
    pub value_lists: Vec<Option<PerfContainerFact>>,
    pub value_maps: Vec<Option<PerfContainerFact>>,
    pub registers: Vec<Option<PerfRegisterFact>>,
    pub local_slots: Vec<bool>,
    pub key_ops: Vec<Option<PerfKeyFact>>,
    pub index_ops: Vec<Option<PerfIndexFact>>,
    pub call_sites: Vec<Option<PerfCallFact>>,
    pub global_ops: Vec<Option<PerfGlobalFact>>,
    pub dead_writes: Vec<bool>,
    pub register_copies: Vec<Option<PerfRegisterCopyFact>>,
    pub local_copies: Vec<Option<PerfLocalCopyFact>>,
    pub container_moves: Vec<Option<PerfContainerMoveFact>>,
    pub container_builds: Vec<Option<PerfContainerBuildFact>>,
    pub cell_moves: Vec<Option<PerfCellMoveFact>>,
    pub for_loops: Vec<Option<PerfForLoopFact>>,
    pub control_flow: PerfControlFlowFacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfLocalCopyFact {
    pub move_source: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfKeyFact {
    pub const_key: Option<u16>,
    pub string_int: Option<PerfStringIntKeyFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfStringIntKeyFact {
    pub prefix_key: u16,
    pub suffix_reg: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfIndexFact {
    pub target_kind: PerfIndexTargetKind,
    pub value_kind: PerfValueKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PerfIndexTargetKind {
    #[default]
    Unknown,
    List,
    Map,
    Object,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfCallFact {
    pub call_base: u16,
    pub positional_count: u16,
    pub named_count: u16,
    pub target_kind: PerfCallTargetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PerfCallTargetKind {
    #[default]
    Unknown,
    Closure,
    Native,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfGlobalFact {
    pub slot: u16,
    pub move_source: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfRegisterCopyFact {
    pub move_source: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfContainerMoveFact {
    pub move_key: bool,
    pub move_value: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfContainerBuildFact {
    pub move_keys: bool,
    pub move_values: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfCellMoveFact {
    pub move_value: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfForLoopFact {
    pub jump_offset: i32,
    pub inclusive: bool,
    pub positive_step: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfFusedBoolBranchFact {
    pub result_reg: u8,
    pub jump_when: bool,
    pub jump_offset: i32,
    pub jump_base_pc_delta: usize,
    pub fallthrough_pc_delta: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PerfControlFlowFacts {
    pub block_ids: Vec<u32>,
    pub branch_targets: Vec<bool>,
    pub fused_bool_branches: Vec<Option<PerfFusedBoolBranchFact>>,
}

impl PerformanceFacts {
    pub fn value(&self, value_id: usize) -> Option<&PerfValueFact> {
        self.values.get(value_id)
    }

    pub fn value_list(&self, value_id: usize) -> Option<&PerfContainerFact> {
        self.value_lists.get(value_id).and_then(Option::as_ref)
    }

    pub fn value_map(&self, value_id: usize) -> Option<&PerfContainerFact> {
        self.value_maps.get(value_id).and_then(Option::as_ref)
    }

    pub fn register(&self, reg: u16) -> Option<&PerfRegisterFact> {
        self.registers.get(reg as usize).and_then(Option::as_ref)
    }

    pub fn is_local_slot(&self, reg: u16) -> bool {
        self.local_slots.get(reg as usize).copied().unwrap_or(false)
    }

    pub fn local_copy(&self, pc: usize) -> Option<&PerfLocalCopyFact> {
        self.local_copies.get(pc).and_then(Option::as_ref)
    }

    pub fn register_copy(&self, pc: usize) -> Option<&PerfRegisterCopyFact> {
        self.register_copies.get(pc).and_then(Option::as_ref)
    }

    pub fn is_dead_write(&self, pc: usize) -> bool {
        self.dead_writes.get(pc).copied().unwrap_or(false)
    }

    pub fn container_move(&self, pc: usize) -> Option<&PerfContainerMoveFact> {
        self.container_moves.get(pc).and_then(Option::as_ref)
    }

    pub fn container_build(&self, pc: usize) -> Option<&PerfContainerBuildFact> {
        self.container_builds.get(pc).and_then(Option::as_ref)
    }

    pub fn cell_move(&self, pc: usize) -> Option<&PerfCellMoveFact> {
        self.cell_moves.get(pc).and_then(Option::as_ref)
    }

    pub fn for_loop(&self, pc: usize) -> Option<&PerfForLoopFact> {
        self.for_loops.get(pc).and_then(Option::as_ref)
    }

    pub fn call_site(&self, pc: usize) -> Option<&PerfCallFact> {
        self.call_sites.get(pc).and_then(Option::as_ref)
    }

    pub fn index_op(&self, pc: usize) -> Option<&PerfIndexFact> {
        self.index_ops.get(pc).and_then(Option::as_ref)
    }

    pub fn global_op(&self, pc: usize) -> Option<&PerfGlobalFact> {
        self.global_ops.get(pc).and_then(Option::as_ref)
    }

    pub fn callable_kind(&self, reg: u16) -> PerfCallTargetKind {
        self.register(reg)
            .map(|fact| fact.callable)
            .unwrap_or(PerfCallTargetKind::Unknown)
    }

    pub fn block_id(&self, pc: usize) -> Option<u32> {
        self.control_flow.block_ids.get(pc).copied()
    }

    pub fn is_branch_target(&self, pc: usize) -> bool {
        self.control_flow.branch_targets.get(pc).copied().unwrap_or(false)
    }

    pub fn fused_bool_branch(&self, pc: usize) -> Option<PerfFusedBoolBranchFact> {
        self.control_flow.fused_bool_branches.get(pc).copied().flatten()
    }

    pub fn has_control_flow_fact_slot(&self, pc: usize) -> bool {
        self.control_flow.fused_bool_branches.get(pc).is_some()
    }

    pub fn same_block(&self, a: usize, b: usize) -> bool {
        self.block_id(a).is_some_and(|block| self.block_id(b) == Some(block))
    }

    pub fn mark_local_slot(&mut self, reg: u16) {
        let idx = reg as usize;
        if self.local_slots.len() <= idx {
            self.local_slots.resize(idx + 1, false);
        }
        self.local_slots[idx] = true;
    }

    pub fn set_local_copy_fact(&mut self, pc: usize, fact: PerfLocalCopyFact) {
        if self.local_copies.len() <= pc {
            self.local_copies.resize_with(pc + 1, Option::default);
        }
        self.local_copies[pc] = Some(fact);
    }

    pub fn set_key_fact(&mut self, pc: usize, fact: PerfKeyFact) {
        if self.key_ops.len() <= pc {
            self.key_ops.resize_with(pc + 1, Option::default);
        }
        self.key_ops[pc] = Some(fact);
    }

    pub fn set_index_fact(&mut self, pc: usize, fact: PerfIndexFact) {
        if self.index_ops.len() <= pc {
            self.index_ops.resize_with(pc + 1, Option::default);
        }
        self.index_ops[pc] = Some(fact);
    }

    pub fn set_call_fact(&mut self, pc: usize, fact: PerfCallFact) {
        if self.call_sites.len() <= pc {
            self.call_sites.resize_with(pc + 1, Option::default);
        }
        self.call_sites[pc] = Some(fact);
    }

    pub fn set_global_fact(&mut self, pc: usize, fact: PerfGlobalFact) {
        if self.global_ops.len() <= pc {
            self.global_ops.resize_with(pc + 1, Option::default);
        }
        self.global_ops[pc] = Some(fact);
    }

    pub fn set_register_copy_fact(&mut self, pc: usize, fact: PerfRegisterCopyFact) {
        if self.register_copies.len() <= pc {
            self.register_copies.resize_with(pc + 1, Option::default);
        }
        self.register_copies[pc] = Some(fact);
    }

    pub fn set_dead_write_fact(&mut self, pc: usize) {
        if self.dead_writes.len() <= pc {
            self.dead_writes.resize(pc + 1, false);
        }
        self.dead_writes[pc] = true;
    }

    pub fn set_container_move_fact(&mut self, pc: usize, fact: PerfContainerMoveFact) {
        if self.container_moves.len() <= pc {
            self.container_moves.resize_with(pc + 1, Option::default);
        }
        self.container_moves[pc] = Some(fact);
    }

    pub fn set_container_build_fact(&mut self, pc: usize, fact: PerfContainerBuildFact) {
        if self.container_builds.len() <= pc {
            self.container_builds.resize_with(pc + 1, Option::default);
        }
        self.container_builds[pc] = Some(fact);
    }

    pub fn set_cell_move_fact(&mut self, pc: usize, fact: PerfCellMoveFact) {
        if self.cell_moves.len() <= pc {
            self.cell_moves.resize_with(pc + 1, Option::default);
        }
        self.cell_moves[pc] = Some(fact);
    }

    pub fn set_for_loop_fact(&mut self, pc: usize, fact: PerfForLoopFact) {
        if self.for_loops.len() <= pc {
            self.for_loops.resize_with(pc + 1, Option::default);
        }
        self.for_loops[pc] = Some(fact);
    }

    pub fn set_control_flow_facts(&mut self, control_flow: PerfControlFlowFacts) {
        self.control_flow = control_flow;
    }

    pub fn set_value_kind(&mut self, value_id: usize, kind: PerfValueKind) {
        self.ensure_value(value_id);
        self.values[value_id].kind = kind;
    }

    pub fn set_value_list_fact(&mut self, value_id: usize, fact: PerfContainerFact) {
        self.ensure_value(value_id);
        if self.value_lists.len() <= value_id {
            self.value_lists.resize_with(value_id + 1, Option::default);
        }
        self.value_lists[value_id] = Some(fact);
    }

    pub fn set_value_map_fact(&mut self, value_id: usize, fact: PerfContainerFact) {
        self.ensure_value(value_id);
        if self.value_maps.len() <= value_id {
            self.value_maps.resize_with(value_id + 1, Option::default);
        }
        self.value_maps[value_id] = Some(fact);
    }

    pub fn set_register_kind(&mut self, reg: u16, kind: PerfValueKind) {
        let fact = self.ensure_register(reg);
        fact.value.kind = kind;
    }

    pub fn set_register_fact(&mut self, reg: u16, fact: PerfRegisterFact) {
        let idx = reg as usize;
        self.ensure_register_len(idx);
        self.registers[idx] = Some(fact);
    }

    pub fn copy_register_fact(&mut self, dst: u16, src: u16) {
        let src_fact = self.register(src).copied();
        let dst_idx = dst as usize;
        self.ensure_register_len(dst_idx);
        self.registers[dst_idx] = src_fact;
    }

    pub fn clear_register(&mut self, reg: u16) {
        let idx = reg as usize;
        if let Some(slot) = self.registers.get_mut(idx) {
            *slot = None;
        }
    }

    pub fn set_register_live_after(&mut self, reg: u16, live_after: bool) {
        if live_after {
            self.ensure_register(reg).live_after = true;
        } else if let Some(Some(fact)) = self.registers.get_mut(reg as usize) {
            fact.live_after = false;
        }
    }

    pub fn ensure_value(&mut self, value_id: usize) {
        if self.values.len() <= value_id {
            self.values.resize_with(value_id + 1, PerfValueFact::default);
        }
        if self.value_lists.len() <= value_id {
            self.value_lists.resize_with(value_id + 1, Option::default);
        }
        if self.value_maps.len() <= value_id {
            self.value_maps.resize_with(value_id + 1, Option::default);
        }
    }

    fn ensure_register(&mut self, reg: u16) -> &mut PerfRegisterFact {
        let idx = reg as usize;
        self.ensure_register_len(idx);
        if self.registers[idx].is_none() {
            self.registers[idx] = Some(PerfRegisterFact::default());
        }
        self.registers[idx].as_mut().expect("register fact just initialized")
    }

    fn ensure_register_len(&mut self, idx: usize) {
        if self.registers.len() <= idx {
            self.registers.resize_with(idx + 1, Option::default);
        }
    }
}

/// Aggregated analysis artifacts produced by the SSA pipeline.
#[derive(Debug, Clone, Default)]
pub struct FunctionAnalysis {
    pub ssa: Option<SsaFunction>,
    pub escape: EscapeSummary,
    pub region_plan: Arc<RegionPlan>,
    pub perf: PerformanceFacts,
}

// ---------------------------------------------------------------------------
// Runtime metrics — three-way cfg:
//   1. #[cfg(test)]                                    → always-on, thread-local
//   2. #[cfg(all(not(test), feature = "vm-profile"))]  → atomic counters, AtomicBool gate
//   3. #[cfg(all(not(test), not(feature = "vm-profile")))]  → compile-time no-ops
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmRuntimeMetrics {
    pub opcode_steps: u64,
    pub copy_policy_heap_clones: u64,
    pub register_copy_heap_clones: u64,
    pub local_copy_heap_clones: u64,
    pub local_load_heap_clones: u64,
    pub local_store_heap_clones: u64,
    pub const_load_heap_clones: u64,
    pub call_arg_heap_clones: u64,
    pub container_copy_heap_clones: u64,
    pub register_writes: u64,
    pub return_value_moves: u64,
    pub branch_ops: u64,
    pub typed_branch_ops: u64,
    pub call_ops: u64,
    pub native_call_ops: u64,
    pub closure_call_ops: u64,
    pub exact_call_ops: u64,
    pub named_call_ops: u64,
    pub method_call_ops: u64,
    pub container_ops: u64,
    pub list_ops: u64,
    pub map_ops: u64,
    pub string_ops: u64,
    pub opcode_histogram: [u64; VM_OPCODE_COUNT],
    pub register_write_sources: [u64; VM_REGISTER_WRITE_SOURCE_COUNT],
    pub index_key_metrics: [u64; VM_INDEX_KEY_METRIC_COUNT],
}

pub const VM_OPCODE_COUNT: usize = 128;
pub const VM_REGISTER_WRITE_SOURCE_COUNT: usize = 10;
pub const VM_INDEX_KEY_METRIC_COUNT: usize = 8;
pub const VM_REGISTER_WRITE_SOURCE_NAMES: [&str; VM_REGISTER_WRITE_SOURCE_COUNT] = [
    "move",
    "const_load",
    "arithmetic",
    "compare",
    "container",
    "index",
    "call_return",
    "global",
    "string",
    "other",
];
pub const VM_INDEX_KEY_METRIC_NAMES: [&str; VM_INDEX_KEY_METRIC_COUNT] = [
    "known_string_key",
    "dynamic_register_key",
    "runtime_map_key",
    "direct_string_key",
    "typed_map_direct",
    "generic_map_lookup",
    "object_key",
    "slow_path",
];

#[derive(Debug, Clone, Copy)]
pub(crate) enum VmIndexKeyMetric {
    KnownStringKey,
    DynamicRegisterKey,
    RuntimeMapKey,
    DirectStringKey,
    TypedMapDirect,
    GenericMapLookup,
    ObjectKey,
    SlowPath,
}

impl VmIndexKeyMetric {
    #[inline]
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::KnownStringKey => 0,
            Self::DynamicRegisterKey => 1,
            Self::RuntimeMapKey => 2,
            Self::DirectStringKey => 3,
            Self::TypedMapDirect => 4,
            Self::GenericMapLookup => 5,
            Self::ObjectKey => 6,
            Self::SlowPath => 7,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum VmRegisterWriteSource {
    Move,
    ConstLoad,
    Arithmetic,
    Compare,
    Container,
    Index,
    CallReturn,
    Global,
    String,
    Other,
}

impl VmRegisterWriteSource {
    #[inline]
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Move => 0,
            Self::ConstLoad => 1,
            Self::Arithmetic => 2,
            Self::Compare => 3,
            Self::Container => 4,
            Self::Index => 5,
            Self::CallReturn => 6,
            Self::Global => 7,
            Self::String => 8,
            Self::Other => 9,
        }
    }
}

impl Default for VmRuntimeMetrics {
    fn default() -> Self {
        Self {
            opcode_steps: 0,
            copy_policy_heap_clones: 0,
            register_copy_heap_clones: 0,
            local_copy_heap_clones: 0,
            local_load_heap_clones: 0,
            local_store_heap_clones: 0,
            const_load_heap_clones: 0,
            call_arg_heap_clones: 0,
            container_copy_heap_clones: 0,
            register_writes: 0,
            return_value_moves: 0,
            branch_ops: 0,
            typed_branch_ops: 0,
            call_ops: 0,
            native_call_ops: 0,
            closure_call_ops: 0,
            exact_call_ops: 0,
            named_call_ops: 0,
            method_call_ops: 0,
            container_ops: 0,
            list_ops: 0,
            map_ops: 0,
            string_ops: 0,
            opcode_histogram: [0; VM_OPCODE_COUNT],
            register_write_sources: [0; VM_REGISTER_WRITE_SOURCE_COUNT],
            index_key_metrics: [0; VM_INDEX_KEY_METRIC_COUNT],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum VmCallMetric {
    Generic,
    Native,
    Closure,
    Exact,
    Named,
    Method,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum VmContainerMetric {
    Generic,
    List,
    Map,
    String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum VmValueCopyMetric {
    Generic,
    Register,
    LocalLoad,
    LocalStore,
    ConstLoad,
    CallArg,
    Container,
}

// ==================== test mode ====================
#[cfg(test)]
impl VmRuntimeMetrics {
    const ZERO: Self = Self {
        opcode_steps: 0,
        copy_policy_heap_clones: 0,
        register_copy_heap_clones: 0,
        local_copy_heap_clones: 0,
        local_load_heap_clones: 0,
        local_store_heap_clones: 0,
        const_load_heap_clones: 0,
        call_arg_heap_clones: 0,
        container_copy_heap_clones: 0,
        register_writes: 0,
        return_value_moves: 0,
        branch_ops: 0,
        typed_branch_ops: 0,
        call_ops: 0,
        native_call_ops: 0,
        closure_call_ops: 0,
        exact_call_ops: 0,
        named_call_ops: 0,
        method_call_ops: 0,
        container_ops: 0,
        list_ops: 0,
        map_ops: 0,
        string_ops: 0,
        opcode_histogram: [0; VM_OPCODE_COUNT],
        register_write_sources: [0; VM_REGISTER_WRITE_SOURCE_COUNT],
        index_key_metrics: [0; VM_INDEX_KEY_METRIC_COUNT],
    };
}

#[cfg(test)]
thread_local! {
    static THREAD_RUNTIME_METRICS: std::cell::Cell<VmRuntimeMetrics> =
        const { std::cell::Cell::new(VmRuntimeMetrics::ZERO) };
}

#[cfg(test)]
#[inline]
fn update_thread_runtime_metrics(update: impl FnOnce(&mut VmRuntimeMetrics)) {
    THREAD_RUNTIME_METRICS.with(|cell| {
        let mut metrics = cell.get();
        update(&mut metrics);
        cell.set(metrics);
    });
}

#[cfg(test)]
#[inline]
pub fn vm_runtime_metrics_enabled() -> bool {
    true
}

#[cfg(test)]
#[inline]
pub(crate) fn record_opcode_step_known_enabled() {
    update_thread_runtime_metrics(|metrics| {
        metrics.opcode_steps += 1;
    });
}

#[cfg(test)]
#[inline]
pub(crate) fn record_opcode_histogram_batch(histogram: &[u64; VM_OPCODE_COUNT]) {
    update_thread_runtime_metrics(|metrics| {
        for (dst, count) in metrics.opcode_histogram.iter_mut().zip(histogram.iter()) {
            *dst += count;
        }
    });
}

#[cfg(test)]
#[inline]
pub(crate) fn record_register_write_sources_batch(sources: &[u64; VM_REGISTER_WRITE_SOURCE_COUNT]) {
    update_thread_runtime_metrics(|metrics| {
        for (dst, count) in metrics.register_write_sources.iter_mut().zip(sources.iter()) {
            *dst += count;
        }
    });
}

#[cfg(test)]
#[inline]
pub(crate) fn record_index_key_metrics_batch(sources: &[u64; VM_INDEX_KEY_METRIC_COUNT]) {
    update_thread_runtime_metrics(|metrics| {
        for (dst, count) in metrics.index_key_metrics.iter_mut().zip(sources.iter()) {
            *dst += count;
        }
    });
}

#[cfg(test)]
#[inline]
pub(crate) fn record_copy_policy_clone(kind: VmValueCopyMetric, heap_backed: bool) {
    if !heap_backed {
        return;
    }
    update_thread_runtime_metrics(|metrics| {
        metrics.copy_policy_heap_clones += 1;
        match kind {
            VmValueCopyMetric::Generic => {}
            VmValueCopyMetric::Register => metrics.register_copy_heap_clones += 1,
            VmValueCopyMetric::LocalLoad => {
                metrics.local_copy_heap_clones += 1;
                metrics.local_load_heap_clones += 1;
            }
            VmValueCopyMetric::LocalStore => {
                metrics.local_copy_heap_clones += 1;
                metrics.local_store_heap_clones += 1;
            }
            VmValueCopyMetric::ConstLoad => metrics.const_load_heap_clones += 1,
            VmValueCopyMetric::CallArg => metrics.call_arg_heap_clones += 1,
            VmValueCopyMetric::Container => metrics.container_copy_heap_clones += 1,
        }
    });
}

#[cfg(test)]
#[inline]
pub(crate) fn record_register_write_known_enabled() {
    update_thread_runtime_metrics(|metrics| metrics.register_writes += 1);
}

#[cfg(test)]
#[inline]
pub(crate) fn record_register_write() {
    update_thread_runtime_metrics(|metrics| metrics.register_writes += 1);
}

#[cfg(test)]
#[inline]
pub(crate) fn record_return_value_move() {
    update_thread_runtime_metrics(|metrics| metrics.return_value_moves += 1);
}

#[cfg(test)]
#[inline]
pub(crate) fn record_branch_op_known_enabled(typed: bool) {
    update_thread_runtime_metrics(|metrics| {
        metrics.branch_ops += 1;
        if typed {
            metrics.typed_branch_ops += 1;
        }
    });
}

#[cfg(test)]
#[inline]
pub(crate) fn record_call_op_known_enabled(kind: VmCallMetric) {
    update_thread_runtime_metrics(|metrics| {
        metrics.call_ops += 1;
        match kind {
            VmCallMetric::Generic => {}
            VmCallMetric::Native => metrics.native_call_ops += 1,
            VmCallMetric::Closure => metrics.closure_call_ops += 1,
            VmCallMetric::Exact => metrics.exact_call_ops += 1,
            VmCallMetric::Named => metrics.named_call_ops += 1,
            VmCallMetric::Method => metrics.method_call_ops += 1,
        }
    });
}

#[cfg(test)]
#[inline]
pub(crate) fn record_container_op_known_enabled(kind: VmContainerMetric) {
    update_thread_runtime_metrics(|metrics| {
        metrics.container_ops += 1;
        match kind {
            VmContainerMetric::Generic => {}
            VmContainerMetric::List => metrics.list_ops += 1,
            VmContainerMetric::Map => metrics.map_ops += 1,
            VmContainerMetric::String => metrics.string_ops += 1,
        }
    });
}

#[cfg(test)]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    THREAD_RUNTIME_METRICS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub fn vm_runtime_metrics_reset() {
    THREAD_RUNTIME_METRICS.with(|cell| cell.set(VmRuntimeMetrics::default()));
}

// ==================== vm-profile mode (atomic counters) ====================
#[cfg(all(not(test), feature = "vm-profile"))]
static COPY_POLICY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static REGISTER_COPY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static LOCAL_COPY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static LOCAL_LOAD_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static LOCAL_STORE_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static CONST_LOAD_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static CALL_ARG_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static CONTAINER_COPY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static REGISTER_WRITES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static RETURN_VALUE_MOVES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static OPCODE_STEPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static BRANCH_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static OPCODE_HISTOGRAM: [AtomicU64; VM_OPCODE_COUNT] = [const { AtomicU64::new(0) }; VM_OPCODE_COUNT];
#[cfg(all(not(test), feature = "vm-profile"))]
static REGISTER_WRITE_SOURCES: [AtomicU64; VM_REGISTER_WRITE_SOURCE_COUNT] =
    [const { AtomicU64::new(0) }; VM_REGISTER_WRITE_SOURCE_COUNT];
#[cfg(all(not(test), feature = "vm-profile"))]
static INDEX_KEY_METRICS: [AtomicU64; VM_INDEX_KEY_METRIC_COUNT] =
    [const { AtomicU64::new(0) }; VM_INDEX_KEY_METRIC_COUNT];
#[cfg(all(not(test), feature = "vm-profile"))]
static TYPED_BRANCH_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static NATIVE_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static CLOSURE_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static EXACT_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static NAMED_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static METHOD_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static CONTAINER_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static LIST_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static MAP_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static STRING_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), feature = "vm-profile"))]
static RUNTIME_METRICS_ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
fn runtime_metrics_enabled() -> bool {
    RUNTIME_METRICS_ENABLED.load(Ordering::Relaxed)
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub fn vm_runtime_metrics_enabled() -> bool {
    runtime_metrics_enabled()
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline(always)]
fn increment(counter: &AtomicU64) {
    if runtime_metrics_enabled() {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline(always)]
pub(crate) fn record_opcode_step_known_enabled() {
    OPCODE_STEPS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_opcode_histogram_batch(histogram: &[u64; VM_OPCODE_COUNT]) {
    for (counter, count) in OPCODE_HISTOGRAM.iter().zip(histogram.iter()) {
        if *count != 0 {
            counter.fetch_add(*count, Ordering::Relaxed);
        }
    }
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_register_write_sources_batch(sources: &[u64; VM_REGISTER_WRITE_SOURCE_COUNT]) {
    for (counter, count) in REGISTER_WRITE_SOURCES.iter().zip(sources.iter()) {
        if *count != 0 {
            counter.fetch_add(*count, Ordering::Relaxed);
        }
    }
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_index_key_metrics_batch(sources: &[u64; VM_INDEX_KEY_METRIC_COUNT]) {
    for (counter, count) in INDEX_KEY_METRICS.iter().zip(sources.iter()) {
        if *count != 0 {
            counter.fetch_add(*count, Ordering::Relaxed);
        }
    }
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_copy_policy_clone(kind: VmValueCopyMetric, heap_backed: bool) {
    if !heap_backed || !runtime_metrics_enabled() {
        return;
    }
    COPY_POLICY_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
    match kind {
        VmValueCopyMetric::Generic => {}
        VmValueCopyMetric::Register => {
            REGISTER_COPY_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
        }
        VmValueCopyMetric::LocalLoad => {
            LOCAL_COPY_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
            LOCAL_LOAD_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
        }
        VmValueCopyMetric::LocalStore => {
            LOCAL_COPY_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
            LOCAL_STORE_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
        }
        VmValueCopyMetric::ConstLoad => {
            CONST_LOAD_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
        }
        VmValueCopyMetric::CallArg => {
            CALL_ARG_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
        }
        VmValueCopyMetric::Container => {
            CONTAINER_COPY_HEAP_CLONES.fetch_add(1, Ordering::Relaxed);
        }
    };
}

/// Known-enabled variant: caller has already checked `collect_metrics`,
/// so this unconditionally increments the counter without reading the
/// global metrics gate atomically.
#[cfg(all(not(test), feature = "vm-profile"))]
#[inline(always)]
pub(crate) fn record_register_write_known_enabled() {
    REGISTER_WRITES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_register_write() {
    increment(&REGISTER_WRITES);
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_return_value_move() {
    increment(&RETURN_VALUE_MOVES);
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_branch_op_known_enabled(typed: bool) {
    BRANCH_OPS.fetch_add(1, Ordering::Relaxed);
    if typed {
        TYPED_BRANCH_OPS.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_call_op_known_enabled(kind: VmCallMetric) {
    CALL_OPS.fetch_add(1, Ordering::Relaxed);
    match kind {
        VmCallMetric::Generic => {}
        VmCallMetric::Native => {
            NATIVE_CALL_OPS.fetch_add(1, Ordering::Relaxed);
        }
        VmCallMetric::Closure => {
            CLOSURE_CALL_OPS.fetch_add(1, Ordering::Relaxed);
        }
        VmCallMetric::Exact => {
            EXACT_CALL_OPS.fetch_add(1, Ordering::Relaxed);
        }
        VmCallMetric::Named => {
            NAMED_CALL_OPS.fetch_add(1, Ordering::Relaxed);
        }
        VmCallMetric::Method => {
            METHOD_CALL_OPS.fetch_add(1, Ordering::Relaxed);
        }
    };
}

#[cfg(all(not(test), feature = "vm-profile"))]
#[inline]
pub(crate) fn record_container_op_known_enabled(kind: VmContainerMetric) {
    CONTAINER_OPS.fetch_add(1, Ordering::Relaxed);
    match kind {
        VmContainerMetric::Generic => {}
        VmContainerMetric::List => {
            LIST_OPS.fetch_add(1, Ordering::Relaxed);
        }
        VmContainerMetric::Map => {
            MAP_OPS.fetch_add(1, Ordering::Relaxed);
        }
        VmContainerMetric::String => {
            STRING_OPS.fetch_add(1, Ordering::Relaxed);
        }
    };
}

#[cfg(all(not(test), feature = "vm-profile"))]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    let mut opcode_histogram = [0; VM_OPCODE_COUNT];
    for (dst, counter) in opcode_histogram.iter_mut().zip(OPCODE_HISTOGRAM.iter()) {
        *dst = counter.load(Ordering::Relaxed);
    }
    let mut register_write_sources = [0; VM_REGISTER_WRITE_SOURCE_COUNT];
    for (dst, counter) in register_write_sources.iter_mut().zip(REGISTER_WRITE_SOURCES.iter()) {
        *dst = counter.load(Ordering::Relaxed);
    }
    let mut index_key_metrics = [0; VM_INDEX_KEY_METRIC_COUNT];
    for (dst, counter) in index_key_metrics.iter_mut().zip(INDEX_KEY_METRICS.iter()) {
        *dst = counter.load(Ordering::Relaxed);
    }

    VmRuntimeMetrics {
        opcode_steps: OPCODE_STEPS.load(Ordering::Relaxed),
        copy_policy_heap_clones: COPY_POLICY_HEAP_CLONES.load(Ordering::Relaxed),
        register_copy_heap_clones: REGISTER_COPY_HEAP_CLONES.load(Ordering::Relaxed),
        local_copy_heap_clones: LOCAL_COPY_HEAP_CLONES.load(Ordering::Relaxed),
        local_load_heap_clones: LOCAL_LOAD_HEAP_CLONES.load(Ordering::Relaxed),
        local_store_heap_clones: LOCAL_STORE_HEAP_CLONES.load(Ordering::Relaxed),
        const_load_heap_clones: CONST_LOAD_HEAP_CLONES.load(Ordering::Relaxed),
        call_arg_heap_clones: CALL_ARG_HEAP_CLONES.load(Ordering::Relaxed),
        container_copy_heap_clones: CONTAINER_COPY_HEAP_CLONES.load(Ordering::Relaxed),
        register_writes: REGISTER_WRITES.load(Ordering::Relaxed),
        return_value_moves: RETURN_VALUE_MOVES.load(Ordering::Relaxed),
        branch_ops: BRANCH_OPS.load(Ordering::Relaxed),
        typed_branch_ops: TYPED_BRANCH_OPS.load(Ordering::Relaxed),
        call_ops: CALL_OPS.load(Ordering::Relaxed),
        native_call_ops: NATIVE_CALL_OPS.load(Ordering::Relaxed),
        closure_call_ops: CLOSURE_CALL_OPS.load(Ordering::Relaxed),
        exact_call_ops: EXACT_CALL_OPS.load(Ordering::Relaxed),
        named_call_ops: NAMED_CALL_OPS.load(Ordering::Relaxed),
        method_call_ops: METHOD_CALL_OPS.load(Ordering::Relaxed),
        container_ops: CONTAINER_OPS.load(Ordering::Relaxed),
        list_ops: LIST_OPS.load(Ordering::Relaxed),
        map_ops: MAP_OPS.load(Ordering::Relaxed),
        string_ops: STRING_OPS.load(Ordering::Relaxed),
        opcode_histogram,
        register_write_sources,
        index_key_metrics,
    }
}

#[cfg(all(not(test), feature = "vm-profile"))]
pub fn vm_runtime_metrics_reset() {
    RUNTIME_METRICS_ENABLED.store(true, Ordering::Relaxed);
    OPCODE_STEPS.store(0, Ordering::Relaxed);
    for counter in &OPCODE_HISTOGRAM {
        counter.store(0, Ordering::Relaxed);
    }
    for counter in &REGISTER_WRITE_SOURCES {
        counter.store(0, Ordering::Relaxed);
    }
    for counter in &INDEX_KEY_METRICS {
        counter.store(0, Ordering::Relaxed);
    }
    COPY_POLICY_HEAP_CLONES.store(0, Ordering::Relaxed);
    REGISTER_COPY_HEAP_CLONES.store(0, Ordering::Relaxed);
    LOCAL_COPY_HEAP_CLONES.store(0, Ordering::Relaxed);
    LOCAL_LOAD_HEAP_CLONES.store(0, Ordering::Relaxed);
    LOCAL_STORE_HEAP_CLONES.store(0, Ordering::Relaxed);
    CONST_LOAD_HEAP_CLONES.store(0, Ordering::Relaxed);
    CALL_ARG_HEAP_CLONES.store(0, Ordering::Relaxed);
    CONTAINER_COPY_HEAP_CLONES.store(0, Ordering::Relaxed);
    REGISTER_WRITES.store(0, Ordering::Relaxed);
    RETURN_VALUE_MOVES.store(0, Ordering::Relaxed);
    BRANCH_OPS.store(0, Ordering::Relaxed);
    TYPED_BRANCH_OPS.store(0, Ordering::Relaxed);
    CALL_OPS.store(0, Ordering::Relaxed);
    NATIVE_CALL_OPS.store(0, Ordering::Relaxed);
    CLOSURE_CALL_OPS.store(0, Ordering::Relaxed);
    EXACT_CALL_OPS.store(0, Ordering::Relaxed);
    NAMED_CALL_OPS.store(0, Ordering::Relaxed);
    METHOD_CALL_OPS.store(0, Ordering::Relaxed);
    CONTAINER_OPS.store(0, Ordering::Relaxed);
    LIST_OPS.store(0, Ordering::Relaxed);
    MAP_OPS.store(0, Ordering::Relaxed);
    STRING_OPS.store(0, Ordering::Relaxed);
}

// ==================== no-profile mode (compile-time no-ops) ====================
#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub fn vm_runtime_metrics_enabled() -> bool {
    false
}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_opcode_step_known_enabled() {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_opcode_histogram_batch(_histogram: &[u64; VM_OPCODE_COUNT]) {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_register_write_sources_batch(_sources: &[u64; VM_REGISTER_WRITE_SOURCE_COUNT]) {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_index_key_metrics_batch(_sources: &[u64; VM_INDEX_KEY_METRIC_COUNT]) {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_copy_policy_clone(_kind: VmValueCopyMetric, _heap_backed: bool) {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_register_write_known_enabled() {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_register_write() {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_return_value_move() {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_branch_op_known_enabled(_typed: bool) {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_call_op_known_enabled(_kind: VmCallMetric) {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
#[inline(always)]
pub(crate) fn record_container_op_known_enabled(_kind: VmContainerMetric) {}

#[cfg(all(not(test), not(feature = "vm-profile")))]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    VmRuntimeMetrics::default()
}

#[cfg(all(not(test), not(feature = "vm-profile")))]
pub fn vm_runtime_metrics_reset() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perf_value_kind_from_literal_classifies_ast_literals() {
        assert_eq!(PerfValueKind::from_literal(&LiteralVal::Nil), PerfValueKind::Nil);
        assert_eq!(
            PerfValueKind::from_literal(&LiteralVal::Bool(true)),
            PerfValueKind::Bool
        );
        assert_eq!(PerfValueKind::from_literal(&LiteralVal::Int(42)), PerfValueKind::Int);
        assert_eq!(
            PerfValueKind::from_literal(&LiteralVal::Float(1.5)),
            PerfValueKind::Float
        );
        assert_eq!(
            PerfValueKind::from_literal(&LiteralVal::from_str("lk")),
            PerfValueKind::String
        );
        assert_eq!(
            PerfValueKind::from_literal(&LiteralVal::from_str("longer-than-short")),
            PerfValueKind::String
        );
    }
}
