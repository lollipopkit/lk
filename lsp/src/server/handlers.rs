use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use regex::Regex;
use ropey::Rope;
use tokio::task;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::LanguageServer;
use tracing::{debug, info};

use crate::analyzer::LkAnalyzer;
use lk_core::token::Tokenizer;

use super::{
    formatting::format_lk,
    inlay_hints::compute_inlay_hints_with_margin,
    semantic::semantic_tokens_delta_edit,
    signature::{sig, sig_owned},
    state::{Document, LkLanguageServer},
    text::{apply_incremental_change_rope, infer_call_at_position, position_to_char_idx},
    utils::compute_content_hash,
    workspace_cache::{build_file_cache, filter_cached_inlay_hints},
    MAX_SEMANTIC_TOKENS,
};

mod initialization;

use initialization::semantic_tokens_provider_from;

fn server_capabilities_from(params: &InitializeParams) -> ServerCapabilities {
    ServerCapabilities {
        // Switch to INCREMENTAL now that we apply ranges with UTF-16 mapping.
        // Save notifications let us refresh the workspace cache after edits.
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::INCREMENTAL),
            will_save: Some(false),
            will_save_wait_until: Some(false),
            save: Some(TextDocumentSyncSaveOptions::Supported(true)),
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![
                ".".to_string(),
                "\"".to_string(),
                "{".to_string(),
                ",".to_string(),
                ":".to_string(),
            ]),
            work_done_progress_options: Default::default(),
            all_commit_characters: None,
            completion_item: None,
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Left(true)),
        // Diagnostics are pushed via textDocument/publishDiagnostics. Advertising
        // pull diagnostics as well makes VS Code show the same diagnostic twice.
        diagnostic_provider: None,
        semantic_tokens_provider: semantic_tokens_provider_from(params),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        document_formatting_provider: Some(OneOf::Left(true)),
        inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(InlayHintOptions {
            work_done_progress_options: Default::default(),
            resolve_provider: Some(false),
        }))),
        // Workspace capabilities left default; client will still send configuration changes
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_capabilities_use_push_diagnostics_only() {
        let params = InitializeParams::default();
        let capabilities = server_capabilities_from(&params);

        assert!(
            capabilities.diagnostic_provider.is_none(),
            "LK pushes diagnostics with textDocument/publishDiagnostics; pull diagnostics duplicate VS Code output"
        );
        assert!(
            capabilities.inlay_hint_provider.is_some(),
            "removing pull diagnostics must not disable inlay hints"
        );
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for LkLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        info!("LK Language Server initializing with params: {:?}", params.root_uri);
        let workspace_root = params
            .root_uri
            .as_ref()
            .and_then(|uri| uri.to_file_path().ok())
            .or_else(|| {
                params
                    .workspace_folders
                    .as_ref()
                    .and_then(|folders| folders.first())
                    .and_then(|folder| folder.uri.to_file_path().ok())
            });
        if let Ok(mut root) = self.workspace_root.lock() {
            *root = workspace_root.clone();
        }
        self.workspace_cache.set_root(workspace_root);

        Ok(InitializeResult {
            capabilities: server_capabilities_from(&params),
            server_info: Some(ServerInfo {
                name: "LK Language Server".to_string(),
                version: Some("0.1.0".to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        info!("LK Language Server initialized");
        let _ = self
            .client
            .log_message(MessageType::INFO, "LK Language Server started")
            .await;
        // Load initial configuration from client
        self.load_config().await;
        self.schedule_workspace_cache_preload();
    }

    async fn shutdown(&self) -> Result<()> {
        info!("LK Language Server shutting down");
        Ok(())
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        // Reload configuration when the client notifies of changes
        self.load_config().await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let start = Instant::now();
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let content_hash = compute_content_hash(&params.text_document.text);
        let cached = self.workspace_cache.get(&uri, content_hash);
        let cache_hit = cached.is_some();
        let document = Document {
            content: Rope::from_str(&params.text_document.text),
            version: params.text_document.version,
            cached_analysis: cached.as_ref().map(|cache| cache.analysis.clone()),
            cached_semantic_tokens: cached.as_ref().map(|cache| cache.semantic_tokens.clone()),
            cached_range_tokens: HashMap::new(),
            cached_inlay_hints: HashMap::new(),
            last_sent_semantic_tokens: None,
            last_sent_result_id: None,
            tokens_result_counter: 0,
            debounce_seq: 0,
            _last_content_hash: Some(content_hash),
        };

        self.documents.insert(uri.clone(), document);
        debug!(
            operation = "handler.did_open",
            uri = %uri,
            cache_hit = cache_hit,
            duration_ms = start.elapsed().as_millis(),
            "LSP did_open handled"
        );
        // Defer heavy analysis on open to keep editor responsive
        // Schedule diagnostics shortly after open instead of blocking here
        self.schedule_diagnostics_and_warmup(uri, version, 150).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        // Apply all changes (supports both full and incremental)
        {
            let mut entry = self.documents.entry(uri.clone()).or_default();
            // Ensure version is monotonic (but still update even if not; clients may resend)
            entry.version = version;

            if params.content_changes.len() == 1 && params.content_changes[0].range.is_none() {
                // Full text replacement
                let change = params.content_changes.into_iter().next().unwrap();
                entry.content = Rope::from_str(&change.text);
            } else {
                // Incremental changes
                let changes = params.content_changes;
                for change in changes {
                    apply_incremental_change_rope(&mut entry.content, &change);
                }
            }

            // Invalidate caches and bump debounce seq
            entry.cached_analysis = None;
            entry.cached_semantic_tokens = None;
            entry.cached_range_tokens.clear();
            entry.cached_inlay_hints.clear();
            entry.debounce_seq = entry.debounce_seq.wrapping_add(1);
        }

        // Periodically clear analyzer caches to prevent memory growth
        if self.documents.len() > 50 {
            if let Ok(mut analyzer) = self.analyzer.lock() {
                analyzer.clear_caches();
            }
        }

        // Debounced diagnostics (no token prewarm to keep edits snappy)
        self.schedule_diagnostics_and_warmup(uri, version, 250).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some((content, base_dir)) = self.documents.get(&uri).and_then(|doc| {
            let base_dir = uri
                .to_file_path()
                .ok()
                .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))?;
            Some((doc.content.to_string(), base_dir))
        }) else {
            return;
        };

        let cache = self.workspace_cache.clone();
        tokio::spawn(async move {
            let cache_for_build = cache.clone();
            let entry = task::spawn_blocking(move || {
                let mut analyzer = LkAnalyzer::new();
                let (base, modules, missing) = cache_for_build.package_context_for(base_dir);
                if modules.is_empty() && missing.is_empty() {
                    analyzer.set_base_dir(base);
                } else {
                    analyzer.set_package_context(base, modules, missing);
                }
                build_file_cache(&mut analyzer, &content)
            })
            .await;
            if let Ok(entry) = entry {
                cache.insert(uri, entry);
            }
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.remove(&uri);
        let _ = self
            .client
            .send_notification::<notification::PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                version: None,
                diagnostics: Vec::new(),
            })
            .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        Ok(self.get_hover_info(uri, position).await)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        Ok(self.completion_response(uri, position, params.context.as_ref()))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        // Snapshot document content for textual replacements
        let content = if let Some(doc) = self.documents.get(uri) {
            doc.content.to_string()
        } else {
            String::new()
        };

        let rope = ropey::Rope::from_str(&content);
        for diag in &params.context.diagnostics {
            let code = diag.code.as_ref().and_then(|c| match c {
                NumberOrString::String(s) => Some(s.as_str()),
                _ => None,
            });
            if code == Some("lk_file_not_found") || diag.message.starts_with("File not found:") {
                // Extract quoted string at diagnostic range
                let start = position_to_char_idx(&rope, diag.range.start);
                let end = position_to_char_idx(&rope, diag.range.end);
                let slice: String = if start < end && end <= rope.len_chars() {
                    rope.slice(start..end).to_string()
                } else {
                    String::new()
                };
                let current = slice.trim_matches('"');

                let mut candidates: Vec<String> = Vec::new();
                if !current.ends_with(".lk") {
                    candidates.push(format!("{}.lk", current));
                }
                if !current.starts_with("./") && !current.starts_with('/') {
                    candidates.push(format!("./{}", current));
                }
                for prefix in ["lib/", "modules/"] {
                    if !current.starts_with(prefix) {
                        candidates.push(format!("{}{}", prefix, current));
                        if !current.ends_with(".lk") {
                            candidates.push(format!("{}{}.lk", prefix, current));
                        }
                    }
                }

                for cand in candidates {
                    let new_text = format!("\"{}\"", cand);
                    let edit = TextEdit {
                        range: diag.range,
                        new_text,
                    };
                    let we = WorkspaceEdit {
                        changes: Some(std::collections::HashMap::from([(uri.clone(), vec![edit])])),
                        ..Default::default()
                    };
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("Use path: {}", cand),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag.clone()]),
                        edit: Some(we),
                        command: None,
                        is_preferred: None,
                        disabled: None,
                        data: None,
                    }));
                }
            }
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }

    async fn diagnostic(&self, params: DocumentDiagnosticParams) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let diagnostics = self.validate_document(uri).await;

        Ok(DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
            RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items: diagnostics,
                },
            },
        )))
    }

    async fn document_symbol(&self, params: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        if let Some(analysis) = self.get_or_compute_analysis(uri).await {
            if !analysis.symbols.is_empty() {
                return Ok(Some(DocumentSymbolResponse::Nested(analysis.symbols.clone())));
            }
        }
        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        // Get document content to find symbol at position
        let content = {
            let doc = match self.documents.get(uri) {
                Some(doc) => doc,
                None => return Ok(None),
            };
            doc.content.to_string()
        };

        // Find the symbol at the cursor position
        if let Some(symbol_name) = self.find_plain_symbol_at_position(&content, position).await {
            // Find all references to this symbol in the document
            let locations = self.find_all_references(&content, &symbol_name, uri).await;

            if !locations.is_empty() {
                return Ok(Some(locations));
            }
        }

        Ok(None)
    }

    async fn document_highlight(&self, params: DocumentHighlightParams) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let content = if let Some(doc) = self.documents.get(uri) {
            doc.content.to_string()
        } else {
            String::new()
        };
        if let Some(symbol) = self.find_plain_symbol_at_position(&content, position).await {
            let locs = self.find_all_references(&content, &symbol, uri).await;
            if !locs.is_empty() {
                let highlights = locs
                    .into_iter()
                    .map(|loc| DocumentHighlight {
                        range: loc.range,
                        kind: Some(DocumentHighlightKind::TEXT),
                    })
                    .collect();
                return Ok(Some(highlights));
            }
        }
        Ok(None)
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name.clone();

        // Basic identifier validation: letters, digits, underscore; not starting with digit
        let is_valid_name = {
            let mut chars = new_name.chars();
            match chars.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                }
                _ => false,
            }
        };
        if !is_valid_name {
            return Ok(None);
        }

        // Snapshot content
        let content = if let Some(doc) = self.documents.get(uri) {
            doc.content.to_string()
        } else {
            String::new()
        };

        // Find symbol name at position
        let Some(symbol_name) = self.find_symbol_at_position(&content, position).await else {
            return Ok(None);
        };
        // '@' context paths removed

        // Prefer precise scope-restricted references using resolver + spans
        let locations = {
            // Tokenize to compute function body line ranges
            if let Ok((tokens, spans)) = Tokenizer::tokenize_enhanced_with_spans(&content) {
                let _analyzer = LkAnalyzer::default();
                // Try to find definition precisely to determine scope
                if let Some(def_loc) = self
                    .find_definition_precise(&content, &symbol_name, position, uri)
                    .await
                {
                    let fbodies = LkAnalyzer::scan_function_blocks(&tokens, &spans);
                    // Identify if this def is inside a function body by comparing lines (0-based)
                    let def_line0 = def_loc.range.start.line;
                    // Build line ranges for each function body
                    let mut body_line_ranges: Vec<(u32, u32)> = Vec::new();
                    for fb in &fbodies {
                        let s_line = spans.get(fb.body_start_idx).map(|s| s.start.line).unwrap_or(1);
                        let e_line = spans.get(fb.body_end_idx).map(|s| s.end.line).unwrap_or(s_line);
                        body_line_ranges.push((s_line.saturating_sub(1), e_line.saturating_sub(1)));
                    }
                    // Determine selected scope range (line-based)
                    let scope_range: Option<(u32, u32)> = body_line_ranges
                        .iter()
                        .find(|(s, e)| def_line0 >= *s && def_line0 <= *e)
                        .cloned();
                    let all = self.find_all_references(&content, &symbol_name, uri).await;
                    if let Some((sline, eline)) = scope_range {
                        // Keep only references within the function body
                        all.into_iter()
                            .filter(|loc| loc.range.start.line >= sline && loc.range.end.line <= eline)
                            .collect()
                    } else {
                        // Top-level definition: include all references across the document
                        all
                    }
                } else {
                    // Fall back to full-document references
                    self.find_all_references(&content, &symbol_name, uri).await
                }
            } else {
                self.find_all_references(&content, &symbol_name, uri).await
            }
        };
        if locations.is_empty() {
            return Ok(None);
        }
        let edits: Vec<TextEdit> = locations
            .into_iter()
            .map(|loc| TextEdit {
                range: loc.range,
                new_text: new_name.clone(),
            })
            .collect();
        let mut changes = std::collections::HashMap::new();
        changes.insert(uri.clone(), edits);
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }))
    }

    async fn prepare_rename(&self, params: TextDocumentPositionParams) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let position = params.position;
        // Snapshot content
        let (content, line_text) = if let Some(doc) = self.documents.get(uri) {
            let rope = &doc.content;
            let line_idx = position.line as usize;
            let line = if line_idx < rope.len_lines() {
                rope.line(line_idx).to_string()
            } else {
                String::new()
            };
            (rope.to_string(), line)
        } else {
            (String::new(), String::new())
        };

        let Some(symbol_name) = self.find_symbol_at_position(&content, position).await else {
            return Ok(None);
        };
        if symbol_name.is_empty() {
            return Ok(None);
        }

        // Compute the word range on the line around the cursor
        let mut start = position.character as usize;
        let mut end = position.character as usize;
        let chars: Vec<char> = line_text.chars().collect();
        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }
        while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            end += 1;
        }
        let range = Range::new(
            Position::new(position.line, start as u32),
            Position::new(position.line, end as u32),
        );
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder: symbol_name,
        }))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Snapshot content
        let content = if let Some(doc) = self.documents.get(uri) {
            doc.content.to_string()
        } else {
            String::new()
        };

        // Heuristically find the function name and active parameter index
        let (func_name, active_param) = infer_call_at_position(&content, position);
        if func_name.is_empty() {
            return Ok(None);
        }

        // Collect signatures from built-ins and current document definitions
        let mut signatures: Vec<SignatureInformation> = Vec::new();

        // Built-ins and selected stdlib functions/meta-methods
        match func_name.as_str() {
            "print" => signatures.push(sig(
                "print(fmt, ...args)",
                ["fmt", "...args"].as_slice(),
                "Global function - print without newline",
            )),
            "println" => signatures.push(sig(
                "println(fmt, ...args)",
                ["fmt", "...args"].as_slice(),
                "Global function - print with newline",
            )),
            "panic" => signatures.push(sig(
                "panic(message)",
                ["message"].as_slice(),
                "Global function - raise runtime error",
            )),
            // iter module
            "enumerate" => signatures.push(sig(
                "enumerate(list)",
                ["list"].as_slice(),
                "iter: Add 0-based index to each element; returns list of [index, value]",
            )),
            "range" => {
                signatures.push(sig(
                    "range(end)",
                    ["end"].as_slice(),
                    "iter: Generate [0, 1, ..., end-1]",
                ));
                signatures.push(sig(
                    "range(start, end)",
                    ["start", "end"].as_slice(),
                    "iter: Generate [start, ..., end) with step 1",
                ));
                signatures.push(sig(
                    "range(start, end, step)",
                    ["start", "end", "step"].as_slice(),
                    "iter: Generate arithmetic progression with given step (nonzero)",
                ));
            }
            "zip" => signatures.push(sig(
                "zip(list1, list2)",
                ["list1", "list2"].as_slice(),
                "iter: Pair elements into [a[i], b[i]] up to the shortest length",
            )),
            "take" => signatures.push(sig(
                "take(list, n)",
                ["list", "n"].as_slice(),
                "iter: First n elements (n <= 0 returns [])",
            )),
            "skip" => signatures.push(sig(
                "skip(list, n)",
                ["list", "n"].as_slice(),
                "iter: Elements after skipping first n (n <= 0 returns original)",
            )),
            "chain" => signatures.push(sig(
                "chain(list1, list2)",
                ["list1", "list2"].as_slice(),
                "iter: Concatenate two lists",
            )),
            "flatten" => signatures.push(sig(
                "flatten(list)",
                ["list"].as_slice(),
                "iter: Flatten one nesting level (non-lists pass through)",
            )),
            "unique" => signatures.push(sig(
                "unique(list)",
                ["list"].as_slice(),
                "iter: Stable de-duplicate preserving first occurrences",
            )),
            "chunk" => signatures.push(sig(
                "chunk(list, size)",
                ["list", "size"].as_slice(),
                "iter: Split into chunks of positive size",
            )),
            // list meta-methods and module functions (common ones)
            "map" => {
                signatures.push(sig(
                    "map(list, func)",
                    ["list", "func(value)"].as_slice(),
                    "Apply function to each element; returns transformed list",
                ));
                signatures.push(sig(
                    "list.map(func)",
                    ["func(value)"].as_slice(),
                    "Meta-method variant of map",
                ));
            }
            "filter" => {
                signatures.push(sig(
                    "filter(list, predicate)",
                    ["list", "predicate(value)"].as_slice(),
                    "Keep elements where predicate returns true (nil/false treated as false)",
                ));
                signatures.push(sig(
                    "list.filter(predicate)",
                    ["predicate(value)"].as_slice(),
                    "Meta-method variant of filter",
                ));
            }
            "reduce" => {
                signatures.push(sig(
                    "reduce(list, init, func)",
                    ["list", "init", "func(acc, value)"].as_slice(),
                    "Fold elements into an accumulator",
                ));
                signatures.push(sig(
                    "list.reduce(init, func)",
                    ["init", "func(acc, value)"].as_slice(),
                    "Meta-method variant of reduce",
                ));
            }
            "push" => signatures.push(sig(
                "push(list, value)",
                ["list", "value"].as_slice(),
                "Return a new list with value appended",
            )),
            "concat" => signatures.push(sig(
                "concat(list, other)",
                ["list", "other"].as_slice(),
                "Concatenate two lists",
            )),
            "join" => signatures.push(sig(
                "join(list<string>, delimiter)",
                ["list", "delimiter"].as_slice(),
                "Join list of strings with delimiter",
            )),
            "get" => signatures.push(sig(
                "get(list, index)",
                ["list", "index"].as_slice(),
                "Safe index access; returns value or nil",
            )),
            "first" => signatures.push(sig("first(list)", ["list"].as_slice(), "First element or nil")),
            "last" => signatures.push(sig("last(list)", ["list"].as_slice(), "Last element or nil")),
            "len" => signatures.push(sig(
                "len(value)",
                ["value"].as_slice(),
                "Length of list/map/string (where applicable)",
            )),
            _ => {}
        }

        // Prefer AST-based scan for user-defined functions to reflect named parameter blocks
        if let Ok(mut analyzer) = self.analyzer.lock() {
            let decls = analyzer.collect_fn_named_param_decls(&content);
            if let Some(named) = decls.get(&func_name) {
                // Build a label that shows positional placeholder and named block
                // Fallback: we don't know positional parameter names here; show only named block
                let mut parts: Vec<String> = Vec::new();
                if !named.is_empty() {
                    let named_parts: Vec<String> = named
                        .iter()
                        .map(|d| {
                            let mut s = String::new();
                            s.push_str(&d.name);
                            s.push_str(": ");
                            if let Some(ty) = &d.type_annotation {
                                s.push_str(&ty.display());
                            } else {
                                s.push_str("Any");
                            }
                            if let Some(def) = &d.default {
                                s.push_str(" = ");
                                s.push_str(&def.to_string());
                            }
                            s
                        })
                        .collect();
                    parts.push(format!("{{{}}}", named_parts.join(", ")));
                }
                let label = if parts.is_empty() {
                    format!("{}()", func_name)
                } else {
                    format!("{}({})", func_name, parts.join(", "))
                };
                // Parameters list for highlighting active index: approximate using named list
                let plist: Vec<String> = named.iter().map(|d| d.name.clone()).collect();
                signatures.push(sig_owned(label, plist, "User-defined function"));
            } else {
                // Fallback: regex-based simple signature from code
                let re = Regex::new(&format!(r"(?m)\bfn\s+{}\s*\(([^)]*)\)", regex::escape(&func_name))).unwrap();
                for caps in re.captures_iter(&content) {
                    if let Some(params_m) = caps.get(1) {
                        let params_str = params_m.as_str();
                        let params_list: Vec<String> = params_str
                            .split(',')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.split(':').next().unwrap_or("").trim().to_string())
                            .collect();
                        let label = format!("{}({})", func_name, params_list.join(", "));
                        signatures.push(sig_owned(label, params_list, "User-defined function"));
                    }
                }
            }
        }

        if signatures.is_empty() {
            return Ok(None);
        }

        let active = active_param.unwrap_or(0).min(
            signatures
                .first()
                .and_then(|s| s.parameters.as_ref())
                .map(|v| v.len().saturating_sub(1))
                .unwrap_or(0),
        ) as u32;
        Ok(Some(SignatureHelp {
            signatures,
            active_signature: Some(0),
            active_parameter: Some(active),
        }))
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Get document content to find symbol at position
        let content = {
            let doc = match self.documents.get(uri) {
                Some(doc) => doc,
                None => return Ok(None),
            };
            doc.content.to_string()
        };

        if let Some(import_location) = self.find_file_import_at_position(&content, position, uri).await {
            return Ok(Some(GotoDefinitionResponse::Scalar(import_location)));
        }
        if let Some(import_location) = self.find_package_import_at_position(&content, position, uri).await {
            return Ok(Some(GotoDefinitionResponse::Scalar(import_location)));
        }

        // Find the symbol at the cursor position
        if let Some(symbol) = self.find_symbol_context_at_position(&content, position).await {
            if let Some(definition_location) = self.find_imported_member_definition(&content, &symbol, uri).await {
                return Ok(Some(GotoDefinitionResponse::Scalar(definition_location)));
            }
            if let Some(definition_location) = self.find_imported_module_location(&content, &symbol.name, uri).await {
                return Ok(Some(GotoDefinitionResponse::Scalar(definition_location)));
            }

            // Prefer precise resolver-based decl spans
            if let Some(definition_location) = self
                .find_definition_precise(&content, &symbol.name, position, uri)
                .await
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(definition_location)));
            }
            // Fallback: heuristic text scan
            if let Some(definition_location) = self.find_definition(&content, &symbol.name, uri).await {
                return Ok(Some(GotoDefinitionResponse::Scalar(definition_location)));
            }
        }

        Ok(None)
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let mut lenses: Vec<CodeLens> = Vec::new();

        // Lens: Identifier roots used (if any)
        if let Some(analysis) = self.get_or_compute_analysis(uri).await {
            if !analysis.identifier_roots.is_empty() {
                let mut keys: Vec<_> = analysis.identifier_roots.iter().cloned().collect();
                keys.sort();
                let preview = if keys.len() <= 3 {
                    keys.join(", ")
                } else {
                    format!("{}, … ({} total)", keys[0..3].join(", "), keys.len())
                };
                lenses.push(CodeLens {
                    range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                    command: Some(Command {
                        title: format!("Identifier roots: {}", preview),
                        command: "lk.showStatusBarMenu".to_string(),
                        arguments: None,
                    }),
                    data: None,
                });
            }
        }

        Ok(Some(lenses))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let options = params.options;
        let content = if let Some(doc) = self.documents.get(uri) {
            doc.content.to_string()
        } else {
            String::new()
        };
        let formatted = format_lk(&content, &options);
        if formatted == content {
            return Ok(Some(vec![]));
        }
        // Full document replacement
        let rope = Rope::from_str(&content);
        let end = Position::new(
            rope.len_lines().saturating_sub(1) as u32,
            rope.line(rope.len_lines().saturating_sub(1)).len_chars() as u32,
        );
        let edit = TextEdit {
            range: Range::new(Position::new(0, 0), end),
            new_text: formatted,
        };
        Ok(Some(vec![edit]))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let start = Instant::now();
        let uri = &params.text_document.uri;
        let (content, version, content_hash, cached_opt) = if let Some(doc) = self.documents.get(uri) {
            let cfg = self.config.lock().unwrap().clone();
            let key = format!(
                "v{}:{}:{}-{}:{}:p{}:t{}",
                doc.version,
                params.range.start.line,
                params.range.start.character,
                params.range.end.line,
                params.range.end.character,
                cfg.inlay_hints_parameters as u8,
                cfg.inlay_hints_types as u8
            );
            if let Some(cached) = doc.cached_inlay_hints.get(&key) {
                return Ok(Some((**cached).clone()));
            }
            let content = doc.content.to_string();
            let content_hash = compute_content_hash(&content);
            (content, doc.version, content_hash, Some(key))
        } else {
            (String::new(), 0, 0, None)
        };
        // Apply server-side configuration for inlay hints
        let cfg = self.config.lock().unwrap().clone();
        if !cfg.inlay_hints_enabled || content.is_empty() {
            return Ok(None);
        }

        if let Some(cached) = self.workspace_cache.get(uri, content_hash) {
            let filtered = filter_cached_inlay_hints(
                cached.inlay_hints.as_ref(),
                params.range,
                cfg.inlay_hints_parameters,
                cfg.inlay_hints_types,
            );
            debug!(
                operation = "handler.inlay_hint",
                uri = %uri,
                cache_hit = true,
                hint_count = filtered.len(),
                duration_ms = start.elapsed().as_millis(),
                "LSP inlay hints handled"
            );
            return Ok((!filtered.is_empty()).then_some(filtered));
        }

        // Limit concurrent heavy computations
        let sem = self.compute_limiter.lock().unwrap().clone();
        let _permit = sem.acquire().await.ok();

        let want_params = cfg.inlay_hints_parameters;
        let want_types = cfg.inlay_hints_types;
        let margin = cfg.inlay_scan_margin_lines;
        let range = params.range;
        let computed = tokio::task::spawn_blocking(move || {
            let mut hints: Vec<InlayHint> = Vec::new();
            if want_params {
                hints.extend(compute_inlay_hints_with_margin(&content, range, margin));
            }
            if want_types {
                // Tokenize once and reuse across individual computations
                if let Ok((tokens, spans)) = Tokenizer::tokenize_enhanced_with_spans(&content) {
                    let analyzer = LkAnalyzer::new_light();
                    let mut h1 = analyzer.compute_type_inlay_hints_from_tokens(&tokens, &spans, range);
                    let mut h2 = analyzer.compute_define_type_hints_from_tokens(&tokens, &spans, range);
                    let mut h3 = analyzer.compute_function_return_type_hints_from_tokens(&tokens, &spans, range);
                    hints.append(&mut h1);
                    hints.append(&mut h2);
                    hints.append(&mut h3);
                }
            }
            hints
        })
        .await
        .ok()
        .unwrap_or_default();

        // Filter kinds based on config flags
        let filtered: Vec<InlayHint> = computed
            .into_iter()
            .filter(|h| match h.kind.unwrap_or(InlayHintKind::TYPE) {
                InlayHintKind::PARAMETER => want_params,
                InlayHintKind::TYPE => want_types,
                _ => true,
            })
            .collect();
        // Cache by version+range+settings
        if let (Some(key), Some(mut doc)) = (cached_opt, self.documents.get_mut(uri)) {
            if doc.version == version {
                if doc.cached_inlay_hints.len() >= 64 {
                    doc.cached_inlay_hints.clear();
                }
                doc.cached_inlay_hints.insert(key, Arc::new(filtered.clone()));
            }
        }
        debug!(
            operation = "handler.inlay_hint",
            uri = %uri,
            cache_hit = false,
            hint_count = filtered.len(),
            duration_ms = start.elapsed().as_millis(),
            "LSP inlay hints handled"
        );
        Ok((!filtered.is_empty()).then_some(filtered))
    }

    async fn semantic_tokens_full(&self, params: SemanticTokensParams) -> Result<Option<SemanticTokensResult>> {
        let start = Instant::now();
        let uri = &params.text_document.uri;
        // Compute or fetch tokens for current doc state
        let tokens_arc = match self.get_or_generate_semantic_tokens(uri).await {
            Some(t) => t,
            None => return Ok(None),
        };

        // Clamp payload size for responsiveness and store the clamped baseline
        let clamped: Vec<SemanticToken> = (*tokens_arc).iter().take(MAX_SEMANTIC_TOKENS).cloned().collect();
        let clamped_arc = Arc::new(clamped.clone());

        // Produce a fresh result_id tied to current version/counter
        let result_id = {
            if let Some(mut doc) = self.documents.get_mut(uri) {
                doc.tokens_result_counter = doc.tokens_result_counter.wrapping_add(1);
                let id = format!("v{}-g{}", doc.version, doc.tokens_result_counter);
                doc.last_sent_semantic_tokens = Some(clamped_arc);
                doc.last_sent_result_id = Some(id.clone());
                Some(id)
            } else {
                None
            }
        };

        debug!(
            operation = "handler.semantic_tokens_full",
            uri = %uri,
            token_count = clamped.len(),
            duration_ms = start.elapsed().as_millis(),
            "LSP semantic tokens full handled"
        );
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id,
            data: clamped,
        })))
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
        let start = Instant::now();
        let uri = &params.text_document.uri;
        // Snapshot and versioned range cache lookup
        let (slice_string, range, version) = if let Some(doc) = self.documents.get(uri) {
            let key = format!(
                "v{}:{}:{}-{}:{}",
                doc.version,
                params.range.start.line,
                params.range.start.character,
                params.range.end.line,
                params.range.end.character
            );
            if let Some(cached) = doc.cached_range_tokens.get(&key) {
                // Return cached result immediately
                let data = (**cached).clone();
                debug!(
                    operation = "handler.semantic_tokens_range",
                    uri = %uri,
                    cache_hit = true,
                    token_count = data.len(),
                    duration_ms = start.elapsed().as_millis(),
                    "LSP semantic tokens range handled"
                );
                return Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
                    result_id: None,
                    data,
                })));
            }
            let start_char = position_to_char_idx(&doc.content, params.range.start);
            let end_char = position_to_char_idx(&doc.content, params.range.end);
            let s = start_char.min(doc.content.len_chars());
            let e = end_char.min(doc.content.len_chars()).max(s);
            let slice_string = doc.content.slice(s..e).to_string();
            (slice_string, params.range, doc.version)
        } else {
            return Ok(None);
        };

        // Limit concurrent heavy computations
        let sem = self.compute_limiter.lock().unwrap().clone();
        let _permit = sem.acquire().await.ok();

        // Generate range tokens off the async runtime
        let generated = tokio::task::spawn_blocking(move || {
            let analyzer = LkAnalyzer::new_light();
            analyzer.generate_semantic_tokens_in_range(&slice_string, range)
        })
        .await
        .ok()
        .unwrap_or_default();

        // Store in versioned range cache if still applicable
        if let Some(mut doc) = self.documents.get_mut(uri) {
            if doc.version == version {
                let key = format!(
                    "v{}:{}:{}-{}:{}",
                    version, range.start.line, range.start.character, range.end.line, range.end.character
                );
                let limit = self.config.lock().unwrap().range_token_cache_limit.max(1);
                if doc.cached_range_tokens.len() >= limit {
                    doc.cached_range_tokens.clear();
                }
                doc.cached_range_tokens.insert(key, Arc::new(generated.clone()));
            }
        }

        debug!(
            operation = "handler.semantic_tokens_range",
            uri = %uri,
            cache_hit = false,
            token_count = generated.len(),
            duration_ms = start.elapsed().as_millis(),
            "LSP semantic tokens range handled"
        );
        Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
            result_id: None,
            data: generated,
        })))
    }

    async fn semantic_tokens_full_delta(
        &self,
        params: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensFullDeltaResult>> {
        let uri = &params.text_document.uri;

        // Compute fresh tokens for current doc state
        let new_tokens_full = match self.get_or_generate_semantic_tokens(uri).await {
            Some(t) => t,
            None => return Ok(None),
        };
        // Clamp to match what we send to clients
        let new_tokens: Vec<SemanticToken> = (*new_tokens_full).iter().take(MAX_SEMANTIC_TOKENS).cloned().collect();

        // Read previous baseline (last sent) and id
        let (prev_tokens_opt, prev_id_opt) = if let Some(doc) = self.documents.get(uri) {
            (doc.last_sent_semantic_tokens.clone(), doc.last_sent_result_id.clone())
        } else {
            (None, None)
        };

        // If client's previousResultId doesn't match our last sent id, fall back to full tokens
        let prev_id_matches = if let Some(server_prev) = prev_id_opt.clone() {
            params.previous_result_id == server_prev
        } else {
            false
        };

        // Compute new result_id and update last_sent baseline (store clamped)
        let new_result_id = if let Some(mut doc) = self.documents.get_mut(uri) {
            doc.tokens_result_counter = doc.tokens_result_counter.wrapping_add(1);
            let id = format!("v{}-g{}", doc.version, doc.tokens_result_counter);
            doc.last_sent_semantic_tokens = Some(Arc::new(new_tokens.clone()));
            doc.last_sent_result_id = Some(id.clone());
            Some(id)
        } else {
            None
        };

        if !prev_id_matches {
            // Resync: send full tokens
            return Ok(Some(SemanticTokensFullDeltaResult::Tokens(SemanticTokens {
                result_id: new_result_id,
                data: new_tokens.clone(),
            })));
        }

        // Compute a compact delta with a single edit using common prefix/suffix.
        // LSP edit offsets are indexes into the flattened uinteger[] token data,
        // not token indexes.
        let prev_tokens = match prev_tokens_opt {
            Some(p) => p,
            None => {
                return Ok(Some(SemanticTokensFullDeltaResult::Tokens(SemanticTokens {
                    result_id: new_result_id,
                    data: new_tokens,
                })));
            }
        };

        let prev_vec: Vec<SemanticToken> = (*prev_tokens).clone();
        let edits = semantic_tokens_delta_edit(&prev_vec, &new_tokens)
            .map(|edit| vec![edit])
            .unwrap_or_default();
        Ok(Some(SemanticTokensFullDeltaResult::TokensDelta(SemanticTokensDelta {
            result_id: new_result_id,
            edits,
        })))
    }
}
