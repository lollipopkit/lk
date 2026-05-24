//! LLVM backend entry points and helpers.
//!
//! The backend lowers supported `Module32Artifact` entry functions directly.
//! Unsupported shapes are rejected instead of falling back to an Instr32 artifact
//! shell or alternate VM.

mod backend;
mod callee_eval;
mod const_display;
mod ir_text;
mod options;
mod scalar_emit;
mod scalar_facts;
mod straightline_value;

pub use backend::{
    LlvmBackend, LlvmBackendError, LlvmModule, LlvmModuleArtifact, compile_module32_artifact_to_llvm,
    compile_program_to_llvm,
};
pub use options::{LlvmBackendOptions, OptLevel};

#[cfg(test)]
mod tests;
