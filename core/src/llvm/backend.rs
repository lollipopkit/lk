use anyhow::{Result, bail};

use crate::stmt::Program;

use super::options::{LlvmBackendOptions, OptLevel};

const DISABLED_MESSAGE: &str =
    "LLVM backend is disabled during the Instr32 VM migration; reintroduce it on top of Instr32, not old bytecode";

pub type LlvmBackendError = anyhow::Error;

/// Metadata for an emitted LLVM module.
#[derive(Debug, Clone)]
pub struct LlvmModule {
    pub name: String,
    pub ir: String,
    pub target_triple: Option<String>,
}

/// Aggregates the raw IR plus optional optimized IR produced by `opt`.
#[derive(Debug, Clone)]
pub struct LlvmModuleArtifact {
    pub module: LlvmModule,
    pub optimised_ir: Option<String>,
    pub opt_level: OptLevel,
}

#[derive(Debug, Default)]
pub struct LlvmBackend {
    options: LlvmBackendOptions,
}

impl LlvmBackend {
    pub fn new(options: LlvmBackendOptions) -> Self {
        Self { options }
    }

    pub fn options(&self) -> &LlvmBackendOptions {
        &self.options
    }

    pub fn with_options(mut self, options: LlvmBackendOptions) -> Self {
        self.options = options;
        self
    }

    pub fn compile_program(&self, _program: &Program) -> Result<LlvmModuleArtifact> {
        bail!(DISABLED_MESSAGE)
    }
}

pub fn compile_program_to_llvm(_program: &Program, _options: LlvmBackendOptions) -> Result<LlvmModuleArtifact> {
    bail!(DISABLED_MESSAGE)
}
