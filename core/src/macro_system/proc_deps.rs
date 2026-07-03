use super::procedural::ProcMacroDependency;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs, hash, io,
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
            self.insert_dependency_path(dependent.clone(), Path::new(&dependency.path), base_dir);
        }
    }

    pub fn insert_paths<P>(&mut self, dependent: T, paths: &[P], base_dir: &Path)
    where
        P: AsRef<Path>,
    {
        self.remove_dependent(&dependent);
        for path in paths {
            self.insert_dependency_path(dependent.clone(), path.as_ref(), base_dir);
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

    fn insert_dependency_path(&mut self, dependent: T, path: &Path, base_dir: &Path) {
        if let Some(path) = resolve_dependency_path(path, Some(base_dir)) {
            self.dependents.entry(path).or_default().insert(dependent);
        }
    }
}

pub fn fingerprint_proc_macro_dependencies(
    dependencies: &[ProcMacroDependency],
    base_dir: Option<&Path>,
) -> ProcMacroDependencyFingerprint {
    let entries = dependencies
        .iter()
        .map(|dependency| fingerprint_entry(&dependency.path, dependency.digest.clone(), base_dir))
        .collect::<Vec<_>>();
    fingerprint_entries(entries)
}

pub fn fingerprint_dependency_paths<P>(paths: &[P], base_dir: Option<&Path>) -> ProcMacroDependencyFingerprint
where
    P: AsRef<Path>,
{
    let entries = paths
        .iter()
        .map(|path| {
            let display = path.as_ref().to_string_lossy().into_owned();
            fingerprint_entry(&display, None, base_dir)
        })
        .collect::<Vec<_>>();
    fingerprint_entries(entries)
}

fn fingerprint_entries(mut entries: Vec<ProcMacroDependencyFingerprintEntry>) -> ProcMacroDependencyFingerprint {
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

fn fingerprint_entry(
    path: &str,
    digest: Option<String>,
    base_dir: Option<&Path>,
) -> ProcMacroDependencyFingerprintEntry {
    let resolved = resolve_proc_macro_dependency_path(path, base_dir);
    let resolved_path = resolved.as_ref().map(|path| display_path(path));
    let state = resolved
        .as_ref()
        .map(|path| fingerprint_file_state(path))
        .unwrap_or(ProcMacroDependencyFileState::Missing);
    ProcMacroDependencyFingerprintEntry {
        path: path.to_string(),
        resolved_path,
        digest,
        state,
    }
}

pub fn resolve_proc_macro_dependency_path(path: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    resolve_dependency_path(Path::new(path), base_dir)
}

fn resolve_dependency_path(path: &Path, base_dir: Option<&Path>) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir
            .map(|base| base.join(path))
            .unwrap_or_else(|| path.to_path_buf())
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
        content_hash: dependency_content_hash(path, &metadata),
    }
}

fn dependency_content_hash(path: &Path, metadata: &fs::Metadata) -> Option<String> {
    if metadata.is_dir() {
        return stable_directory_hash_hex(path).ok();
    }
    fs::read(path).ok().map(|bytes| stable_hash_hex(&bytes))
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = StableHash64::new();
    hash.bytes(bytes);
    format!("{:016x}", hash.finish())
}

// Maximum directory nesting depth to prevent stack overflow.
const MAX_DIRECTORY_DEPTH: u32 = 64;
// Maximum individual file size to hash; skip hashing (but record length) beyond this.
const MAX_FILE_READ_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB

fn stable_directory_hash_hex(path: &Path) -> io::Result<String> {
    let mut hash = StableHash64::new();
    hash.str("dir");
    hash_directory(&mut hash, path, Path::new(""), 0)?;
    Ok(format!("{:016x}", hash.finish()))
}

