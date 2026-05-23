//! LLVM backend entry points and helpers.
//!
//! The backend emits a small LLVM shell around canonical `Instr32` module
//! artifacts. It does not target the removed bytecode VM.

mod backend;
mod options;
pub mod runtime;

pub use backend::{
    LlvmBackend, LlvmBackendError, LlvmModule, LlvmModuleArtifact, compile_module32_artifact_to_llvm,
    compile_program_to_llvm,
};
pub use options::{LlvmBackendOptions, OptLevel};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{stmt::stmt_parser::StmtParser, token::Tokenizer};

    #[test]
    fn llvm_backend_embeds_instr32_module_artifact() {
        let tokens = Tokenizer::tokenize("1 + 2;").expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");

        let artifact = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect("llvm artifact");

        assert!(artifact.module.ir.contains("@lk_module32_json"));
        assert!(artifact.module.ir.contains("lk_rt_run_module32_json"));
        assert!(artifact.module.ir.contains("lk.module32"));
    }
}
