use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::macro_system::{ProcMacroProcessConfig, ProcMacroProviders};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

mod registry;
pub use registry::{
    RegistryAsymmetricSigningKey, RegistryIndex, RegistryManifestSignature, RegistryPackageIndex,
    RegistryPackageIndexResponse, RegistryPackageVersionResponse, RegistryPublicSigningKey, RegistryPublishDependency,
    RegistryPublishIntegrity, RegistryPublishMacroProviders, RegistryPublishManifest, RegistryPublishRequest,
    RegistryPublishServerValidation, RegistryPublishStoredManifest, RegistryService, RegistrySigningKey,
    RegistrySigningKeyring, RegistryStoredPackage, RegistryStoredVersion,
};

pub const MANIFEST_FILE: &str = "Lk.toml";
pub const LOCK_FILE: &str = "Lk.lock";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Option<PackageSection>,
    pub workspace: Option<WorkspaceSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<RegistrySection>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
    #[serde(default, skip_serializing_if = "MacroSection::is_empty")]
    pub macros: MacroSection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackageSection {
    pub name: String,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub license: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceSection {
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistrySection {
    pub name: Option<String>,
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MacroSection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub derive: BTreeMap<String, ProcMacroSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attribute: BTreeMap<String, ProcMacroSpec>,
    #[serde(default, rename = "function_like", skip_serializing_if = "BTreeMap::is_empty")]
    pub function_like: BTreeMap<String, ProcMacroSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcMacroSpec {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    GitHub(String),
    Detailed(DetailedDependency),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetailedDependency {
    pub github: Option<String>,
    pub git: Option<String>,
    pub path: Option<String>,
    pub registry: Option<String>,
    pub version: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub rev: Option<String>,
    #[serde(default)]
    pub workspace: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockFile {
    #[serde(default)]
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub source: String,
    pub rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PackageModule {
    pub name: String,
    pub package_root: PathBuf,
    pub manifest_path: PathBuf,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PackageGraph {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
    pub modules: Vec<PackageModule>,
    pub missing: Vec<String>,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path).with_context(|| format!("read manifest {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parse manifest {}", path.display()))
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let raw = toml::to_string_pretty(self).context("serialize Lk.toml")?;
        fs::write(path, raw).with_context(|| format!("write manifest {}", path.display()))
    }

    pub fn proc_macro_providers(&self, manifest_dir: &Path) -> ProcMacroProviders {
        let mut providers = ProcMacroProviders::default();
        for (name, spec) in &self.macros.derive {
            providers.register_derive(name.clone(), spec.to_process_config(manifest_dir));
        }
        for (name, spec) in &self.macros.attribute {
            providers.register_attribute(name.clone(), spec.to_process_config(manifest_dir));
        }
        for (name, spec) in &self.macros.function_like {
            providers.register_function_like(name.clone(), spec.to_process_config(manifest_dir));
        }
        providers
    }

    pub fn trusted_proc_macro_dependencies(&self) -> BTreeSet<String> {
        self.macros.trusted_dependencies.iter().cloned().collect()
    }

    pub fn validate_macro_distribution(
        &self,
        manifest_dir: &Path,
        dependency_macro_providers: &BTreeMap<String, bool>,
    ) -> Result<()> {
        let mut issues = Vec::new();
        validate_proc_macro_specs(manifest_dir, "derive", &self.macros.derive, &mut issues);
        validate_proc_macro_specs(manifest_dir, "attribute", &self.macros.attribute, &mut issues);
        validate_proc_macro_specs(manifest_dir, "function_like", &self.macros.function_like, &mut issues);
        for dependency in &self.macros.trusted_dependencies {
            match dependency_macro_providers.get(dependency) {
                Some(true) => {}
                Some(false) => issues.push(format!(
                    "trusted macro dependency `{dependency}` does not declare derive, attribute, or function-like providers"
                )),
                None => issues.push(format!(
                    "trusted macro dependency `{dependency}` is not a resolved dependency or workspace member"
                )),
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("macro package check failed:\n{}", issues.join("\n")))
        }
    }
}

impl MacroSection {
    pub fn is_empty(&self) -> bool {
        self.trusted_dependencies.is_empty()
            && self.derive.is_empty()
            && self.attribute.is_empty()
            && self.function_like.is_empty()
    }

    fn has_providers(&self) -> bool {
        !self.derive.is_empty() || !self.attribute.is_empty() || !self.function_like.is_empty()
    }
}

impl ProcMacroSpec {
    pub fn to_process_config(&self, manifest_dir: &Path) -> ProcMacroProcessConfig {
        let mut config = ProcMacroProcessConfig::new(resolve_proc_macro_command(manifest_dir, &self.command));
        config.args = self.args.clone();
        if let Some(timeout_ms) = self.timeout_ms {
            config.timeout = Duration::from_millis(timeout_ms);
        }
        if let Some(max_output_bytes) = self.max_output_bytes {
            config.max_output_bytes = max_output_bytes;
        }
        config
    }
}

impl LockFile {
    pub fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path).with_context(|| format!("read lockfile {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parse lockfile {}", path.display()))
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let raw = toml::to_string_pretty(self).context("serialize Lk.lock")?;
        fs::write(path, raw).with_context(|| format!("write lockfile {}", path.display()))
    }
}

impl DependencySpec {
    pub fn github_repo(&self) -> Option<&str> {
        match self {
            DependencySpec::GitHub(repo) => Some(repo.as_str()),
            DependencySpec::Detailed(dep) => dep.github.as_deref(),
        }
    }

    pub fn git_url(&self) -> Option<String> {
        match self {
            DependencySpec::GitHub(repo) => Some(github_url(repo)),
            DependencySpec::Detailed(dep) => dep.git.clone().or_else(|| dep.github.as_deref().map(github_url)),
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            DependencySpec::GitHub(_) => None,
            DependencySpec::Detailed(dep) => dep.path.as_deref(),
        }
    }

    pub fn is_workspace(&self) -> bool {
        matches!(self, DependencySpec::Detailed(dep) if dep.workspace)
    }

    pub fn registry_version(&self) -> Option<&str> {
        match self {
            DependencySpec::GitHub(_) => None,
            DependencySpec::Detailed(dep) => dep.version.as_deref(),
        }
    }

    pub fn registry_override(&self) -> Option<&str> {
        match self {
            DependencySpec::GitHub(_) => None,
            DependencySpec::Detailed(dep) => dep.registry.as_deref(),
        }
    }
}

impl PackageGraph {
    pub fn discover(start: &Path) -> Result<Option<Self>> {
        let manifests = find_manifests(start);
        if manifests.is_empty() {
            return Ok(None);
        };
        let mut manifest_path = manifests[0].clone();
        for candidate in &manifests {
            if Manifest::read(candidate)?.workspace.is_some() {
                manifest_path = candidate.clone();
            }
        }
        Self::from_manifest_path(&manifest_path).map(Some)
    }

    pub fn from_manifest_path(manifest_path: &Path) -> Result<Self> {
        let manifest = Manifest::read(manifest_path)?;
        let root = manifest_path
            .parent()
            .ok_or_else(|| anyhow!("manifest has no parent: {}", manifest_path.display()))?
            .to_path_buf();
        let mut graph = Self {
            root: root.clone(),
            manifest_path: manifest_path.to_path_buf(),
            manifest,
            modules: Vec::new(),
            missing: Vec::new(),
        };
        graph.collect_workspace_modules()?;
        graph.collect_dependency_modules()?;
        Ok(graph)
    }

    pub fn manifest_dir(&self) -> &Path {
        &self.root
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    pub fn proc_macro_providers_for_manifest(&self, manifest_path: &Path) -> Result<ProcMacroProviders> {
        let manifest = Manifest::read(manifest_path)?;
        let manifest_dir = manifest_path
            .parent()
            .ok_or_else(|| anyhow!("manifest has no parent: {}", manifest_path.display()))?;
        let trusted = manifest.trusted_proc_macro_dependencies();
        let mut providers = manifest.proc_macro_providers(manifest_dir);
        if trusted.is_empty() {
            return Ok(providers);
        }

        for module in &self.modules {
            if module.manifest_path == manifest_path || !trusted.contains(&module.name) {
                continue;
            }
            let dependency_manifest = Manifest::read(&module.manifest_path)?;
            let dependency_providers = dependency_manifest.proc_macro_providers(&module.package_root);
            providers.register_trusted_dependency(&module.name, dependency_providers);
        }
        Ok(providers)
    }

    pub fn validate_macro_distribution(&self) -> Result<()> {
        let mut manifests = BTreeSet::new();
        manifests.insert(self.manifest_path.clone());
        for module in &self.modules {
            manifests.insert(module.manifest_path.clone());
        }
        for manifest_path in manifests {
            self.validate_macro_distribution_for_manifest(&manifest_path)
                .with_context(|| format!("check macro package manifest {}", manifest_path.display()))?;
        }
        Ok(())
    }

    pub fn validate_macro_distribution_for_manifest(&self, manifest_path: &Path) -> Result<()> {
        let manifest = Manifest::read(manifest_path)?;
        let manifest_dir = manifest_path
            .parent()
            .ok_or_else(|| anyhow!("manifest has no parent: {}", manifest_path.display()))?;
        let mut dependency_macro_providers = BTreeMap::new();
        for module in &self.modules {
            if module.manifest_path == manifest_path {
                continue;
            }
            let dependency_manifest = Manifest::read(&module.manifest_path)?;
            dependency_macro_providers.insert(module.name.clone(), dependency_manifest.macros.has_providers());
        }
        manifest.validate_macro_distribution(manifest_dir, &dependency_macro_providers)
    }

    fn collect_workspace_modules(&mut self) -> Result<()> {
        let mut seen = BTreeSet::new();
        if let Some(package) = self.manifest.package.as_ref()
            && let Some(root) = package_entry(&self.root, &package.name)
        {
            seen.insert(package.name.clone());
            self.modules.push(package_module(&self.root, &package.name, root));
        }

        let Some(workspace) = self.manifest.workspace.as_ref() else {
            return Ok(());
        };
        for member in expand_members(&self.root, &workspace.members)? {
            let manifest_path = member.join(MANIFEST_FILE);
            if !manifest_path.exists() {
                continue;
            }
            let manifest = Manifest::read(&manifest_path)?;
            let Some(package) = manifest.package else {
                continue;
            };
            if seen.insert(package.name.clone())
                && let Some(root) = package_entry(&member, &package.name)
            {
                self.modules.push(package_module(&member, &package.name, root));
            }
        }
        Ok(())
    }

    fn collect_dependency_modules(&mut self) -> Result<()> {
        let lock = LockFile::read(&self.lock_path())?;
        let locked: BTreeMap<_, _> = lock.package.into_iter().map(|p| (p.name.clone(), p)).collect();
        let dependencies = self.effective_dependencies();
        for (name, spec) in dependencies {
            if self.modules.iter().any(|module| module.name == name) {
                continue;
            }
            let dep_dir = if spec.is_workspace() {
                continue;
            } else if let Some(path) = spec.path() {
                self.root.join(path)
            } else if let Some(locked) = locked.get(&name) {
                cache_dir_for_source(&locked.source)
            } else if let Some(url) = spec.git_url() {
                cache_dir_for_source(&url)
            } else {
                self.missing.push(name);
                continue;
            };
            if let Some(root) = package_entry(&dep_dir, &name) {
                self.modules.push(package_module(&dep_dir, &name, root));
            } else {
                self.missing.push(name);
            }
        }
        Ok(())
    }

    fn effective_dependencies(&self) -> BTreeMap<String, DependencySpec> {
        let mut deps = BTreeMap::new();
        for (name, spec) in &self.manifest.dependencies {
            let resolved = if spec.is_workspace() {
                self.manifest
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.dependencies.get(name).cloned())
            } else {
                Some(spec.clone())
            };
            if let Some(spec) = resolved {
                deps.insert(name.clone(), spec);
            }
        }
        deps
    }
}

pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    find_manifests(start).into_iter().next()
}

fn find_manifests(start: &Path) -> Vec<PathBuf> {
    let mut current = if start.is_file() {
        match start.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return Vec::new(),
        }
    } else {
        start.to_path_buf()
    };
    let mut manifests = Vec::new();
    loop {
        let manifest = current.join(MANIFEST_FILE);
        if manifest.exists() {
            manifests.push(manifest);
        }
        if !current.pop() {
            return manifests;
        }
    }
}

pub fn package_entry(root: &Path, name: &str) -> Option<PathBuf> {
    let src = root.join("src");
    let mod_file = src.join("mod.lk");
    if mod_file.exists() {
        return Some(mod_file);
    }
    let named = src.join(format!("{name}.lk"));
    if named.exists() {
        return Some(named);
    }
    None
}

fn package_module(package_root: &Path, name: &str, root: PathBuf) -> PackageModule {
    PackageModule {
        name: name.to_string(),
        package_root: package_root.to_path_buf(),
        manifest_path: package_root.join(MANIFEST_FILE),
        root,
    }
}

pub fn github_url(repo: &str) -> String {
    if repo.starts_with("http://") || repo.starts_with("https://") || repo.starts_with("git@") {
        repo.to_string()
    } else {
        format!("https://github.com/{repo}.git")
    }
}

pub fn cache_dir_for_source(source: &str) -> PathBuf {
    let mut root = lk_home().join("git");
    let normalized = source
        .trim_end_matches(".git")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("git@")
        .replace(':', "/");
    for part in normalized.split('/').filter(|part| !part.is_empty()) {
        root.push(part);
    }
    root
}

fn resolve_proc_macro_command(manifest_dir: &Path, command: &str) -> PathBuf {
    let path = PathBuf::from(command);
    if path.is_absolute() || !is_path_like_command(command) {
        return path;
    }
    manifest_dir.join(path)
}

fn is_path_like_command(command: &str) -> bool {
    command.starts_with('.') || command.contains('/') || command.contains('\\')
}

fn validate_proc_macro_specs(
    manifest_dir: &Path,
    kind: &str,
    specs: &BTreeMap<String, ProcMacroSpec>,
    issues: &mut Vec<String>,
) {
    for (name, spec) in specs {
        if !is_valid_macro_provider_name(name) {
            issues.push(format!(
                "[macros.{kind}.{name}] must use an identifier macro name without path separators"
            ));
        }
        if spec.command.trim().is_empty() {
            issues.push(format!("[macros.{kind}.{name}] command must not be empty"));
        } else if is_path_like_command(&spec.command) {
            let command_path = resolve_proc_macro_command(manifest_dir, &spec.command);
            if !command_path.exists() {
                issues.push(format!(
                    "[macros.{kind}.{name}] command path does not exist: {}",
                    command_path.display()
                ));
            }
        }
        if spec.timeout_ms == Some(0) {
            issues.push(format!("[macros.{kind}.{name}] timeout_ms must be greater than 0"));
        }
        if spec.max_output_bytes == Some(0) {
            issues.push(format!(
                "[macros.{kind}.{name}] max_output_bytes must be greater than 0"
            ));
        }
    }
}

fn is_valid_macro_provider_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '-' || ch.is_ascii_alphanumeric())
}

