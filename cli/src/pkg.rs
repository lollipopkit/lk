use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use lk_core::package::{
    DependencySpec, DetailedDependency, LOCK_FILE, LockFile, LockedPackage, MANIFEST_FILE, Manifest, PackageGraph,
    PackageSection, cache_dir_for_source, find_manifest,
};

use crate::PkgCommand;

pub(crate) fn run_pkg_command(command: PkgCommand) -> anyhow::Result<()> {
    match command {
        PkgCommand::Init { name } => init_package(name),
        PkgCommand::Add {
            name,
            source,
            branch,
            tag,
            rev,
        } => add_dependency(name, source, branch, tag, rev),
        PkgCommand::Fetch => fetch_dependencies(None),
        PkgCommand::Update { name } => fetch_dependencies(name),
        PkgCommand::Check => check_package(),
        PkgCommand::Tree => print_package_tree(),
    }
}

fn load_project_manifest() -> anyhow::Result<(PathBuf, Manifest)> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let manifest_path = find_manifest(&cwd).ok_or_else(|| anyhow::anyhow!("No {MANIFEST_FILE} found"))?;
    let manifest = Manifest::read(&manifest_path)?;
    Ok((manifest_path, manifest))
}

fn add_dependency(
    name: String,
    source: String,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
) -> anyhow::Result<()> {
    let (manifest_path, mut manifest) = load_project_manifest()?;
    let spec = if branch.is_none() && tag.is_none() && rev.is_none() {
        DependencySpec::GitHub(source)
    } else {
        DependencySpec::Detailed(DetailedDependency {
            github: Some(source),
            branch,
            tag,
            rev,
            ..Default::default()
        })
    };
    manifest.dependencies.insert(name, spec);
    manifest.write(&manifest_path)?;
    eprintln!("Updated {}", manifest_path.display());
    Ok(())
}

/// Resolve every git/GitHub dependency into `Lk.lock` (Deno/Go-style
/// decentralized deps — git URL + pinned rev, no central registry). Workspace
/// and path dependencies are local and need no fetch.
fn fetch_dependencies(only: Option<String>) -> anyhow::Result<()> {
    let (manifest_path, manifest) = load_project_manifest()?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("manifest has no parent"))?;
    let mut lock = LockFile::read(&root.join(LOCK_FILE))?;
    let mut locked = BTreeMap::new();
    for pkg in lock.package {
        locked.insert(pkg.name.clone(), pkg);
    }

    for (name, spec) in manifest.dependencies {
        if only.as_ref().is_some_and(|only| only != &name) {
            continue;
        }
        if spec.is_workspace() || spec.path().is_some() {
            continue;
        }
        let source = spec
            .git_url()
            .ok_or_else(|| anyhow::anyhow!("dependency '{name}' has no git source"))?;
        let dir = cache_dir_for_source(&source);
        fetch_git_dependency(&source, &dir, &spec)?;
        let rev = git_output(&dir, ["rev-parse", "HEAD"])?;
        locked.insert(
            name.clone(),
            LockedPackage {
                name,
                source,
                rev,
                checksum: None,
            },
        );
    }

    lock = LockFile {
        package: locked.into_values().collect(),
    };
    lock.write(&root.join(LOCK_FILE))?;
    eprintln!("Updated {}", root.join(LOCK_FILE).display());
    Ok(())
}

fn fetch_git_dependency(source: &str, dir: &Path, spec: &DependencySpec) -> anyhow::Result<()> {
    if dir.exists() {
        git_status(
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .arg("fetch")
                .arg("--tags")
                .arg("--prune"),
        )?;
    } else {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        git_status(Command::new("git").arg("clone").arg(source).arg(dir))?;
    }

    if let DependencySpec::Detailed(dep) = spec {
        if let Some(rev) = dep.rev.as_ref() {
            git_status(Command::new("git").arg("-C").arg(dir).arg("checkout").arg(rev))?;
        } else if let Some(tag) = dep.tag.as_ref() {
            git_status(
                Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .arg("checkout")
                    .arg(format!("tags/{tag}")),
            )?;
        } else if let Some(branch) = dep.branch.as_ref() {
            git_status(Command::new("git").arg("-C").arg(dir).arg("checkout").arg(branch))?;
            git_status(Command::new("git").arg("-C").arg(dir).arg("pull").arg("--ff-only"))?;
        }
    }
    Ok(())
}

fn git_status(cmd: &mut Command) -> anyhow::Result<()> {
    let status = cmd.status().context("run git")?;
    if !status.success() {
        anyhow::bail!("git failed with status {status}");
    }
    Ok(())
}

fn git_output<const N: usize>(dir: &Path, args: [&str; N]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("run git")?;
    if !output.status.success() {
        anyhow::bail!("git failed with status {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn print_package_tree() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let graph = PackageGraph::discover(&cwd)?.ok_or_else(|| anyhow::anyhow!("No {MANIFEST_FILE} found"))?;
    let root_name = graph
        .manifest
        .package
        .as_ref()
        .map(|package| package.name.as_str())
        .unwrap_or("<workspace>");
    println!("{root_name} ({})", graph.manifest_dir().display());
    for module in &graph.modules {
        println!("  {} -> {}", module.name, module.root.display());
    }
    for missing in &graph.missing {
        println!("  {} -> <missing; run lk pkg fetch>", missing);
    }
    Ok(())
}

fn check_package() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let graph = PackageGraph::discover(&cwd)?.ok_or_else(|| anyhow::anyhow!("No {MANIFEST_FILE} found"))?;
    graph.validate_macro_distribution()?;
    if graph.missing.is_empty() {
        println!("package check ok");
    } else {
        println!(
            "package check ok ({} missing dependencies; run lk pkg fetch)",
            graph.missing.len()
        );
    }
    Ok(())
}

pub(crate) fn init_package(name: Option<String>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let package_name = name.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("lk-package")
            .to_string()
    });
    let manifest_path = cwd.join(MANIFEST_FILE);
    if manifest_path.exists() {
        anyhow::bail!("{} already exists", manifest_path.display());
    }
    let manifest = Manifest {
        package: Some(PackageSection {
            name: package_name.clone(),
            version: Some("0.1.0".to_string()),
            edition: Some("2026".to_string()),
            license: None,
            authors: Vec::new(),
            description: None,
        }),
        workspace: None,
        dependencies: BTreeMap::new(),
        macros: Default::default(),
    };
    manifest.write(&manifest_path)?;
    let src_dir = cwd.join("src");
    fs::create_dir_all(&src_dir).with_context(|| format!("create {}", src_dir.display()))?;
    let main_path = src_dir.join("main.lk");
    if !main_path.exists() {
        fs::write(
            &main_path,
            "println(\"hello from ${pkg}\");\n".replace("${pkg}", &package_name),
        )
        .with_context(|| format!("write {}", main_path.display()))?;
    }
    eprintln!("Created {}", manifest_path.display());
    Ok(())
}
