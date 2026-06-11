use super::procedural::ProcMacroDependency;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs, hash,
    path::{Path, PathBuf},
    rc::Rc,
    time::UNIX_EPOCH,
};

#[derive(Debug, Clone, Default)]
pub struct ProcMacroDependencyRecorder {
    dependencies: Rc<RefCell<Vec<ProcMacroDependency>>>,
}

impl ProcMacroDependencyRecorder {
    pub fn record(&self, dependencies: &[ProcMacroDependency]) {
        self.dependencies.borrow_mut().extend(dependencies.iter().cloned());
    }

    pub fn dependencies(&self) -> Vec<ProcMacroDependency> {
        let mut seen = HashSet::new();
        self.dependencies
            .borrow()
            .iter()
            .filter(|dependency| seen.insert((dependency.path.clone(), dependency.digest.clone())))
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroDependencyFingerprint {
    pub hash: String,
    pub entries: Vec<ProcMacroDependencyFingerprintEntry>,
}

impl ProcMacroDependencyFingerprint {
    pub fn is_current(&self, dependencies: &[ProcMacroDependency], base_dir: Option<&Path>) -> bool {
        self == &fingerprint_proc_macro_dependencies(dependencies, base_dir)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroDependencyFingerprintEntry {
    pub path: String,
    pub resolved_path: Option<String>,
    pub digest: Option<String>,
    pub state: ProcMacroDependencyFileState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcMacroDependencyFileState {
    Present {
        len: u64,
        modified_secs: Option<u64>,
        modified_nanos: Option<u32>,
        content_hash: Option<String>,
    },
    Missing,
}

#[derive(Debug, Clone)]
pub struct ProcMacroDependencyGraph<T>
where
    T: Clone + Eq + hash::Hash,
{
    dependents: HashMap<PathBuf, HashSet<T>>,
}

impl<T> Default for ProcMacroDependencyGraph<T>
where
    T: Clone + Eq + hash::Hash,
{
    fn default() -> Self {
        Self {
            dependents: HashMap::new(),
        }
    }
}

impl<T> ProcMacroDependencyGraph<T>
where
    T: Clone + Eq + hash::Hash,
{
    pub fn clear(&mut self) {
        self.dependents.clear();
    }

    pub fn insert(&mut self, dependent: T, dependencies: &[ProcMacroDependency], base_dir: &Path) {
        self.remove_dependent(&dependent);
        for dependency in dependencies {
            if let Some(path) = resolve_proc_macro_dependency_path(&dependency.path, Some(base_dir)) {
                self.dependents.entry(path).or_default().insert(dependent.clone());
            }
        }
    }

    pub fn remove_dependent(&mut self, dependent: &T) {
        self.dependents.retain(|_, dependents| {
            dependents.remove(dependent);
            !dependents.is_empty()
        });
    }

    pub fn take_dependents_for_changed_path(&mut self, changed_path: &Path) -> HashSet<T> {
        let changed = normalize_proc_macro_dependency_path(changed_path);
        let matching_dependencies = self
            .dependents
            .keys()
            .filter(|dependency| changed == **dependency || changed.starts_with(dependency))
            .cloned()
            .collect::<Vec<_>>();
        matching_dependencies
            .into_iter()
            .filter_map(|dependency| self.dependents.remove(&dependency))
            .flatten()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.dependents.is_empty()
    }
}

pub fn fingerprint_proc_macro_dependencies(
    dependencies: &[ProcMacroDependency],
    base_dir: Option<&Path>,
) -> ProcMacroDependencyFingerprint {
    let mut entries = dependencies
        .iter()
        .map(|dependency| fingerprint_entry(dependency, base_dir))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.digest.cmp(&right.digest))
            .then_with(|| left.resolved_path.cmp(&right.resolved_path))
    });

    let mut hash = StableHash64::new();
    for entry in &entries {
        entry.hash_into(&mut hash);
    }
    ProcMacroDependencyFingerprint {
        hash: format!("{:016x}", hash.finish()),
        entries,
    }
}

fn fingerprint_entry(dependency: &ProcMacroDependency, base_dir: Option<&Path>) -> ProcMacroDependencyFingerprintEntry {
    let resolved = resolve_proc_macro_dependency_path(&dependency.path, base_dir);
    let resolved_path = resolved.as_ref().map(|path| display_path(path));
    let state = resolved
        .as_ref()
        .map(|path| fingerprint_file_state(path))
        .unwrap_or(ProcMacroDependencyFileState::Missing);
    ProcMacroDependencyFingerprintEntry {
        path: dependency.path.clone(),
        resolved_path,
        digest: dependency.digest.clone(),
        state,
    }
}

pub fn resolve_proc_macro_dependency_path(path: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let raw = Path::new(path);
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base_dir.map(|base| base.join(raw)).unwrap_or_else(|| raw.to_path_buf())
    };
    Some(normalize_proc_macro_dependency_path(&resolved))
}

pub fn normalize_proc_macro_dependency_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn fingerprint_file_state(path: &Path) -> ProcMacroDependencyFileState {
    let Ok(metadata) = fs::metadata(path) else {
        return ProcMacroDependencyFileState::Missing;
    };
    let (modified_secs, modified_nanos) = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| (Some(duration.as_secs()), Some(duration.subsec_nanos())))
        .unwrap_or((None, None));
    ProcMacroDependencyFileState::Present {
        len: metadata.len(),
        modified_secs,
        modified_nanos,
        content_hash: fs::read(path).ok().map(|bytes| stable_hash_hex(&bytes)),
    }
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = StableHash64::new();
    hash.bytes(bytes);
    format!("{:016x}", hash.finish())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl ProcMacroDependencyFingerprintEntry {
    fn hash_into(&self, hash: &mut StableHash64) {
        hash.str(&self.path);
        hash.option_str(self.resolved_path.as_deref());
        hash.option_str(self.digest.as_deref());
        match &self.state {
            ProcMacroDependencyFileState::Present {
                len,
                modified_secs,
                modified_nanos,
                content_hash,
            } => {
                hash.str("present");
                hash.u64(*len);
                hash.option_u64(*modified_secs);
                hash.option_u32(*modified_nanos);
                hash.option_str(content_hash.as_deref());
            }
            ProcMacroDependencyFileState::Missing => hash.str("missing"),
        }
    }
}

struct StableHash64(u64);

impl StableHash64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn str(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    fn option_str(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.bytes(&[1]);
                self.str(value);
            }
            None => self.bytes(&[0]),
        }
    }

    fn option_u64(&mut self, value: Option<u64>) {
        match value {
            Some(value) => {
                self.bytes(&[1]);
                self.u64(value);
            }
            None => self.bytes(&[0]),
        }
    }

    fn option_u32(&mut self, value: Option<u32>) {
        match value {
            Some(value) => {
                self.bytes(&[1]);
                self.bytes(&value.to_le_bytes());
            }
            None => self.bytes(&[0]),
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dependency(path: &str) -> ProcMacroDependency {
        ProcMacroDependency {
            path: path.to_string(),
            digest: None,
        }
    }

    #[test]
    fn fingerprint_changes_when_dependency_content_changes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("schema.txt");
        fs::write(&path, "one").expect("write dependency");
        let deps = vec![dependency("schema.txt")];

        let first = fingerprint_proc_macro_dependencies(&deps, Some(dir.path()));
        fs::write(&path, "two").expect("rewrite dependency");

        assert!(!first.is_current(&deps, Some(dir.path())));
    }

    #[test]
    fn fingerprint_changes_when_missing_dependency_appears() {
        let dir = tempfile::tempdir().expect("temp dir");
        let deps = vec![dependency("schema.txt")];
        let missing = fingerprint_proc_macro_dependencies(&deps, Some(dir.path()));

        fs::write(dir.path().join("schema.txt"), "created").expect("create dependency");

        assert!(!missing.is_current(&deps, Some(dir.path())));
    }

    #[test]
    fn fingerprint_ignores_dependency_order() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("a.txt"), "a").expect("write a");
        fs::write(dir.path().join("b.txt"), "b").expect("write b");

        let first = fingerprint_proc_macro_dependencies(&[dependency("a.txt"), dependency("b.txt")], Some(dir.path()));
        let second = fingerprint_proc_macro_dependencies(&[dependency("b.txt"), dependency("a.txt")], Some(dir.path()));

        assert_eq!(first, second);
    }

    #[test]
    fn dependency_graph_invalidates_file_dependents() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("schema.txt"), "schema").expect("write dependency");
        let mut graph = ProcMacroDependencyGraph::default();

        graph.insert("main.lk", &[dependency("schema.txt")], dir.path());
        graph.insert("other.lk", &[dependency("other.txt")], dir.path());

        let dependents = graph.take_dependents_for_changed_path(&dir.path().join("schema.txt"));

        assert!(dependents.contains("main.lk"));
        assert!(!dependents.contains("other.lk"));
    }

    #[test]
    fn dependency_graph_invalidates_directory_dependents() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("schema")).expect("create schema dir");
        fs::write(dir.path().join("schema/user.lk"), "schema").expect("write dependency");
        let mut graph = ProcMacroDependencyGraph::default();

        graph.insert("main.lk", &[dependency("schema")], dir.path());
        graph.insert("other.lk", &[dependency("other-schema")], dir.path());

        let dependents = graph.take_dependents_for_changed_path(&dir.path().join("schema/user.lk"));

        assert!(dependents.contains("main.lk"));
        assert!(!dependents.contains("other.lk"));
    }

    #[test]
    fn dependency_graph_skips_empty_dependency_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut graph = ProcMacroDependencyGraph::default();

        graph.insert("main.lk", &[dependency("")], dir.path());

        assert!(graph.is_empty());
    }
}
