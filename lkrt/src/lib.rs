//! Typed native runtime support for LK LLVM AOT binaries.
//!
//! This crate is intentionally not the LK VM. It may provide low-level typed
//! helpers that LLVM-generated code links against, but it must not depend on the
//! parser, compiler, `Module32Artifact`, `VmContext`, or the Instr32 executor.

/// Called by the CLI to make the Cargo dependency explicit.
pub fn link_anchor() -> u8 {
    0
}

/// Version string embedded in the static library for diagnostics.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