fn hash_directory(hash: &mut StableHash64, dir: &Path, relative_dir: &Path, depth: u32) -> io::Result<()> {
    if depth > MAX_DIRECTORY_DEPTH {
        return Err(io::Error::other(format!(
            "directory depth exceeds {MAX_DIRECTORY_DEPTH}: {}",
            relative_dir.display()
        )));
    }
    let mut entries = fs::read_dir(dir)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let relative_path = relative_dir.join(&name);
        let relative_display = relative_path.to_string_lossy();
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        hash.str(&relative_display);
        if file_type.is_dir() {
            hash.str("dir");
            hash.u64(metadata.len());
            hash_directory(hash, &path, &relative_path, depth + 1)?;
        } else if file_type.is_file() {
            hash.str("file");
            hash.u64(metadata.len());
            if metadata.len() <= MAX_FILE_READ_BYTES
                && let Ok(bytes) = fs::read(&path)
            {
                hash.bytes(&bytes);
            } else if let Ok(modified) = metadata.modified()
                && let Ok(elapsed) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                // Oversized files skip the content read; fold in the mtime so
                // a same-length content change still invalidates the cache.
                hash.u64(elapsed.as_secs());
                hash.u64(u64::from(elapsed.subsec_nanos()));
            }
        } else if file_type.is_symlink() {
            hash.str("symlink");
            if let Ok(target) = fs::read_link(&path) {
                hash.str(&target.to_string_lossy());
            }
        } else {
            hash.str("other");
            hash.u64(metadata.len());
        }
    }
    Ok(())
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
    fn fingerprint_changes_when_directory_child_content_changes() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("schema")).expect("create schema dir");
        fs::write(dir.path().join("schema/user.lk"), "one").expect("write dependency");
        let deps = vec![dependency("schema")];

        let first = fingerprint_proc_macro_dependencies(&deps, Some(dir.path()));
        fs::write(dir.path().join("schema/user.lk"), "two").expect("rewrite dependency");

        assert!(!first.is_current(&deps, Some(dir.path())));
    }

    #[test]
    fn fingerprint_changes_when_nested_directory_dependency_appears() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("schema/models")).expect("create schema dir");
        fs::write(dir.path().join("schema/models/user.lk"), "user").expect("write dependency");
        let deps = vec![dependency("schema")];

        let first = fingerprint_proc_macro_dependencies(&deps, Some(dir.path()));
        fs::write(dir.path().join("schema/models/order.lk"), "order").expect("write nested dependency");

        assert!(!first.is_current(&deps, Some(dir.path())));
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
    fn generic_dependency_path_fingerprint_changes_when_content_changes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("module.lk");
        fs::write(&path, "export fn value() { return 1; }").expect("write module");
        let deps = vec![PathBuf::from("module.lk")];

        let first = fingerprint_dependency_paths(&deps, Some(dir.path()));
        fs::write(&path, "export fn value() { return 2; }").expect("rewrite module");

        assert_ne!(first, fingerprint_dependency_paths(&deps, Some(dir.path())));
    }

    #[test]
    fn generic_dependency_path_fingerprint_changes_when_missing_file_appears() {
        let dir = tempfile::tempdir().expect("temp dir");
        let deps = vec![PathBuf::from("module.lk")];
        let missing = fingerprint_dependency_paths(&deps, Some(dir.path()));

        fs::write(dir.path().join("module.lk"), "export fn value() {}").expect("create module");

        assert_ne!(missing, fingerprint_dependency_paths(&deps, Some(dir.path())));
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
    fn dependency_graph_invalidates_missing_directory_dependents_when_child_appears() {
        let dir = tempfile::tempdir().expect("temp dir");
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

    #[test]
    fn dependency_graph_accepts_generic_project_dependency_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("modules")).expect("create modules dir");
        fs::write(dir.path().join("modules/math.lk"), "export fn add() {}").expect("write module");
        let mut graph = ProcMacroDependencyGraph::default();

        graph.insert_paths("main.lk", &["modules"], dir.path());
        graph.insert_paths("other.lk", &["other-modules"], dir.path());

        let dependents = graph.take_dependents_for_changed_path(&dir.path().join("modules/math.lk"));

        assert!(dependents.contains("main.lk"));
        assert!(!dependents.contains("other.lk"));
    }

    #[test]
    fn dependency_graph_accepts_absolute_project_dependency_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let dependency_path = dir.path().join("generated.lk");
        fs::write(&dependency_path, "export fn generated() {}").expect("write dependency");
        let mut graph = ProcMacroDependencyGraph::default();

        graph.insert_paths("main.lk", &[dependency_path.as_path()], dir.path());

        let dependents = graph.take_dependents_for_changed_path(&dependency_path);

        assert!(dependents.contains("main.lk"));
    }

    #[test]
    fn dependency_graph_skips_empty_project_dependency_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut graph = ProcMacroDependencyGraph::default();

        graph.insert_paths("main.lk", &[""], dir.path());

        assert!(graph.is_empty());
    }
}
