//! LLVM backend entry points and helpers.
//!
//! The backend translates lowered VM bytecode functions into textual LLVM IR.

mod backend;
mod encoding;
mod options;
mod passes;
mod runtime;

#[cfg(test)]
mod tests;

pub use backend::{
    LlvmBackend, LlvmBackendError, LlvmModule, LlvmModuleArtifact, compile_function_to_llvm, compile_program_to_llvm,
};
pub use options::{LlvmBackendOptions, OptLevel};
