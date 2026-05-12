use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ropey::Rope;
use tokio::task;
use tokio::time::{sleep, Duration};
use tower_lsp::lsp_types::*;

use crate::analyzer::{AnalysisResult, LkrAnalyzer};
use lkr_core::{resolve, stmt, token};

use super::state::LkrLanguageServer;
use super::text::{describe_token_hover, find_token_at_offset, position_to_char_idx};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolContext {
    pub(crate) name: String,
    pub(crate) qualifier: Option<String>,
}

impl LkrLanguageServer {
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

        let (tokens, spans) = {
            if let Ok(mut analyzer) = self.analyzer.lock() {
                match analyzer.tokenize_with_spans_cached(&content) {
                    Ok(entry) => {
                        let tokens = entry.tokens.clone();
                        let spans = entry.spans.clone();
                        (tokens, spans)
                    }
                    Err(_) => return None,
                }
            } else {
                return None;
            }
        };

        if let Some((idx, _token)) = find_token_at_offset(spans.as_ref(), tokens.as_ref(), offset) {
            let hover_text = describe_token_hover(tokens.as_ref(), spans.as_ref(), idx);
            return Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(hover_text)),
                range: None,
            });
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

        let content_for_compute = content_snapshot.clone();
        let base_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));

        let sem = self.compute_limiter.lock().unwrap().clone();
        let _permit = sem.acquire().await.ok()?;

        let start = Instant::now();
        let computed_result = task::spawn_blocking(move || {
            let mut analyzer = LkrAnalyzer::new();
            if let Some(b) = base_dir {
                analyzer.set_base_dir(b);
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
        let base_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let sem = self.compute_limiter.lock().unwrap().clone();
        let _permit = sem.acquire().await.ok();
        let start = Instant::now();
        let generated_result = task::spawn_blocking(move || {
            let mut analyzer = LkrAnalyzer::new_light();
            if let Some(b) = base_dir {
                analyzer.set_base_dir(b);
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
        tokio::spawn(async move {
            sleep(Duration::from_millis(delay_ms)).await;

            let (content_snapshot, seq_snapshot, version_snapshot) = if let Some(doc) = documents.get(&uri) {
                (doc.content.to_string(), doc.debounce_seq, doc.version)
            } else {
                return;
            };

            if version_snapshot != scheduled_version
                || documents.get(&uri).map_or(true, |doc| doc.debounce_seq != seq_snapshot)
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
            let computed_result = task::spawn_blocking(move || {
                let mut analyzer = LkrAnalyzer::new();
                if let Some(b) = base_dir {
                    analyzer.set_base_dir(b);
                }
                analyzer.analyze(&content_for_compute)
            })
            .await
            .ok();

            if let Some(diagnostics_len) = computed_result.as_ref().map(|c| c.diagnostics.len()) {
                if let Some(elapsed) = Instant::now().checked_duration_since(start).map(|d| d.as_millis()) {
                    log_timing(
                        "schedule_diagnostics_and_warmup",
                        &uri,
                        elapsed,
                        &format!(
                            "diag_count={diagnostics_len}, content_len={}",
                            content_for_compute.len()
                        ),
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
            if !matches!(tokens.get(i.checked_sub(1)?), Some(token::Token::Import)) {
                continue;
            }
            let path = self.resolve_lkr_import_path(import_path, current_uri)?;
            let uri = Url::from_file_path(path).ok()?;
            return Some(Location::new(uri, Range::new(Position::new(0, 0), Position::new(0, 0))));
        }
        None
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
        let import_path = imports.get(qualifier)?;
        let imported_uri = Url::from_file_path(import_path).ok()?;
        let imported_content = fs::read_to_string(import_path).ok()?;
        self.find_definition_precise(&imported_content, &symbol.name, Position::new(0, 0), &imported_uri)
            .await
            .or_else(|| find_definition_in_content(&imported_content, &symbol.name, &imported_uri))
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
                Some(token::Token::Import)
            ) {
                continue;
            }
            let Some(path) = self.resolve_lkr_import_path(import_path, current_uri) else {
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

    fn resolve_lkr_import_path(&self, import_path: &str, current_uri: &Url) -> Option<PathBuf> {
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
        let (tokens, spans) = match token::Tokenizer::tokenize_enhanced_with_spans(content) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let mut parser = stmt::stmt_parser::StmtParser::new_with_spans(&tokens, &spans);
        let program = parser.parse_program_with_enhanced_errors(content).ok()?;
        let mut resolver = resolve::slots::SlotResolver::new();
        let resolution = resolver.resolve_program_slots(&program);
        let analyzer = LkrAnalyzer::default();
        let enriched = analyzer.enrich_layout_spans(&resolution.root, &tokens, &spans);
        let fblocks = LkrAnalyzer::scan_function_blocks(&tokens, &spans);
        let cursor_line = pos.line + 1;
        let cursor_col = pos.character + 1;
        let mut cursor_offset = 0usize;
        for sp in &spans {
            if sp.start.line == cursor_line {
                cursor_offset = sp.start.offset + (cursor_col.saturating_sub(sp.start.column)) as usize;
                break;
            }
        }
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
        None
    }
}

fn import_path_candidates(base: &Path) -> Vec<PathBuf> {
    if base.extension().is_some() {
        vec![base.to_path_buf()]
    } else {
        vec![base.with_extension("lkr"), base.join("mod.lkr")]
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
    let path = repo_root_from_manifest()
        .join("stdlib")
        .join("src")
        .join(format!("{module_name}.rs"));
    path.is_file().then_some(path)
}

fn find_stdlib_module_location(module_name: &str) -> Option<Location> {
    let path = stdlib_source_path(module_name)?;
    let uri = Url::from_file_path(path).ok()?;
    Some(Location::new(uri, Range::new(Position::new(0, 0), Position::new(0, 0))))
}

fn find_stdlib_export_location(module_name: &str, export_name: &str) -> Option<Location> {
    let path = stdlib_source_path(module_name)?;
    let content = fs::read_to_string(&path).ok()?;
    let uri = Url::from_file_path(path).ok()?;

    for (line_idx, line) in content.lines().enumerate() {
        let fn_name_start = line
            .find(&format!("fn {export_name}("))
            .map(|pos| pos + 3)
            .or_else(|| line.find(&format!("pub fn {export_name}(")).map(|pos| pos + 7));
        let Some(fn_name_start) = fn_name_start else { continue };
        let fn_name_end = fn_name_start + export_name.len();
        return Some(Location::new(
            uri,
            Range::new(
                Position::new(line_idx as u32, fn_name_start as u32),
                Position::new(line_idx as u32, fn_name_end as u32),
            ),
        ));
    }

    find_stdlib_module_location(module_name)
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

        if (trimmed.starts_with("import ") && trimmed.contains(&format!("import {}", symbol_name)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_path_candidates_adds_lkr_and_mod_file() {
        let candidates = import_path_candidates(Path::new("examples/fib"));
        assert_eq!(
            candidates,
            vec![PathBuf::from("examples/fib.lkr"), PathBuf::from("examples/fib/mod.lkr")]
        );
    }

    #[test]
    fn stdlib_export_location_points_to_export_function() {
        let location = find_stdlib_export_location("math", "sqrt").expect("math.sqrt location");
        assert!(location.uri.as_str().ends_with("/stdlib/src/math.rs"));
        assert!(location.range.start.line > 0);
    }
}
