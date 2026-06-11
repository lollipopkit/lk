use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use dashmap::DashMap;
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, Position, Range, SemanticToken, Url};
use tracing::{debug, warn};

use crate::analyzer::{AnalysisResult, LkAnalyzer};
use lk_core::{
    macro_system::{
        fingerprint_proc_macro_dependencies, ProcMacroDependency, ProcMacroDependencyFingerprint,
        ProcMacroDependencyGraph,
    },
    package::PackageGraph,
    token::Tokenizer,
};

use super::{inlay_hints::compute_inlay_hints_with_margin, utils::compute_content_hash};

const MAX_WORKSPACE_FILES: usize = 2_000;
const MAX_CACHED_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceFileCache {
    pub(crate) content_hash: u64,
    pub(crate) analysis: Arc<AnalysisResult>,
    pub(crate) semantic_tokens: Arc<Vec<SemanticToken>>,
    pub(crate) inlay_hints: Arc<Vec<InlayHint>>,
    proc_macro_dependencies: Arc<Vec<ProcMacroDependency>>,
    proc_macro_dependency_fingerprint: ProcMacroDependencyFingerprint,
}

#[derive(Debug, Default)]
struct PackageContext {
    modules: HashMap<String, PathBuf>,
    missing: HashSet<String>,
}

#[derive(Debug, Default)]
pub(crate) struct WorkspaceCache {
    files: DashMap<Url, WorkspaceFileCache>,
    root: Mutex<Option<PathBuf>>,
    package_context: Mutex<PackageContext>,
    proc_macro_dependents: Mutex<ProcMacroDependencyGraph<Url>>,
    preloading: AtomicBool,
}

impl WorkspaceCache {
    pub(crate) fn set_root(&self, root: Option<PathBuf>) {
        let normalized = root.and_then(|root| root.canonicalize().ok().or(Some(root)));
        if let Ok(mut current) = self.root.lock() {
            if *current == normalized {
                return;
            }
            *current = normalized;
        }
        self.files.clear();
        if let Ok(mut ctx) = self.package_context.lock() {
            *ctx = PackageContext::default();
        }
        self.clear_proc_macro_dependents();
    }

    pub(crate) fn get(&self, uri: &Url, content_hash: u64) -> Option<WorkspaceFileCache> {
        let cached = self.files.get(uri)?;
        if cached.content_hash != content_hash {
            return None;
        }
        let entry = cached.clone();
        drop(cached);
        if entry.proc_macro_dependency_fingerprint.is_current(
            entry.proc_macro_dependencies.as_ref(),
            dependency_base_dir(uri).as_deref(),
        ) {
            Some(entry)
        } else {
            self.files.remove(uri);
            self.remove_proc_macro_dependents_for_uri(uri);
            None
        }
    }

    pub(crate) fn insert(&self, uri: Url, entry: WorkspaceFileCache) {
        self.remove_proc_macro_dependents_for_uri(&uri);
        self.insert_proc_macro_dependents(&uri, &entry);
        self.files.insert(uri, entry);
    }

    pub(crate) fn invalidate_proc_macro_dependents(&self, changed_path: &Path) {
        let dependents = self
            .proc_macro_dependents
            .lock()
            .ok()
            .map(|mut graph| graph.take_dependents_for_changed_path(changed_path));
        let Some(dependents) = dependents.filter(|dependents| !dependents.is_empty()) else {
            return;
        };
        for uri in dependents {
            self.files.remove(&uri);
            self.remove_proc_macro_dependents_for_uri(&uri);
        }
    }

    pub(crate) fn package_context_for(
        &self,
        base_dir: PathBuf,
    ) -> (PathBuf, HashMap<String, PathBuf>, HashSet<String>) {
        if let Ok(ctx) = self.package_context.lock() {
            return (base_dir, ctx.modules.clone(), ctx.missing.clone());
        }
        (base_dir, HashMap::new(), HashSet::new())
    }

