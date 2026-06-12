use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ropey::Rope;
use tokio::task;
use tokio::time::{sleep, Duration};
use tower_lsp::lsp_types::*;

use crate::analyzer::{AnalysisResult, LkAnalyzer};
use lk_core::{
    package::{self, PackageGraph},
    resolve, stmt, syntax, token,
};

use super::hover::document_hover;
use super::macro_definition::{
    find_exported_macro_definition_in_content, find_local_macro_definition, generated_ast_item_definition_location,
    imported_macro_definition, ImportedMacroSource,
};
use super::state::LkLanguageServer;
use super::text::{find_token_at_offset, position_to_char_idx};
use super::utils::compute_content_hash;
use tracing::debug;

fn log_timing(stage: &str, uri: &Url, duration_ms: u128, details: &str) {
    debug!(
        operation = %stage,
        uri = %uri,
        duration_ms = duration_ms,
        details = %details,
        "LSP timing"
    );
}

fn import_module_name_at_position(content: &str, position: Position) -> Option<String> {
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).ok()?;
    let offset = position_to_char_idx(&Rope::from_str(content), position);
    for (idx, span) in spans.iter().enumerate() {
        if offset < span.start.offset || offset > span.end.offset {
            continue;
        }
        let token::Token::Id(module_name) = tokens.get(idx)? else {
            continue;
        };
        if is_import_module_token(&tokens, idx) {
            return Some(module_name.clone());
        }
    }
    None
}

fn plain_symbol_name_at_position(content: &str, position: Position) -> Option<String> {
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).ok()?;
    let offset = position_to_char_idx(&Rope::from_str(content), position);
    for (idx, span) in spans.iter().enumerate() {
        if offset >= span.start.offset && offset < span.end.offset {
            if let Some(token::Token::Id(name)) = tokens.get(idx) {
                return Some(name.clone());
            }
        }
    }
    None
}

