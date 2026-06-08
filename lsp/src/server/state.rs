use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use lk_completion::CompletionEngine;
use ropey::Rope;
use tokio::sync::Semaphore;
use tower_lsp::lsp_types::{InlayHint, SemanticToken, Url};
use tower_lsp::Client;

use super::workspace_cache::WorkspaceCache;
use crate::analyzer::{AnalysisResult, LkAnalyzer};

/// In-memory representation of an open LK document and its cached LSP artifacts.
#[derive(Debug, Default)]
pub(crate) struct Document {
    pub(crate) content: Rope,
    pub(crate) version: i32,
    pub(crate) cached_analysis: Option<Arc<AnalysisResult>>,
    pub(crate) cached_semantic_tokens: Option<Arc<Vec<SemanticToken>>>,
    pub(crate) cached_range_tokens: HashMap<String, Arc<Vec<SemanticToken>>>,
    pub(crate) cached_inlay_hints: HashMap<String, Arc<Vec<InlayHint>>>,
    pub(crate) last_sent_semantic_tokens: Option<Arc<Vec<SemanticToken>>>,
    pub(crate) last_sent_result_id: Option<String>,
    pub(crate) tokens_result_counter: u64,
    pub(crate) debounce_seq: u64,
    pub(crate) _last_content_hash: Option<u64>,
}

/// Primary LSP server state shared across handlers.
pub(crate) struct LkLanguageServer {
    pub(crate) client: Client,
    pub(crate) documents: Arc<DashMap<Url, Document>>,
    pub(crate) analyzer: Mutex<LkAnalyzer>,
    pub(crate) completion_engine: CompletionEngine,
    pub(crate) config: Mutex<super::config::ServerConfig>,
    // Shared limiter for all heavy analysis work (diagnostics, hover-derived lookups, etc.).
    pub(crate) compute_limiter: Mutex<Arc<Semaphore>>,
    pub(crate) workspace_root: Mutex<Option<PathBuf>>,
    pub(crate) workspace_cache: Arc<WorkspaceCache>,
}

impl LkLanguageServer {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(DashMap::new()),
            analyzer: Mutex::new(LkAnalyzer::new()),
            completion_engine: CompletionEngine::default(),
            config: Mutex::new(super::config::ServerConfig::default()),
            compute_limiter: Mutex::new(Arc::new(Semaphore::new(2))),
            workspace_root: Mutex::new(None),
            workspace_cache: Arc::new(WorkspaceCache::default()),
        }
    }
}