    pub(crate) fn preload(&self) {
        if self.preloading.swap(true, Ordering::AcqRel) {
            return;
        }

        let start = Instant::now();
        let root = self.root.lock().ok().and_then(|root| root.clone());
        let Some(root) = root else {
            self.preloading.store(false, Ordering::Release);
            return;
        };

        self.refresh_package_context(&root);

        let files = collect_lk_files(&root, MAX_WORKSPACE_FILES);
        let file_count = files.len();
        let mut indexed = 0usize;
        let mut analyzer = LkAnalyzer::new();
        let (modules, missing) = self
            .package_context
            .lock()
            .map(|ctx| (ctx.modules.clone(), ctx.missing.clone()))
            .unwrap_or_default();

        for path in files {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.len() > MAX_CACHED_FILE_BYTES {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| root.clone());
            analyzer.set_package_context(base_dir, modules.clone(), missing.clone());
            let entry = build_file_cache(&mut analyzer, &content);
            self.insert(uri, entry);
            indexed += 1;
        }

        debug!(
            operation = "workspace_cache.preload",
            root = %root.display(),
            files_seen = file_count,
            files_indexed = indexed,
            duration_ms = start.elapsed().as_millis(),
            "LSP workspace cache preload finished"
        );
        self.preloading.store(false, Ordering::Release);
    }

    fn refresh_package_context(&self, root: &Path) {
        let start = Instant::now();
        match PackageGraph::discover(root) {
            Ok(Some(graph)) => {
                let modules = graph
                    .modules
                    .into_iter()
                    .map(|module| (module.name, module.root))
                    .collect();
                let missing = graph.missing.into_iter().collect();
                if let Ok(mut ctx) = self.package_context.lock() {
                    ctx.modules = modules;
                    ctx.missing = missing;
                }
                debug!(
                    operation = "workspace_cache.package_graph",
                    root = %root.display(),
                    duration_ms = start.elapsed().as_millis(),
                    "LSP workspace package graph cached"
                );
            }
            Ok(None) => {}
            Err(err) => {
                warn!(
                    operation = "workspace_cache.package_graph",
                    root = %root.display(),
                    error = %err,
                    "failed to cache LK package graph"
                );
            }
        }
    }

    fn insert_proc_macro_dependents(&self, uri: &Url, entry: &WorkspaceFileCache) {
        let Some(base_dir) = dependency_base_dir(uri) else {
            return;
        };
        let Ok(mut graph) = self.proc_macro_dependents.lock() else {
            return;
        };
        graph.insert(uri.clone(), entry.proc_macro_dependencies.as_ref(), &base_dir);
    }

    fn remove_proc_macro_dependents_for_uri(&self, uri: &Url) {
        let Ok(mut graph) = self.proc_macro_dependents.lock() else {
            return;
        };
        graph.remove_dependent(uri);
    }

    fn clear_proc_macro_dependents(&self) {
        if let Ok(mut graph) = self.proc_macro_dependents.lock() {
            graph.clear();
        }
    }
}

pub(crate) fn build_file_cache(analyzer: &mut LkAnalyzer, content: &str) -> WorkspaceFileCache {
    let analysis = Arc::new(analyzer.analyze(content));
    let semantic_tokens = Arc::new(analyzer.generate_semantic_tokens(content));
    let inlay_hints = Arc::new(compute_full_inlay_hints(content));
    let proc_macro_dependencies = Arc::new(analyzer.proc_macro_dependencies(content));
    let proc_macro_dependency_fingerprint =
        fingerprint_proc_macro_dependencies(proc_macro_dependencies.as_ref(), analyzer.base_dir());
    WorkspaceFileCache {
        content_hash: compute_content_hash(content),
        analysis,
        semantic_tokens,
        inlay_hints,
        proc_macro_dependencies,
        proc_macro_dependency_fingerprint,
    }
}

