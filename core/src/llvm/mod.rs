//! LLVM backend entry points and helpers.
//!
//! The backend translates lowered VM bytecode functions into textual LLVM IR.

mod backend;
mod encoding;
mod options;
pub mod runtime;

pub use backend::{LlvmBackend, LlvmBackendError, LlvmModule, LlvmModuleArtifact, compile_program_to_llvm};
pub use options::{LlvmBackendOptions, OptLevel};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{stmt::stmt_parser::StmtParser, token::Tokenizer};

    #[test]
    fn llvm_backend_is_disabled_during_instr32_migration() {
        let tokens = Tokenizer::tokenize("1 + 2;").expect("tokens");
        let program = StmtParser::new(&tokens).parse_program().expect("program");

        let err = compile_program_to_llvm(&program, LlvmBackendOptions::default()).expect_err("llvm is disabled");

        assert!(
            err.to_string().contains("Instr32 VM migration"),
            "unexpected LLVM disabled error: {err}"
        );
    }
}
