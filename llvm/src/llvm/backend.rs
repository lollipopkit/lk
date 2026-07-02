use anyhow::{Result, bail};

use crate::{
    stmt::Program,
    vm::{Compiler, ModuleArtifact},
};

use super::{
    diagnostics::unsupported_module_artifact_reason,
    options::{LlvmBackendOptions, OptLevel},
    straightline_main::compile_native_scalar_main_artifact,
};

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
    // Strangler path (`docs/llvm/aot-redesign.md`): shapes the typed MIR lowering
    // accepts compile through `lk-aot-lower` → `lk-aot-codegen`; anything it
    // rejects falls through to the legacy text backend. **Default on** — the MIR
    // pipeline is the primary backend; `LK_AOT_MIR=0` (or
    // `options.use_mir_pipeline = Some(false)`) opts a compile back onto the
    // legacy path (used by tests that assert legacy-backend IR structure, which
    // retire together with the legacy backend).
    let use_mir = options
        .use_mir_pipeline
        .unwrap_or_else(|| std::env::var_os("LK_AOT_MIR").is_none_or(|v| v != "0"));
    let lowered = if use_mir {
        Some(lk_aot_lower::lower(artifact))
    } else {
        None
    };
    if let Some(Ok(mir)) = lowered {
        let mut ir = lk_aot_codegen::render_module(&mir);
        if let Some(triple) = &options.target_triple {
            ir = ir.replacen(
                "; ModuleID = 'lk_aot'\n",
                &format!("; ModuleID = 'lk_aot'\ntarget triple = \"{triple}\"\n"),
                1,
            );
        }
        return Ok(LlvmModuleArtifact {
            module: LlvmModule {
                name: options.module_name,
                ir,
                target_triple: options.target_triple,
            },
            optimised_ir: None,
            opt_level: options.opt_level,
        });
    }

    if let Some(ir) = compile_native_scalar_main_artifact(artifact, &options)? {
        return Ok(LlvmModuleArtifact {
            module: LlvmModule {
                name: options.module_name,
                ir,
                target_triple: options.target_triple,
            },
            optimised_ir: None,
            opt_level: options.opt_level,
        });
    }

    bail!(
        "LLVM native lowering does not support this ModuleArtifact shape yet: {}{}",
        unsupported_module_artifact_reason(artifact),
        match lowered {
            Some(Err(unsupported)) => format!(" (MIR lowering: {unsupported})"),
            _ => String::new(),
        }
    )
}