pub(super) fn is_publish_version(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .split_once('-')
                    .map_or(*part, |(base, _)| base)
                    .chars()
                    .all(|ch| ch.is_ascii_digit())
        })
}

fn registry_include(registry: &RegistrySection) -> Vec<String> {
    if registry.include.is_empty() {
        vec!["Lk.toml".to_string(), "src/**".to_string()]
    } else {
        registry.include.clone()
    }
}

fn dependency_publish_source(spec: &DependencySpec) -> String {
    match spec {
        DependencySpec::GitHub(repo) => format!("github:{repo}"),
        DependencySpec::Detailed(dep) if dep.workspace => "workspace".to_string(),
        DependencySpec::Detailed(dep) if dep.path.is_some() => {
            format!("path:{}", dep.path.as_deref().unwrap_or_default())
        }
        DependencySpec::Detailed(dep) if dep.github.is_some() => {
            format!("github:{}", dep.github.as_deref().unwrap_or_default())
        }
        DependencySpec::Detailed(dep) if dep.git.is_some() => format!("git:{}", dep.git.as_deref().unwrap_or_default()),
        DependencySpec::Detailed(dep) if dep.version.is_some() => dep
            .registry
            .as_deref()
            .map(|registry| format!("registry:{registry}"))
            .unwrap_or_else(|| "registry:default".to_string()),
        DependencySpec::Detailed(_) => "unknown".to_string(),
    }
}

