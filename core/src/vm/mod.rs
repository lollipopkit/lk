//! LK VM subsystem.
//!
//! The public surface exposes the canonical `Instr32` compiler/executor path.

#[allow(dead_code, unused_imports)]
pub(crate) mod alloc;
#[allow(dead_code, unused_imports)]
pub(crate) mod analysis;
#[allow(dead_code, unused_imports)]
mod analysis_queries;
mod compiler32;
mod context;
mod exec32;
mod frame32;
mod ir32;
#[allow(dead_code, unused_imports)]
pub(crate) mod registers;
#[allow(dead_code)]
pub(crate) mod ssa;

pub use compiler32::*;
pub use context::VmContext;
pub use exec32::*;
pub use frame32::*;
pub use ir32::*;
