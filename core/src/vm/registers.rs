use crate::val::Val;

#[cfg(test)]
use super::analysis::record_register_write;
#[cfg(test)]
use super::analysis::vm_runtime_metrics_enabled;
use super::analysis::{
    VmValueCopyMetric, record_copy_policy_clone, record_register_write_known_enabled, record_return_value_move,
    record_val_clone,
};

#[inline(always)]
#[cfg(test)]
pub(crate) fn write_register_value(regs: &mut [Val], idx: usize, value: Val) {
    debug_assert!(idx < regs.len(), "register write out of frame window");
    record_register_write();
    regs[idx] = value;
}

/// Same as `write_register_value` but skips the atomic metrics gate read when
/// `collect_metrics` is false. The execution hot path already reads the gate
/// once per frame via `FrameRuntimeView::collect_metrics`; this avoids
/// repeating the `runtime_metrics_enabled()` atomic read on every register
/// write in tight arithmetic loops.
#[inline(always)]
pub(crate) fn write_register_value_with_metrics(regs: &mut [Val], idx: usize, value: Val, collect_metrics: bool) {
    debug_assert!(idx < regs.len(), "register write out of frame window");
    if collect_metrics {
        record_register_write_known_enabled();
    }
    regs[idx] = value;
}

#[inline(always)]
pub(crate) fn copy_value_for_register_with_metrics(value: &Val, collect_metrics: bool) -> Val {
    copy_value_for_register_with_metric_gate(value, VmValueCopyMetric::Generic, collect_metrics)
}

#[inline(always)]
#[cfg(test)]
pub(crate) fn copy_call_arg_value_for_register(value: &Val) -> Val {
    copy_value_for_register_with_metric(value, VmValueCopyMetric::CallArg)
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) fn copy_call_arg_value_for_register_with_metrics(value: &Val, collect_metrics: bool) -> Val {
    copy_value_for_register_with_metric_gate(value, VmValueCopyMetric::CallArg, collect_metrics)
}

#[inline(always)]
#[cfg(test)]
pub(crate) fn copy_const_value_for_register(value: &Val) -> Val {
    copy_value_for_register_with_metric(value, VmValueCopyMetric::ConstLoad)
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) fn copy_const_value_for_register_with_metrics(value: &Val, collect_metrics: bool) -> Val {
    copy_value_for_register_with_metric_gate(value, VmValueCopyMetric::ConstLoad, collect_metrics)
}

#[inline(always)]
#[cfg(test)]
pub(crate) fn copy_container_value_for_register(value: &Val) -> Val {
    copy_value_for_register_with_metric(value, VmValueCopyMetric::Container)
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) fn copy_container_value_for_register_with_metrics(value: &Val, collect_metrics: bool) -> Val {
    copy_value_for_register_with_metric_gate(value, VmValueCopyMetric::Container, collect_metrics)
}

#[inline(always)]
#[cfg(test)]
fn copy_value_for_register_with_metric(value: &Val, metric: VmValueCopyMetric) -> Val {
    copy_value_for_register_with_metric_gate(value, metric, vm_runtime_metrics_enabled())
}

#[inline(always)]
fn copy_value_for_register_with_metric_gate(value: &Val, metric: VmValueCopyMetric, collect_metrics: bool) -> Val {
    match value {
        Val::ShortStr(value) => Val::ShortStr(*value),
        Val::Int(value) => Val::Int(*value),
        Val::Float(value) => Val::Float(*value),
        Val::Bool(value) => Val::Bool(*value),
        Val::RustFunction(value) => Val::RustFunction(*value),
        Val::RustFastFunction(value) => Val::RustFastFunction(*value),
        Val::RustFastFunctionNamed(value) => Val::RustFastFunctionNamed(*value),
        Val::RustFunctionNamed(value) => Val::RustFunctionNamed(*value),
        Val::Nil => Val::Nil,
        value => copy_heap_value_for_register_with_metric_gate(value, metric, collect_metrics),
    }
}

