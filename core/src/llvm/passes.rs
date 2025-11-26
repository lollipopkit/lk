use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use llvm_tools::LlvmTools;
use tempfile::NamedTempFile;

use super::options::OptLevel;

fn resolve_opt_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("LKR_LLVM_OPT") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(tools) = LlvmTools::new()
        && let Some(path) = tools.tool("opt")
    {
        return Some(path);
    }

    Some(PathBuf::from("opt"))
}

/// Runs LLVM's `opt` tool on the provided IR, returning the optimised IR when successful.
///
/// If `opt` is not installed or exits with a non-zero status, the original IR is returned and
/// a warning is emitted via `tracing::warn!`.
pub fn run_opt(ir: &str, opt_level: OptLevel) -> Result<Option<String>> {
    let opt_path = resolve_opt_path().ok_or_else(|| anyhow!("missing opt path"))?;

    let mut input_file = NamedTempFile::new().context("create temporary input for opt")?;
    input_file
        .write_all(ir.as_bytes())
        .context("write LLVM IR to temporary file")?;
    let input_path = input_file.into_temp_path();

    let output_file = NamedTempFile::new().context("create temporary output for opt")?;
    let output_path = output_file.into_temp_path();

    let status = Command::new(&opt_path)
        .arg(opt_level.as_flag())
        .arg("-S")
        .arg(&*input_path)
        .arg("-o")
        .arg(&*output_path)
        .status();

    match status {
        Ok(status) => {
            if !status.success() {
                tracing::warn!(
                    "opt exited with {} (path={:?}); keeping unoptimised IR",
                    status,
                    opt_path
                );
                return Ok(None);
            }
            let optimised = fs::read_to_string(&*output_path).context("read optimised LLVM IR from opt")?;
            Ok(Some(optimised))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("opt binary not found at {:?}; keeping unoptimised IR", opt_path);
            Ok(None)
        }
        Err(err) => Err(err).context("failed to spawn opt"),
    }
}
