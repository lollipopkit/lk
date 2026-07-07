//! LLVM backend for LK.

pub(crate) mod stmt {
    pub(crate) use lk_core::stmt::*;
}

pub(crate) mod vm {
    pub(crate) use lk_core::vm::*;
}

#[cfg(test)]
pub(crate) mod token {
    pub(crate) use lk_core::token::*;
}

pub mod llvm;
mod native_executable;

pub use lk_aot_lower::BundledImport;
pub use llvm::{
    LlvmBackend, LlvmBackendError, LlvmBackendOptions, LlvmModule, LlvmModuleArtifact, OptLevel,
    compile_bundled_module_artifact_to_llvm, compile_module_artifact_to_llvm, compile_program_to_llvm,
};
pub use native_executable::{
    HybridLink, compile_native_executable_from_llvm, compile_native_executable_from_llvm_hybrid,
};
