//! `lk fmt` — format the project's `.lk` sources in place.
//!
//! With no path argument it formats the whole project: the directory holding
//! the nearest `Lk.toml`, or the current directory when there is no manifest.
//! Explicit paths may be files or directories.

use std::path::{Path, PathBuf};

use lk_core::fmt::{FormatOptions, format_source};
use lk_core::package::find_manifest;

use crate::diagnostic;

/// Directories never worth walking: VCS/editor metadata and build output.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules"];

pub(crate) fn run_fmt(paths: &[PathBuf], check: bool) -> anyhow::Result<()> {
    let files = collect_files(paths)?;
    if files.is_empty() {
        println!("no .lk files found");
        return Ok(());
    }

    let options = FormatOptions::default();
    let mut changed: Vec<PathBuf> = Vec::new();
    let mut failed = 0usize;

    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(err) => {
                diagnostic::error(format!("read {}: {}", file.display(), err));
                failed += 1;
                continue;
            }
        };
        let formatted = match format_source(&source, options) {
            Ok(formatted) => formatted,
            Err(parse_err) => {
                // A file that does not tokenize is left untouched rather than
                // rewritten on a guess; the run still reports failure.
                eprintln!("{}:", file.display());
                diagnostic::parse_error(&parse_err, &source);
                failed += 1;
                continue;
            }
        };
        if formatted == source {
            continue;
        }
        changed.push(file.clone());
        if check {
            continue;
        }
        if let Err(err) = std::fs::write(file, &formatted) {
            diagnostic::error(format!("write {}: {}", file.display(), err));
            failed += 1;
            changed.pop();
            continue;
        }
        println!("formatted {}", file.display());
    }

    if check {
        for file in &changed {
            println!("would reformat {}", file.display());
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} of {} file(s) could not be formatted", files.len());
    }
    if check && !changed.is_empty() {
        anyhow::bail!(
            "{} of {} file(s) are not formatted (run `lk fmt`)",
            changed.len(),
            files.len()
        );
    }
    if changed.is_empty() {
        println!("{} file(s) already formatted", files.len());
    }
    Ok(())
}

/// Resolve CLI arguments to a deduplicated, deterministic file list.
fn collect_files(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if paths.is_empty() {
        collect_dir(&project_root()?, &mut files)?;
    } else {
        for path in paths {
            if path.is_dir() {
                collect_dir(path, &mut files)?;
            } else if path.is_file() {
                // An explicitly named file is formatted whatever its extension.
                files.push(path.clone());
            } else {
                anyhow::bail!("path not found: {}", path.display());
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Project root for a bare `lk fmt`: the nearest manifest's directory, else cwd.
fn project_root() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let root = find_manifest(&cwd)
        .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
        .unwrap_or(cwd);
    Ok(root)
}

fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| anyhow::anyhow!("read directory {}: {}", dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| anyhow::anyhow!("read directory {}: {}", dir.display(), e))?;
        // `file_type` does not follow symlinks, so a link loop cannot hang the walk.
        let file_type = entry.file_type()?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_dir(&path, out)?;
        } else if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("lk") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod fmt_test;
