use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use lk_core::package::{MANIFEST_FILE, Manifest, PackageGraph, find_manifest};
use lk_core::syntax::{ParseOptions, ProgramExpansion, expand_program_source};

use crate::{CompileMode, diagnostic};

fn read_file_content(path: &str) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", path, e))
}

pub(crate) fn sanitize_path(raw: &str) -> anyhow::Result<PathBuf> {
    let p = Path::new(raw);

    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(anyhow::anyhow!(
                "Parent directory components ('..') are not allowed in file paths."
            ));
        }
    }

    Ok(p.to_path_buf())
}

pub(crate) fn parse_sanitized_path(raw: &str) -> Result<PathBuf, String> {
    sanitize_path(raw).map_err(|e| e.to_string())
}

pub(crate) fn expand_program_file(path: &Path) -> anyhow::Result<ProgramExpansion> {
    let src = read_file_content(&path.to_string_lossy())?;
    let options = parse_options_for_file(path)?;
    match expand_program_source(&src, options) {
        Ok(expansion) => Ok(expansion),
        Err(parse_err) => {
            diagnostic::parse_error(&parse_err, &src);
            std::process::exit(1);
        }
    }
}

pub(crate) fn parse_options_for_file(path: &Path) -> anyhow::Result<ParseOptions> {
    let mut options = ParseOptions {
        base_dir: path.parent().map(Path::to_path_buf),
        ..ParseOptions::default()
    };
    let Some(manifest_path) = find_manifest(path) else {
        return Ok(options);
    };
    let manifest = Manifest::read(&manifest_path)?;
    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("manifest has no parent: {}", manifest_path.display()))?;
    options.proc_macro_providers = match PackageGraph::discover(path)? {
        Some(graph) => graph.proc_macro_providers_for_manifest(&manifest_path)?,
        None => manifest.proc_macro_providers(manifest_dir),
    };
    Ok(options)
}

pub(crate) fn split_compile_args(args: &[String]) -> anyhow::Result<(CompileMode, PathBuf, Option<PathBuf>)> {
    let cwd = std::env::current_dir().context("read current directory")?;
    split_compile_args_with_cwd(args, &cwd)
}

/// The third element is the **default output path** when the entry was resolved
/// from a manifest rather than named on the command line.
///
/// A build output does not belong in `src/`. `lk compile` in a package resolves
/// the entry to `<pkg>/src/main.lk`, and the output path was "the entry without
/// its extension" — so `lk compile` dropped a 20 MB executable (and
/// `lk compile bytecode` a `.lkm`) *inside the source directory*, next to the
/// file it was built from, where the next `git add .` picks it up. A file the
/// user named keeps that rule (`lk compile foo.lk` → `foo`, which is what
/// naming a file means); a package build goes to the package root, the way
/// `go build` puts its binary in the module directory rather than under `src`.
pub(crate) fn split_compile_args_with_cwd(
    args: &[String],
    cwd: &Path,
) -> anyhow::Result<(CompileMode, PathBuf, Option<PathBuf>)> {
    match args.len() {
        0 => {
            let (entry, output) = default_compile_entry_with_output(cwd)?;
            Ok((CompileMode::Exe, entry, output))
        }
        1 => {
            if let Some(mode) = parse_compile_mode(&args[0])? {
                let (entry, output) = default_compile_entry_with_output(cwd)?;
                return Ok((mode, entry, output));
            }
            Ok((CompileMode::Exe, sanitize_path(&args[0])?, None))
        }
        2 => {
            let mode =
                parse_compile_mode(&args[0])?.ok_or_else(|| anyhow::anyhow!("Unknown compile target '{}'", args[0]))?;
            let file = sanitize_path(&args[1])?;
            Ok((mode, file, None))
        }
        _ => anyhow::bail!("compile requires [FILE], [TARGET], or [TARGET FILE]"),
    }
}

/// The implicit entry plus where its output belongs.
///
/// `None` for the `./main.lk` case: that is a loose file in the current
/// directory, and `main.lk` → `main` beside it is what naming it would have
/// done anyway.
fn default_compile_entry_with_output(cwd: &Path) -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    let entry = default_compile_entry(cwd)?;
    let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    // Resolved through `<root>/src/main.lk`: the output goes to `<root>/<stem>`,
    // named after the *package directory* rather than "main", which is what a
    // built program is called.
    let from_src = entry.parent().is_some_and(|dir| dir.ends_with("src"));
    if !from_src {
        return Ok((entry, None));
    }
    let name = entry
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".to_string());
    let package_root = entry
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or(root);
    Ok((entry, Some(package_root.join(name))))
}

