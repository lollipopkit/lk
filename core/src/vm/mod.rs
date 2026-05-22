//! LK VM subsystem.
//!
//! The public surface exposes the canonical `Instr32` compiler/executor path.

#[allow(dead_code, unused_imports)]
pub(crate) mod alloc;
#[allow(dead_code, unused_imports)]
pub(crate) mod analysis;
#[allow(dead_code, unused_imports)]
mod analysis_queries;
mod call_window32;
mod compiler32;
mod context;
mod exec32;
mod gc32;
mod ir32;
#[allow(dead_code, unused_imports)]
#[path = "registers.rs"]
pub(crate) mod legacy_registers;
mod runtime32;
#[allow(dead_code)]
pub(crate) mod ssa;

pub use call_window32::*;
pub use compiler32::*;
pub use context::VmContext;
pub use exec32::*;
pub use gc32::*;
pub use ir32::*;
pub use runtime32::*;
