use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use lk_core::package::{
    DependencySpec, DetailedDependency, LOCK_FILE, LockFile, LockedPackage, MANIFEST_FILE, Manifest, PackageGraph,
    PackageSection, RegistryPublishManifest, cache_dir_for_source, find_manifest, lk_home,
};
use semver::{Version, VersionReq};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{PkgCommand, PkgIndexCommand};

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
        PkgCommand::Fetch { offline } => fetch_dependencies(None, offline),
        PkgCommand::Update { name, offline } => fetch_dependencies(name, offline),
        PkgCommand::Check => check_package(),
        PkgCommand::Publish { dry_run } => publish_package(dry_run),
        PkgCommand::Yank { name, version, undo } => yank_package_version(name, version, undo),
        PkgCommand::Index { command } => run_pkg_index_command(command),
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

fn fetch_dependencies(only: Option<String>, offline: bool) -> anyhow::Result<()> {
    let (manifest_path, manifest) = load_project_manifest()?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("manifest has no parent"))?;
    let mut lock = LockFile::read(&root.join(LOCK_FILE))?;
    let mut locked = BTreeMap::new();
    for pkg in lock.package {
        locked.insert(pkg.name.clone(), pkg);
    }

    let registry_url = manifest.registry.as_ref().and_then(|registry| registry.url.clone());
    let registry_name = manifest
        .registry
        .as_ref()
        .and_then(|registry| registry.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let dependencies = manifest.dependencies;
    for (name, spec) in dependencies {
        if only.as_ref().is_some_and(|only| only != &name) {
            continue;
        }
        if spec.is_workspace() || spec.path().is_some() {
            continue;
        }
        let registry_resolution = if spec.git_url().is_none() && spec.registry_version().is_some() {
            Some(resolve_registry_dependency(
                &name,
                &spec,
                registry_url.as_deref(),
                registry_name.as_deref(),
                offline,
            )?)
        } else {
            None
        };
        let source = registry_resolution
            .as_ref()
            .map(|resolution| resolution.source.clone())
            .or_else(|| spec.git_url())
            .ok_or_else(|| anyhow::anyhow!("dependency '{name}' has no git source or registry version"))?;
        let dir = cache_dir_for_source(&source);
        fetch_git_dependency(&source, &dir, &spec)?;
        if let Some(resolution) = registry_resolution.as_ref() {
            git_status(
                Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .arg("checkout")
                    .arg(&resolution.rev),
            )?;
            verify_registry_checksum(&name, &dir, resolution.checksum.as_deref())?;
        }
        let rev = git_output(&dir, ["rev-parse", "HEAD"])?;
        locked.insert(
            name.clone(),
            LockedPackage {
                name,
                source,
                rev,
                checksum: registry_resolution.and_then(|resolution| resolution.checksum),
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

#[derive(Debug, Deserialize)]
struct RegistryDependencyResolution {
    source: String,
    rev: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    publish_manifest: Option<RegistryPublishManifest>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct RegistryPackageVersion {
    version: String,
    source: String,
    rev: String,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    publish_manifest: Option<RegistryPublishManifest>,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
struct RegistryIndexMacroProviders {
    #[serde(default)]
    derive: Vec<String>,
    #[serde(default)]
    attribute: Vec<String>,
    #[serde(default)]
    function_like: Vec<String>,
    #[serde(default)]
    trusted_dependencies: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct RegistryIndexPackage {
    name: String,
    #[serde(default)]
    versions: Vec<RegistryPackageVersion>,
    #[serde(default)]
    macro_providers: RegistryIndexMacroProviders,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct RegistryIndexSnapshot {
    #[serde(default)]
    packages: Vec<RegistryIndexPackage>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct RegistryIndexCache {
    registry: String,
    registry_url: String,
    packages: Vec<RegistryIndexPackage>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RegistryPackageVersionsResponse {
    Versions { versions: Vec<RegistryPackageVersion> },
    List(Vec<RegistryPackageVersion>),
}

impl RegistryPackageVersionsResponse {
    fn into_versions(self) -> Vec<RegistryPackageVersion> {
        match self {
            Self::Versions { versions } => versions,
            Self::List(versions) => versions,
        }
    }
}

fn resolve_registry_dependency(
    name: &str,
    spec: &DependencySpec,
    default_registry_url: Option<&str>,
    default_registry_name: Option<&str>,
    offline: bool,
) -> anyhow::Result<RegistryDependencyResolution> {
    let version = spec
        .registry_version()
        .ok_or_else(|| anyhow::anyhow!("dependency '{name}' has no registry version"))?;
    let registry_url = spec
        .registry_override()
        .or(default_registry_url)
        .ok_or_else(|| anyhow::anyhow!("dependency '{name}' requires [registry].url or dependency.registry"))?;
    let registry_name = if spec.registry_override().is_some() {
        None
    } else {
        default_registry_name
    };
    if offline {
        return resolve_registry_dependency_from_index(name, version, registry_name, registry_url);
    }
    if !is_exact_semver(version) {
        return resolve_registry_version_range(name, version, registry_url);
    }
    let endpoint = registry_dependency_endpoint(registry_url, name, version);
    match ureq::get(&endpoint).call() {
        Ok(response) if (200..300).contains(&response.status()) => {
            let body = response.into_string().context("read registry dependency response")?;
            let resolution: RegistryDependencyResolution =
                serde_json::from_str(&body).context("parse registry dependency response")?;
            validate_registry_dependency_publish_manifest(name, version, registry_url, &resolution)?;
            Ok(resolution)
        }
        Ok(response) => {
            let status = response.status();
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry dependency resolution failed for {name} {version} with status {status}: {body}");
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry dependency resolution failed for {name} {version} with status {status}: {body}");
        }
        Err(error) => Err(anyhow::anyhow!(
            "registry dependency resolution request failed for {name} {version}: {error}"
        )),
    }
}

fn resolve_registry_dependency_from_index(
    name: &str,
    requirement: &str,
    registry_name: Option<&str>,
    registry_url: &str,
) -> anyhow::Result<RegistryDependencyResolution> {
    let cache = read_registry_index_cache(registry_name, registry_url)?;
    if cache.registry_url.trim_end_matches('/') != registry_url.trim_end_matches('/') {
        anyhow::bail!(
            "registry index cache URL mismatch for `{name}`: expected {}, found {}; run `lk pkg index sync`",
            registry_url,
            cache.registry_url
        );
    }
    let package = cache
        .packages
        .into_iter()
        .find(|package| package.name == name)
        .ok_or_else(|| anyhow::anyhow!("registry index cache has no package `{name}`; run `lk pkg index sync`"))?;
    if is_exact_semver(requirement) {
        return select_registry_exact_version(name, requirement, package.versions);
    }
    let requirement = VersionReq::parse(requirement)
        .with_context(|| format!("parse registry version requirement `{requirement}` for dependency `{name}`"))?;
    select_registry_version(name, &requirement, package.versions)
}

fn read_registry_index_cache(registry_name: Option<&str>, registry_url: &str) -> anyhow::Result<RegistryIndexCache> {
    let cache_path = registry_index_cache_path(registry_name, registry_url);
    let body = fs::read_to_string(&cache_path)
        .with_context(|| format!("read registry index cache {}", cache_path.display()))?;
    let cache: RegistryIndexCache =
        serde_json::from_str(&body).with_context(|| format!("parse registry index cache {}", cache_path.display()))?;
    validate_registry_index_publish_manifests(&cache)?;
    Ok(cache)
}

fn select_registry_exact_version(
    name: &str,
    version: &str,
    versions: Vec<RegistryPackageVersion>,
) -> anyhow::Result<RegistryDependencyResolution> {
    versions
        .into_iter()
        .filter(|candidate| !candidate.yanked)
        .find(|candidate| candidate.version == version)
        .map(registry_dependency_resolution_from_version)
        .ok_or_else(|| {
            anyhow::anyhow!("registry index cache has no non-yanked version of `{name}` matching `{version}`")
        })
}

fn resolve_registry_version_range(
    name: &str,
    requirement: &str,
    registry_url: &str,
) -> anyhow::Result<RegistryDependencyResolution> {
    let requirement = VersionReq::parse(requirement)
        .with_context(|| format!("parse registry version requirement `{requirement}` for dependency `{name}`"))?;
    let endpoint = registry_package_versions_endpoint(registry_url, name);
    let versions = match ureq::get(&endpoint).call() {
        Ok(response) if (200..300).contains(&response.status()) => {
            let body = response
                .into_string()
                .context("read registry package versions response")?;
            let response: RegistryPackageVersionsResponse =
                serde_json::from_str(&body).context("parse registry package versions response")?;
            let versions = response.into_versions();
            validate_registry_package_version_manifests(name, registry_url, &versions)?;
            versions
        }
        Ok(response) => {
            let status = response.status();
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry version listing failed for {name} {requirement} with status {status}: {body}");
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry version listing failed for {name} {requirement} with status {status}: {body}");
        }
        Err(error) => Err(anyhow::anyhow!(
            "registry version listing request failed for {name} {requirement}: {error}"
        ))?,
    };

    select_registry_version(name, &requirement, versions)
}

fn select_registry_version(
    name: &str,
    requirement: &VersionReq,
    versions: Vec<RegistryPackageVersion>,
) -> anyhow::Result<RegistryDependencyResolution> {
    versions
        .into_iter()
        .filter(|candidate| !candidate.yanked)
        .filter_map(|candidate| {
            let version = Version::parse(&candidate.version).ok()?;
            requirement.matches(&version).then_some((version, candidate))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, candidate)| registry_dependency_resolution_from_version(candidate))
        .ok_or_else(|| anyhow::anyhow!("registry has no non-yanked version of `{name}` matching `{requirement}`"))
}

fn registry_dependency_resolution_from_version(candidate: RegistryPackageVersion) -> RegistryDependencyResolution {
    RegistryDependencyResolution {
        source: candidate.source,
        rev: candidate.rev,
        checksum: candidate.checksum,
        publish_manifest: candidate.publish_manifest,
    }
}

fn validate_registry_index_publish_manifests(cache: &RegistryIndexCache) -> anyhow::Result<()> {
    for package in &cache.packages {
        validate_registry_package_version_manifests(&package.name, &cache.registry_url, &package.versions)?;
    }
    Ok(())
}

fn validate_registry_package_version_manifests(
    name: &str,
    registry_url: &str,
    versions: &[RegistryPackageVersion],
) -> anyhow::Result<()> {
    for version in versions {
        if let Some(manifest) = &version.publish_manifest {
            validate_registry_publish_manifest(name, &version.version, registry_url, manifest)?;
        }
    }
    Ok(())
}

fn validate_registry_dependency_publish_manifest(
    name: &str,
    version: &str,
    registry_url: &str,
    resolution: &RegistryDependencyResolution,
) -> anyhow::Result<()> {
    if let Some(manifest) = &resolution.publish_manifest {
        validate_registry_publish_manifest(name, version, registry_url, manifest)?;
    }
    Ok(())
}

fn validate_registry_publish_manifest(
    name: &str,
    version: &str,
    registry_url: &str,
    manifest: &RegistryPublishManifest,
) -> anyhow::Result<()> {
    if manifest.package != name {
        anyhow::bail!(
            "registry publish manifest package mismatch for `{name}` {version}: found `{}`",
            manifest.package
        );
    }
    if manifest.version != version {
        anyhow::bail!(
            "registry publish manifest version mismatch for `{name}` {version}: found `{}`",
            manifest.version
        );
    }
    if manifest.registry_url.trim_end_matches('/') != registry_url.trim_end_matches('/') {
        anyhow::bail!(
            "registry publish manifest URL mismatch for `{name}` {version}: expected {}, found {}",
            registry_url,
            manifest.registry_url
        );
    }
    if !manifest.verify_integrity() {
        anyhow::bail!("registry publish manifest integrity mismatch for `{name}` {version}");
    }
    Ok(())
}

fn is_exact_semver(version: &str) -> bool {
    Version::parse(version).is_ok()
}

fn registry_dependency_endpoint(registry_url: &str, name: &str, version: &str) -> String {
    format!(
        "{}/api/v1/packages/{}/{}",
        registry_url.trim_end_matches('/'),
        name,
        version
    )
}

fn registry_package_versions_endpoint(registry_url: &str, name: &str) -> String {
    format!("{}/api/v1/packages/{}", registry_url.trim_end_matches('/'), name)
}

fn registry_index_endpoint(registry_url: &str) -> String {
    format!("{}/api/v1/index", registry_url.trim_end_matches('/'))
}

fn verify_registry_checksum(name: &str, package_dir: &Path, expected: Option<&str>) -> anyhow::Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = expected.trim();
    let Some(expected_hex) = expected.strip_prefix("sha256:") else {
        anyhow::bail!("registry checksum for `{name}` must use sha256:<hex>, got `{expected}`");
    };
    let actual = package_dir_checksum(package_dir)?;
    if !expected_hex.eq_ignore_ascii_case(&actual) {
        anyhow::bail!("registry checksum mismatch for `{name}`: expected sha256:{expected_hex}, got sha256:{actual}");
    }
    Ok(())
}

fn package_dir_checksum(package_dir: &Path) -> anyhow::Result<String> {
    let mut files = Vec::new();
    collect_checksum_files(package_dir, package_dir, &mut files)?;
    files.sort();

    let mut hasher = Sha256::new();
    for relative in files {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        let mut file = fs::File::open(package_dir.join(&relative)).with_context(|| {
            format!(
                "open package file for checksum {}",
                package_dir.join(&relative).display()
            )
        })?;
        let mut buffer = [0; 8192];
        loop {
            let read = file.read(&mut buffer).with_context(|| {
                format!(
                    "read package file for checksum {}",
                    package_dir.join(&relative).display()
                )
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        hasher.update([0]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn collect_checksum_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read package checksum directory {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        if file_name == ".git" || file_name == LOCK_FILE {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_checksum_files(root, &path, files)?;
        } else if file_type.is_file() {
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
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

fn publish_package(dry_run: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let graph = PackageGraph::discover(&cwd)?.ok_or_else(|| anyhow::anyhow!("No {MANIFEST_FILE} found"))?;
    let manifest = graph.registry_publish_manifest()?;
    if dry_run {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        upload_registry_publish_manifest(&manifest)?;
        eprintln!(
            "Published {} {} to {}",
            manifest.package, manifest.version, manifest.registry
        );
    }
    Ok(())
}

fn run_pkg_index_command(command: PkgIndexCommand) -> anyhow::Result<()> {
    match command {
        PkgIndexCommand::Sync => sync_registry_index(),
    }
}

fn sync_registry_index() -> anyhow::Result<()> {
    let (_manifest_path, manifest) = load_project_manifest()?;
    let registry = manifest
        .registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("registry index sync requires a [registry] section"))?;
    let registry_url = registry
        .url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("registry index sync requires [registry].url"))?;
    let registry_name = registry.name.as_deref().map(str::trim).filter(|name| !name.is_empty());
    let registry_key = registry_cache_key(registry_name, registry_url);
    let snapshot = download_registry_index(registry_url)?;
    let cache = RegistryIndexCache {
        registry: registry_name.unwrap_or(&registry_key).to_string(),
        registry_url: registry_url.to_string(),
        packages: snapshot.packages,
    };
    let cache_path = registry_index_cache_path(registry_name, registry_url);
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create registry index cache {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(&cache).context("serialize registry index cache")?;
    fs::write(&cache_path, body).with_context(|| format!("write registry index cache {}", cache_path.display()))?;
    eprintln!(
        "Synced registry index {} from {} to {}",
        cache.registry,
        registry_index_endpoint(registry_url),
        cache_path.display()
    );
    Ok(())
}

fn download_registry_index(registry_url: &str) -> anyhow::Result<RegistryIndexSnapshot> {
    let endpoint = registry_index_endpoint(registry_url);
    match ureq::get(&endpoint)
        .set("X-LK-Registry-Scope", RegistryAuthScope::Index.header_value())
        .call()
    {
        Ok(response) if (200..300).contains(&response.status()) => {
            let body = response.into_string().context("read registry index response")?;
            serde_json::from_str(&body).context("parse registry index response")
        }
        Ok(response) => {
            let status = response.status();
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry index sync failed with status {status}: {body}");
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry index sync failed with status {status}: {body}");
        }
        Err(error) => Err(anyhow::anyhow!("registry index sync request failed: {error}")),
    }
}

fn registry_index_cache_path(registry_name: Option<&str>, registry_url: &str) -> PathBuf {
    registry_index_cache_path_from_home(&lk_home(), registry_name, registry_url)
}

fn registry_index_cache_path_from_home(home: &Path, registry_name: Option<&str>, registry_url: &str) -> PathBuf {
    home.join("registry")
        .join(registry_cache_key(registry_name, registry_url))
        .join("index.json")
}

fn registry_cache_key(registry_name: Option<&str>, registry_url: &str) -> String {
    let source = registry_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(registry_url);
    let mut key = String::new();
    for ch in source
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .chars()
    {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            key.push(ch);
        } else {
            key.push('_');
        }
    }
    let key = key.trim_matches('_');
    if key.is_empty() {
        "default".to_string()
    } else {
        key.to_string()
    }
}

fn upload_registry_publish_manifest(manifest: &RegistryPublishManifest) -> anyhow::Result<()> {
    let token = registry_auth_token(RegistryAuthScope::Publish)?;
    upload_registry_publish_manifest_with_token(manifest, &token)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistryAuthScope {
    Index,
    Publish,
    Yank,
}

impl RegistryAuthScope {
    fn header_value(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Publish => "publish",
            Self::Yank => "yank",
        }
    }

    fn specific_token_env(self) -> &'static str {
        match self {
            Self::Index => "LK_REGISTRY_INDEX_TOKEN",
            Self::Publish => "LK_REGISTRY_PUBLISH_TOKEN",
            Self::Yank => "LK_REGISTRY_YANK_TOKEN",
        }
    }
}

fn registry_auth_token(scope: RegistryAuthScope) -> anyhow::Result<String> {
    registry_auth_token_from(scope, |name| std::env::var(name).ok())
}

fn registry_auth_token_from(
    scope: RegistryAuthScope,
    lookup: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<String> {
    [scope.specific_token_env(), "LK_REGISTRY_TOKEN", "LK_PUBLISH_TOKEN"]
        .into_iter()
        .find_map(|name| {
            lookup(name)
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty())
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "registry {} requires {}, LK_REGISTRY_TOKEN, or LK_PUBLISH_TOKEN",
                scope.header_value(),
                scope.specific_token_env()
            )
        })
}

fn upload_registry_publish_manifest_with_token(manifest: &RegistryPublishManifest, token: &str) -> anyhow::Result<()> {
    if !manifest.verify_integrity() {
        anyhow::bail!(
            "registry publish manifest integrity mismatch for {} {}",
            manifest.package,
            manifest.version
        );
    }
    let endpoint = registry_publish_endpoint(&manifest.registry_url);
    let body = serde_json::to_string(manifest).context("serialize registry publish manifest")?;
    let auth = format!("Bearer {token}");
    match ureq::post(&endpoint)
        .set("Authorization", &auth)
        .set("X-LK-Registry-Scope", RegistryAuthScope::Publish.header_value())
        .set("Content-Type", "application/json")
        .send_string(&body)
    {
        Ok(response) if (200..300).contains(&response.status()) => Ok(()),
        Ok(response) => {
            let status = response.status();
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry publish failed with status {status}: {body}");
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry publish failed with status {status}: {body}");
        }
        Err(error) => Err(anyhow::anyhow!("registry publish request failed: {error}")),
    }
}

fn registry_publish_endpoint(registry_url: &str) -> String {
    format!("{}/api/v1/packages", registry_url.trim_end_matches('/'))
}

fn yank_package_version(name: String, version: String, undo: bool) -> anyhow::Result<()> {
    let (_manifest_path, manifest) = load_project_manifest()?;
    let registry_url = manifest
        .registry
        .as_ref()
        .and_then(|registry| registry.url.as_deref())
        .ok_or_else(|| anyhow::anyhow!("registry yank requires [registry].url"))?;
    let token = registry_auth_token(RegistryAuthScope::Yank)?;
    upload_registry_yank(registry_url, &name, &version, undo, &token)?;
    if undo {
        eprintln!("Un-yanked {name} {version}");
    } else {
        eprintln!("Yanked {name} {version}");
    }
    Ok(())
}

fn upload_registry_yank(registry_url: &str, name: &str, version: &str, undo: bool, token: &str) -> anyhow::Result<()> {
    let endpoint = registry_yank_endpoint(registry_url, name, version);
    let auth = format!("Bearer {token}");
    let request = if undo {
        ureq::delete(&endpoint)
    } else {
        ureq::post(&endpoint)
    }
    .set("Authorization", &auth)
    .set("X-LK-Registry-Scope", RegistryAuthScope::Yank.header_value());
    match request.call() {
        Ok(response) if (200..300).contains(&response.status()) => Ok(()),
        Ok(response) => {
            let status = response.status();
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry yank failed for {name} {version} with status {status}: {body}");
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!("registry yank failed for {name} {version} with status {status}: {body}");
        }
        Err(error) => Err(anyhow::anyhow!(
            "registry yank request failed for {name} {version}: {error}"
        )),
    }
}

fn registry_yank_endpoint(registry_url: &str, name: &str, version: &str) -> String {
    format!(
        "{}/api/v1/packages/{}/{}/yank",
        registry_url.trim_end_matches('/'),
        name,
        version
    )
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
        registry: None,
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

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use lk_core::package::{
        RegistryPublishDependency, RegistryPublishIntegrity, RegistryPublishMacroProviders, RegistryPublishManifest,
    };

    use super::{
        RegistryAuthScope, RegistryPackageVersion, package_dir_checksum, registry_auth_token_from,
        registry_index_cache_path_from_home, registry_index_endpoint, registry_package_versions_endpoint,
        registry_publish_endpoint, registry_yank_endpoint, select_registry_version,
        upload_registry_publish_manifest_with_token, upload_registry_yank, validate_registry_package_version_manifests,
        verify_registry_checksum,
    };

    #[test]
    fn registry_publish_endpoint_appends_api_path() {
        assert_eq!(
            registry_publish_endpoint("https://registry.lk.example"),
            "https://registry.lk.example/api/v1/packages"
        );
        assert_eq!(
            registry_publish_endpoint("https://registry.lk.example/root/"),
            "https://registry.lk.example/root/api/v1/packages"
        );
    }

    #[test]
    fn registry_package_versions_endpoint_appends_package_path() {
        assert_eq!(
            registry_package_versions_endpoint("https://registry.lk.example", "helper"),
            "https://registry.lk.example/api/v1/packages/helper"
        );
        assert_eq!(
            registry_package_versions_endpoint("https://registry.lk.example/root/", "helper"),
            "https://registry.lk.example/root/api/v1/packages/helper"
        );
    }

    #[test]
    fn registry_yank_endpoint_appends_yank_path() {
        assert_eq!(
            registry_yank_endpoint("https://registry.lk.example", "helper", "0.2.3"),
            "https://registry.lk.example/api/v1/packages/helper/0.2.3/yank"
        );
        assert_eq!(
            registry_yank_endpoint("https://registry.lk.example/root/", "helper", "0.2.3"),
            "https://registry.lk.example/root/api/v1/packages/helper/0.2.3/yank"
        );
    }

    #[test]
    fn registry_index_endpoint_and_cache_path_are_stable() {
        let temp = tempfile::tempdir().expect("tempdir");

        assert_eq!(
            registry_index_endpoint("https://registry.lk.example/root/"),
            "https://registry.lk.example/root/api/v1/index"
        );
        assert_eq!(
            registry_index_cache_path_from_home(temp.path(), Some("local"), "https://registry.lk.example"),
            temp.path().join("registry").join("local").join("index.json")
        );
        assert_eq!(
            registry_index_cache_path_from_home(temp.path(), None, "https://registry.lk.example/root"),
            temp.path()
                .join("registry")
                .join("registry.lk.example_root")
                .join("index.json")
        );
    }

    #[test]
    fn registry_auth_token_prefers_index_specific_token() {
        let values = BTreeMap::from([
            ("LK_REGISTRY_TOKEN", "fallback-token"),
            ("LK_REGISTRY_INDEX_TOKEN", "index-token"),
        ]);
        let lookup = |name: &str| values.get(name).map(|value| (*value).to_string());

        assert_eq!(
            registry_auth_token_from(RegistryAuthScope::Index, lookup).expect("index token"),
            "index-token"
        );
    }

    #[test]
    fn registry_range_selection_uses_highest_non_yanked_compatible_version() {
        let requirement = semver::VersionReq::parse(">=0.2.0, <0.4.0").expect("version requirement");
        let selected = select_registry_version(
            "helper",
            &requirement,
            vec![
                RegistryPackageVersion {
                    version: "0.1.9".to_string(),
                    source: "git://old".to_string(),
                    rev: "old".to_string(),
                    checksum: Some("sha256:old".to_string()),
                    yanked: false,
                    publish_manifest: None,
                },
                RegistryPackageVersion {
                    version: "0.3.1".to_string(),
                    source: "git://selected".to_string(),
                    rev: "selected".to_string(),
                    checksum: Some("sha256:selected".to_string()),
                    yanked: false,
                    publish_manifest: None,
                },
                RegistryPackageVersion {
                    version: "0.3.2".to_string(),
                    source: "git://yanked".to_string(),
                    rev: "yanked".to_string(),
                    checksum: Some("sha256:yanked".to_string()),
                    yanked: true,
                    publish_manifest: None,
                },
                RegistryPackageVersion {
                    version: "0.4.0".to_string(),
                    source: "git://new".to_string(),
                    rev: "new".to_string(),
                    checksum: Some("sha256:new".to_string()),
                    yanked: false,
                    publish_manifest: None,
                },
            ],
        )
        .expect("compatible version should resolve");

        assert_eq!(selected.source, "git://selected");
        assert_eq!(selected.rev, "selected");
        assert_eq!(selected.checksum.as_deref(), Some("sha256:selected"));
    }

    #[test]
    fn package_dir_checksum_is_stable_and_ignores_git_and_lock_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::create_dir_all(root.join(".git")).expect("create git dir");
        std::fs::write(root.join("Lk.toml"), "[package]\nname = \"helper\"\n").expect("write manifest");
        std::fs::write(root.join("src/mod.lk"), "fn helper() { return 1; }\n").expect("write source");
        std::fs::write(root.join(".git/HEAD"), "ignored").expect("write git metadata");
        std::fs::write(root.join("Lk.lock"), "ignored").expect("write lock");

        let first = package_dir_checksum(root).expect("first checksum");
        std::fs::write(root.join(".git/HEAD"), "changed").expect("change git metadata");
        std::fs::write(root.join("Lk.lock"), "changed").expect("change lock");
        let second = package_dir_checksum(root).expect("second checksum");
        std::fs::write(root.join("src/mod.lk"), "fn helper() { return 2; }\n").expect("change source");
        let third = package_dir_checksum(root).expect("third checksum");

        assert_eq!(first, second);
        assert_ne!(first, third);
    }

    #[test]
    fn registry_checksum_verification_rejects_mismatches_and_unknown_algorithms() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(root.join("src/mod.lk"), "fn helper() { return 1; }\n").expect("write source");

        verify_registry_checksum("helper", root, None).expect("missing checksum is allowed");

        let bad_algorithm = verify_registry_checksum("helper", root, Some("sha512:abc"))
            .expect_err("unsupported checksum algorithm should fail");
        assert!(bad_algorithm.to_string().contains("sha256:<hex>"), "{bad_algorithm}");

        let mismatch =
            verify_registry_checksum("helper", root, Some("sha256:0000")).expect_err("checksum mismatch should fail");
        assert!(mismatch.to_string().contains("checksum mismatch"), "{mismatch}");
    }

    #[test]
    fn registry_auth_token_prefers_scope_specific_tokens() {
        let values = BTreeMap::from([
            ("LK_REGISTRY_TOKEN", "fallback-token"),
            ("LK_REGISTRY_PUBLISH_TOKEN", "publish-token"),
            ("LK_REGISTRY_YANK_TOKEN", "yank-token"),
        ]);
        let lookup = |name: &str| values.get(name).map(|value| (*value).to_string());

        assert_eq!(
            registry_auth_token_from(RegistryAuthScope::Publish, lookup).expect("publish token"),
            "publish-token"
        );
        assert_eq!(
            registry_auth_token_from(RegistryAuthScope::Yank, lookup).expect("yank token"),
            "yank-token"
        );
    }

    #[test]
    fn registry_publish_upload_posts_manifest_with_bearer_token() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
        let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept publish request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("read request line");
            let mut content_length = 0usize;
            let mut authorization = String::new();
            let mut content_type = String::new();
            let mut scope = String::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("read header");
                if header == "\r\n" {
                    break;
                }
                if let Some((name, value)) = header.trim_end().split_once(':') {
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => {
                            content_length = value.trim().parse().expect("content length");
                        }
                        "authorization" => authorization = value.trim().to_string(),
                        "content-type" => content_type = value.trim().to_string(),
                        "x-lk-registry-scope" => scope = value.trim().to_string(),
                        _ => {}
                    }
                }
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).expect("read request body");
            stream
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok")
                .expect("write mock response");
            (
                request_line,
                authorization,
                content_type,
                scope,
                String::from_utf8(body).expect("request body utf8"),
            )
        });

        let mut manifest = RegistryPublishManifest {
            package: "app".to_string(),
            version: "0.2.3".to_string(),
            registry: "local".to_string(),
            registry_url,
            include: vec!["Lk.toml".to_string(), "src/**".to_string()],
            dependencies: vec![RegistryPublishDependency {
                name: "helper".to_string(),
                source: "path".to_string(),
                version_or_rev: Some("0.1.0".to_string()),
            }],
            macro_providers: RegistryPublishMacroProviders {
                derive: vec!["MakeAnswer".to_string()],
                attribute: vec!["route".to_string()],
                function_like: vec!["sql".to_string()],
                trusted_dependencies: vec!["helper".to_string()],
            },
            integrity: RegistryPublishIntegrity::default(),
        };
        manifest.integrity = manifest.integrity();

        upload_registry_publish_manifest_with_token(&manifest, "secret-token").expect("publish upload succeeds");
        let (request_line, authorization, content_type, scope, body) = server.join().expect("mock registry thread");
        assert_eq!(request_line, "POST /api/v1/packages HTTP/1.1\r\n");
        assert_eq!(authorization, "Bearer secret-token");
        assert!(content_type.starts_with("application/json"), "{content_type}");
        assert_eq!(scope, "publish");
        let body: serde_json::Value = serde_json::from_str(&body).expect("request body json");
        assert_eq!(body["package"], "app");
        assert_eq!(body["version"], "0.2.3");
        assert_eq!(body["macro_providers"]["function_like"][0], "sql");
        assert_eq!(body["dependencies"][0]["version_or_rev"], "0.1.0");
        assert_eq!(body["integrity"]["algorithm"], "sha256");
        assert_eq!(body["integrity"]["digest"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn registry_publish_upload_rejects_tampered_manifest_integrity() {
        let mut manifest = registry_publish_manifest_fixture("http://127.0.0.1:9");
        manifest.include.push("tampered/**".to_string());

        let err = upload_registry_publish_manifest_with_token(&manifest, "secret-token")
            .expect_err("tampered manifest should not be uploaded");
        assert!(
            err.to_string().contains("integrity mismatch"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn registry_version_publish_manifest_integrity_is_validated() {
        let registry_url = "https://registry.lk.example";
        let manifest = registry_publish_manifest_fixture(registry_url);
        let version = RegistryPackageVersion {
            version: "0.2.3".to_string(),
            source: "https://example.invalid/helper.git".to_string(),
            rev: "abc123".to_string(),
            checksum: None,
            yanked: false,
            publish_manifest: Some(manifest.clone()),
        };
        validate_registry_package_version_manifests("app", registry_url, &[version])
            .expect("matching publish manifest should validate");

        let mut tampered = manifest;
        tampered.registry_url = "https://evil.invalid".to_string();
        let version = RegistryPackageVersion {
            version: "0.2.3".to_string(),
            source: "https://example.invalid/helper.git".to_string(),
            rev: "abc123".to_string(),
            checksum: None,
            yanked: false,
            publish_manifest: Some(tampered),
        };
        let err = validate_registry_package_version_manifests("app", registry_url, &[version])
            .expect_err("tampered publish manifest should fail");
        assert!(
            err.to_string().contains("URL mismatch") || err.to_string().contains("integrity mismatch"),
            "unexpected error: {err:#}"
        );
    }

    fn registry_publish_manifest_fixture(registry_url: &str) -> RegistryPublishManifest {
        let mut manifest = RegistryPublishManifest {
            package: "app".to_string(),
            version: "0.2.3".to_string(),
            registry: "local".to_string(),
            registry_url: registry_url.to_string(),
            include: vec!["Lk.toml".to_string(), "src/**".to_string()],
            dependencies: vec![RegistryPublishDependency {
                name: "helper".to_string(),
                source: "path".to_string(),
                version_or_rev: Some("0.1.0".to_string()),
            }],
            macro_providers: RegistryPublishMacroProviders {
                derive: vec!["MakeAnswer".to_string()],
                attribute: vec!["route".to_string()],
                function_like: vec!["sql".to_string()],
                trusted_dependencies: vec!["helper".to_string()],
            },
            integrity: RegistryPublishIntegrity::default(),
        };
        manifest.integrity = manifest.integrity();
        manifest
    }

    #[test]
    fn registry_yank_upload_sends_scoped_authenticated_request() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
        let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept yank request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("read request line");
            let mut authorization = String::new();
            let mut scope = String::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("read header");
                if header == "\r\n" {
                    break;
                }
                if let Some((name, value)) = header.trim_end().split_once(':') {
                    match name.to_ascii_lowercase().as_str() {
                        "authorization" => authorization = value.trim().to_string(),
                        "x-lk-registry-scope" => scope = value.trim().to_string(),
                        _ => {}
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .expect("write mock response");
            (request_line, authorization, scope)
        });

        upload_registry_yank(&registry_url, "helper", "0.2.3", false, "secret-token").expect("yank upload succeeds");
        let (request_line, authorization, scope) = server.join().expect("mock registry thread");
        assert_eq!(request_line, "POST /api/v1/packages/helper/0.2.3/yank HTTP/1.1\r\n");
        assert_eq!(authorization, "Bearer secret-token");
        assert_eq!(scope, "yank");
    }
}
