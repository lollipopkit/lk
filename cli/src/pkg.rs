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

/// What a `<SOURCE>` argument names.
///
/// Decided by shape, and an unrecognised shape is **refused**. It used to be
/// written into the manifest verbatim as a GitHub repo, so
/// `lk pkg add dep ../dep` produced `dep = "../dep"` and the failure arrived
/// much later, from git, as
/// `repository 'https://github.com/../dep.git/' not found`. The manifest has
/// had `path` and `git` since it existed; only `add` could not spell them.
enum AddedSource {
    GitHub(String),
    Git(String),
    Path(String),
}

fn classify_source(source: &str) -> anyhow::Result<AddedSource> {
    let trimmed = source.trim();
    if trimmed.contains("://") || trimmed.starts_with("git@") {
        return Ok(AddedSource::Git(trimmed.to_string()));
    }
    if trimmed.starts_with("./") || trimmed.starts_with("../") || trimmed.starts_with('/') || trimmed.starts_with('~') {
        return Ok(AddedSource::Path(trimmed.to_string()));
    }
    // `owner/repo`: exactly one separator, both halves present, no spaces.
    let mut parts = trimmed.split('/');
    if let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next())
        && !owner.is_empty()
        && !repo.is_empty()
        && !trimmed.contains(char::is_whitespace)
    {
        return Ok(AddedSource::GitHub(trimmed.to_string()));
    }
    anyhow::bail!(
        "`{source}` is not a dependency source. Write `owner/repo` for GitHub, a URL \
         (`https://…` or `git@…`) for any other git host, or a path starting with `./`, `../` or `/` \
         for a local package"
    )
}

fn add_dependency(
    name: String,
    source: String,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
) -> anyhow::Result<()> {
    let (manifest_path, mut manifest) = load_project_manifest()?;
    let pinned = branch.is_some() || tag.is_some() || rev.is_some();
    let spec = match classify_source(&source)? {
        // A local package has no revision to pin, and silently keeping one in
        // the manifest would read as if it did.
        AddedSource::Path(path) if pinned => {
            anyhow::bail!("--branch/--tag/--rev do not apply to the path dependency `{path}`")
        }
        AddedSource::Path(path) => DependencySpec::Detailed(DetailedDependency {
            path: Some(path),
            ..Default::default()
        }),
        AddedSource::Git(url) => DependencySpec::Detailed(DetailedDependency {
            git: Some(url),
            branch,
            tag,
            rev,
            ..Default::default()
        }),
        // The bare-string form is the manifest's shorthand for GitHub, and it
        // only survives when there is nothing else to say.
        AddedSource::GitHub(repo) if !pinned => DependencySpec::GitHub(repo),
        AddedSource::GitHub(repo) => DependencySpec::Detailed(DetailedDependency {
            github: Some(repo),
            branch,
            tag,
            rev,
            ..Default::default()
        }),
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
        let dir = cache_dir_for_source(&source)?;
        // Named here: `git failed with status exit status: 128` says which
        // *process* failed, not which dependency — and with several of them the
        // reader has to guess. git's own message above already explains the
        // cause; this says what LK was doing when it appeared.
        fetch_git_dependency(&source, &dir, &spec)
            .with_context(|| format!("fetching dependency `{name}` from {source}"))?;
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
        // git has already printed its own diagnosis to stderr; repeating the
        // exit status adds nothing a reader can act on, so this only names the
        // command. The caller supplies which dependency it was for.
        let program = cmd.get_program().to_string_lossy().into_owned();
        let args: Vec<String> = cmd.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        anyhow::bail!("`{program} {}` failed (see git's message above)", args.join(" "));
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
        println!("  {} -> <{}>", missing.name, missing.advice());
    }
    Ok(())
}

fn check_package() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let graph = PackageGraph::discover(&cwd)?.ok_or_else(|| anyhow::anyhow!("No {MANIFEST_FILE} found"))?;
    if let Some(package) = &graph.manifest.package {
        lk_core::package::validate_package_section(package)?;
    }
    graph.validate_macro_distribution()?;
    if graph.missing.is_empty() {
        println!("package check ok");
    } else {
        for missing in &graph.missing {
            println!("  {} -> {}", missing.name, missing.advice());
        }
        // The per-dependency lines above already say what each one needs; a
        // summary that repeats one of the two answers for all of them is how a
        // path dependency got told to run `lk pkg fetch`.
        //
        // And it **fails**. "package check ok (1 dependencies unresolved)" said
        // two opposite things in one line and exited 0, so a CI step running
        // `lk pkg check` passed on a package that cannot run — which is the one
        // question this command exists to answer.
        anyhow::bail!(
            "{} dependencies unresolved — the package cannot run until they are",
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
