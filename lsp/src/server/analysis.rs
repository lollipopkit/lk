use std::sync::Arc;

use ropey::Rope;
use tokio::task;
use tokio::time::{sleep, Duration};
use tower_lsp::lsp_types::{request::WorkDoneProgressCreate, *};

use crate::analyzer::{AnalysisResult, LkrAnalyzer};
use lkr_core::{resolve, stmt, token};

use super::state::LkrLanguageServer;
use super::text::{describe_token_hover, find_token_at_offset, position_to_char_idx};

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
        tokio::spawn(async move {
            sleep(Duration::from_millis(delay_ms)).await;

            let (content_snapshot, seq_snapshot, version_snapshot) = if let Some(doc) = documents.get(&uri) {
                (doc.content.to_string(), doc.debounce_seq, doc.version)
            } else {
                return;
            };

            if version_snapshot != scheduled_version {
                return;
            }

            let token = NumberOrString::String(format!("lkr:diag:{}", uri));
            let _ = client
                .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams { token: token.clone() })
                .await;
            let _ = client
                .send_notification::<notification::Progress>(ProgressParams {
                    token: token.clone(),
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(WorkDoneProgressBegin {
                        title: "LKR: Checking".to_string(),
                        cancellable: Some(false),
                        message: Some(uri.to_string()),
                        percentage: None,
                    })),
                })
                .await;

            let content_for_compute = content_snapshot.clone();
            let computed_result = task::spawn_blocking(move || {
                let mut analyzer = LkrAnalyzer::new();
                analyzer.analyze(&content_for_compute)
            })
            .await
            .ok();

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

            let _ = client
                .send_notification::<notification::Progress>(ProgressParams {
                    token,
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                        message: Some("Checking complete".to_string()),
                    })),
                })
                .await;
        });
    }

    pub(crate) async fn find_symbol_at_position(&self, content: &str, position: Position) -> Option<String> {
        let (tokens, spans) = match token::Tokenizer::tokenize_enhanced_with_spans(content) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let offset = position_to_char_idx(&Rope::from_str(content), position);
        for (i, span) in spans.iter().enumerate() {
            if offset >= span.start.offset && offset <= span.end.offset {
                if let token::Token::Id(name) = &tokens[i] {
                    return Some(name.clone());
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
