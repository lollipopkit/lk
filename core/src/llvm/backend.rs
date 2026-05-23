use anyhow::Result;

use crate::{
    stmt::Program,
    vm::{Compiler32, Module32Artifact},
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
        let module = Compiler32::compile_module(program)?;
        let artifact = Module32Artifact::new(crate::stmt::import::collect_program_imports(program), &module)?;
        compile_module32_artifact_to_llvm(&artifact, self.options.clone())
    }
}

pub fn compile_program_to_llvm(program: &Program, options: LlvmBackendOptions) -> Result<LlvmModuleArtifact> {
    LlvmBackend::new(options).compile_program(program)
}

pub fn compile_module32_artifact_to_llvm(
    artifact: &Module32Artifact,
    options: LlvmBackendOptions,
) -> Result<LlvmModuleArtifact> {
    let artifact_json = artifact.to_json_string()?;
    let escaped = llvm_escape_bytes(artifact_json.as_bytes());
    let len = artifact_json.len();
    let mut ir = String::new();
    ir.push_str(&format!("; ModuleID = '{}'\n", options.module_name));
    if let Some(triple) = &options.target_triple {
        ir.push_str(&format!("target triple = \"{}\"\n", llvm_escape_string(triple)));
    }
    ir.push_str(&format!(
        "@lk_module32_json = private unnamed_addr constant [{} x i8] c\"{}\", align 1\n\n",
        len, escaped
    ));
    ir.push_str("declare i32 @lk_rt_run_module32_json(ptr, i64)\n\n");
    ir.push_str("define i32 @main() {\n");
    ir.push_str("entry:\n");
    ir.push_str(&format!(
        "  %status = call i32 @lk_rt_run_module32_json(ptr @lk_module32_json, i64 {})\n",
        len
    ));
    ir.push_str("  ret i32 %status\n");
    ir.push_str("}\n");

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

fn llvm_escape_string(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:02X}")),
        }
    }
    out
}

fn llvm_escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &byte in bytes {
        match byte {
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\{byte:02X}")),
        }
    }
    out
}
