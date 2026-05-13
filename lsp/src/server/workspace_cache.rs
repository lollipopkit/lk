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
use lk_core::{package::PackageGraph, token::Tokenizer};

use super::{inlay_hints::compute_inlay_hints_with_margin, utils::compute_content_hash};

const MAX_WORKSPACE_FILES: usize = 2_000;
const MAX_CACHED_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceFileCache {
    pub(crate) content_hash: u64,
    pub(crate) analysis: Arc<AnalysisResult>,
    pub(crate) semantic_tokens: Arc<Vec<SemanticToken>>,
    pub(crate) inlay_hints: Arc<Vec<InlayHint>>,
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
    }

    pub(crate) fn get(&self, uri: &Url, content_hash: u64) -> Option<WorkspaceFileCache> {
        let cached = self.files.get(uri)?;
        (cached.content_hash == content_hash).then(|| cached.clone())
    }

    pub(crate) fn insert(&self, uri: Url, entry: WorkspaceFileCache) {
        self.files.insert(uri, entry);
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
            self.files.insert(uri, entry);
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
}

pub(crate) fn build_file_cache(analyzer: &mut LkAnalyzer, content: &str) -> WorkspaceFileCache {
    let analysis = Arc::new(analyzer.analyze(content));
    let semantic_tokens = Arc::new(analyzer.generate_semantic_tokens(content));
    let inlay_hints = Arc::new(compute_full_inlay_hints(content));
    WorkspaceFileCache {
        content_hash: compute_content_hash(content),
        analysis,
        semantic_tokens,
        inlay_hints,
    }
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
}