fn dependency_version(spec: &DependencySpec) -> Option<String> {
    match spec {
        DependencySpec::GitHub(_) => None,
        DependencySpec::Detailed(dep) => dep
            .version
            .clone()
            .or_else(|| dep.rev.clone())
            .or_else(|| dep.tag.clone())
            .or_else(|| dep.branch.clone()),
    }
}

pub fn lk_home() -> PathBuf {
    if let Ok(home) = env::var("LK_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    env::var("HOME")
        .map(|home| PathBuf::from(home).join(".lk"))
        .unwrap_or_else(|_| PathBuf::from(".lk"))
}

fn expand_members(root: &Path, members: &[String]) -> Result<Vec<PathBuf>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_workspace_dependencies() -> Result<()> {
        let raw = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            util = { workspace = true }
            remote = "owner/repo"

            [workspace]
            members = ["crates/*"]

            [workspace.dependencies]
            util = { path = "crates/util" }
        "#;
        let manifest: Manifest = toml::from_str(raw)?;
        assert_eq!(manifest.package.as_ref().unwrap().name, "app");
        assert!(manifest.dependencies["util"].is_workspace());
        assert_eq!(
            manifest.workspace.as_ref().unwrap().dependencies["util"].path(),
            Some("crates/util")
        );
        assert_eq!(
            manifest.dependencies["remote"].git_url().unwrap(),
            github_url("owner/repo")
        );
        Ok(())
    }

    #[test]
    fn manifest_builds_proc_macro_providers() -> Result<()> {
        let raw = r#"
            [package]
            name = "app"

            [macros.derive.MakeAnswer]
            command = "./tools/derive-make-answer"
            args = ["--json"]
            timeout_ms = 250
            max_output_bytes = 1024

            [macros.attribute.route]
            command = "lk-route-macro"

            [macros.function_like.sql]
            command = "tools/sql-macro"
        "#;
        let manifest: Manifest = toml::from_str(raw)?;
        assert!(manifest.trusted_proc_macro_dependencies().is_empty());
        let providers = manifest.proc_macro_providers(Path::new("/tmp/app"));

        let derive = providers
            .derive_provider("MakeAnswer")
            .expect("derive provider should be registered");
        assert_eq!(derive.program, PathBuf::from("/tmp/app/./tools/derive-make-answer"));
        assert_eq!(derive.args, vec!["--json"]);
        assert_eq!(derive.timeout, Duration::from_millis(250));
        assert_eq!(derive.max_output_bytes, 1024);

        let attr = providers
            .attribute_provider("route")
            .expect("attribute provider should be registered");
        assert_eq!(attr.program, PathBuf::from("lk-route-macro"));

        let function_like = providers
            .function_like_provider("sql")
            .expect("function-like provider should be registered");
        assert_eq!(function_like.program, PathBuf::from("/tmp/app/tools/sql-macro"));
        Ok(())
    }

    #[test]
    fn graph_loads_only_trusted_dependency_proc_macro_providers() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("deps/helper/src"))?;
        fs::write(root.join("src/mod.lk"), "return helper::value();\n")?;
        fs::write(root.join("deps/helper/src/mod.lk"), "fn value() { return 1; }\n")?;
        fs::write(
            root.join("deps/helper").join(MANIFEST_FILE),
            r#"
                [package]
                name = "helper"

                [macros.function_like.sql]
                command = "./tools/sql"
            "#,
        )?;
        fs::write(
            root.join(MANIFEST_FILE),
            r#"
                [package]
                name = "app"

                [dependencies]
                helper = { path = "deps/helper" }
            "#,
        )?;

        let graph = PackageGraph::discover(root)?.unwrap();
        let providers = graph.proc_macro_providers_for_manifest(&root.join(MANIFEST_FILE))?;
        assert!(providers.function_like_provider("helper::sql").is_none());

        fs::write(
            root.join(MANIFEST_FILE),
            r#"
                [package]
                name = "app"

                [dependencies]
                helper = { path = "deps/helper" }

                [macros]
                trusted_dependencies = ["helper"]
            "#,
        )?;

        let graph = PackageGraph::discover(root)?.unwrap();
        let providers = graph.proc_macro_providers_for_manifest(&root.join(MANIFEST_FILE))?;
        let provider = providers
            .function_like_provider("helper::sql")
            .expect("trusted dependency function-like provider should be namespaced");
        assert_eq!(provider.program, root.join("deps/helper/tools/sql"));
        assert!(providers.function_like_provider("sql").is_none());
        Ok(())
    }

    #[test]
    fn macro_distribution_check_accepts_trusted_provider_package() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir_all(root.join("deps/helper/src"))?;
        fs::create_dir_all(root.join("deps/helper/tools"))?;
        fs::write(root.join("deps/helper/src/mod.lk"), "fn value() { return 1; }\n")?;
        fs::write(root.join("deps/helper/tools/sql"), "#!/bin/sh\n")?;
        fs::write(
            root.join("deps/helper").join(MANIFEST_FILE),
            r#"
                [package]
                name = "helper"

                [macros.function_like.sql]
                command = "./tools/sql"
                timeout_ms = 100
                max_output_bytes = 1024
            "#,
        )?;
        fs::write(
            root.join(MANIFEST_FILE),
            r#"
                [package]
                name = "app"

                [dependencies]
                helper = { path = "deps/helper" }

                [macros]
                trusted_dependencies = ["helper"]
            "#,
        )?;

        let graph = PackageGraph::discover(root)?.unwrap();
        graph.validate_macro_distribution()?;
        Ok(())
    }

    #[test]
    fn macro_distribution_check_reports_bad_provider_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir_all(root.join("deps/helper/src"))?;
        fs::write(root.join("deps/helper/src/mod.lk"), "fn value() { return 1; }\n")?;
        fs::write(
            root.join("deps/helper").join(MANIFEST_FILE),
            r#"
                [package]
                name = "helper"
            "#,
        )?;
        fs::write(
            root.join(MANIFEST_FILE),
            r#"
                [package]
                name = "app"

                [dependencies]
                helper = { path = "deps/helper" }

                [macros]
                trusted_dependencies = ["helper", "missing"]

                [macros.function_like."bad::name"]
                command = "./tools/missing"
                timeout_ms = 0
                max_output_bytes = 0
            "#,
        )?;

        let graph = PackageGraph::discover(root)?.unwrap();
        let err = graph
            .validate_macro_distribution()
            .expect_err("bad macro distribution metadata should fail");
        let message = format!("{err:#}");
        assert!(message.contains("bad::name"), "{message}");
        assert!(message.contains("command path does not exist"), "{message}");
        assert!(message.contains("timeout_ms must be greater than 0"), "{message}");
        assert!(message.contains("max_output_bytes must be greater than 0"), "{message}");
        assert!(
            message.contains("trusted macro dependency `helper` does not declare"),
            "{message}"
        );
        assert!(message.contains("trusted macro dependency `missing`"), "{message}");
        Ok(())
    }

    #[test]
    fn graph_discovers_workspace_members_and_path_dependencies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir_all(root.join("crates/util/src"))?;
        fs::create_dir_all(root.join("deps/helper/src"))?;
        fs::write(
            root.join(MANIFEST_FILE),
            r#"
                [package]
                name = "app"

                [dependencies]
                helper = { path = "deps/helper" }

                [workspace]
                members = ["crates/*"]
            "#,
        )?;
        fs::write(root.join("src-placeholder"), "")?;
        fs::write(
            root.join("crates/util").join(MANIFEST_FILE),
            r#"
                [package]
                name = "util"
            "#,
        )?;
        fs::write(root.join("crates/util/src/mod.lk"), "fn value() { return 1; }\n")?;
        fs::write(
            root.join("deps/helper").join(MANIFEST_FILE),
            r#"
                [package]
                name = "helper"
            "#,
        )?;
        fs::write(root.join("deps/helper/src/mod.lk"), "fn value() { return 2; }\n")?;

        let graph = PackageGraph::discover(root)?.unwrap();
        let modules: BTreeMap<_, _> = graph
            .modules
            .into_iter()
            .map(|module| (module.name, module.root))
            .collect();
        assert!(modules.contains_key("util"));
        assert!(modules.contains_key("helper"));
        Ok(())
    }
}