fn interpolation_symbol_context_at_offset(content: &str, offset: usize) -> Option<SymbolContext> {
    let chars: Vec<char> = content.chars().collect();
    let token_offset = if chars.get(offset).is_some_and(|ch| is_ident_continue(*ch)) {
        offset
    } else if offset > 0 && chars.get(offset - 1).is_some_and(|ch| is_ident_continue(*ch)) {
        offset - 1
    } else {
        return None;
    };

    let mut start = token_offset;
    while start > 0 && is_ident_continue(chars[start - 1]) {
        start -= 1;
    }
    if !chars.get(start).is_some_and(|ch| is_ident_start(*ch)) {
        return None;
    }

    let mut end = token_offset + 1;
    while chars.get(end).is_some_and(|ch| is_ident_continue(*ch)) {
        end += 1;
    }

    let mut interpolation_open = None;
    let mut i = start;
    while i > 0 {
        if chars[i - 1] == '$' && chars[i] == '{' {
            interpolation_open = Some(i + 1);
            break;
        }
        if chars[i] == '}' || chars[i] == '\n' {
            return None;
        }
        i -= 1;
    }
    let interpolation_open = interpolation_open?;
    if start < interpolation_open {
        return None;
    }

    let mut interpolation_closed = false;
    for ch in chars.iter().skip(end) {
        if *ch == '}' {
            interpolation_closed = true;
            break;
        }
        if *ch == '\n' {
            return None;
        }
    }
    if !interpolation_closed {
        return None;
    }

    let name: String = chars[start..end].iter().collect();
    let qualifier = if start >= 2 && chars[start - 1] == '.' {
        let mut qualifier_start = start - 2;
        while qualifier_start > interpolation_open && is_ident_continue(chars[qualifier_start - 1]) {
            qualifier_start -= 1;
        }
        if chars.get(qualifier_start).is_some_and(|ch| is_ident_start(*ch)) {
            Some(chars[qualifier_start..start - 1].iter().collect())
        } else {
            None
        }
    } else {
        None
    };

    Some(SymbolContext { name, qualifier })
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

fn is_import_module_token(tokens: &[token::Token], idx: usize) -> bool {
    matches!(
        idx.checked_sub(1).and_then(|prev| tokens.get(prev)),
        Some(token::Token::Use | token::Token::From)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolContext {
    pub(crate) name: String,
    pub(crate) qualifier: Option<String>,
}

impl LkLanguageServer {
    pub(crate) fn schedule_workspace_cache_preload(&self) {
        let cache = self.workspace_cache.clone();
        tokio::spawn(async move {
            let _ = task::spawn_blocking(move || cache.preload()).await;
        });
    }

    pub(crate) async fn validate_document(&self, uri: &Url) -> Vec<Diagnostic> {
        match self.get_or_compute_analysis(uri).await {
            Some(analysis) => analysis.diagnostics.clone(),
            None => Vec::new(),
        }
    }

    pub(crate) async fn get_hover_info(&self, uri: &Url, _position: Position) -> Option<Hover> {
        let (content, offset) = {
            let doc = self.documents.get(uri)?;
            let off = position_to_char_idx(&doc.content, _position);
            (doc.content.to_string(), off)
        };

        let (tokens, spans, ast_macro_origins) = {
            if let Ok(mut analyzer) = self.analyzer.lock() {
                match analyzer.tokenize_with_spans_cached(&content) {
                    Ok(entry) => {
                        let tokens = entry.tokens.clone();
                        let spans = entry.spans.clone();
                        let ast_macro_origins = analyzer.ast_macro_origins(&content);
                        (tokens, spans, ast_macro_origins)
                    }
                    Err(_) => return None,
                }
            } else {
                return None;
            }
        };

        if let Some((idx, _token)) = find_token_at_offset(spans.as_ref(), tokens.as_ref(), offset) {
            let package_modules = uri
                .to_file_path()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .map(|base_dir| {
                    let (_, modules, _, _) = self.workspace_cache.package_context_for(base_dir);
                    modules
                })
                .unwrap_or_default();
            return Some(document_hover(
                &content,
                uri,
                tokens.as_ref(),
                spans.as_ref(),
                idx,
                &ast_macro_origins,
                &package_modules,
            ));
        }

        if let Some(analysis) = self.get_or_compute_analysis(uri).await {
            if !analysis.identifier_roots.is_empty() {
                let hover_text = format!("Identifier roots: {:?}", analysis.identifier_roots);
                return Some(Hover {
                    contents: HoverContents::Scalar(MarkedString::String(hover_text)),
                    range: None,
                });
            }
        }
        None
    }

    pub(crate) async fn get_or_compute_analysis(&self, uri: &Url) -> Option<Arc<AnalysisResult>> {
        if let Some(doc) = self.documents.get(uri) {
            if let Some(cached) = doc.cached_analysis.clone() {
                return Some(cached);
            }
        }

        let (content_snapshot, version_snapshot, seq_snapshot) = {
            let doc = self.documents.get(uri)?;
            (doc.content.to_string(), doc.version, doc.debounce_seq)
        };

        if let Some(cached) = self.workspace_cache.get(uri, compute_content_hash(&content_snapshot)) {
            if let Some(mut doc) = self.documents.get_mut(uri) {
                if doc.version == version_snapshot && doc.debounce_seq == seq_snapshot {
                    doc.cached_analysis = Some(cached.analysis.clone());
                    doc.cached_semantic_tokens = Some(cached.semantic_tokens.clone());
                }
            }
            return Some(cached.analysis);
        }

        let content_for_compute = content_snapshot.clone();
        let base_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let workspace_cache = self.workspace_cache.clone();

        let sem = self.compute_limiter.lock().unwrap().clone();
        let _permit = sem.acquire().await.ok()?;

        let start = Instant::now();
        let computed_result = task::spawn_blocking(move || {
            let mut analyzer = LkAnalyzer::new();
            if let Some(b) = base_dir {
                let (base, modules, missing, proc_macro_providers) = workspace_cache.package_context_for(b);
                if modules.is_empty() && missing.is_empty() {
                    analyzer.set_base_dir(base);
                } else {
                    analyzer.set_package_context(base, modules, missing, proc_macro_providers);
                }
            }
            analyzer.analyze(&content_for_compute)
        })
        .await
        .ok()?;

        let computed = Arc::new(computed_result);

        if let Some(elapsed) = Instant::now().checked_duration_since(start).map(|d| d.as_millis()) {
            log_timing(
                "get_or_compute_analysis",
                uri,
                elapsed,
                "full analysis for demand request",
            );
        }

        if let Some(mut doc) = self.documents.get_mut(uri) {
            if doc.version == version_snapshot && doc.debounce_seq == seq_snapshot {
                doc.cached_analysis = Some(computed.clone());
            }
        }
        Some(computed)
    }

    pub(crate) async fn get_or_generate_semantic_tokens(&self, uri: &Url) -> Option<Arc<Vec<SemanticToken>>> {
        if let Some(doc) = self.documents.get(uri) {
            if let Some(cached) = doc.cached_semantic_tokens.clone() {
                return Some(cached);
            }
        }

        let (content_snapshot, version_snapshot, seq_snapshot) = {
            let doc = self.documents.get(uri)?;
            (doc.content.to_string(), doc.version, doc.debounce_seq)
        };

        let content_for_tokens = content_snapshot.clone();
        if let Some(cached) = self.workspace_cache.get(uri, compute_content_hash(&content_snapshot)) {
            if let Some(mut doc) = self.documents.get_mut(uri) {
                if doc.version == version_snapshot && doc.debounce_seq == seq_snapshot {
                    doc.cached_analysis = Some(cached.analysis.clone());
                    doc.cached_semantic_tokens = Some(cached.semantic_tokens.clone());
                }
            }
            return Some(cached.semantic_tokens);
        }

        let base_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let sem = self.compute_limiter.lock().unwrap().clone();
        let _permit = sem.acquire().await.ok();
        let workspace_cache = self.workspace_cache.clone();
        let start = Instant::now();
        let generated_result = task::spawn_blocking(move || {
            let mut analyzer = LkAnalyzer::new_light();
            if let Some(b) = base_dir {
                let (base, modules, missing, proc_macro_providers) = workspace_cache.package_context_for(b);
                if modules.is_empty() && missing.is_empty() {
                    analyzer.set_base_dir(base);
                } else {
                    analyzer.set_package_context(base, modules, missing, proc_macro_providers);
                }
            }
            analyzer.generate_semantic_tokens(&content_for_tokens)
        })
        .await
        .ok()?;
        let generated = Arc::new(generated_result);

        if let Some(elapsed) = Instant::now().checked_duration_since(start).map(|d| d.as_millis()) {
            log_timing(
                "generate_semantic_tokens",
                uri,
                elapsed,
                "full semantic token generation",
            );
        }

        if let Some(mut doc) = self.documents.get_mut(uri) {
            if doc.version == version_snapshot && doc.debounce_seq == seq_snapshot {
                doc.cached_semantic_tokens = Some(generated.clone());
            }
        }
        Some(generated)
    }

    pub(crate) async fn schedule_diagnostics_and_warmup(&self, uri: Url, scheduled_version: i32, delay_ms: u64) {
        let documents = self.documents.clone();
        let client = self.client.clone();
        let sem = self.compute_limiter.lock().unwrap().clone();
        let workspace_cache = self.workspace_cache.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(delay_ms)).await;

            let (content_snapshot, seq_snapshot, version_snapshot) = if let Some(doc) = documents.get(&uri) {
                (doc.content.to_string(), doc.debounce_seq, doc.version)
            } else {
                return;
            };

            if version_snapshot != scheduled_version
                || documents.get(&uri).is_none_or(|doc| doc.debounce_seq != seq_snapshot)
            {
                return;
            }

            let Some(_permit) = sem.acquire().await.ok() else {
                return;
            };

            if let Some(doc) = documents.get(&uri) {
                if doc.version != scheduled_version || doc.debounce_seq != seq_snapshot {
                    return;
                }
            } else {
                return;
            }

            let content_for_compute = content_snapshot.clone();
            let base_dir = uri
                .to_file_path()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()));

            let start = Instant::now();
            let content_len = content_for_compute.len();
            let computed_result =
                if let Some(cached) = workspace_cache.get(&uri, compute_content_hash(&content_for_compute)) {
                    Some((*cached.analysis).clone())
                } else {
                    task::spawn_blocking(move || {
                        let mut analyzer = LkAnalyzer::new();
                        if let Some(b) = base_dir {
                            let (base, modules, missing, proc_macro_providers) = workspace_cache.package_context_for(b);
                            if modules.is_empty() && missing.is_empty() {
                                analyzer.set_base_dir(base);
                            } else {
                                analyzer.set_package_context(base, modules, missing, proc_macro_providers);
                            }
                        }
                        analyzer.analyze(&content_for_compute)
                    })
                    .await
                    .ok()
                };

            if let Some(diagnostics_len) = computed_result.as_ref().map(|c| c.diagnostics.len()) {
                if let Some(elapsed) = Instant::now().checked_duration_since(start).map(|d| d.as_millis()) {
                    log_timing(
                        "schedule_diagnostics_and_warmup",
                        &uri,
                        elapsed,
                        &format!("diag_count={diagnostics_len}, content_len={}", content_len),
                    );
                }
            }

            let mut diagnostics_to_publish: Option<Vec<Diagnostic>> = None;
            if let Some(computed) = computed_result {
                diagnostics_to_publish = Some(computed.diagnostics.clone());
                if let Some(mut doc) = documents.get_mut(&uri) {
                    if doc.debounce_seq == seq_snapshot && doc.version == version_snapshot {
                        doc.cached_analysis = Some(Arc::new(computed));
                    }
                }
            }

            if let Some(diags) = diagnostics_to_publish {
                if !documents.contains_key(&uri) {
                    return;
                }
                let _ = client
                    .send_notification::<notification::PublishDiagnostics>(PublishDiagnosticsParams {
                        uri: uri.clone(),
                        version: Some(version_snapshot),
                        diagnostics: diags,
                    })
                    .await;
            }
        });
    }

    pub(crate) async fn find_symbol_at_position(&self, content: &str, position: Position) -> Option<String> {
        self.find_symbol_context_at_position(content, position)
            .await
            .map(|ctx| ctx.name)
    }

    pub(crate) async fn find_plain_symbol_at_position(&self, content: &str, position: Position) -> Option<String> {
        plain_symbol_name_at_position(content, position)
    }

    pub(crate) async fn find_symbol_context_at_position(
        &self,
        content: &str,
        position: Position,
    ) -> Option<SymbolContext> {
        let (tokens, spans) = match token::Tokenizer::tokenize_enhanced_with_spans(content) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let offset = position_to_char_idx(&Rope::from_str(content), position);
        if let Some(context) = interpolation_symbol_context_at_offset(content, offset) {
            return Some(context);
        }
        if let Some(context) = qualified_symbol_context_at_offset(&tokens, &spans, offset) {
            return Some(context);
        }
        for (i, span) in spans.iter().enumerate() {
            if offset >= span.start.offset && offset <= span.end.offset {
                if let token::Token::Id(name) = &tokens[i] {
                    let qualifier = match (i.checked_sub(2), i.checked_sub(1)) {
                        (Some(qualifier_idx), Some(dot_idx)) => {
                            if matches!(tokens.get(dot_idx), Some(token::Token::Dot)) {
                                match tokens.get(qualifier_idx) {
                                    Some(token::Token::Id(qualifier)) => Some(qualifier.clone()),
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    return Some(SymbolContext {
                        name: name.clone(),
                        qualifier,
                    });
                }
            }
        }
        None
    }

    pub(crate) async fn find_file_import_at_position(
        &self,
        content: &str,
        position: Position,
        current_uri: &Url,
    ) -> Option<Location> {
        let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).ok()?;
        let offset = position_to_char_idx(&Rope::from_str(content), position);
        for (i, span) in spans.iter().enumerate() {
            if offset < span.start.offset || offset > span.end.offset {
                continue;
            }
            let token::Token::Str(import_path) = &tokens[i] else {
                continue;
            };
            if !matches!(tokens.get(i.checked_sub(1)?), Some(token::Token::Use)) {
                continue;
            }
            let path = self.resolve_lk_import_path(import_path, current_uri)?;
            let uri = Url::from_file_path(path).ok()?;
            return Some(Location::new(uri, Range::new(Position::new(0, 0), Position::new(0, 0))));
        }
        None
    }

    pub(crate) async fn find_package_import_at_position(
        &self,
        content: &str,
        position: Position,
        current_uri: &Url,
    ) -> Option<Location> {
        let module_name = import_module_name_at_position(content, position)?;
        let path = self.resolve_package_module_path(&module_name, current_uri)?;
        let uri = Url::from_file_path(path).ok()?;
        Some(Location::new(uri, Range::new(Position::new(0, 0), Position::new(0, 0))))
    }

    pub(crate) async fn find_imported_member_definition(
        &self,
        content: &str,
        symbol: &SymbolContext,
        current_uri: &Url,
    ) -> Option<Location> {
        let qualifier = symbol.qualifier.as_ref()?;

        if let Some(module_name) = self.stdlib_module_for_alias(content, qualifier).await {
            if let Some(location) = find_stdlib_export_location(&module_name, &symbol.name) {
                return Some(location);
            }
        }

        let imports = self.collect_file_import_aliases(content, current_uri).await;
        if let Some(import_path) = imports.get(qualifier) {
            let imported_uri = Url::from_file_path(import_path).ok()?;
            let imported_content = fs::read_to_string(import_path).ok()?;
            if let Some(location) = self
                .find_definition_precise(&imported_content, &symbol.name, Position::new(0, 0), &imported_uri)
                .await
                .or_else(|| find_definition_in_content(&imported_content, &symbol.name, &imported_uri))
            {
                return Some(location);
            }
        }

        self.find_imported_package_member_definition(content, symbol, current_uri)
    }

    fn find_imported_package_member_definition(
        &self,
        content: &str,
        symbol: &SymbolContext,
        current_uri: &Url,
    ) -> Option<Location> {
        let qualifier = symbol.qualifier.as_ref()?;
        let module_name = self.imported_module_for_alias(content, qualifier)?;
        let import_path = self.resolve_package_module_path(&module_name, current_uri)?;
        let imported_uri = Url::from_file_path(&import_path).ok()?;
        let imported_content = fs::read_to_string(import_path).ok()?;
        find_definition_in_content(&imported_content, &symbol.name, &imported_uri)
    }

    pub(crate) async fn find_imported_module_location(
        &self,
        content: &str,
        symbol_name: &str,
        current_uri: &Url,
    ) -> Option<Location> {
        if let Some(path) = self
            .collect_file_import_aliases(content, current_uri)
            .await
            .get(symbol_name)
        {
            let uri = Url::from_file_path(path).ok()?;
            return Some(Location::new(uri, Range::new(Position::new(0, 0), Position::new(0, 0))));
        }

        if let Some(module_name) = self.stdlib_module_for_alias(content, symbol_name).await {
            return find_stdlib_module_location(&module_name);
        }

        None
    }

    pub(crate) async fn find_macro_definition_at_position(
        &self,
        content: &str,
        position: Position,
        uri: &Url,
    ) -> Option<Location> {
        let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).ok()?;
        let offset = position_to_char_idx(&Rope::from_str(content), position);
        find_macro_definition_at_offset(&tokens, &spans, offset, uri)
            .or_else(|| self.find_imported_macro_definition_at_offset(&tokens, &spans, offset, uri))
    }

    fn find_imported_macro_definition_at_offset(
        &self,
        tokens: &[token::Token],
        spans: &[token::Span],
        offset: usize,
        uri: &Url,
    ) -> Option<Location> {
        let imported = imported_macro_definition(tokens, spans, offset)?;
        let path = match imported.source {
            ImportedMacroSource::File(path) => self.resolve_lk_import_path(&path, uri)?,
            ImportedMacroSource::Package(module) => self.resolve_package_module_path(&module, uri)?,
        };
        let imported_uri = Url::from_file_path(&path).ok()?;
        let imported_content = fs::read_to_string(path).ok()?;
        find_exported_macro_definition_in_content(&imported_content, &imported.name, &imported_uri)
    }

    async fn collect_file_import_aliases(&self, content: &str, current_uri: &Url) -> HashMap<String, PathBuf> {
        let mut aliases = HashMap::new();
        let Ok((tokens, _spans)) = token::Tokenizer::tokenize_enhanced_with_spans(content) else {
            return aliases;
        };

        for (idx, tok) in tokens.iter().enumerate() {
            let token::Token::Str(import_path) = tok else {
                continue;
            };
            if !matches!(
                idx.checked_sub(1).and_then(|prev| tokens.get(prev)),
                Some(token::Token::Use)
            ) {
                continue;
            }
            let Some(path) = self.resolve_lk_import_path(import_path, current_uri) else {
                continue;
            };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            aliases.insert(stem.to_string(), path);
        }

        aliases
    }

    async fn stdlib_module_for_alias(&self, content: &str, alias: &str) -> Option<String> {
        let mut analyzer = self.analyzer.lock().ok()?;
        let aliases = analyzer.collect_import_aliases(content);
        aliases.get(alias).cloned()
    }

    fn imported_module_for_alias(&self, content: &str, alias: &str) -> Option<String> {
        let mut analyzer = LkAnalyzer::new_light();
        let aliases = analyzer.collect_import_aliases(content);
        aliases.get(alias).cloned()
    }

    fn resolve_lk_import_path(&self, import_path: &str, current_uri: &Url) -> Option<PathBuf> {
        if import_path.is_empty() || import_path.contains("..") || Path::new(import_path).is_absolute() {
            return None;
        }

        let root = self.workspace_root.lock().ok().and_then(|root| root.clone());
        let current_dir = current_uri
            .to_file_path()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));

        let mut bases = Vec::new();
        if let Some(root) = root {
            bases.push(root);
        }
        if let Some(current_dir) = current_dir {
            bases.push(current_dir);
        }

        for base in bases {
            let raw = base.join(import_path);
            for candidate in import_path_candidates(&raw) {
                if candidate.is_file() {
                    return candidate.canonicalize().ok().or(Some(candidate));
                }
            }
        }

        None
    }

    fn resolve_package_module_path(&self, module_name: &str, current_uri: &Url) -> Option<PathBuf> {
        let current_dir = current_uri
            .to_file_path()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))?;

        let (_, cached_modules, _, _) = self.workspace_cache.package_context_for(current_dir.clone());
        if let Some(path) = cached_modules.get(module_name) {
            return path.canonicalize().ok().or_else(|| Some(path.clone()));
        }

        let graph = PackageGraph::discover(&current_dir).ok().flatten()?;
        graph
            .modules
            .into_iter()
            .find(|module| module.name == module_name)
            .map(|module| module.root)
            .and_then(|path| path.canonicalize().ok().or(Some(path)))
    }

    pub(crate) async fn find_all_references(&self, content: &str, symbol_name: &str, uri: &Url) -> Vec<Location> {
        let mut references = Vec::new();
        let rope = Rope::from_str(content);
        let total_lines = rope.len_lines();

        for line_idx in 0..total_lines {
            let line = rope.line(line_idx).to_string();
            if line.contains(symbol_name) {
                if let Some(pos) = line.find(symbol_name) {
                    let range = Range::new(
                        Position::new(line_idx as u32, pos as u32),
                        Position::new(line_idx as u32, (pos + symbol_name.len()) as u32),
                    );
                    references.push(Location::new(uri.clone(), range));
                }
            }
        }

        references
    }

    pub(crate) async fn find_definition(&self, content: &str, symbol_name: &str, uri: &Url) -> Option<Location> {
        find_definition_in_content(content, symbol_name, uri)
    }

    pub(crate) async fn find_definition_precise(
        &self,
        content: &str,
        symbol_name: &str,
        pos: Position,
        uri: &Url,
    ) -> Option<Location> {
        let cursor_offset = position_to_char_idx(&Rope::from_str(content), pos);
        if let Ok((tokens, spans)) = token::Tokenizer::tokenize_enhanced_with_spans(content) {
            let mut parser = stmt::stmt_parser::StmtParser::new_with_spans(&tokens, &spans);
            if let Ok(program) = parser.parse_program_with_enhanced_errors(content) {
                if let Some(location) =
                    definition_location_in_program(&program, &tokens, &spans, symbol_name, cursor_offset, uri)
                {
                    return Some(location);
                }
            }
        }

        let expansion = syntax::expand_program_source(content, parse_options_for_uri(uri)).ok()?;

        definition_location_in_program(
            &expansion.program,
            &expansion.source.tokens,
            &expansion.source.spans,
            symbol_name,
            cursor_offset,
            uri,
        )
        .or_else(|| generated_ast_item_definition_location(&expansion.ast_macro_origins, symbol_name, uri))
    }
}