#[inline(always)]
fn copy_heap_value_for_register_with_metric_gate(value: &Val, metric: VmValueCopyMetric, collect_metrics: bool) -> Val {
    if collect_metrics {
        record_copy_policy_clone(metric, true);
        record_val_clone(true);
    }
    match value {
        Val::Str(value) => Val::Str(value.clone()),
        Val::Map(value) => Val::Map(value.clone()),
        Val::List(value) => Val::List(value.clone()),
        Val::Closure(value) => Val::Closure(value.clone()),
        Val::AotFunction(value) => Val::AotFunction(value.clone()),
        Val::Task(value) => Val::Task(value.clone()),
        Val::Channel(value) => Val::Channel(value.clone()),
        Val::Stream(value) => Val::Stream(value.clone()),
        Val::Iterator(value) => Val::Iterator(value.clone()),
        Val::MutationGuard(value) => Val::MutationGuard(value.clone()),
        Val::StreamCursor(value) => Val::StreamCursor(value.clone()),
        Val::Object(value) => Val::Object(value.clone()),
        _ => unreachable!("copy_heap_value_for_register_with_metric only accepts heap-backed values"),
    }
}

#[inline(always)]
#[cfg(test)]
pub(crate) fn write_register_copy(regs: &mut [Val], idx: usize, value: &Val) {
    write_register_copy_with_metrics(regs, idx, value, vm_runtime_metrics_enabled());
}

#[inline(always)]
pub(crate) fn write_register_copy_with_metrics(regs: &mut [Val], idx: usize, value: &Val, collect_metrics: bool) {
    write_register_value_with_metrics(
        regs,
        idx,
        copy_value_for_register_with_metric_gate(value, VmValueCopyMetric::Register, collect_metrics),
        collect_metrics,
    );
}

#[inline(always)]
pub(crate) fn write_register_const_copy_with_metrics(regs: &mut [Val], idx: usize, value: &Val, collect_metrics: bool) {
    write_register_value_with_metrics(
        regs,
        idx,
        copy_value_for_register_with_metric_gate(value, VmValueCopyMetric::ConstLoad, collect_metrics),
        collect_metrics,
    );
}

#[inline(always)]
#[cfg(test)]
pub(crate) fn copy_register_value(regs: &mut [Val], dst_idx: usize, src_idx: usize) {
    copy_register_value_with_metrics(regs, dst_idx, src_idx, vm_runtime_metrics_enabled());
}

#[inline(always)]
pub(crate) fn copy_register_value_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    collect_metrics: bool,
) {
    debug_assert!(dst_idx < regs.len(), "register copy destination out of frame window");
    debug_assert!(src_idx < regs.len(), "register copy source out of frame window");
    if dst_idx == src_idx {
        return;
    }
    let value = copy_value_for_register_with_metric_gate(&regs[src_idx], VmValueCopyMetric::Register, collect_metrics);
    write_register_value_with_metrics(regs, dst_idx, value, collect_metrics);
}

#[inline(always)]
#[cfg(test)]
pub(crate) fn copy_local_load_register_value(regs: &mut [Val], dst_idx: usize, src_idx: usize) {
    copy_local_load_register_value_with_metrics(regs, dst_idx, src_idx, vm_runtime_metrics_enabled());
}

#[inline(always)]
pub(crate) fn copy_local_load_register_value_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    collect_metrics: bool,
) {
    copy_local_register_value_with_metric(regs, dst_idx, src_idx, VmValueCopyMetric::LocalLoad, collect_metrics);
}

#[inline(always)]
#[cfg(test)]
pub(crate) fn copy_local_store_register_value(regs: &mut [Val], dst_idx: usize, src_idx: usize) {
    copy_local_store_register_value_with_metrics(regs, dst_idx, src_idx, vm_runtime_metrics_enabled());
}

#[inline(always)]
pub(crate) fn copy_local_store_register_value_with_metrics(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    collect_metrics: bool,
) {
    copy_local_register_value_with_metric(regs, dst_idx, src_idx, VmValueCopyMetric::LocalStore, collect_metrics);
}

