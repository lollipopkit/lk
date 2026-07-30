//! LK VM subsystem.
//!
//! The public surface exposes the canonical `Instr` compiler/executor path.

// Load-bearing for a `cfg` reason, not for a dead-code one: the profiling
// machinery is compiled always and *used* only under `test` or `vm-profile`, so
// the default build sees items with no caller. That is also why this hid three
// real bugs — counters that could never count — until they were looked for by
// hand. The fix is to gate the items themselves so nothing needs silencing; see
// the task that records which ones.
#[allow(dead_code, unused_imports)]
pub mod analysis;
#[allow(dead_code, unused_imports)]
mod analysis_queries;
mod artifact;
mod cache;
mod call_window;
mod compiler;
mod context;
mod exec;
mod gc;
/// Hardware-touching intrinsics. The one place under `vm/` allowed `unsafe`;
/// see the module docs and the migration guard's exception list.
mod hardware;
mod ir;
#[cfg(test)]
mod migration_guard;
mod repl;
mod resolver;
mod runtime;
#[allow(dead_code)]
mod type_info;
pub mod verify;
#[cfg(all(test, feature = "std"))]
mod verify_fuzz_tests;

pub use artifact::*;
pub use cache::*;
pub use call_window::*;
pub use compiler::*;
pub use context::{MethodImpl, VmContext, core_call_method_windowed, receiver_type_scope};
#[cfg(test)]
pub use exec::test_support;
pub use exec::*;
pub use gc::*;
pub use ir::*;
pub use repl::*;
pub use resolver::*;
pub use runtime::*;
pub use type_info::*;

pub use analysis::{
    VM_INDEX_KEY_METRIC_NAMES, VM_REGISTER_WRITE_SOURCE_NAMES, VmRuntimeMetrics, vm_runtime_metrics_enabled,
    vm_runtime_metrics_reset, vm_runtime_metrics_snapshot,
};
