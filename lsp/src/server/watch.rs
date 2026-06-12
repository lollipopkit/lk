use std::{collections::HashSet, sync::atomic::Ordering};

use tower_lsp::lsp_types::*;
use tracing::debug;

use super::state::{Document, LkLanguageServer};

const WATCHED_FILES_REGISTRATION_ID: &str = "lk.procMacroDependencyWatch";
const DID_CHANGE_WATCHED_FILES_METHOD: &str = "workspace/didChangeWatchedFiles";

pub(crate) fn supports_watched_files_dynamic_registration(params: &InitializeParams) -> bool {
    params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.did_change_watched_files)
        .and_then(|watched| watched.dynamic_registration)
        .unwrap_or(false)
}

fn watched_files_registration() -> Option<Registration> {
    let options = DidChangeWatchedFilesRegistrationOptions {
        watchers: vec![FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*".to_string()),
            kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
        }],
    };
    let register_options = serde_json::to_value(options).ok()?;
    Some(Registration {
        id: WATCHED_FILES_REGISTRATION_ID.to_string(),
        method: DID_CHANGE_WATCHED_FILES_METHOD.to_string(),
        register_options: Some(register_options),
    })
}

impl LkLanguageServer {
    pub(crate) async fn register_watched_files_if_supported(&self) {
        if !self.watched_files_dynamic_registration.load(Ordering::Acquire) {
            return;
        }
        let Some(registration) = watched_files_registration() else {
            return;
        };
        if let Err(err) = self.client.register_capability(vec![registration]).await {
            debug!(
                operation = "lsp.register_watched_files",
                error = %err,
                "failed to register watched files capability"
            );
        }
    }
}

pub(crate) fn clear_cached_document_artifacts(
    documents: &dashmap::DashMap<Url, Document>,
    affected: &HashSet<Url>,
) -> Vec<(Url, i32)> {
    let mut cleared = Vec::new();
    for uri in affected {
        if let Some(mut document) = documents.get_mut(uri) {
            document.cached_analysis = None;
            document.cached_semantic_tokens = None;
            document.cached_range_tokens.clear();
            document.cached_inlay_hints.clear();
            document.debounce_seq = document.debounce_seq.wrapping_add(1);
            cleared.push((uri.clone(), document.version));
        }
    }
    cleared
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use ropey::Rope;

    use super::*;

    #[test]
    fn watched_files_dynamic_registration_requires_client_support() {
        let mut params = InitializeParams::default();
        assert!(!supports_watched_files_dynamic_registration(&params));

        params.capabilities.workspace = Some(WorkspaceClientCapabilities {
            did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                dynamic_registration: Some(true),
                relative_pattern_support: None,
            }),
            ..WorkspaceClientCapabilities::default()
        });

        assert!(supports_watched_files_dynamic_registration(&params));
    }

    #[test]
    fn watched_files_registration_tracks_workspace_changes() {
        let registration = watched_files_registration().expect("watched-files registration");

        assert_eq!(registration.id, WATCHED_FILES_REGISTRATION_ID);
        assert_eq!(registration.method, DID_CHANGE_WATCHED_FILES_METHOD);
        let options: DidChangeWatchedFilesRegistrationOptions =
            serde_json::from_value(registration.register_options.expect("registration options"))
                .expect("decode registration options");
        assert_eq!(options.watchers.len(), 1);
        assert_eq!(
            options.watchers[0].glob_pattern,
            GlobPattern::String("**/*".to_string())
        );
        assert_eq!(
            options.watchers[0].kind,
            Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete)
        );
    }

    #[test]
    fn clearing_affected_documents_drops_open_document_caches() {
        let documents = dashmap::DashMap::new();
        let uri = Url::parse("file:///tmp/dependent.lk").expect("uri");
        let unaffected_uri = Url::parse("file:///tmp/unaffected.lk").expect("uri");
        documents.insert(
            uri.clone(),
            Document {
                content: Rope::from_str("return 1;"),
                version: 7,
                cached_analysis: Some(Arc::new(crate::analyzer::AnalysisResult {
                    diagnostics: Vec::new(),
                    symbols: Vec::new(),
                    identifier_roots: Default::default(),
                })),
                cached_semantic_tokens: Some(Arc::new(Vec::new())),
                cached_range_tokens: HashMap::from([("range".to_string(), Arc::new(Vec::new()))]),
                cached_inlay_hints: HashMap::from([("hints".to_string(), Arc::new(Vec::new()))]),
                last_sent_semantic_tokens: None,
                last_sent_result_id: None,
                tokens_result_counter: 0,
                debounce_seq: 3,
                _last_content_hash: Some(1),
            },
        );
        documents.insert(
            unaffected_uri.clone(),
            Document {
                content: Rope::from_str("return 2;"),
                version: 1,
                cached_analysis: Some(Arc::new(crate::analyzer::AnalysisResult {
                    diagnostics: Vec::new(),
                    symbols: Vec::new(),
                    identifier_roots: Default::default(),
                })),
                cached_semantic_tokens: Some(Arc::new(Vec::new())),
                cached_range_tokens: HashMap::new(),
                cached_inlay_hints: HashMap::new(),
                last_sent_semantic_tokens: None,
                last_sent_result_id: None,
                tokens_result_counter: 0,
                debounce_seq: 0,
                _last_content_hash: Some(2),
            },
        );
        let affected = HashSet::from([uri.clone()]);

        let cleared = clear_cached_document_artifacts(&documents, &affected);

        assert_eq!(cleared, vec![(uri.clone(), 7)]);
        let document = documents.get(&uri).expect("document");
        assert!(document.cached_analysis.is_none());
        assert!(document.cached_semantic_tokens.is_none());
        assert!(document.cached_range_tokens.is_empty());
        assert!(document.cached_inlay_hints.is_empty());
        assert_eq!(document.debounce_seq, 4);
        let unaffected = documents.get(&unaffected_uri).expect("unaffected document");
        assert!(unaffected.cached_analysis.is_some());
    }
}