fn dependency_base_dir(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

pub(crate) fn filter_cached_inlay_hints(
    hints: &[InlayHint],
    range: Range,
    want_params: bool,
    want_types: bool,
) -> Vec<InlayHint> {
    hints
        .iter()
        .filter(|hint| position_in_range(hint.position, range))
        .filter(|hint| match hint.kind.unwrap_or(InlayHintKind::TYPE) {
            InlayHintKind::PARAMETER => want_params,
            InlayHintKind::TYPE => want_types,
            _ => true,
        })
        .cloned()
        .collect()
}

fn compute_full_inlay_hints(content: &str) -> Vec<InlayHint> {
    if content.is_empty() {
        return Vec::new();
    }
    let range = full_range(content);
    let mut hints = compute_inlay_hints_with_margin(content, range, 0);
    if let Ok((tokens, spans)) = Tokenizer::tokenize_enhanced_with_spans(content) {
        let analyzer = LkAnalyzer::new_light();
        hints.extend(analyzer.compute_type_inlay_hints_from_tokens(&tokens, &spans, range));
        hints.extend(analyzer.compute_define_type_hints_from_tokens(&tokens, &spans, range));
        hints.extend(analyzer.compute_function_return_type_hints_from_tokens(&tokens, &spans, range));
    }
    hints
}

fn full_range(content: &str) -> Range {
    let mut line = 0u32;
    let mut character = 0u32;
    for current in content.split('\n') {
        character = current.chars().count() as u32;
        line += 1;
    }
    Range::new(Position::new(0, 0), Position::new(line.saturating_sub(1), character))
}

fn position_in_range(position: Position, range: Range) -> bool {
    if position.line < range.start.line || position.line > range.end.line {
        return false;
    }
    if position.line == range.start.line && position.character < range.start.character {
        return false;
    }
    if position.line == range.end.line && position.character > range.end.character {
        return false;
    }
    true
}

fn collect_lk_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_lk_files_inner(root, limit, &mut out);
    out
}

fn collect_lk_files_inner(dir: &Path, limit: usize, out: &mut Vec<PathBuf>) {
    if out.len() >= limit || should_skip_dir(dir) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_lk_files_inner(&path, limit, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("lk") {
            out.push(path);
        }
    }
}

