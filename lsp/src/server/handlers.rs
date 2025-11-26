use std::collections::HashMap;
use std::sync::Arc;

use regex::Regex;
use ropey::Rope;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::LanguageServer;
use tracing::info;

use crate::analyzer::LkrAnalyzer;
use lkr_core::{token::Tokenizer, val};

use super::{
    formatting::format_lkr,
    inlay_hints::compute_inlay_hints_with_margin,
    semantic::common_prefix_suffix_delete_count,
    signature::{sig, sig_owned},
    state::{Document, LkrLanguageServer},
    text::{
        apply_incremental_change_rope, collect_named_keys_in_args, find_call_before_cursor, infer_call_at_position,
        position_to_char_idx,
    },
    utils::compute_content_hash,
    MAX_SEMANTIC_TOKENS,
};

#[tower_lsp::async_trait]
impl LanguageServer for LkrLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        info!("LKR Language Server initializing with params: {:?}", params.root_uri);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // Switch to INCREMENTAL now that we apply ranges with UTF-16 mapping
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::INCREMENTAL)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![".".to_string()]),
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
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
                    identifier: Some("lkr".to_string()),
                    inter_file_dependencies: false,
                    workspace_diagnostics: false,
                    work_done_progress_options: Default::default(),
                })),
                semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
                    SemanticTokensOptions {
                        work_done_progress_options: Default::default(),
                        legend: SemanticTokensLegend {
                            token_types: vec![
                                SemanticTokenType::COMMENT,
                                SemanticTokenType::KEYWORD,
                                SemanticTokenType::VARIABLE,
                                SemanticTokenType::FUNCTION,
                                SemanticTokenType::STRING,
                                SemanticTokenType::NUMBER,
                                SemanticTokenType::OPERATOR,
                                SemanticTokenType::PARAMETER,
                                SemanticTokenType::PROPERTY,
                                SemanticTokenType::NAMESPACE,
                                SemanticTokenType::TYPE,
                            ],
                            token_modifiers: vec![
                                SemanticTokenModifier::DECLARATION,
                                SemanticTokenModifier::DEFINITION,
                                SemanticTokenModifier::READONLY,
                                SemanticTokenModifier::STATIC,
                            ],
                        },
                        // Enable range-based semantic tokens so the editor can request
                        // only the visible region while typing for better responsiveness
                        range: Some(true),
                        // Enable delta to reduce payloads and UI work
                        full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                    },
                )),
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
            },
            server_info: Some(ServerInfo {
                name: "LKR Language Server".to_string(),
                version: Some("0.1.0".to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        info!("LKR Language Server initialized");
        let _ = self
            .client
            .log_message(MessageType::INFO, "LKR Language Server started")
            .await;
        // Load initial configuration from client
        self.load_config().await;
    }

    async fn shutdown(&self) -> Result<()> {
        info!("LKR Language Server shutting down");
        Ok(())
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        // Reload configuration when the client notifies of changes
        self.load_config().await;
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let document = Document {
            content: Rope::from_str(&params.text_document.text),
            version: params.text_document.version,
            cached_analysis: None,
            cached_semantic_tokens: None,
            cached_range_tokens: HashMap::new(),
            cached_inlay_hints: HashMap::new(),
            last_sent_semantic_tokens: None,
            last_sent_result_id: None,
            tokens_result_counter: 0,
            debounce_seq: 0,
            _last_content_hash: Some(compute_content_hash(&params.text_document.text)),
        };

        self.documents.insert(uri.clone(), document);
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

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        Ok(self.get_hover_info(uri, position).await)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let mut items = self.get_completions();

        // Add identifier-aware and stdlib-aware completions based on current line
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        if let Some(doc) = self.documents.get(uri) {
            let line_idx = position.line as usize;
            if line_idx < doc.content.len_lines() {
                let line = doc.content.line(line_idx).to_string();
                let line_start_char = doc.content.line_to_char(line_idx);
                let abs_char = position_to_char_idx(&doc.content, position);
                let within_line = abs_char.saturating_sub(line_start_char).min(line.chars().count());
                let line_prefix: String = line.chars().take(within_line).collect();
                let line_suffix: String = line.chars().skip(within_line).collect();

                // '@' context completions removed

                // Regexes for import/from and module dot access
                let import_re = Regex::new(r"(?:^|\s)import\s+([A-Za-z_]\w*)?$").ok();
                let from_re = Regex::new(r"(?:^|\s)from\s+([A-Za-z_]\w*)?$").ok();
                let moddot_re = Regex::new(r"([A-Za-z_]\w*)\.$").ok();
                let alias_method_re = Regex::new(r"([A-Za-z_]\w*)\.([A-Za-z_]*)$").ok();
                // import { ... } cursor inside braces; capture content before cursor
                let import_brace_re = Regex::new(r"(?:^|\s)import\s*\{([^}]*)$").ok();
                // In the suffix, look for '} from <module>' after the cursor
                let suffix_from_re = Regex::new(r"^\s*\}?\s*from\s+([A-Za-z_]\w*)").ok();
                // import "<path> cursor inside quotes
                let import_path_re = Regex::new(r#"(?:^|\s)import\s+\"([^\"]*)$"#).ok();

                if let Ok(mut analyzer) = self.analyzer.lock() {
                    // Suggest module names after `import` or `from`
                    if let Some(re) = &import_re {
                        if let Some(caps) = re.captures(&line_prefix) {
                            let typed = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            let modules = analyzer.list_stdlib_modules();
                            for m in modules.into_iter().filter(|m| m.starts_with(typed)) {
                                items.push(CompletionItem {
                                    label: m,
                                    kind: Some(CompletionItemKind::MODULE),
                                    detail: Some("LKR stdlib module".to_string()),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                    if let Some(re) = &from_re {
                        if let Some(caps) = re.captures(&line_prefix) {
                            let typed = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            let modules = analyzer.list_stdlib_modules();
                            for m in modules.into_iter().filter(|m| m.starts_with(typed)) {
                                items.push(CompletionItem {
                                    label: m,
                                    kind: Some(CompletionItemKind::MODULE),
                                    detail: Some("LKR stdlib module".to_string()),
                                    ..Default::default()
                                });
                            }
                        }
                    }

                    // Suggest exports after `alias.` or `alias.pref...` if alias is an imported module;
                    // otherwise suggest common type meta-methods (with optional prefix filtering).
                    if let (Some(am_re), Some(md_re)) = (&alias_method_re, &moddot_re) {
                        // Extract alias and typed prefix after the dot
                        let (alias_opt, typed_prefix) = if let Some(caps) = am_re.captures(&line_prefix) {
                            let alias = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            let typed = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                            (Some(alias), typed)
                        } else if let Some(caps) = md_re.captures(&line_prefix) {
                            let alias = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            (Some(alias), "")
                        } else {
                            (None, "")
                        };

                        if let Some(alias) = alias_opt {
                            let full_content = doc.content.to_string();
                            let alias_map = analyzer.collect_import_aliases(&full_content);
                            if let Some(module_name) = alias_map.get(alias) {
                                if let Some(mut exports) = analyzer.list_module_exports(module_name) {
                                    if !typed_prefix.is_empty() {
                                        exports.retain(|e| e.starts_with(typed_prefix));
                                    }
                                    for e in exports {
                                        items.push(CompletionItem {
                                            label: e,
                                            kind: Some(CompletionItemKind::FUNCTION),
                                            detail: Some(format!("{}.{}", module_name, alias)),
                                            ..Default::default()
                                        });
                                    }
                                }
                            } else {
                                // Fallback: suggest common type meta-methods; filter by typed_prefix when present
                                const LIST_METHODS: &[&str] = &[
                                    "len",
                                    "push",
                                    "concat",
                                    "join",
                                    "get",
                                    "first",
                                    "last",
                                    "map",
                                    "filter",
                                    "reduce",
                                    "take",
                                    "skip",
                                    "chain",
                                    "flatten",
                                    "unique",
                                    "chunk",
                                    "enumerate",
                                    "zip",
                                ];
                                const MAP_METHODS: &[&str] = &["len", "keys", "values", "has", "get"];
                                const STRING_METHODS: &[&str] = &[
                                    "len",
                                    "lower",
                                    "upper",
                                    "trim",
                                    "starts_with",
                                    "ends_with",
                                    "contains",
                                    "replace",
                                    "substring",
                                    "split",
                                    "join",
                                ];
                                let mut push_method = |name: &str, ty: &str| {
                                    if typed_prefix.is_empty() || name.starts_with(typed_prefix) {
                                        items.push(CompletionItem {
                                            label: name.to_string(),
                                            kind: Some(CompletionItemKind::METHOD),
                                            detail: Some(format!("{} method (meta)", ty)),
                                            ..Default::default()
                                        });
                                    }
                                };
                                for m in LIST_METHODS {
                                    push_method(m, "List");
                                }
                                for m in MAP_METHODS {
                                    push_method(m, "Map");
                                }
                                for m in STRING_METHODS {
                                    push_method(m, "String");
                                }
                            }
                        }
                    }

                    // Suggest exports inside `import { … } from <module>`
                    if let (Some(br_re), Some(sf_re)) = (&import_brace_re, &suffix_from_re) {
                        if let Some(br_caps) = br_re.captures(&line_prefix) {
                            if let Some(sf_caps) = sf_re.captures(&line_suffix) {
                                let module_name = sf_caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                if let Some(mut exports) = analyzer.list_module_exports(module_name) {
                                    // Determine typed prefix within braces
                                    let raw = br_caps.get(1).map(|m| m.as_str()).unwrap_or("");
                                    let last = raw.split(',').next_back().unwrap_or("").trim();
                                    let typed = last.split_whitespace().last().unwrap_or("");
                                    if !typed.is_empty() {
                                        exports.retain(|e| e.starts_with(typed));
                                    }
                                    for e in exports {
                                        items.push(CompletionItem {
                                            label: e,
                                            kind: Some(CompletionItemKind::FUNCTION),
                                            detail: Some(format!("from {}", module_name)),
                                            ..Default::default()
                                        });
                                    }
                                }
                            }
                        }
                    }

                    // Suggest file paths inside import "..."
                    if let Some(re) = &import_path_re {
                        if let Some(caps) = re.captures(&line_prefix) {
                            let typed = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                            // Determine base directories
                            let mut base_dirs = Vec::new();
                            if let Ok(mut p) = uri.to_file_path() {
                                if p.pop() {
                                    base_dirs.push(p.clone());
                                    base_dirs.push(p.join("lib"));
                                    base_dirs.push(p.join("modules"));
                                }
                            }
                            // Split typed into dir and file prefix
                            let (dir_part, file_prefix) = if let Some(pos) = typed.rfind('/') {
                                (&typed[..pos], &typed[pos + 1..])
                            } else {
                                ("", typed)
                            };
                            for base in base_dirs {
                                let root = if dir_part.is_empty() {
                                    base.clone()
                                } else {
                                    base.join(dir_part)
                                };
                                if let Ok(entries) = std::fs::read_dir(&root) {
                                    for e in entries.flatten() {
                                        if let Ok(ft) = e.file_type() {
                                            let name = e.file_name().to_string_lossy().to_string();
                                            if name.starts_with(file_prefix) {
                                                let rel = if dir_part.is_empty() {
                                                    name.clone()
                                                } else {
                                                    format!("{}/{}", dir_part, name)
                                                };
                                                let (label, kind) = if ft.is_dir() {
                                                    (format!("{}/", rel), CompletionItemKind::FOLDER)
                                                } else {
                                                    (rel, CompletionItemKind::FILE)
                                                };
                                                items.push(CompletionItem {
                                                    label,
                                                    kind: Some(kind),
                                                    detail: Some("File path".to_string()),
                                                    ..Default::default()
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Generic identifier/path completions based on current prefix
                    // Extract a simple alnum/underscore/dot suffix from the current line prefix
                    let prefix: String = {
                        let mut collected: Vec<char> = Vec::new();
                        for ch in line_prefix.chars().rev() {
                            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                                collected.push(ch);
                            } else {
                                break;
                            }
                        }
                        collected.reverse();
                        collected.into_iter().collect()
                    };
                    if !prefix.is_empty() {
                        let var_items = analyzer.get_var_completions(&prefix);
                        if !var_items.is_empty() {
                            let existing: std::collections::HashSet<String> =
                                items.iter().map(|ci| ci.label.clone()).collect();
                            for it in var_items {
                                if !existing.contains(&it.label) {
                                    items.push(it);
                                }
                            }
                        }
                    }

                    // Named-argument completions inside function calls
                    // Heuristic: find nearest '(' before cursor on this line, extract callee identifier,
                    // and propose remaining named parameters not yet provided.
                    if let Some((fname, start_idx)) = find_call_before_cursor(&line_prefix) {
                        // Collect provided named keys up to cursor in this argument list
                        let provided = collect_named_keys_in_args(&line_prefix[start_idx..]);
                        // Parse document to find function named parameters
                        let sigs = analyzer.collect_fn_named_param_decls(&doc.content.to_string());
                        if let Some(named_decls) = sigs.get(&fname) {
                            use std::collections::HashSet;
                            let provided_set: HashSet<&str> = provided.iter().map(|s| s.as_str()).collect();
                            for decl in named_decls {
                                if provided_set.contains(decl.name.as_str()) {
                                    continue;
                                }
                                let label = format!("{}:", decl.name);
                                let mut detail = String::new();
                                if let Some(ty) = &decl.type_annotation {
                                    detail.push_str(&ty.display());
                                }
                                let is_optional = matches!(decl.type_annotation, Some(val::Type::Optional(_)))
                                    || decl.default.is_some();
                                if !detail.is_empty() {
                                    detail.push(' ');
                                }
                                detail.push_str(if is_optional { "[optional]" } else { "[required]" });
                                items.push(CompletionItem {
                                    label,
                                    kind: Some(CompletionItemKind::FIELD),
                                    detail: if detail.is_empty() { None } else { Some(detail) },
                                    insert_text: None,
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(Some(CompletionResponse::Array(items)))
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
            if code == Some("lkr_file_not_found") || diag.message.starts_with("File not found:") {
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
                if !current.ends_with(".lkr") {
                    candidates.push(format!("{}.lkr", current));
                }
                if !current.starts_with("./") && !current.starts_with('/') {
                    candidates.push(format!("./{}", current));
                }
                for prefix in ["lib/", "modules/"] {
                    if !current.starts_with(prefix) {
                        candidates.push(format!("{}{}", prefix, current));
                        if !current.ends_with(".lkr") {
                            candidates.push(format!("{}{}.lkr", prefix, current));
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
        if let Some(symbol_name) = self.find_symbol_at_position(&content, position).await {
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
        if let Some(symbol) = self.find_symbol_at_position(&content, position).await {
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
                let _analyzer = LkrAnalyzer::default();
                // Try to find definition precisely to determine scope
                if let Some(def_loc) = self
                    .find_definition_precise(&content, &symbol_name, position, uri)
                    .await
                {
                    let fbodies = LkrAnalyzer::scan_function_blocks(&tokens, &spans);
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

        // Find the symbol at the cursor position
        if let Some(symbol_name) = self.find_symbol_at_position(&content, position).await {
            // Prefer precise resolver-based decl spans
            if let Some(definition_location) = self
                .find_definition_precise(&content, &symbol_name, position, uri)
                .await
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(definition_location)));
            }
            // Fallback: heuristic text scan
            if let Some(definition_location) = self.find_definition(&content, &symbol_name, uri).await {
                return Ok(Some(GotoDefinitionResponse::Scalar(definition_location)));
            }
        }

        Ok(None)
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let mut lenses: Vec<CodeLens> = Vec::new();
        // Lens: Analyze file
        lenses.push(CodeLens {
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            command: Some(Command {
                title: "Analyze file".to_string(),
                command: "lkr.analyzeCurrentFile".to_string(),
                arguments: None,
            }),
            data: None,
        });

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
                        command: "lkr.showStatusBarMenu".to_string(),
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
        let formatted = format_lkr(&content, &options);
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
        let uri = &params.text_document.uri;
        let (content, version, cached_opt) = if let Some(doc) = self.documents.get(uri) {
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
            (doc.content.to_string(), doc.version, Some(key))
        } else {
            (String::new(), 0, None)
        };
        // Apply server-side configuration for inlay hints
        let cfg = self.config.lock().unwrap().clone();
        if !cfg.inlay_hints_enabled || content.is_empty() {
            return Ok(None);
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
                    let analyzer = LkrAnalyzer::new_light();
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
        Ok((!filtered.is_empty()).then_some(filtered))
    }

    async fn semantic_tokens_full(&self, params: SemanticTokensParams) -> Result<Option<SemanticTokensResult>> {
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

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id,
            data: clamped,
        })))
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
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
            let analyzer = LkrAnalyzer::new_light();
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

        // Compute a compact delta with a single edit using common prefix/suffix
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
        let (cp, cs, delete_count) = common_prefix_suffix_delete_count(&prev_vec, &new_tokens);
        if delete_count == 0 {
            // No structural change; in theory could return empty edits
            return Ok(Some(SemanticTokensFullDeltaResult::TokensDelta(SemanticTokensDelta {
                result_id: new_result_id,
                edits: vec![],
            })));
        }

        let insert_slice: Vec<SemanticToken> = new_tokens[cp..(new_tokens.len() - cs)].to_vec();
        let edit = SemanticTokensEdit {
            start: cp as u32,
            delete_count: delete_count as u32,
            data: Some(insert_slice),
        };
        Ok(Some(SemanticTokensFullDeltaResult::TokensDelta(SemanticTokensDelta {
            result_id: new_result_id,
            edits: vec![edit],
        })))
    }
}
