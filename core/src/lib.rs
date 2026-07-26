// The VM core builds as no_std under `--no-default-features`. On a std-capable
// host, no_std only forbids `lk-core`'s *own* source from using `std::` — its
// std-using dependencies (anyhow/dashmap/serde_json) still link std themselves,
// so only lk-core's direct std leaves (macro_system file-imports/proc-macros,
// stmt::import file resolver) are `std`-feature-gated. `alloc` is always
// available so the compat shims compile identically under both builds.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// The unit-test harness (libtest) is std-only, so a no_std build still has to
// link std to *run* its tests. This is scaffolding, not a hole in the no_std
// guarantee: `#![no_std]` still applies to every non-test item, so what the
// tests exercise is the real no_std VM core. `#[macro_use]` puts std's exported
// macros (`println!`, `thread_local!`, …) back in scope for test code that
// needs them; types still come from `compat::prelude`.
#[cfg(all(test, not(feature = "std")))]
#[macro_use]
extern crate std;

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
