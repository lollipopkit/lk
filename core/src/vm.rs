//! LK VM subsystem.
//!
//! The public surface exposes the canonical `Instr` compiler/executor path.

#[allow(dead_code, unused_imports)]
pub(crate) mod alloc;
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
mod ir;
#[cfg(test)]
mod migration_guard;
mod runtime;
#[allow(dead_code)]
pub(crate) mod ssa;

pub use artifact::*;
pub use cache::*;
pub use call_window::*;
pub use compiler::*;
pub use context::VmContext;
pub use exec::*;
pub use gc::*;
pub use ir::*;
pub use runtime::*;

pub use analysis::{
    VM_INDEX_KEY_METRIC_NAMES, VM_REGISTER_WRITE_SOURCE_NAMES, VmRuntimeMetrics, vm_runtime_metrics_enabled,
    vm_runtime_metrics_reset, vm_runtime_metrics_snapshot,
};
