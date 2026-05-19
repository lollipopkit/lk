use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(all(not(test), debug_assertions))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::vm::RegionPlan;
use crate::vm::bc32::{Bc32PackStatus, bc32_pack_status};
use crate::vm::bytecode::{Function, Op};
use crate::vm::compiler::SsaFunction;

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

/// Aggregated analysis artifacts produced by the SSA pipeline.
#[derive(Debug, Clone, Default)]
pub struct FunctionAnalysis {
    pub ssa: Option<SsaFunction>,
    pub escape: EscapeSummary,
    pub region_plan: Arc<RegionPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VmOpcodeCategory {
    Data,
    Numeric,
    Compare,
    Local,
    Global,
    Access,
    Container,
    Control,
    Call,
    Closure,
    Pattern,
    Error,
}

impl VmOpcodeCategory {
    pub fn label(self) -> &'static str {
        match self {
            VmOpcodeCategory::Data => "data",
            VmOpcodeCategory::Numeric => "numeric",
            VmOpcodeCategory::Compare => "compare",
            VmOpcodeCategory::Local => "local",
            VmOpcodeCategory::Global => "global",
            VmOpcodeCategory::Access => "access",
            VmOpcodeCategory::Container => "container",
            VmOpcodeCategory::Control => "control",
            VmOpcodeCategory::Call => "call",
            VmOpcodeCategory::Closure => "closure",
            VmOpcodeCategory::Pattern => "pattern",
            VmOpcodeCategory::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmOpcodeCount {
    pub opcode: &'static str,
    pub category: VmOpcodeCategory,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct VmFunctionCoverage {
    pub name: String,
    pub depth: usize,
    pub instruction_count: usize,
    pub const_count: usize,
    pub register_count: u16,
    pub proto_count: usize,
    pub unmaterialized_closures: usize,
    pub call_sites: usize,
    pub named_call_sites: usize,
    pub closure_sites: usize,
    pub bc32_status: Bc32PackStatus,
    pub code32_words: Option<usize>,
    pub has_decoded_bc32: bool,
    pub opcode_counts: Vec<VmOpcodeCount>,
    pub bc32_typed_gate_counts: Vec<VmOpcodeCount>,
    pub category_counts: Vec<(VmOpcodeCategory, usize)>,
}

#[derive(Debug, Clone, Default)]
pub struct VmCoverageTotals {
    pub functions: usize,
    pub instructions: usize,
    pub packed_functions: usize,
    pub code32_words: usize,
    pub call_sites: usize,
    pub named_call_sites: usize,
    pub closure_sites: usize,
    pub unmaterialized_closures: usize,
    pub bc32_fallback_reasons: Vec<(String, usize)>,
    pub bc32_fallback_opcodes: Vec<(String, usize)>,
    pub opcode_counts: Vec<VmOpcodeCount>,
    pub bc32_typed_gate_counts: Vec<VmOpcodeCount>,
    pub category_counts: Vec<(VmOpcodeCategory, usize)>,
}

#[derive(Debug, Clone, Default)]
pub struct VmCoverageReport {
    pub functions: Vec<VmFunctionCoverage>,
    pub totals: VmCoverageTotals,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VmRuntimeMetrics {
    pub val_clones: u64,
    pub immediate_val_clones: u64,
    pub heap_val_clones: u64,
    pub register_writes: u64,
    pub return_value_moves: u64,
    pub quickening_hits: u64,
    pub quickening_build_attempts: u64,
    pub quickening_build_successes: u64,
    pub quickening_misses: u64,
    pub quickening_deopts: u64,
    pub quickening_sentinel_skips: u64,
}

#[cfg(test)]
thread_local! {
    static THREAD_RUNTIME_METRICS: std::cell::Cell<VmRuntimeMetrics> =
        const { std::cell::Cell::new(VmRuntimeMetrics {
            val_clones: 0,
            immediate_val_clones: 0,
            heap_val_clones: 0,
            register_writes: 0,
            return_value_moves: 0,
            quickening_hits: 0,
            quickening_build_attempts: 0,
            quickening_build_successes: 0,
            quickening_misses: 0,
            quickening_deopts: 0,
            quickening_sentinel_skips: 0,
        }) };
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

#[cfg(all(not(test), debug_assertions))]
static VAL_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static IMMEDIATE_VAL_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static HEAP_VAL_CLONES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static REGISTER_WRITES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static RETURN_VALUE_MOVES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static QUICKENING_HITS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static QUICKENING_BUILD_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static QUICKENING_BUILD_SUCCESSES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static QUICKENING_MISSES: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static QUICKENING_DEOPTS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static QUICKENING_SENTINEL_SKIPS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(not(test), debug_assertions))]
static RUNTIME_METRICS_ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(all(not(test), debug_assertions))]
#[inline]
fn runtime_metrics_enabled() -> bool {
    RUNTIME_METRICS_ENABLED.load(Ordering::Relaxed)
}

#[cfg(all(not(test), debug_assertions))]
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

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_val_clone(_heap_backed: bool) {}

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

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_register_write() {
    if !runtime_metrics_enabled() {
        return;
    }
    REGISTER_WRITES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_register_write() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_register_write() {
    update_thread_runtime_metrics(|metrics| metrics.register_writes += 1);
}

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_return_value_move() {
    if !runtime_metrics_enabled() {
        return;
    }
    RETURN_VALUE_MOVES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_return_value_move() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_return_value_move() {
    update_thread_runtime_metrics(|metrics| metrics.return_value_moves += 1);
}

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_quickening_hit() {
    if !runtime_metrics_enabled() {
        return;
    }
    QUICKENING_HITS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_quickening_hit() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_quickening_hit() {
    update_thread_runtime_metrics(|metrics| metrics.quickening_hits += 1);
}

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_quickening_build_attempt() {
    if !runtime_metrics_enabled() {
        return;
    }
    QUICKENING_BUILD_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_quickening_build_attempt() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_quickening_build_attempt() {
    update_thread_runtime_metrics(|metrics| metrics.quickening_build_attempts += 1);
}

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_quickening_build_success() {
    if !runtime_metrics_enabled() {
        return;
    }
    QUICKENING_BUILD_SUCCESSES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_quickening_build_success() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_quickening_build_success() {
    update_thread_runtime_metrics(|metrics| metrics.quickening_build_successes += 1);
}

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_quickening_miss() {
    if !runtime_metrics_enabled() {
        return;
    }
    QUICKENING_MISSES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_quickening_miss() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_quickening_miss() {
    update_thread_runtime_metrics(|metrics| metrics.quickening_misses += 1);
}

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_quickening_deopt() {
    if !runtime_metrics_enabled() {
        return;
    }
    QUICKENING_DEOPTS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_quickening_deopt() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_quickening_deopt() {
    update_thread_runtime_metrics(|metrics| metrics.quickening_deopts += 1);
}

#[cfg(all(not(test), debug_assertions))]
#[inline]
pub(crate) fn record_quickening_sentinel_skip() {
    if !runtime_metrics_enabled() {
        return;
    }
    QUICKENING_SENTINEL_SKIPS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
#[inline]
pub(crate) fn record_quickening_sentinel_skip() {}

#[cfg(test)]
#[inline]
pub(crate) fn record_quickening_sentinel_skip() {
    update_thread_runtime_metrics(|metrics| metrics.quickening_sentinel_skips += 1);
}

#[cfg(all(not(test), debug_assertions))]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    VmRuntimeMetrics {
        val_clones: VAL_CLONES.load(Ordering::Relaxed),
        immediate_val_clones: IMMEDIATE_VAL_CLONES.load(Ordering::Relaxed),
        heap_val_clones: HEAP_VAL_CLONES.load(Ordering::Relaxed),
        register_writes: REGISTER_WRITES.load(Ordering::Relaxed),
        return_value_moves: RETURN_VALUE_MOVES.load(Ordering::Relaxed),
        quickening_hits: QUICKENING_HITS.load(Ordering::Relaxed),
        quickening_build_attempts: QUICKENING_BUILD_ATTEMPTS.load(Ordering::Relaxed),
        quickening_build_successes: QUICKENING_BUILD_SUCCESSES.load(Ordering::Relaxed),
        quickening_misses: QUICKENING_MISSES.load(Ordering::Relaxed),
        quickening_deopts: QUICKENING_DEOPTS.load(Ordering::Relaxed),
        quickening_sentinel_skips: QUICKENING_SENTINEL_SKIPS.load(Ordering::Relaxed),
    }
}

#[cfg(all(not(test), not(debug_assertions)))]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    VmRuntimeMetrics::default()
}

#[cfg(test)]
pub fn vm_runtime_metrics_snapshot() -> VmRuntimeMetrics {
    THREAD_RUNTIME_METRICS.with(std::cell::Cell::get)
}

#[cfg(all(not(test), debug_assertions))]
pub fn vm_runtime_metrics_reset() {
    RUNTIME_METRICS_ENABLED.store(true, Ordering::Relaxed);
    VAL_CLONES.store(0, Ordering::Relaxed);
    IMMEDIATE_VAL_CLONES.store(0, Ordering::Relaxed);
    HEAP_VAL_CLONES.store(0, Ordering::Relaxed);
    REGISTER_WRITES.store(0, Ordering::Relaxed);
    RETURN_VALUE_MOVES.store(0, Ordering::Relaxed);
    QUICKENING_HITS.store(0, Ordering::Relaxed);
    QUICKENING_BUILD_ATTEMPTS.store(0, Ordering::Relaxed);
    QUICKENING_BUILD_SUCCESSES.store(0, Ordering::Relaxed);
    QUICKENING_MISSES.store(0, Ordering::Relaxed);
    QUICKENING_DEOPTS.store(0, Ordering::Relaxed);
    QUICKENING_SENTINEL_SKIPS.store(0, Ordering::Relaxed);
}

#[cfg(all(not(test), not(debug_assertions)))]
pub fn vm_runtime_metrics_reset() {}

#[cfg(test)]
pub fn vm_runtime_metrics_reset() {
    THREAD_RUNTIME_METRICS.with(|cell| cell.set(VmRuntimeMetrics::default()));
}

pub fn vm_coverage_report(entry: &Function) -> VmCoverageReport {
    let mut report = VmCoverageReport::default();
    collect_function_coverage("entry".to_string(), entry, 0, &mut report);
    report.totals = aggregate_coverage(&report.functions);
    report
}

fn collect_function_coverage(name: String, function: &Function, depth: usize, report: &mut VmCoverageReport) {
    let mut opcode_counts = BTreeMap::<(&'static str, VmOpcodeCategory), usize>::new();
    let mut bc32_typed_gate_counts = BTreeMap::<(&'static str, VmOpcodeCategory), usize>::new();
    let mut category_counts = BTreeMap::<VmOpcodeCategory, usize>::new();
    let mut call_sites = 0;
    let mut named_call_sites = 0;
    let mut closure_sites = 0;

    for op in &function.code {
        let name = opcode_name(op);
        let category = opcode_category(op);
        *opcode_counts.entry((name, category)).or_default() += 1;
        if let Some(name) = op.bc32_typed_gate_name() {
            *bc32_typed_gate_counts.entry((name, category)).or_default() += 1;
        }
        *category_counts.entry(category).or_default() += 1;
        match op {
            Op::Call { .. } | Op::CallExact { .. } | Op::CallClosureExact { .. } | Op::CallNativeFast { .. } => {
                call_sites += 1
            }
            Op::CallNamed { .. } | Op::CallNamedFallback { .. } => named_call_sites += 1,
            Op::MakeClosure { .. } => closure_sites += 1,
            _ => {}
        }
    }

    let bc32_status = bc32_pack_status(function);
    let unmaterialized_closures = function.protos.iter().filter(|proto| proto.func.is_none()).count();
    report.functions.push(VmFunctionCoverage {
        name: name.clone(),
        depth,
        instruction_count: function.code.len(),
        const_count: function.consts.len(),
        register_count: function.n_regs,
        proto_count: function.protos.len(),
        unmaterialized_closures,
        call_sites,
        named_call_sites,
        closure_sites,
        bc32_status,
        code32_words: function.code32.as_ref().map(Vec::len),
        has_decoded_bc32: function.bc32_decoded.is_some(),
        opcode_counts: opcode_counts
            .into_iter()
            .map(|((opcode, category), count)| VmOpcodeCount {
                opcode,
                category,
                count,
            })
            .collect(),
        bc32_typed_gate_counts: bc32_typed_gate_counts
            .into_iter()
            .map(|((opcode, category), count)| VmOpcodeCount {
                opcode,
                category,
                count,
            })
            .collect(),
        category_counts: category_counts.into_iter().collect(),
    });

    for (idx, proto) in function.protos.iter().enumerate() {
        if let Some(nested) = proto.func.as_ref() {
            let proto_name = proto
                .self_name
                .as_deref()
                .map(|self_name| format!("{name}.closure[{idx}] {self_name}"))
                .unwrap_or_else(|| format!("{name}.closure[{idx}]"));
            collect_function_coverage(proto_name, nested.as_ref(), depth + 1, report);
        }
    }
}

fn aggregate_coverage(functions: &[VmFunctionCoverage]) -> VmCoverageTotals {
    let mut totals = VmCoverageTotals {
        functions: functions.len(),
        ..VmCoverageTotals::default()
    };
    let mut fallback_reasons = BTreeMap::<String, usize>::new();
    let mut fallback_opcodes = BTreeMap::<String, usize>::new();
    let mut opcode_counts = BTreeMap::<(&'static str, VmOpcodeCategory), usize>::new();
    let mut bc32_typed_gate_counts = BTreeMap::<(&'static str, VmOpcodeCategory), usize>::new();
    let mut category_counts = BTreeMap::<VmOpcodeCategory, usize>::new();

    for function in functions {
        totals.instructions += function.instruction_count;
        totals.call_sites += function.call_sites;
        totals.named_call_sites += function.named_call_sites;
        totals.closure_sites += function.closure_sites;
        totals.unmaterialized_closures += function.unmaterialized_closures;
        if function.bc32_status.packed {
            totals.packed_functions += 1;
            totals.code32_words += function.bc32_status.words.unwrap_or(0);
        } else {
            if let Some(reason) = function.bc32_status.reason.as_ref() {
                *fallback_reasons.entry(reason.clone()).or_default() += 1;
            }
            if let Some(opcode) = function.bc32_status.opcode.as_ref() {
                *fallback_opcodes.entry(opcode.clone()).or_default() += 1;
            }
        }
        for entry in &function.opcode_counts {
            *opcode_counts.entry((entry.opcode, entry.category)).or_default() += entry.count;
        }
        for entry in &function.bc32_typed_gate_counts {
            *bc32_typed_gate_counts
                .entry((entry.opcode, entry.category))
                .or_default() += entry.count;
        }
        for (category, count) in &function.category_counts {
            *category_counts.entry(*category).or_default() += *count;
        }
    }

    totals.bc32_fallback_reasons = fallback_reasons.into_iter().collect();
    totals.bc32_fallback_opcodes = fallback_opcodes.into_iter().collect();
    totals.opcode_counts = opcode_counts
        .into_iter()
        .map(|((opcode, category), count)| VmOpcodeCount {
            opcode,
            category,
            count,
        })
        .collect();
    totals.bc32_typed_gate_counts = bc32_typed_gate_counts
        .into_iter()
        .map(|((opcode, category), count)| VmOpcodeCount {
            opcode,
            category,
            count,
        })
        .collect();
    totals.category_counts = category_counts.into_iter().collect();
    totals
}

pub fn opcode_name(op: &Op) -> &'static str {
    match op {
        Op::LoadK(..) => "LoadK",
        Op::Move(..) => "Move",
        Op::Not(..) => "Not",
        Op::ToStr(..) => "ToStr",
        Op::ToBool(..) => "ToBool",
        Op::JmpIfNil(..) => "JmpIfNil",
        Op::JmpIfNotNil(..) => "JmpIfNotNil",
        Op::NullishPick { .. } => "NullishPick",
        Op::JmpFalseSet { .. } => "JmpFalseSet",
        Op::JmpTrueSet { .. } => "JmpTrueSet",
        Op::Add(..) => "Add",
        Op::StrConcatKnownCap(..) => "StrConcatKnownCap",
        Op::Sub(..) => "Sub",
        Op::Mul(..) => "Mul",
        Op::Div(..) => "Div",
        Op::Mod(..) => "Mod",
        Op::AddInt(..) => "AddInt",
        Op::AddFloat(..) => "AddFloat",
        Op::AddIntImm(..) => "AddIntImm",
        Op::SubInt(..) => "SubInt",
        Op::SubFloat(..) => "SubFloat",
        Op::MulInt(..) => "MulInt",
        Op::MulFloat(..) => "MulFloat",
        Op::DivFloat(..) => "DivFloat",
        Op::ModInt(..) => "ModInt",
        Op::ModFloat(..) => "ModFloat",
        Op::CmpEq(..) => "CmpEq",
        Op::CmpNe(..) => "CmpNe",
        Op::CmpLt(..) => "CmpLt",
        Op::CmpLe(..) => "CmpLe",
        Op::CmpGt(..) => "CmpGt",
        Op::CmpGe(..) => "CmpGe",
        Op::CmpI { .. } => "CmpI",
        Op::CmpEqImm(..) => "CmpEqImm",
        Op::CmpNeImm(..) => "CmpNeImm",
        Op::CmpLtImm(..) => "CmpLtImm",
        Op::CmpLeImm(..) => "CmpLeImm",
        Op::CmpGtImm(..) => "CmpGtImm",
        Op::CmpGeImm(..) => "CmpGeImm",
        Op::In(..) => "In",
        Op::LoadLocal(..) => "LoadLocal",
        Op::StoreLocal(..) => "StoreLocal",
        Op::LoadGlobal(..) => "LoadGlobal",
        Op::DefineGlobal(..) => "DefineGlobal",
        Op::LoadCapture { .. } => "LoadCapture",
        Op::Access(..) => "Access",
        Op::AccessK(..) => "AccessK",
        Op::IndexK(..) => "IndexK",
        Op::ListIndexI(..) => "ListIndexI",
        Op::ListSetI { .. } => "ListSetI",
        Op::StrIndexI(..) => "StrIndexI",
        Op::Len { .. } => "Len",
        Op::ListLen { .. } => "ListLen",
        Op::MapLen { .. } => "MapLen",
        Op::StrLen { .. } => "StrLen",
        Op::Floor { .. } => "Floor",
        Op::StartsWithK(..) => "StartsWithK",
        Op::ContainsK(..) => "ContainsK",
        Op::MapHas(..) => "MapHas",
        Op::MapGetInterned(..) => "MapGetInterned",
        Op::MapGetDynamic(..) => "MapGetDynamic",
        Op::MapSetInterned(..) => "MapSetInterned",
        Op::MapHasK(..) => "MapHasK",
        Op::ListFoldAdd { .. } => "ListFoldAdd",
        Op::MapValuesFoldAdd { .. } => "MapValuesFoldAdd",
        Op::Index { .. } => "Index",
        Op::PatternMatch { .. } => "PatternMatch",
        Op::PatternMatchOrFail { .. } => "PatternMatchOrFail",
        Op::Raise { .. } => "Raise",
        Op::ToIter { .. } => "ToIter",
        Op::BuildList { .. } => "BuildList",
        Op::BuildMap { .. } => "BuildMap",
        Op::ListSlice { .. } => "ListSlice",
        Op::ListPush { .. } => "ListPush",
        Op::MapSet { .. } => "MapSet",
        Op::MapSetMove { .. } => "MapSetMove",
        Op::MakeClosure { .. } => "MakeClosure",
        Op::Jmp(..) => "Jmp",
        Op::JmpFalse(..) => "JmpFalse",
        Op::BoolBranch(..) => "BoolBranch",
        Op::CmpLtImmJmp { .. } => "CmpLtImmJmp",
        Op::JmpNilOrFalseJmp { .. } => "JmpNilOrFalseJmp",
        Op::AddIntImmJmp { .. } => "AddIntImmJmp",
        Op::AddRangeCountImm { .. } => "AddRangeCountImm",
        Op::CmpLeImmJmp { .. } => "CmpLeImmJmp",
        Op::CmpNeImmJmp { .. } => "CmpNeImmJmp",
        Op::Call { .. } => "Call",
        Op::CallExact { .. } => "CallExact",
        Op::CallClosureExact { .. } => "CallClosureExact",
        Op::CallNativeFast { .. } => "CallNativeFast",
        Op::CallNamed { .. } => "CallNamed",
        Op::CallNamedFallback { .. } => "CallNamedFallback",
        Op::Ret { .. } => "Ret",
        Op::ForRangePrep { .. } => "ForRangePrep",
        Op::ForRangeLoop { .. } => "ForRangeLoop",
        Op::RangeLoopI { .. } => "RangeLoopI",
        Op::ForRangeStep { .. } => "ForRangeStep",
        Op::Break(..) => "Break",
        Op::Continue(..) => "Continue",
    }
}

pub fn opcode_category(op: &Op) -> VmOpcodeCategory {
    match op {
        Op::LoadK(..) | Op::Move(..) | Op::Not(..) | Op::ToStr(..) | Op::ToBool(..) => VmOpcodeCategory::Data,
        Op::Add(..)
        | Op::StrConcatKnownCap(..)
        | Op::Sub(..)
        | Op::Mul(..)
        | Op::Div(..)
        | Op::Mod(..)
        | Op::AddInt(..)
        | Op::AddFloat(..)
        | Op::AddIntImm(..)
        | Op::SubInt(..)
        | Op::SubFloat(..)
        | Op::MulInt(..)
        | Op::MulFloat(..)
        | Op::DivFloat(..)
        | Op::ModInt(..)
        | Op::ModFloat(..)
        | Op::Floor { .. }
        | Op::AddIntImmJmp { .. }
        | Op::AddRangeCountImm { .. } => VmOpcodeCategory::Numeric,
        Op::CmpEq(..)
        | Op::CmpNe(..)
        | Op::CmpLt(..)
        | Op::CmpLe(..)
        | Op::CmpGt(..)
        | Op::CmpGe(..)
        | Op::CmpI { .. }
        | Op::CmpEqImm(..)
        | Op::CmpNeImm(..)
        | Op::CmpLtImm(..)
        | Op::CmpLeImm(..)
        | Op::CmpGtImm(..)
        | Op::CmpGeImm(..)
        | Op::CmpLtImmJmp { .. }
        | Op::CmpLeImmJmp { .. }
        | Op::CmpNeImmJmp { .. }
        | Op::In(..) => VmOpcodeCategory::Compare,
        Op::LoadLocal(..) | Op::StoreLocal(..) => VmOpcodeCategory::Local,
        Op::LoadGlobal(..) | Op::DefineGlobal(..) | Op::LoadCapture { .. } => VmOpcodeCategory::Global,
        Op::Access(..)
        | Op::AccessK(..)
        | Op::IndexK(..)
        | Op::ListIndexI(..)
        | Op::ListSetI { .. }
        | Op::StrIndexI(..)
        | Op::Index { .. }
        | Op::Len { .. }
        | Op::ListLen { .. }
        | Op::MapLen { .. }
        | Op::StrLen { .. }
        | Op::StartsWithK(..)
        | Op::ContainsK(..)
        | Op::MapHas(..)
        | Op::MapGetInterned(..)
        | Op::MapGetDynamic(..)
        | Op::MapHasK(..) => VmOpcodeCategory::Access,
        Op::ListFoldAdd { .. }
        | Op::MapValuesFoldAdd { .. }
        | Op::ToIter { .. }
        | Op::BuildList { .. }
        | Op::BuildMap { .. }
        | Op::ListSlice { .. }
        | Op::ListPush { .. }
        | Op::MapSet { .. }
        | Op::MapSetInterned(..)
        | Op::MapSetMove { .. } => VmOpcodeCategory::Container,
        Op::JmpIfNil(..)
        | Op::JmpIfNotNil(..)
        | Op::NullishPick { .. }
        | Op::JmpFalseSet { .. }
        | Op::JmpTrueSet { .. }
        | Op::Jmp(..)
        | Op::JmpFalse(..)
        | Op::BoolBranch(..)
        | Op::JmpNilOrFalseJmp { .. }
        | Op::Ret { .. }
        | Op::ForRangePrep { .. }
        | Op::ForRangeLoop { .. }
        | Op::RangeLoopI { .. }
        | Op::ForRangeStep { .. }
        | Op::Break(..)
        | Op::Continue(..) => VmOpcodeCategory::Control,
        Op::Call { .. }
        | Op::CallExact { .. }
        | Op::CallClosureExact { .. }
        | Op::CallNativeFast { .. }
        | Op::CallNamed { .. }
        | Op::CallNamedFallback { .. } => VmOpcodeCategory::Call,
        Op::MakeClosure { .. } => VmOpcodeCategory::Closure,
        Op::PatternMatch { .. } | Op::PatternMatchOrFail { .. } => VmOpcodeCategory::Pattern,
        Op::Raise { .. } => VmOpcodeCategory::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::val::Val;
    use crate::vm::bytecode::Function;

    fn test_function(code: Vec<Op>) -> Function {
        Function {
            consts: Vec::new(),
            code,
            n_regs: 4,
            protos: Vec::new(),
            param_regs: Vec::new(),
            named_param_regs: Vec::new(),
            named_param_layout: Vec::new(),
            pattern_plans: Vec::new(),
            code32: None,
            bc32_decoded: None,
            analysis: None,
        }
    }

    #[test]
    fn vm_coverage_report_counts_opcodes_categories_and_call_sites() {
        let function = test_function(vec![
            Op::LoadK(0, 0),
            Op::AddInt(1, 0, 0),
            Op::Call {
                f: 1,
                base: 0,
                argc: 1,
                retc: 1,
            },
            Op::Ret { base: 0, retc: 1 },
        ]);

        let report = vm_coverage_report(&function);

        assert_eq!(report.totals.functions, 1);
        assert_eq!(report.totals.instructions, 4);
        assert_eq!(report.totals.call_sites, 1);
        assert_eq!(
            report
                .totals
                .category_counts
                .iter()
                .find(|(category, _)| *category == VmOpcodeCategory::Numeric)
                .map(|(_, count)| *count),
            Some(1)
        );
        assert!(
            report
                .totals
                .opcode_counts
                .iter()
                .any(|entry| entry.opcode == "Call" && entry.count == 1)
        );
        assert!(
            report
                .totals
                .bc32_typed_gate_counts
                .iter()
                .any(|entry| entry.opcode == "AddInt" && entry.count == 1)
        );
    }

    #[test]
    fn runtime_metrics_count_immediate_and_heap_val_clones() {
        vm_runtime_metrics_reset();

        let _ = Val::Int(1).clone();
        let _ = Val::from_str("longer-than-short").clone();

        let metrics = vm_runtime_metrics_snapshot();
        assert_eq!(metrics.val_clones, 2);
        assert_eq!(metrics.immediate_val_clones, 1);
        assert_eq!(metrics.heap_val_clones, 1);
    }
}
