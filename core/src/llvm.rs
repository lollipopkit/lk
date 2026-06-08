//! LLVM backend entry points and helpers.
//!
//! The backend lowers supported `ModuleArtifact` entry functions directly.
//! Unsupported shapes are rejected instead of falling back to an Instr artifact
//! shell or alternate VM.

mod backend;
mod callee_eval;
mod const_display;
mod diagnostics;
mod dynamic_containers;
mod intrinsics;
mod ir_text;
mod known_key;
mod map_mutate;
mod options;
mod output;
mod scalar;
mod straightline_main;
mod straightline_value;
mod subfunction;

pub use backend::{
    LlvmBackend, LlvmBackendError, LlvmModule, LlvmModuleArtifact, compile_module_artifact_to_llvm,
    compile_program_to_llvm,
};
pub use options::{LlvmBackendOptions, OptLevel};

#[cfg(test)]
mod tests;