fn parse_options_for_uri(uri: &Url) -> syntax::ParseOptions {
    let base_dir = uri
        .to_file_path()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let mut options = syntax::ParseOptions {
        base_dir: base_dir.clone(),
        ..syntax::ParseOptions::default()
    };
    let Some(base_dir) = base_dir else {
        return options;
    };
    let Some(manifest_path) = package::find_manifest(&base_dir) else {
        return options;
    };
    let Ok(Some(graph)) = PackageGraph::discover(&base_dir) else {
        return options;
    };
    if let Ok(providers) = graph.proc_macro_providers_for_manifest(&manifest_path) {
        options.proc_macro_providers = providers;
    }
    options
}

fn definition_location_in_program(
    program: &stmt::Program,
    tokens: &[token::Token],
    spans: &[token::Span],
    symbol_name: &str,
    cursor_offset: usize,
    uri: &Url,
) -> Option<Location> {
    let mut resolver = resolve::slots::SlotResolver::new();
    let resolution = resolver.resolve_program_slots(program);
    let analyzer = LkAnalyzer::default();
    let enriched = analyzer.enrich_layout_spans(&resolution.root, tokens, spans);
    let fblocks = LkAnalyzer::scan_function_blocks(tokens, spans);
    let mut candidate_spans: Vec<token::Span> = Vec::new();
    let mut pick_child: Option<usize> = None;
    for (i, fb) in fblocks.iter().enumerate() {
        let s = spans.get(fb.body_start_idx)?.start.offset;
        let e = spans.get(fb.body_end_idx)?.end.offset;
        if cursor_offset >= s && cursor_offset <= e {
            pick_child = Some(i);
            break;
        }
    }
    if let Some(ci) = pick_child {
        if let Some(child) = enriched.children.get(ci) {
            for d in &child.decls {
                if d.name == symbol_name {
                    if let Some(sp) = &d.span {
                        candidate_spans.push(sp.clone());
                    }
                }
            }
        }
        if candidate_spans.is_empty() {
            for d in &enriched.decls {
                if d.name == symbol_name {
                    if let Some(sp) = &d.span {
                        candidate_spans.push(sp.clone());
                    }
                }
            }
        }
    } else {
        for d in &enriched.decls {
            if d.name == symbol_name {
                if let Some(sp) = &d.span {
                    candidate_spans.push(sp.clone());
                }
            }
        }
    }
    if let Some(sp) = candidate_spans.first() {
        let range = Range::new(
            Position::new(sp.start.line - 1, sp.start.column - 1),
            Position::new(sp.end.line - 1, sp.end.column - 1),
        );
        return Some(Location::new(uri.clone(), range));
    }
    find_definition_in_tokens(tokens, spans, symbol_name, uri)
}