#[inline(always)]
fn copy_local_register_value_with_metric(
    regs: &mut [Val],
    dst_idx: usize,
    src_idx: usize,
    metric: VmValueCopyMetric,
    collect_metrics: bool,
) {
    debug_assert!(dst_idx < regs.len(), "local copy destination out of frame window");
    debug_assert!(src_idx < regs.len(), "local copy source out of frame window");
    if dst_idx == src_idx {
        return;
    }
    let value = copy_value_for_register_with_metric_gate(&regs[src_idx], metric, collect_metrics);
    write_register_value_with_metrics(regs, dst_idx, value, collect_metrics);
}

#[inline(always)]
pub(crate) fn take_register_value(regs: &mut [Val], idx: usize) -> Val {
    debug_assert!(idx < regs.len(), "register take out of frame window");
    std::mem::replace(&mut regs[idx], Val::Nil)
}

#[inline(always)]
pub(crate) fn restore_register_value(regs: &mut [Val], idx: usize, value: Val) {
    debug_assert!(idx < regs.len(), "register restore out of frame window");
    regs[idx] = value;
}

#[inline(always)]
pub(crate) fn move_register_value(regs: &mut [Val], idx: usize) -> Val {
    let value = take_register_value(regs, idx);
    record_return_value_move();
    value
}

#[cfg(test)]
mod tests {
    use crate::val::Val;
    use crate::vm::{vm_runtime_metrics_reset, vm_runtime_metrics_snapshot};

    use super::{
        copy_call_arg_value_for_register, copy_const_value_for_register, copy_container_value_for_register,
        copy_local_load_register_value, copy_local_store_register_value, copy_register_value, move_register_value,
        restore_register_value, take_register_value, write_register_copy, write_register_value,
    };

    #[test]
    fn write_register_value_counts_one_write() {
        let mut regs = [Val::Nil];

        vm_runtime_metrics_reset();
        write_register_value(&mut regs, 0, Val::Int(7));
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(regs[0], Val::Int(7));
        assert_eq!(metrics.register_writes, 1);
        assert_eq!(metrics.val_clones, 0);
    }

    #[test]
    fn copy_register_value_skips_self_copy() {
        let mut regs = [Val::from_str("longer-than-short")];

        vm_runtime_metrics_reset();
        copy_register_value(&mut regs, 0, 0);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(metrics.register_writes, 0);
        assert_eq!(metrics.val_clones, 0);
    }

    #[test]
    fn copy_register_value_copies_immediate_without_val_clone() {
        let mut regs = [Val::Int(42), Val::Nil];

        vm_runtime_metrics_reset();
        copy_register_value(&mut regs, 1, 0);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(regs[1], Val::Int(42));
        assert_eq!(metrics.register_writes, 1);
        assert_eq!(metrics.val_clones, 0);
    }

    #[test]
    fn write_register_copy_clones_only_heap_backed_values() {
        let mut regs = [Val::Nil, Val::Nil];
        let heap_value = Val::from_str("longer-than-short");

        vm_runtime_metrics_reset();
        write_register_copy(&mut regs, 0, &Val::Int(7));
        write_register_copy(&mut regs, 1, &heap_value);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(regs[0], Val::Int(7));
        assert_eq!(regs[1], heap_value);
        assert_eq!(metrics.register_writes, 2);
        assert_eq!(metrics.val_clones, 1);
        assert_eq!(metrics.heap_val_clones, 1);
        assert_eq!(metrics.copy_policy_heap_clones, 1);
        assert_eq!(metrics.register_copy_heap_clones, 1);
        assert_eq!(metrics.local_copy_heap_clones, 0);
        assert_eq!(metrics.local_load_heap_clones, 0);
        assert_eq!(metrics.local_store_heap_clones, 0);
        assert_eq!(metrics.const_load_heap_clones, 0);
        assert_eq!(metrics.call_arg_heap_clones, 0);
        assert_eq!(metrics.container_copy_heap_clones, 0);
    }

