use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use ropey::Rope;
use tokio::sync::Semaphore;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, InlayHint, SemanticToken, Url};
use tower_lsp::Client;

use crate::analyzer::{AnalysisResult, LkrAnalyzer};

/// In-memory representation of an open LKR document and its cached LSP artifacts.
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
pub(crate) struct LkrLanguageServer {
    pub(crate) client: Client,
    pub(crate) documents: Arc<DashMap<Url, Document>>,
    pub(crate) analyzer: Mutex<LkrAnalyzer>,
    pub(crate) config: Mutex<super::config::ServerConfig>,
    pub(crate) compute_limiter: Mutex<Arc<Semaphore>>,
}

impl LkrLanguageServer {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(DashMap::new()),
            analyzer: Mutex::new(LkrAnalyzer::new()),
            config: Mutex::new(super::config::ServerConfig::default()),
            compute_limiter: Mutex::new(Arc::new(Semaphore::new(2))),
        }
    }

    pub(crate) fn get_completions(&self) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        // LKR keywords
        let keywords = [
            "if", "else", "while", "let", "fn", "return", "break", "continue", "import", "from", "as", "go", "select",
            "case", "default", "true", "false", "nil", "spawn", "chan", "send", "recv",
        ];

        for keyword in keywords {
            items.push(CompletionItem {
                label: keyword.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some("LKR keyword".to_string()),
                ..Default::default()
            });
        }

        // Operators
        let operators = ["==", "!=", "<=", ">=", "&&", "||", "in", "<-"];
        for op in operators {
            items.push(CompletionItem {
                label: op.to_string(),
                kind: Some(CompletionItemKind::OPERATOR),
                detail: Some("LKR operator".to_string()),
                ..Default::default()
            });
        }

        // Standard library helper functions
        let stdlib_functions = [
            ("print", "Global function - print without newline"),
            ("println", "Global function - print with newline"),
            ("panic", "Global function - raise runtime error"),
        ];

        for (func, desc) in stdlib_functions {
            items.push(CompletionItem {
                label: func.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(desc.to_string()),
                ..Default::default()
            });
        }

        items
    }
}