fn parse_compile_mode(raw: &str) -> anyhow::Result<Option<CompileMode>> {
    let target = raw.to_ascii_lowercase();
    match target.as_str() {
        "bytecode" => Ok(Some(CompileMode::Bytecode)),
        // `object:<triple>` rather than a separate flag: the triple is not
        // optional for this mode, and pairing them makes an incomplete
        // invocation unrepresentable.
        other if other.starts_with("object:") => {
            let triple = other.trim_start_matches("object:").trim();
            if triple.is_empty() {
                anyhow::bail!("`object:` needs a target triple, e.g. `object:aarch64-unknown-none`");
            }
            Ok(Some(CompileMode::Object {
                triple: triple.to_string(),
            }))
        }
        "llvm" => anyhow::bail!(
            "`lk compile llvm` was removed with the LLVM-text backend; native executables are emitted via Cranelift (`lk compile [FILE]`)"
        ),
        "exe" => anyhow::bail!("`lk compile exe` was removed; use `lk compile [FILE]` for native executables"),
        _ => Ok(None),
    }
}

fn default_compile_entry(cwd: &Path) -> anyhow::Result<PathBuf> {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let root_main = cwd.join("main.lk");
    if root_main.exists() {
        return Ok(root_main);
    }

    let manifest_path = cwd.join(MANIFEST_FILE);
    if !manifest_path.exists() {
        anyhow::bail!("compile requires a file, or run it in a directory containing main.lk or Lk.toml");
    }

    let manifest = Manifest::read(&manifest_path)?;
    if manifest.package.is_none() {
        return default_workspace_compile_entry(&manifest, &cwd, &manifest_path);
    }

    let src_main = cwd.join("src").join("main.lk");
    if src_main.exists() {
        return Ok(src_main);
    }

    anyhow::bail!(
        "{} does not define an implicit compile entry; expected {}",
        manifest_path.display(),
        src_main.display()
    );
}

fn default_workspace_compile_entry(manifest: &Manifest, cwd: &Path, manifest_path: &Path) -> anyhow::Result<PathBuf> {
    let Some(workspace) = manifest.workspace.as_ref() else {
        anyhow::bail!(
            "{} does not define a package or workspace entry; specify the app entry file explicitly",
            manifest_path.display()
        );
    };

    let mut entries = Vec::new();
    for member in expand_workspace_member_dirs(cwd, &workspace.members)? {
        let member_manifest_path = member.join(MANIFEST_FILE);
        if !member_manifest_path.exists() {
            continue;
        }
        let member_manifest = Manifest::read(&member_manifest_path)?;
        if member_manifest.package.is_none() {
            continue;
        }
        let main = member.join("src").join("main.lk");
        if main.exists() {
            entries.push(main.canonicalize().unwrap_or(main));
        }
    }

    match entries.len() {
        1 => Ok(entries.remove(0)),
        0 => anyhow::bail!(
            "{} is a workspace manifest without a package entry and no member src/main.lk was found; specify the app entry file explicitly",
            manifest_path.display()
        ),
        _ => {
            entries.sort();
            let mut candidates = String::new();
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    candidates.push('\n');
                }
                candidates.push_str("  - ");
                candidates.push_str(&entry.display().to_string());
            }
            anyhow::bail!(
                "{} has multiple workspace app entries; specify one explicitly:\n{}",
                manifest_path.display(),
                candidates
            );
        }
    }
}

fn expand_workspace_member_dirs(root: &Path, members: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for member in members {
        if let Some(prefix) = member.strip_suffix("/*") {
            let dir = root.join(prefix);
            if !dir.exists() {
                continue;
            }
            for entry in fs::read_dir(&dir).with_context(|| format!("read workspace member glob {}", dir.display()))? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    out.push(entry.path());
                }
            }
        } else {
            out.push(root.join(member));
        }
    }
    out.sort();
    Ok(out)
}
