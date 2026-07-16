// The VM core builds as no_std under `--no-default-features`. On a std-capable
// host, no_std only forbids `lk-core`'s *own* source from using `std::` — its
// std-using dependencies (anyhow/dashmap/serde_json) still link std themselves,
// so only lk-core's direct std leaves (macro_system file-imports/proc-macros,
// stmt::import file resolver) are `std`-feature-gated. `alloc` is always
// available so the compat shims compile identically under both builds.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod compat;

pub mod ast;
pub mod expr;
pub mod macro_system;
pub mod mem;
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
