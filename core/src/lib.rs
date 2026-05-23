#![cfg_attr(feature = "aot-minimal-runtime", allow(dead_code))]

pub mod ast;
pub mod expr;
pub mod module;
mod operator;
pub mod package;
pub mod rt;
pub mod stmt;
pub mod token;
pub mod typ;
pub mod util;
pub mod val;

// Canonical Instr32 VM.
pub mod vm;

// Name resolution to slot indices
pub mod resolve;

#[cfg(feature = "llvm")]
pub mod llvm;
