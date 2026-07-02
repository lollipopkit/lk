use anyhow::{Result, bail};

use crate::{
    stmt::Program,
    vm::{Compiler, ModuleArtifact},
};

use super::options::{LlvmBackendOptions, OptLevel};

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

    pub fn compile_program(&self, program: &Program) -> Result<LlvmModuleArtifact> {
        let module = Compiler::compile_module(program)?;
        let artifact = ModuleArtifact::new(crate::stmt::import::collect_program_imports(program), &module)?;
        compile_module_artifact_to_llvm(&artifact, self.options.clone())
    }
}

pub fn compile_program_to_llvm(program: &Program, options: LlvmBackendOptions) -> Result<LlvmModuleArtifact> {
    LlvmBackend::new(options).compile_program(program)
}

pub fn compile_module_artifact_to_llvm(
    artifact: &ModuleArtifact,
    options: LlvmBackendOptions,
) -> Result<LlvmModuleArtifact> {
    // The typed MIR pipeline (`docs/llvm/aot-redesign.md`) is the only backend:
    // `lk-aot-lower` is the total capability predicate, `lk_aot_mir::validate`
    // is enforced on the production path, and `lk-aot-codegen` renders the
    // validated module. Shapes the lowering rejects fail with their precise
    // `Unsupported` reason instead of falling back or embedding a VM shell.
    let mir = match lk_aot_lower::lower(artifact) {
        Ok(mir) => mir,
        Err(unsupported) => {
            bail!("LLVM native lowering does not support this ModuleArtifact shape yet (MIR lowering: {unsupported})")
        }
    };
    // Correctness gate: codegen documents "renders a *validated* module", so
    // enforce that precondition on the production path instead of only in
    // tests. A failure here is a lowering bug, never a user error.
    if let Err(error) = lk_aot_mir::validate(&mir) {
        bail!("internal AOT error: MIR validation failed after lowering: {error:?}");
    }
    let mut ir = lk_aot_codegen::render_module(&mir);
    if let Some(triple) = &options.target_triple {
        ir = ir.replacen(
            "; ModuleID = 'lk_aot'\n",
            &format!("; ModuleID = 'lk_aot'\ntarget triple = \"{triple}\"\n"),
            1,
        );
    }
    Ok(LlvmModuleArtifact {
        module: LlvmModule {
            name: options.module_name,
            ir,
            target_triple: options.target_triple,
        },
        optimised_ir: None,
        opt_level: options.opt_level,
    })
}
