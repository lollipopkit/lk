use std::path::Path;

pub(crate) fn run_coverage_report(path: &Path, runtime: bool) -> anyhow::Result<()> {
    let _ = runtime;
    anyhow::bail!(
        "coverage is disabled during the Instr32 VM migration and must be rebuilt on the new IR: {}",
        path.display()
    )
}
