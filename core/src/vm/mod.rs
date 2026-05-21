//! Register bytecode VM subsystem
//!
//! This module contains the bytecode definitions, compiler, and VM runtime that
//! back the LK evaluator. It is now always part of the core crate.

mod alloc;
mod analysis;
mod bc32;
mod bytecode;
mod compiler;
mod context;
mod liveness;
mod lkb;
mod registers;
#[allow(clippy::module_inception)]
mod vm;

pub use alloc::*;
pub use analysis::*;
pub use bc32::*;
pub use bytecode::*;
pub use compiler::*;
pub use context::VmContext;
pub(crate) use liveness::*;
pub use lkb::*;
pub(crate) use registers::*;
pub(crate) use vm::with_current_vm;
pub use vm::*;

#[cfg(test)]
mod compiler_test;
#[cfg(test)]
mod vm_test;
