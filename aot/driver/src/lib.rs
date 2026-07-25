//! AOT driver: the orchestration crate for LK's native compilation path.
//!
//! It owns the two steps around the typed MIR pipeline — calling
//! `lk-aot-lower` → `lk_aot_mir::validate` → `lk-aot-codegen` (Cranelift) to
//! get a relocatable object, then driving the linker that turns that object
//! into an executable against `liblkrt.a` (plus `liblk_api.a` for a Tier 1
//! hybrid binary).
//!
//! This crate carries no backend of its own; it deliberately holds the
//! process-facing glue (env knobs, `clang` as a link driver, hybrid wrapper
//! emission) that the pure `aot/{abi,mir,lower,codegen}` crates stay free of.

pub(crate) mod vm {
    pub(crate) use lk_core::vm::*;
}

mod backend;
mod native_executable;

pub use backend::{ClifArtifact, compile_artifact_to_clif_object};
pub use lk_aot_lower::BundledImport;
pub use native_executable::{
    HybridLink, compile_native_executable_from_object, compile_native_executable_from_object_hybrid,
};
