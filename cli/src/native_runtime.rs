use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use llvm_tools::LlvmTools;

const RUNTIME_CRATE_NAME: &str = "lk-aot-runtime";

pub(crate) fn resolve_llvm_tool(tool: &str, env_var: &str) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(env_var) {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(tools) = LlvmTools::new()
        && let Some(path) = tools.tool(tool)
    {
        return Some(path);
    }
    let fallback = PathBuf::from(tool);
    if Command::new(&fallback)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
    {
        Some(fallback)
    } else {
        None
    }
}

pub(crate) fn ensure_runtime_staticlib(target_triple: Option<&str>, use_release: bool) -> anyhow::Result<Vec<PathBuf>> {
    if let Some(packaged) = find_packaged_staticlibs(target_triple, use_release) {
        return Ok(packaged);
    }
    Ok(vec![build_staticlib(RUNTIME_CRATE_NAME, target_triple, use_release)?])
}

fn build_staticlib(crate_name: &str, target_triple: Option<&str>, use_release: bool) -> anyhow::Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let runtime_target_root = std::env::var("LK_RUNTIME_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            }
        })
        .unwrap_or_else(|_| workspace_root.join("target").join("lk-native"));

    let mut cmd = Command::new(&cargo);
    cmd.arg("build");
    if crate_name == RUNTIME_CRATE_NAME {
        cmd.arg("--manifest-path")
            .arg(workspace_root.join("aot-runtime").join("Cargo.toml"));
    } else {
        cmd.arg("-p").arg(crate_name);
    }
    cmd.arg("--lib");
    if use_release {
        cmd.arg("--release");
    }
    if let Some(triple) = target_triple {
        cmd.arg("--target").arg(triple);
    }
    cmd.current_dir(&workspace_root);
    cmd.env("CARGO_TARGET_DIR", &runtime_target_root);
    let status = cmd
        .status()
        .with_context(|| format!("failed to run `{cargo} build` for {crate_name} staticlib"))?;
    if !status.success() {
        anyhow::bail!("{cargo} build exited with status {status}");
    }

    let mut lib_path = runtime_target_root.clone();
    if let Some(triple) = target_triple {
        lib_path.push(triple);
    }
    lib_path.push(if use_release { "release" } else { "debug" });
    let crate_stub = crate_name.replace('-', "_");
    lib_path.push(format!("lib{crate_stub}.a"));
    if !lib_path.exists() {
        anyhow::bail!(
            "runtime static library {} was not produced (expected `{}`)",
            crate_name,
            lib_path.display()
        );
    }
    Ok(lib_path)
}

fn find_packaged_staticlibs(target_triple: Option<&str>, use_release: bool) -> Option<Vec<PathBuf>> {
    let mut roots = Vec::new();
    if let Ok(env_dir) = std::env::var("LK_RUNTIME_LIB_DIR") {
        let candidate = PathBuf::from(env_dir);
        if candidate.exists() {
            roots.push(candidate);
        }
    }

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(bin_dir) = exe_path.parent()
    {
        roots.push(bin_dir.to_path_buf());
        roots.push(bin_dir.join("lib"));
        if let Some(parent) = bin_dir.parent() {
            roots.push(parent.to_path_buf());
            roots.push(parent.join("lib"));
        }
    }

    let profile_dir = if use_release { "release" } else { "debug" };
    let mut seen = std::collections::HashSet::new();

    for root in roots.into_iter() {
        if !seen.insert(root.clone()) {
            continue;
        }

        let mut dirs = vec![
            root.clone(),
            root.join(profile_dir),
            root.join("lib"),
            root.join("lib").join(profile_dir),
        ];
        if let Some(triple) = target_triple {
            dirs.push(root.join(triple));
            dirs.push(root.join(triple).join(profile_dir));
            dirs.push(root.join("lib").join(triple));
            dirs.push(root.join("lib").join(triple).join(profile_dir));
        }

        for dir in dirs {
            if let Some(paths) = staticlibs_from_dir(&dir) {
                return Some(paths);
            }
        }
    }

    None
}

fn staticlibs_from_dir(dir: &Path) -> Option<Vec<PathBuf>> {
    if !dir.exists() {
        return None;
    }

    let filename = format!("lib{}.a", RUNTIME_CRATE_NAME.replace('-', "_"));
    let path = dir.join(filename);
    if !path.exists() {
        return None;
    }

    Some(vec![path])
}

#[cfg(test)]
mod tests {
    use super::staticlibs_from_dir;
    use std::fs;

    #[test]
    fn uses_env_dir_when_all_libs_present() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("liblk_aot_runtime.a");
        fs::write(&path, []).expect("write stub lib");

        let libs = staticlibs_from_dir(temp.path()).expect("should discover libs");
        assert_eq!(libs.len(), 1);
        assert!(libs.iter().all(|p| p.exists()));
    }
}