    #[test]
    fn copy_policy_classifies_call_arg_and_container_heap_clones() {
        let heap_value = Val::from_str("longer-than-short");

        vm_runtime_metrics_reset();
        let call_arg = copy_call_arg_value_for_register(&heap_value);
        let container = copy_container_value_for_register(&heap_value);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(call_arg, heap_value);
        assert_eq!(container, heap_value);
        assert_eq!(metrics.val_clones, 2);
        assert_eq!(metrics.heap_val_clones, 2);
        assert_eq!(metrics.copy_policy_heap_clones, 2);
        assert_eq!(metrics.register_copy_heap_clones, 0);
        assert_eq!(metrics.local_copy_heap_clones, 0);
        assert_eq!(metrics.local_load_heap_clones, 0);
        assert_eq!(metrics.local_store_heap_clones, 0);
        assert_eq!(metrics.const_load_heap_clones, 0);
        assert_eq!(metrics.call_arg_heap_clones, 1);
        assert_eq!(metrics.container_copy_heap_clones, 1);
    }

    #[test]
    fn copy_policy_classifies_const_heap_clones() {
        let heap_value = Val::from_str("longer-than-short");

        vm_runtime_metrics_reset();
        let value = copy_const_value_for_register(&heap_value);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(value, heap_value);
        assert_eq!(metrics.val_clones, 1);
        assert_eq!(metrics.heap_val_clones, 1);
        assert_eq!(metrics.copy_policy_heap_clones, 1);
        assert_eq!(metrics.register_copy_heap_clones, 0);
        assert_eq!(metrics.local_copy_heap_clones, 0);
        assert_eq!(metrics.local_load_heap_clones, 0);
        assert_eq!(metrics.local_store_heap_clones, 0);
        assert_eq!(metrics.const_load_heap_clones, 1);
        assert_eq!(metrics.call_arg_heap_clones, 0);
        assert_eq!(metrics.container_copy_heap_clones, 0);
    }

    #[test]
    fn copy_policy_classifies_local_load_and_store_heap_clones() {
        let heap_value = Val::from_str("longer-than-short");
        let mut regs = [heap_value.clone(), Val::Nil, Val::Nil];

        vm_runtime_metrics_reset();
        copy_local_load_register_value(&mut regs, 1, 0);
        copy_local_store_register_value(&mut regs, 2, 1);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(regs[1], heap_value);
        assert_eq!(regs[2], heap_value);
        assert_eq!(metrics.val_clones, 2);
        assert_eq!(metrics.heap_val_clones, 2);
        assert_eq!(metrics.copy_policy_heap_clones, 2);
        assert_eq!(metrics.register_copy_heap_clones, 0);
        assert_eq!(metrics.local_copy_heap_clones, 2);
        assert_eq!(metrics.local_load_heap_clones, 1);
        assert_eq!(metrics.local_store_heap_clones, 1);
        assert_eq!(metrics.const_load_heap_clones, 0);
    }

    #[test]
    fn take_and_restore_register_value_do_not_count_register_writes() {
        let mut regs = [Val::from_str("longer-than-short")];

        vm_runtime_metrics_reset();
        let value = take_register_value(&mut regs, 0);
        restore_register_value(&mut regs, 0, value);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(regs[0], Val::from_str("longer-than-short"));
        assert_eq!(metrics.register_writes, 0);
        assert_eq!(metrics.return_value_moves, 0);
    }

    #[test]
    fn move_register_value_counts_return_move_without_write() {
        let mut regs = [Val::Int(3)];

        vm_runtime_metrics_reset();
        let value = move_register_value(&mut regs, 0);
        let metrics = vm_runtime_metrics_snapshot();

        assert_eq!(value, Val::Int(3));
        assert_eq!(regs[0], Val::Nil);
        assert_eq!(metrics.register_writes, 0);
        assert_eq!(metrics.return_value_moves, 1);
    }
}