fn find_definition_in_tokens(
    tokens: &[token::Token],
    spans: &[token::Span],
    symbol_name: &str,
    uri: &Url,
) -> Option<Location> {
    for (idx, token) in tokens.iter().enumerate() {
        match token {
            token::Token::Fn | token::Token::Struct | token::Token::Trait | token::Token::Type => {
                let Some(token::Token::Id(name)) = tokens.get(idx + 1) else {
                    continue;
                };
                if name != symbol_name {
                    continue;
                }
                let sp = spans.get(idx + 1)?;
                return Some(Location::new(
                    uri.clone(),
                    Range::new(
                        Position::new(sp.start.line - 1, sp.start.column - 1),
                        Position::new(sp.end.line - 1, sp.end.column - 1),
                    ),
                ));
            }
            _ => {}
        }
    }
    None
}

fn qualified_symbol_context_at_offset(
    tokens: &[token::Token],
    spans: &[token::Span],
    offset: usize,
) -> Option<SymbolContext> {
    for (dot_idx, dot_span) in spans.iter().enumerate() {
        if !matches!(tokens.get(dot_idx), Some(token::Token::Dot | token::Token::OptionalDot)) {
            continue;
        }
        let qualifier_idx = dot_idx.checked_sub(1)?;
        let member_idx = dot_idx + 1;
        let (Some(token::Token::Id(qualifier)), Some(token::Token::Id(member))) =
            (tokens.get(qualifier_idx), tokens.get(member_idx))
        else {
            continue;
        };
        let qualifier_span = spans.get(qualifier_idx)?;
        let member_span = spans.get(member_idx)?;

        // VS Code can send definition positions on the dot or token boundary for
        // qualified names. Treat the dot/member side as a member lookup, while a
        // clear click inside the qualifier still resolves the module itself.
        if offset >= qualifier_span.start.offset && offset.saturating_add(1) < qualifier_span.end.offset {
            continue;
        }
        if offset.saturating_add(1) >= dot_span.start.offset && offset <= member_span.end.offset {
            return Some(SymbolContext {
                name: member.clone(),
                qualifier: Some(qualifier.clone()),
            });
        }
    }
    None
}

