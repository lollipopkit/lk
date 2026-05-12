use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

pub const MANIFEST_FILE: &str = "Lkr.toml";
pub const LOCK_FILE: &str = "Lkr.lock";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub package: Option<PackageSection>,
    pub workspace: Option<WorkspaceSection>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
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
}

#[derive(Debug, Clone)]
pub struct PackageModule {
    pub name: String,
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
        let raw = toml::to_string_pretty(self).context("serialize Lkr.toml")?;
        fs::write(path, raw).with_context(|| format!("write manifest {}", path.display()))
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
        let raw = toml::to_string_pretty(self).context("serialize Lkr.lock")?;
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

    fn collect_workspace_modules(&mut self) -> Result<()> {
        let mut seen = BTreeSet::new();
        if let Some(package) = self.manifest.package.as_ref()
            && let Some(root) = package_entry(&self.root, &package.name)
        {
            seen.insert(package.name.clone());
            self.modules.push(PackageModule {
                name: package.name.clone(),
                root,
            });
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
                self.modules.push(PackageModule {
                    name: package.name,
                    root,
                });
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
                self.modules.push(PackageModule { name, root });
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
    let mod_file = src.join("mod.lkr");
    if mod_file.exists() {
        return Some(mod_file);
    }
    let named = src.join(format!("{name}.lkr"));
    if named.exists() {
        return Some(named);
    }
    None
}

pub fn github_url(repo: &str) -> String {
    if repo.starts_with("http://") || repo.starts_with("https://") || repo.starts_with("git@") {
        repo.to_string()
    } else {
        format!("https://github.com/{repo}.git")
    }
}

pub fn cache_dir_for_source(source: &str) -> PathBuf {
    let mut root = lkr_home().join("git");
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

pub fn lkr_home() -> PathBuf {
    if let Ok(home) = env::var("LKR_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    env::var("HOME")
        .map(|home| PathBuf::from(home).join(".lkr"))
        .unwrap_or_else(|_| PathBuf::from(".lkr"))
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
        fs::write(root.join("crates/util/src/mod.lkr"), "fn value() { return 1; }\n")?;
        fs::write(
            root.join("deps/helper").join(MANIFEST_FILE),
            r#"
                [package]
                name = "helper"
            "#,
        )?;
        fs::write(root.join("deps/helper/src/mod.lkr"), "fn value() { return 2; }\n")?;

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
