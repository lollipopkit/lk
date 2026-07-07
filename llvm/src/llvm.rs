//! LLVM backend entry points.
//!
//! The backend lowers `ModuleArtifact`s through the typed MIR pipeline
//! (`lk-aot-lower` → `lk_aot_mir::validate` → `lk-aot-codegen`). Shapes the
//! lowering rejects fail the compile with a precise `Unsupported` reason;
//! there is no fallback backend and no VM shell embedding.

mod backend;
mod options;

pub use backend::{
    LlvmBackend, LlvmBackendError, LlvmModule, LlvmModuleArtifact, compile_bundled_module_artifact_to_llvm,
    compile_module_artifact_to_llvm, compile_program_to_llvm,
};
pub use options::{LlvmBackendOptions, OptLevel};

#[cfg(test)]
mod tests;
