//! Native (Cranelift) backend for LK.

pub(crate) mod vm {
    pub(crate) use lk_core::vm::*;
}

pub mod llvm;
mod native_executable;

pub use lk_aot_lower::BundledImport;
pub use llvm::{ClifArtifact, compile_artifact_to_clif_object};
pub use native_executable::{
    HybridLink, compile_native_executable_from_object, compile_native_executable_from_object_hybrid,
};
