pub mod ast;
pub mod expr;
pub mod macro_system;
pub mod module;
mod operator;
// std-heavy, VM-core-independent; gated so `--no-default-features` yields the
// no_std-bound VM core surface (plan M0.7/8 lk-vm-core groundwork).
#[cfg(feature = "std")]
pub mod package;
pub mod rt;
pub mod stmt;
pub mod syntax;
pub mod token;
pub mod typ;
pub mod util;
pub mod val;

// Canonical Instr VM.
pub mod vm;

// Name resolution to slot indices
pub mod resolve;