fn should_skip_dir(dir: &Path) -> bool {
    matches!(
        dir.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "node_modules" | ".vscode" | ".idea")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tower_lsp::lsp_types::InlayHintLabel;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("lsp crate has workspace parent")
            .to_path_buf()
    }

    #[test]
    fn filters_cached_inlay_hints_by_range_and_kind() {
        let hints = vec![
            InlayHint {
                position: Position::new(1, 2),
                label: InlayHintLabel::from("x:".to_string()),
                kind: Some(InlayHintKind::PARAMETER),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: None,
                data: None,
            },
            InlayHint {
                position: Position::new(3, 0),
                label: InlayHintLabel::from(": Int".to_string()),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: None,
                data: None,
            },
        ];

        let filtered =
            filter_cached_inlay_hints(&hints, Range::new(Position::new(0, 0), Position::new(2, 0)), true, true);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].kind, Some(InlayHintKind::PARAMETER));
    }

    #[test]
    fn preloads_example_workspace_cache_quickly() {
        let root = repo_root().join("examples/lk-example-workspace");
        let main_path = root.join("apps/demo/src/main.lk");
        let content = fs::read_to_string(&main_path).expect("read example workspace main.lk");
        let uri = Url::from_file_path(&main_path).expect("example main uri");
        let cache = WorkspaceCache::default();
        cache.set_root(Some(root));

        let start = Instant::now();
        cache.preload();
        let elapsed = start.elapsed();
        eprintln!("workspace_cache.preload(example workspace) took: {elapsed:?}");

        let entry = cache
            .get(&uri, compute_content_hash(&content))
            .expect("example workspace main should be cached after preload");
        let messages: Vec<&str> = entry
            .analysis
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect();
        assert!(
            !messages.iter().any(|msg| msg.contains("Unknown module")),
            "example workspace cached analysis should resolve imports; diagnostics: {messages:?}"
        );
        assert!(
            elapsed <= Duration::from_millis(1000),
            "workspace cache preload took too long: {elapsed:?}"
        );
    }

    #[test]
    fn cache_entry_stales_when_proc_macro_dependency_changes() {
        let dir = std::env::temp_dir().join(format!(
            "lk_lsp_macro_dep_cache_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        let source_path = dir.join("main.lk");
        let dependency_path = dir.join("schema.txt");
        let content = "return 1;\n";
        fs::write(&source_path, content).expect("write source");
        fs::write(&dependency_path, "one").expect("write dependency");
        let uri = Url::from_file_path(&source_path).expect("source uri");
        let dependencies = Arc::new(vec![ProcMacroDependency {
            path: "schema.txt".to_string(),
            digest: None,
        }]);
        let cache = WorkspaceCache::default();
        cache.insert(
            uri.clone(),
            WorkspaceFileCache {
                content_hash: compute_content_hash(content),
                analysis: Arc::new(AnalysisResult {
                    diagnostics: Vec::new(),
                    symbols: Vec::new(),
                    identifier_roots: HashSet::new(),
                }),
                semantic_tokens: Arc::new(Vec::new()),
                inlay_hints: Arc::new(Vec::new()),
                proc_macro_dependencies: dependencies.clone(),
                proc_macro_dependency_fingerprint: fingerprint_proc_macro_dependencies(
                    dependencies.as_ref(),
                    Some(&dir),
                ),
            },
        );

        assert!(cache.get(&uri, compute_content_hash(content)).is_some());
        fs::write(dependency_path, "two").expect("rewrite dependency");

        assert!(cache.get(&uri, compute_content_hash(content)).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn proc_macro_dependency_graph_invalidates_dependent_cached_files() {
        let dir = std::env::temp_dir().join(format!(
            "lk_lsp_macro_dep_graph_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        let dependent_path = dir.join("dependent.lk");
        let independent_path = dir.join("independent.lk");
        let dependency_path = dir.join("schema.txt");
        let content = "return 1;\n";
        fs::write(&dependent_path, content).expect("write dependent");
        fs::write(&independent_path, content).expect("write independent");
        fs::write(&dependency_path, "schema").expect("write dependency");

        let dependent_uri = Url::from_file_path(&dependent_path).expect("dependent uri");
        let independent_uri = Url::from_file_path(&independent_path).expect("independent uri");
        let cache = WorkspaceCache::default();
        cache.insert(
            dependent_uri.clone(),
            test_workspace_file_cache(
                content,
                vec![ProcMacroDependency {
                    path: "schema.txt".to_string(),
                    digest: None,
                }],
                &dir,
            ),
        );
        cache.insert(
            independent_uri.clone(),
            test_workspace_file_cache(content, Vec::new(), &dir),
        );

        assert!(cache.get(&dependent_uri, compute_content_hash(content)).is_some());
        assert!(cache.get(&independent_uri, compute_content_hash(content)).is_some());

        cache.invalidate_proc_macro_dependents(&dependency_path);

        assert!(cache.get(&dependent_uri, compute_content_hash(content)).is_none());
        assert!(cache.get(&independent_uri, compute_content_hash(content)).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn proc_macro_dependency_graph_invalidates_directory_dependents() {
        let dir = std::env::temp_dir().join(format!(
            "lk_lsp_macro_dep_dir_graph_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("schema")).expect("create schema dir");
        let dependent_path = dir.join("dependent.lk");
        let unrelated_path = dir.join("unrelated.lk");
        let changed_path = dir.join("schema").join("user.lk");
        let content = "return 1;\n";
        fs::write(&dependent_path, content).expect("write dependent");
        fs::write(&unrelated_path, content).expect("write unrelated");
        fs::write(&changed_path, "schema").expect("write schema file");

        let dependent_uri = Url::from_file_path(&dependent_path).expect("dependent uri");
        let unrelated_uri = Url::from_file_path(&unrelated_path).expect("unrelated uri");
        let cache = WorkspaceCache::default();
        cache.insert(
            dependent_uri.clone(),
            test_workspace_file_cache(
                content,
                vec![ProcMacroDependency {
                    path: "schema".to_string(),
                    digest: None,
                }],
                &dir,
            ),
        );
        cache.insert(
            unrelated_uri.clone(),
            test_workspace_file_cache(
                content,
                vec![ProcMacroDependency {
                    path: "other-schema".to_string(),
                    digest: None,
                }],
                &dir,
            ),
        );

        cache.invalidate_proc_macro_dependents(&changed_path);

        assert!(cache.get(&dependent_uri, compute_content_hash(content)).is_none());
        assert!(cache.get(&unrelated_uri, compute_content_hash(content)).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    fn test_workspace_file_cache(
        content: &str,
        dependencies: Vec<ProcMacroDependency>,
        base_dir: &Path,
    ) -> WorkspaceFileCache {
        let dependencies = Arc::new(dependencies);
        WorkspaceFileCache {
            content_hash: compute_content_hash(content),
            analysis: Arc::new(AnalysisResult {
                diagnostics: Vec::new(),
                symbols: Vec::new(),
                identifier_roots: HashSet::new(),
            }),
            semantic_tokens: Arc::new(Vec::new()),
            inlay_hints: Arc::new(Vec::new()),
            proc_macro_dependencies: dependencies.clone(),
            proc_macro_dependency_fingerprint: fingerprint_proc_macro_dependencies(
                dependencies.as_ref(),
                Some(base_dir),
            ),
        }
    }
}
