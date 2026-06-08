use crate::val::RuntimeVal;
use crate::vm::analysis::{
    PerfIndexFact, PerfIndexTargetKind, VM_INDEX_KEY_METRIC_COUNT, VmContainerMetric, VmIndexKeyMetric,
    VmRegisterWriteSource,
};
#[cfg(any(test, feature = "vm-profile"))]
use crate::vm::analysis::{
    VM_OPCODE_COUNT, VM_REGISTER_WRITE_SOURCE_COUNT, record_index_key_metrics_batch, record_opcode_histogram_batch,
    record_opcode_step_known_enabled, record_register_write_sources_batch,
};

#[inline]
pub(super) fn index_metric_kind(index_fact: Option<PerfIndexFact>) -> VmContainerMetric {
    match index_fact.map(|fact| fact.target_kind) {
        Some(PerfIndexTargetKind::List) => VmContainerMetric::List,
        Some(PerfIndexTargetKind::Map) => VmContainerMetric::Map,
        Some(PerfIndexTargetKind::String) => VmContainerMetric::String,
        Some(PerfIndexTargetKind::Object | PerfIndexTargetKind::Unknown) | None => VmContainerMetric::Generic,
    }
}

#[inline]
pub(in crate::vm::exec) fn record_index_key_metric(
    metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    metric: VmIndexKeyMetric,
) {
    if let Some(metrics) = metrics {
        metrics[metric.index()] += 1;
    }
}

#[inline]
pub(in crate::vm::exec) fn record_dynamic_index_key_metric(
    metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    key: &RuntimeVal,
) {
    let Some(metrics) = metrics else {
        return;
    };
    metrics[VmIndexKeyMetric::DynamicRegisterKey.index()] += 1;
    let metric = match key {
        RuntimeVal::Int(_) => VmIndexKeyMetric::DynamicIntKey,
        RuntimeVal::ShortStr(_) => VmIndexKeyMetric::DynamicShortStringKey,
        RuntimeVal::Obj(_) => VmIndexKeyMetric::DynamicObjectKey,
        RuntimeVal::Nil | RuntimeVal::Bool(_) | RuntimeVal::Float(_) => VmIndexKeyMetric::DynamicOtherKey,
    };
    metrics[metric.index()] += 1;
}

#[cfg(any(test, feature = "vm-profile"))]
pub(super) struct RuntimeProfileFrame {
    opcode_histogram: [u64; VM_OPCODE_COUNT],
    register_write_sources: [u64; VM_REGISTER_WRITE_SOURCE_COUNT],
    index_key_metrics: [u64; VM_INDEX_KEY_METRIC_COUNT],
}

#[cfg(any(test, feature = "vm-profile"))]
impl RuntimeProfileFrame {
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            opcode_histogram: [0; VM_OPCODE_COUNT],
            register_write_sources: [0; VM_REGISTER_WRITE_SOURCE_COUNT],
            index_key_metrics: [0; VM_INDEX_KEY_METRIC_COUNT],
        }
    }

    #[inline]
    pub(super) fn record_opcode(&mut self, opcode: crate::vm::Opcode, collect_metrics: bool) {
        if collect_metrics {
            record_opcode_step_known_enabled();
            self.opcode_histogram[opcode as usize] += 1;
        }
    }

    #[inline]
    pub(super) fn record_write_source(&mut self, source: VmRegisterWriteSource, collect_metrics: bool) {
        if collect_metrics {
            self.register_write_sources[source.index()] += 1;
        }
    }

    #[inline]
    pub(super) fn index_key_metrics(&mut self, collect_metrics: bool) -> Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]> {
        collect_metrics.then_some(&mut self.index_key_metrics)
    }

    #[inline]
    pub(super) fn flush(&self, collect_metrics: bool) {
        if collect_metrics {
            record_opcode_histogram_batch(&self.opcode_histogram);
            record_register_write_sources_batch(&self.register_write_sources);
            record_index_key_metrics_batch(&self.index_key_metrics);
        }
    }
}

#[cfg(all(not(test), not(feature = "vm-profile")))]
pub(super) struct RuntimeProfileFrame;

#[cfg(all(not(test), not(feature = "vm-profile")))]
impl RuntimeProfileFrame {
    #[inline(always)]
    pub(super) fn new() -> Self {
        Self
    }

    #[inline(always)]
    pub(super) fn record_opcode(&mut self, _opcode: crate::vm::Opcode, _collect_metrics: bool) {}

    #[inline(always)]
    pub(super) fn record_write_source(&mut self, _source: VmRegisterWriteSource, _collect_metrics: bool) {}

    #[inline(always)]
    pub(super) fn index_key_metrics(
        &mut self,
        _collect_metrics: bool,
    ) -> Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]> {
        None
    }

    #[inline(always)]
    pub(super) fn flush(&self, _collect_metrics: bool) {}
}
