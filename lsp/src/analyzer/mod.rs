use lkr_core::{
    ast,
    ast::Parser as ExprParser,
    expr::Expr,
    module::ModuleRegistry,
    resolve,
    resolve::slots::{FunctionLayout, SlotResolver},
    stmt,
    stmt::{stmt_parser::StmtParser, ImportStmt, Program, Stmt},
    token,
    token::{Span, Tokenizer},
    typ,
    typ::TypeChecker,
    val,
};
use lkr_core::{stmt::NamedParamDecl, util::fast_map::FastHashMap};
use once_cell::sync::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_lsp::lsp_types::*;

mod analysis_impl;
mod completions;
mod core_impl;
mod semantic_tokens;
#[cfg(test)]
mod tests;
mod utils;

pub use utils::extract_variables_from_pattern;

// Soft limits to keep LSP responsive on large/broken files
const MAX_SCAN_LINES: usize = 400; // max lines to line-scan
const MAX_SCAN_CHUNKS: usize = 300; // max logical chunks to scan
const MAX_DIAGNOSTICS: usize = 200; // cap diagnostics volume
                                    // Caps to avoid overwhelming the editor with semantic tokens
pub(super) const MAX_TOKENS_PER_DOC: usize = 20_000; // hard ceiling for full-document tokens
pub(super) const MAX_TOKENS_PER_RANGE: usize = 8_000; // hard ceiling for range tokens

/// Result of analyzing LKR code, containing diagnostics, symbols, and identifier roots
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<DocumentSymbol>,
    pub identifier_roots: HashSet<String>,
}

/// LKR Language analyzer for providing LSP functionality
pub(crate) struct TokenCacheEntry {
    pub(crate) tokens: Arc<[token::Token]>,
    pub(crate) spans: Arc<[Span]>,
    named_param_decls: OnceCell<Arc<HashMap<String, Vec<NamedParamDecl>>>>,
    program_ast: OnceCell<Arc<Program>>,
    expr_ast: OnceCell<Arc<Expr>>,
}

impl TokenCacheEntry {
    fn new(tokens: Vec<token::Token>, spans: Vec<Span>) -> Self {
        Self {
            tokens: tokens.into(),
            spans: spans.into(),
            named_param_decls: OnceCell::new(),
            program_ast: OnceCell::new(),
            expr_ast: OnceCell::new(),
        }
    }

    fn parse_program_arc(&self, content: &str) -> std::result::Result<Arc<Program>, lkr_core::token::ParseError> {
        self.program_ast
            .get_or_try_init(|| {
                let mut parser = StmtParser::new_with_spans(self.tokens.as_ref(), self.spans.as_ref());
                parser.parse_program_with_enhanced_errors(content).map(Arc::new)
            })
            .cloned()
    }

    fn parse_expression_arc(&self, content: &str) -> std::result::Result<Arc<Expr>, token::ParseError> {
        self.expr_ast
            .get_or_try_init(|| {
                let mut parser = ExprParser::new_with_spans(self.tokens.as_ref(), self.spans.as_ref());
                parser.parse_with_enhanced_errors(content).map(Arc::new)
            })
            .cloned()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FnBlockInfo {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) body_start_idx: usize,
    pub(crate) body_end_idx: usize,
    pub(crate) param_spans: Vec<(String, Span)>,
}

#[derive(Default)]
pub struct LkrAnalyzer {
    // Cache for tokenization results to avoid re-tokenizing same content
    token_cache: FastHashMap<String, Arc<TokenCacheEntry>>,
    // Cache for completion items that don't change
    completion_cache: Option<Vec<CompletionItem>>,
    // Registered stdlib modules for resolution/completions
    registry: ModuleRegistry,
    // Base directory for resolving relative file imports
    base_dir: Option<PathBuf>,
}