fn import_path_candidates(base: &Path) -> Vec<PathBuf> {
    if base.extension().is_some() {
        vec![base.to_path_buf()]
    } else {
        vec![base.with_extension("lk"), base.join("mod.lk")]
    }
}

fn repo_root_from_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn stdlib_source_path(module_name: &str) -> Option<PathBuf> {
    if !module_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let root = repo_root_from_manifest();
    [
        root.join("stdlib")
            .join("crates")
            .join(module_name)
            .join("src")
            .join("lib.rs"),
        root.join("stdlib").join("src").join(format!("{module_name}.rs")),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

pub(crate) fn find_stdlib_module_location(module_name: &str) -> Option<Location> {
    let path = stdlib_source_path(module_name)?;
    let uri = Url::from_file_path(path).ok()?;
    Some(Location::new(uri, Range::new(Position::new(0, 0), Position::new(0, 0))))
}

pub(crate) fn find_stdlib_export_location(module_name: &str, export_name: &str) -> Option<Location> {
    let path = stdlib_source_path(module_name)?;
    let content = fs::read_to_string(&path).ok()?;
    let uri = Url::from_file_path(path).ok()?;

    if let Some(native_fn) = stdlib_native_export_impl_name(&content, export_name) {
        if let Some(location) = find_rust_function_location(&content, &native_fn, &uri) {
            return Some(location);
        }
    }

    if let Some(location) = find_rust_function_location(&content, export_name, &uri) {
        return Some(location);
    }

    find_stdlib_module_location(module_name)
}

fn stdlib_native_export_impl_name(content: &str, export_name: &str) -> Option<String> {
    let export_literal = format!("\"{export_name}\"");
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("RuntimeNativeExport::") || !trimmed.contains(&export_literal) {
            continue;
        }
        let after_self = trimmed.split("Self::").nth(1)?;
        let name: String = after_self
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

fn find_rust_function_location(content: &str, function_name: &str, uri: &Url) -> Option<Location> {
    for (line_idx, line) in content.lines().enumerate() {
        let fn_name_start = line
            .find(&format!("fn {function_name}("))
            .map(|pos| pos + 3)
            .or_else(|| line.find(&format!("pub fn {function_name}(")).map(|pos| pos + 7));
        let Some(fn_name_start) = fn_name_start else { continue };
        let fn_name_end = fn_name_start + function_name.len();
        return Some(Location::new(
            uri.clone(),
            Range::new(
                Position::new(line_idx as u32, fn_name_start as u32),
                Position::new(line_idx as u32, fn_name_end as u32),
            ),
        ));
    }
    None
}

fn find_definition_in_content(content: &str, symbol_name: &str, uri: &Url) -> Option<Location> {
    let rope = Rope::from_str(content);
    let total_lines = rope.len_lines();

    for line_idx in 0..total_lines {
        let line = rope.line(line_idx).to_string();
        let trimmed = line.trim();
        if trimmed.starts_with("let ") && trimmed.contains(&format!("{} ", symbol_name)) {
            if let Some(pos) = line.find(symbol_name) {
                let range = Range::new(
                    Position::new(line_idx as u32, pos as u32),
                    Position::new(line_idx as u32, (pos + symbol_name.len()) as u32),
                );
                return Some(Location::new(uri.clone(), range));
            }
        }

        if trimmed.starts_with("fn ") && trimmed.contains(&format!("fn {}", symbol_name)) {
            if let Some(pos) = line.find(&format!("fn {}", symbol_name)) {
                let range = Range::new(
                    Position::new(line_idx as u32, (pos + 3) as u32),
                    Position::new(line_idx as u32, (pos + 3 + symbol_name.len()) as u32),
                );
                return Some(Location::new(uri.clone(), range));
            }
        }

        if trimmed.starts_with(&format!("{}:", symbol_name)) {
            if let Some(pos) = line.find(&format!("{}:", symbol_name)) {
                let range = Range::new(
                    Position::new(line_idx as u32, pos as u32),
                    Position::new(line_idx as u32, (pos + symbol_name.len()) as u32),
                );
                return Some(Location::new(uri.clone(), range));
            }
        }

        if (trimmed.starts_with("use ") && trimmed.contains(&format!("use {}", symbol_name)))
            || (trimmed.starts_with("from ") && trimmed.contains(&format!("from {}", symbol_name)))
        {
            if let Some(pos) = line.find(symbol_name) {
                let range = Range::new(
                    Position::new(line_idx as u32, pos as u32),
                    Position::new(line_idx as u32, (pos + symbol_name.len()) as u32),
                );
                return Some(Location::new(uri.clone(), range));
            }
        }
    }

    None
}

fn find_macro_definition_at_offset(
    tokens: &[token::Token],
    spans: &[token::Span],
    offset: usize,
    uri: &Url,
) -> Option<Location> {
    find_local_macro_definition(tokens, spans, offset, uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_path_candidates_adds_lk_and_mod_file() {
        let candidates = import_path_candidates(Path::new("examples/fib"));
        assert_eq!(
            candidates,
            vec![PathBuf::from("examples/fib.lk"), PathBuf::from("examples/fib/mod.lk")]
        );
    }

    #[test]
    fn stdlib_export_location_points_to_export_function() {
        let location = find_stdlib_export_location("math", "sqrt").expect("math.sqrt location");
        assert!(location.uri.as_str().ends_with("/stdlib/crates/math/src/lib.rs"));
        assert!(location.range.start.line > 0);
    }

    #[test]
    fn import_module_name_at_position_detects_module_import_target() {
        let content = "use mathlib;\nuse mathlib as ml;\nuse { add } from mathlib;\n";

        assert_eq!(
            import_module_name_at_position(content, Position::new(0, 8)).as_deref(),
            Some("mathlib")
        );
        assert_eq!(
            import_module_name_at_position(content, Position::new(1, 8)).as_deref(),
            Some("mathlib")
        );
        assert_eq!(
            import_module_name_at_position(content, Position::new(2, 22)).as_deref(),
            Some("mathlib")
        );
    }

    #[test]
    fn import_module_name_at_position_ignores_alias_and_imported_items() {
        let content = "use mathlib as ml;\nuse { add } from mathlib;\n";

        assert_eq!(import_module_name_at_position(content, Position::new(0, 18)), None);
        assert_eq!(import_module_name_at_position(content, Position::new(1, 9)), None);
    }

    #[test]
    fn qualified_symbol_context_prefers_member_on_dot_and_member_side() {
        let content = "let doubled = mathlib.double(n);\n";
        let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");

        let on_dot = qualified_symbol_context_at_offset(&tokens, &spans, content.find('.').expect("dot"));
        assert_eq!(
            on_dot,
            Some(SymbolContext {
                name: "double".to_string(),
                qualifier: Some("mathlib".to_string()),
            })
        );

        let on_member =
            qualified_symbol_context_at_offset(&tokens, &spans, content.rfind("double").expect("member") + 2);
        assert_eq!(
            on_member,
            Some(SymbolContext {
                name: "double".to_string(),
                qualifier: Some("mathlib".to_string()),
            })
        );
    }

    #[test]
    fn qualified_symbol_context_keeps_clear_qualifier_click_as_module() {
        let content = "println(greetings.message(\"workspace\"));\n";
        let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");

        let on_qualifier =
            qualified_symbol_context_at_offset(&tokens, &spans, content.find("greetings").expect("qualifier") + 2);

        assert_eq!(on_qualifier, None);
    }

    #[test]
    fn plain_symbol_at_position_uses_only_the_current_token() {
        let content = "let doubled = mathlib.double(n);\n";
        let mathlib_pos = Position::new(0, content.find("mathlib").expect("mathlib") as u32 + 2);
        let double_pos = Position::new(0, content.rfind("double").expect("double") as u32 + 2);

        assert_eq!(
            plain_symbol_name_at_position(content, mathlib_pos).as_deref(),
            Some("mathlib")
        );
        assert_eq!(
            plain_symbol_name_at_position(content, double_pos).as_deref(),
            Some("double")
        );
    }

    #[test]
    fn macro_definition_lookup_resolves_same_file_macro_call_name() {
        let content = "macro_rules! answer { () => { 42 }; }\nlet x = answer!();\n";
        let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
        let uri = Url::parse("file:///tmp/macros.lk").expect("uri");
        let call_offset = content.rfind("answer").expect("call answer") + 2;

        let location =
            find_macro_definition_at_offset(&tokens, &spans, call_offset, &uri).expect("macro definition location");

        assert_eq!(location.uri, uri);
        assert_eq!(location.range.start, Position::new(0, 13));
        assert_eq!(location.range.end, Position::new(0, 19));
    }

    #[test]
    fn macro_definition_lookup_resolves_same_file_macro_call_bang() {
        let content = "macro_rules! answer { () => { 42 }; }\nlet x = answer!();\n";
        let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
        let uri = Url::parse("file:///tmp/macros.lk").expect("uri");
        let bang_offset = content.rfind('!').expect("call bang");

        let location =
            find_macro_definition_at_offset(&tokens, &spans, bang_offset, &uri).expect("macro definition location");

        assert_eq!(location.range.start, Position::new(0, 13));
        assert_eq!(location.range.end, Position::new(0, 19));
    }

    #[test]
    fn generated_macro_definition_lookup_uses_expanded_token_spans() {
        let content = r#"macro_rules! make_answer {
    () => { fn answer() { return 42; } };
}
make_answer!();
return answer();
"#;
        let expansion =
            syntax::expand_program_source(content, syntax::ParseOptions::default()).expect("macro-expanded program");
        let uri = Url::parse("file:///tmp/generated-symbol.lk").expect("uri");
        let usage_offset = content.rfind("answer").expect("answer call") + 2;

        let location = definition_location_in_program(
            &expansion.program,
            &expansion.source.tokens,
            &expansion.source.spans,
            "answer",
            usage_offset,
            &uri,
        )
        .expect("generated function definition location");

        let generated_name_offset = content.find("fn answer").expect("generated template function") + 3;
        let expected_start = position_for_ascii_offset(content, generated_name_offset);
        let expected_end = Position::new(expected_start.line, expected_start.character + "answer".len() as u32);
        assert_eq!(location.uri, uri);
        assert_eq!(location.range.start, expected_start);
        assert_eq!(location.range.end, expected_end);
    }

    #[test]
    fn package_proc_macro_provider_generated_definition_uses_manifest_options() {
        let dir = unique_test_dir("lsp_proc_macro_definition");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp package");
        let provider_path = dir.join("provider.sh");
        fs::write(
            dir.join("Lk.toml"),
            format!(
                r#"
[package]
name = "demo"
version = "0.1.0"

[macros.derive.MakeAnswer]
command = "/bin/sh"
args = [{}]
timeout_ms = 1000
max_output_bytes = 4096
"#,
                toml_string(provider_path.to_string_lossy().as_ref())
            ),
        )
        .expect("write manifest");
        fs::write(
            &provider_path,
            r#"cat >/dev/null
printf '%s' '{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"77","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}'
"#,
        )
        .expect("write provider");
        let source = r#"
#[derive(MakeAnswer)]
struct User { id: Int }
return generated();
"#;
        let source_path = dir.join("main.lk");
        fs::write(&source_path, source).expect("write source");
        let uri = Url::from_file_path(&source_path).expect("file uri");
        let options = parse_options_for_uri(&uri);
        let expansion = syntax::expand_program_source(source, options).expect("manifest provider should expand");
        let usage_offset = source.rfind("generated").expect("generated call") + 2;

        let location = definition_location_in_program(
            &expansion.program,
            &expansion.source.tokens,
            &expansion.source.spans,
            "generated",
            usage_offset,
            &uri,
        )
        .or_else(|| generated_ast_item_definition_location(&expansion.ast_macro_origins, "generated", &uri))
        .expect("generated provider item definition");

        assert_eq!(location.uri, uri);
        assert!(
            location.range.start.line <= 1,
            "null provider spans should fall back to the derive input span: {:?}",
            location.range
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ast_generated_member_definition_lookup_uses_member_origins() {
        let source = r#"
#[derive(Debug)]
struct User { id: Int }
let value = User { id: 1 };
return value.show();
"#;
        let uri = Url::parse("file:///tmp/derive-member-origin.lk").expect("uri");
        let expansion =
            syntax::expand_program_source(source, syntax::ParseOptions::default()).expect("derive should expand");

        let location = generated_ast_item_definition_location(&expansion.ast_macro_origins, "show", &uri)
            .expect("generated show member definition");

        assert_eq!(location.uri, uri);
        assert_eq!(
            location.range.start.line, 1,
            "generated member origin should fall back to derive input span: {:?}",
            location.range
        );
        assert!(
            location.range.end.character > location.range.start.character,
            "generated member definition should expose a non-empty range"
        );
    }

    #[test]
    fn ast_generated_field_expression_definition_lookup_uses_member_origins() {
        let source = r#"
#[derive(Debug)]
struct User { id: Int }
let value = User { id: 1 };
return "${value}";
"#;
        let uri = Url::parse("file:///tmp/derive-field-origin.lk").expect("uri");
        let expansion =
            syntax::expand_program_source(source, syntax::ParseOptions::default()).expect("derive should expand");

        let location = generated_ast_item_definition_location(&expansion.ast_macro_origins, "id", &uri)
            .expect("generated field expression definition");

        assert_eq!(location.uri, uri);
        assert_eq!(
            location.range.start.line, 1,
            "generated field expression origin should fall back to derive input span: {:?}",
            location.range
        );
        assert!(
            location.range.end.character > location.range.start.character,
            "generated field expression definition should expose a non-empty range"
        );
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("{name}_{}", std::process::id()));
        dir
    }

    fn toml_string(value: &str) -> String {
        serde_json::to_string(value).expect("TOML basic string compatible JSON string")
    }

    fn position_for_ascii_offset(content: &str, offset: usize) -> Position {
        let mut line = 0u32;
        let mut line_start = 0usize;
        for (idx, byte) in content.bytes().enumerate() {
            if idx == offset {
                break;
            }
            if byte == b'\n' {
                line += 1;
                line_start = idx + 1;
            }
        }
        Position::new(line, (offset - line_start) as u32)
    }

    #[test]
    fn interpolation_symbol_context_extracts_identifiers_inside_strings() {
        let content = "println(\"double(${n}) = ${doubled}\");\n";
        let n_offset = content.find("${n}").expect("n interpolation") + 2;
        let doubled_offset = content.find("${doubled}").expect("doubled interpolation") + 4;

        assert_eq!(
            interpolation_symbol_context_at_offset(content, n_offset),
            Some(SymbolContext {
                name: "n".to_string(),
                qualifier: None,
            })
        );
        assert_eq!(
            interpolation_symbol_context_at_offset(content, doubled_offset),
            Some(SymbolContext {
                name: "doubled".to_string(),
                qualifier: None,
            })
        );
    }
}
