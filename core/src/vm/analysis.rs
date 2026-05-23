use std::sync::Arc;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::val::{Type, Val};
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
            Type::Optional(inner) => Self::from_type(inner).join(Self::Nil),
            _ => Self::Unknown,
        }
    }

    pub fn from_val(value: &Val) -> Self {
        match value {
            Val::Nil => Self::Nil,
            Val::Bool(_) => Self::Bool,
            Val::Int(_) => Self::Int,
            Val::Float(_) => Self::Float,
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
    pub live_after: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PerformanceFacts {
    pub values: Vec<PerfValueFact>,
    pub registers: Vec<Option<PerfRegisterFact>>,
    pub local_slots: Vec<bool>,
    pub key_ops: Vec<Option<PerfKeyFact>>,
    pub dead_writes: Vec<bool>,
    pub register_copies: Vec<Option<PerfRegisterCopyFact>>,
    pub local_copies: Vec<Option<PerfLocalCopyFact>>,
    pub container_moves: Vec<Option<PerfContainerMoveFact>>,
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
pub struct PerfRegisterCopyFact {
    pub move_source: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfContainerMoveFact {
    pub move_key: bool,
    pub move_value: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PerfControlFlowFacts {
    pub block_ids: Vec<u32>,
    pub branch_targets: Vec<bool>,
}

impl PerformanceFacts {
    pub fn value(&self, value_id: usize) -> Option<&PerfValueFact> {
        self.values.get(value_id)
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

    pub fn block_id(&self, pc: usize) -> Option<u32> {
        self.control_flow.block_ids.get(pc).copied()
    }

    pub fn is_branch_target(&self, pc: usize) -> bool {
        self.control_flow.branch_targets.get(pc).copied().unwrap_or(false)
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

    pub fn set_control_flow_facts(&mut self, control_flow: PerfControlFlowFacts) {
        self.control_flow = control_flow;
    }

    pub fn set_value_kind(&mut self, value_id: usize, kind: PerfValueKind) {
        self.ensure_value(value_id);
        self.values[value_id].kind = kind;
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VmRuntimeMetrics {
    pub opcode_steps: u64,
    pub val_clones: u64,
    pub immediate_val_clones: u64,
    pub heap_val_clones: u64,
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
}

#[cfg(test)]
impl VmRuntimeMetrics {
    const ZERO: Self = Self {
        opcode_steps: 0,
        val_clones: 0,
        immediate_val_clones: 0,
        heap_val_clones: 0,
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

#[cfg(not(test))]
static VAL_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static IMMEDIATE_VAL_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static HEAP_VAL_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static COPY_POLICY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static REGISTER_COPY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static LOCAL_COPY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static LOCAL_LOAD_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static LOCAL_STORE_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static CONST_LOAD_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static CALL_ARG_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static CONTAINER_COPY_HEAP_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static REGISTER_WRITES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static RETURN_VALUE_MOVES: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static OPCODE_STEPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static BRANCH_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static TYPED_BRANCH_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static NATIVE_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static CLOSURE_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static EXACT_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static NAMED_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static METHOD_CALL_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static CONTAINER_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static LIST_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static MAP_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static STRING_OPS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static RUNTIME_METRICS_ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(not(test))]
#[inline]
fn runtime_metrics_enabled() -> bool {
    RUNTIME_METRICS_ENABLED.load(Ordering::Relaxed)
}

#[cfg(not(test))]
#[inline]
pub(crate) fn vm_runtime_metrics_enabled() -> bool {
    runtime_metrics_enabled()
}

#[cfg(test)]
#[inline]
pub(crate) fn vm_runtime_metrics_enabled() -> bool {
    true
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

#[cfg(not(test))]
#[inline(always)]
fn increment(counter: &AtomicU64) {
    if runtime_metrics_enabled() {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[inline]
fn increment_thread(update: impl FnOnce(&mut VmRuntimeMetrics)) {
    update_thread_runtime_metrics(update);
}

#[cfg(not(test))]
#[cfg(not(test))]
#[inline(always)]
pub(crate) fn record_opcode_step_known_enabled() {
    OPCODE_STEPS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
#[inline]
pub(crate) fn record_opcode_step_known_enabled() {
    increment_thread(|metrics| metrics.opcode_steps += 1);
}

#[cfg(not(test))]
#[inline]
pub(crate) fn record_val_clone(heap_backed: bool) {
    if !runtime_metrics_enabled() {
        return;
    }
    VAL_CLONES.fetch_add(1, Ordering::Relaxed);
    if heap_backed {
        HEAP_VAL_CLONES.fetch_add(1, Ordering::Relaxed);
    } else {
        IMMEDIATE_VAL_CLONES.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[inline]
pub(crate) fn record_val_clone(heap_backed: bool) {
    update_thread_runtime_metrics(|metrics| {
        metrics.val_clones += 1;
        if heap_backed {
            metrics.heap_val_clones += 1;
        } else {
            metrics.immediate_val_clones += 1;
        }
    });
}

#[cfg(not(test))]
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

/// Known-enabled variant: caller has already checked `collect_metrics`,
/// so this unconditionally increments the counter without reading the
/// global metrics gate atomically.
#[cfg(not(test))]
#[inline(always)]
pub(crate) fn record_register_write_known_enabled() {
    REGISTER_WRITES.fetch_add(1, Ordering::Relaxed);
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

#[cfg(not(test))]
#[inline]
pub(crate) fn record_return_value_move() {
    increment(&RETURN_VALUE_MOVES);
}

#[cfg(test)]
#[inline]
pub(crate) fn record_return_value_move() {
    update_thread_runtime_metrics(|metrics| metrics.return_value_moves += 1);
}

#[cfg(not(test))]
#[inline]
pub(crate) fn record_branch_op_known_enabled(typed: bool) {
    BRANCH_OPS.fetch_add(1, Ordering::Relaxed);
    if typed {
        TYPED_BRANCH_OPS.fetch_add(1, Ordering::Relaxed);
    }
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

#[cfg(not(test))]
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

#[cfg(not(test))]
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

#[cfg(not(test))]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    VmRuntimeMetrics {
        opcode_steps: OPCODE_STEPS.load(Ordering::Relaxed),
        val_clones: VAL_CLONES.load(Ordering::Relaxed),
        immediate_val_clones: IMMEDIATE_VAL_CLONES.load(Ordering::Relaxed),
        heap_val_clones: HEAP_VAL_CLONES.load(Ordering::Relaxed),
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
    }
}

#[cfg(test)]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    THREAD_RUNTIME_METRICS.with(std::cell::Cell::get)
}

#[cfg(not(test))]
pub fn vm_runtime_metrics_reset() {
    RUNTIME_METRICS_ENABLED.store(true, Ordering::Relaxed);
    OPCODE_STEPS.store(0, Ordering::Relaxed);
    VAL_CLONES.store(0, Ordering::Relaxed);
    IMMEDIATE_VAL_CLONES.store(0, Ordering::Relaxed);
    HEAP_VAL_CLONES.store(0, Ordering::Relaxed);
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

#[cfg(test)]
pub fn vm_runtime_metrics_reset() {
    THREAD_RUNTIME_METRICS.with(|cell| cell.set(VmRuntimeMetrics::default()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn perf_value_kind_from_val_keeps_containers_unknown() {
        assert_eq!(PerfValueKind::from_val(&Val::Nil), PerfValueKind::Nil);
        assert_eq!(PerfValueKind::from_val(&Val::Bool(true)), PerfValueKind::Bool);
        assert_eq!(PerfValueKind::from_val(&Val::Int(42)), PerfValueKind::Int);
        assert_eq!(PerfValueKind::from_val(&Val::Float(1.5)), PerfValueKind::Float);
        assert_eq!(PerfValueKind::from_val(&Val::from_str("lk")), PerfValueKind::String);

        let list = Val::test_list_from_values(vec![Val::Int(1)]);
        let map = Val::test_string_map_from_hashmap(HashMap::from([("answer".to_string(), Val::Int(42))]));

        assert_eq!(PerfValueKind::from_val(&list), PerfValueKind::Unknown);
        assert_eq!(PerfValueKind::from_val(&map), PerfValueKind::Unknown);
    }
}
