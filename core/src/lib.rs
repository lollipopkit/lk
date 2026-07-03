// no_std flip for the VM core is staged. The compat shims (below) and the
// `core::`/`alloc::` relocations across every VM-core module are in place and
// exercised by the `--no-default-features` build (which selects
// hashbrown/spin/compat-path via `not(feature = "std")`). The final
// `#![no_std]` attribute lands once the remaining std-only leaves are gated:
// the file-import resolver in `stmt::import` (fs/dashmap/std::path) and the
// `macro_system` file-import/proc-macro functions. `alloc` is always available
// so the shims compile identically under both builds.
extern crate alloc;

pub mod compat;

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
