use lk_core::{
    ast,
    ast::Parser as ExprParser,
    expr::Expr,
    macro_system,
    package::PackageGraph,
    resolve,
    resolve::slots::{FunctionLayout, SlotResolver},
    stmt,
    stmt::{stmt_parser::StmtParser, ImportStmt, Program, Stmt},
    syntax::{
        expand_program_source, macro_origin_note_for_span, parse_expr_source, parse_program_source, ParseOptions,
        ProgramExpansion,
    },
    token,
    token::{Span, Tokenizer},
    typ,
    typ::TypeChecker,
    val,
};
use lk_core::{stmt::NamedParamDecl, util::fast_map::FastHashMap};
use once_cell::sync::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower_lsp::lsp_types::*;

mod analysis_impl;
mod completions;
mod core_impl;
mod generated_symbols;
mod semantic_tokens;
#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use semantic_tokens::SemanticTokenValidationSummary;

// Soft limits to keep LSP responsive on large/broken files
const MAX_SCAN_LINES: usize = 400; // max lines to line-scan
const MAX_SCAN_CHUNKS: usize = 300; // max logical chunks to scan
const MAX_DIAGNOSTICS: usize = 200; // cap diagnostics volume
                                    // Caps to avoid overwhelming the editor with semantic tokens
pub(super) const MAX_TOKENS_PER_DOC: usize = 20_000; // hard ceiling for full-document tokens
pub(super) const MAX_TOKENS_PER_RANGE: usize = 8_000; // hard ceiling for range tokens

/// Result of analyzing LK code, containing diagnostics, symbols, and identifier roots
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<DocumentSymbol>,
    pub identifier_roots: HashSet<String>,
}

/// LK Language analyzer for providing LSP functionality
pub(crate) struct TokenCacheEntry {
    pub(crate) tokens: Arc<[token::Token]>,
    pub(crate) spans: Arc<[Span]>,
    parse_options: ParseOptions,
    project_dependencies: Arc<Vec<PathBuf>>,
    project_dependency_fingerprint: macro_system::ProcMacroDependencyFingerprint,
    named_param_decls: OnceCell<Arc<HashMap<String, Vec<NamedParamDecl>>>>,
    program_expansion: OnceCell<CachedProgramExpansion>,
    program_ast: OnceCell<Arc<Program>>,
    expr_ast: OnceCell<Arc<Expr>>,
}

#[derive(Debug, Clone)]
struct CachedProgramExpansion {
    expansion: Arc<ProgramExpansion>,
    proc_macro_dependency_fingerprint: macro_system::ProcMacroDependencyFingerprint,
}

impl TokenCacheEntry {
    fn new(
        tokens: Vec<token::Token>,
        spans: Vec<Span>,
        parse_options: ParseOptions,
        project_dependencies: Vec<PathBuf>,
    ) -> Self {
        let project_dependency_fingerprint =
            macro_system::fingerprint_dependency_paths(&project_dependencies, parse_options.base_dir.as_deref());
        Self {
            tokens: tokens.into(),
            spans: spans.into(),
            parse_options,
            project_dependencies: Arc::new(project_dependencies),
            project_dependency_fingerprint,
            named_param_decls: OnceCell::new(),
            program_expansion: OnceCell::new(),
            program_ast: OnceCell::new(),
            expr_ast: OnceCell::new(),
        }
    }

    fn parse_program_arc(&self, content: &str) -> std::result::Result<Arc<Program>, lk_core::token::ParseError> {
        if self.parse_options.expand_macros {
            return self
                .parse_program_expansion_arc(content)
                .map(|expansion| Arc::new(expansion.program.clone()));
        }
        self.program_ast
            .get_or_try_init(|| parse_program_source(content, self.parse_options.clone()).map(Arc::new))
            .cloned()
    }

    fn parse_program_expansion_arc(
        &self,
        content: &str,
    ) -> std::result::Result<Arc<ProgramExpansion>, lk_core::token::ParseError> {
        self.program_expansion
            .get_or_try_init(|| {
                let expansion = expand_program_source(content, self.parse_options.clone())?;
                let proc_macro_dependency_fingerprint = macro_system::fingerprint_proc_macro_dependencies(
                    &expansion.proc_macro_dependencies,
                    self.parse_options.base_dir.as_deref(),
                );
                Ok(CachedProgramExpansion {
                    expansion: Arc::new(expansion),
                    proc_macro_dependency_fingerprint,
                })
            })
            .map(|cached| cached.expansion.clone())
    }

    fn parse_expression_arc(&self, content: &str) -> std::result::Result<Arc<Expr>, token::ParseError> {
        self.expr_ast
            .get_or_try_init(|| parse_expr_source(content, self.parse_options.clone()).map(Arc::new))
            .cloned()
    }

    fn dependencies_current(&self) -> bool {
        if self.project_dependency_fingerprint
            != macro_system::fingerprint_dependency_paths(
                self.project_dependencies.as_ref(),
                self.parse_options.base_dir.as_deref(),
            )
        {
            return false;
        }
        let Some(cached) = self.program_expansion.get() else {
            return true;
        };
        cached.proc_macro_dependency_fingerprint.is_current(
            &cached.expansion.proc_macro_dependencies,
            self.parse_options.base_dir.as_deref(),
        )
    }
}

#[allow(dead_code)]
pub(crate) fn collect_project_file_dependencies(content: &str) -> Vec<PathBuf> {
    let Ok((tokens, _)) = Tokenizer::tokenize_enhanced_with_spans(content) else {
        return Vec::new();
    };
    collect_project_file_dependencies_from_tokens(&tokens)
}

fn collect_project_file_dependencies_from_tokens(tokens: &[token::Token]) -> Vec<PathBuf> {
    let mut dependencies = Vec::new();
    let mut seen = HashSet::new();
    for (idx, token) in tokens.iter().enumerate() {
        if !matches!(token, token::Token::Use) {
            continue;
        }
        if let Some(path) = file_import_dependency_at(tokens, idx) {
            if seen.insert(path.clone()) {
                dependencies.push(PathBuf::from(path));
            }
        }
    }
    dependencies
}

fn file_import_dependency_at(tokens: &[token::Token], use_idx: usize) -> Option<String> {
    match tokens.get(use_idx + 1)? {
        token::Token::Str(path) => Some(path.clone()),
        token::Token::LBrace => file_import_source_after_group(tokens, use_idx + 1),
        token::Token::Mul => {
            if matches!(tokens.get(use_idx + 2), Some(token::Token::As)) {
                file_import_source_after_from(tokens, use_idx + 4)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn file_import_source_after_group(tokens: &[token::Token], group_start: usize) -> Option<String> {
    let group_end = matching_group_end(tokens, group_start)?;
    file_import_source_after_from(tokens, group_end + 1)
}

fn file_import_source_after_from(tokens: &[token::Token], from_idx: usize) -> Option<String> {
    if !matches!(tokens.get(from_idx), Some(token::Token::From)) {
        return None;
    }
    match tokens.get(from_idx + 1)? {
        token::Token::Str(path) => Some(path.clone()),
        _ => None,
    }
}

fn matching_group_end(tokens: &[token::Token], start: usize) -> Option<usize> {
    let (open, close) = match tokens.get(start)? {
        token::Token::LBrace => (token::Token::LBrace, token::Token::RBrace),
        token::Token::LParen => (token::Token::LParen, token::Token::RParen),
        token::Token::LBracket => (token::Token::LBracket, token::Token::RBracket),
        _ => return None,
    };
    let mut depth = 0usize;
    for (idx, token) in tokens.iter().enumerate().skip(start) {
        if *token == open {
            depth += 1;
        } else if *token == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
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
pub struct LkAnalyzer {
    // Cache for tokenization results to avoid re-tokenizing same content
    token_cache: FastHashMap<String, Arc<TokenCacheEntry>>,
    // Cache for completion items that don't change
    completion_cache: Option<Vec<CompletionItem>>,
    // Base directory for resolving relative file imports
    base_dir: Option<PathBuf>,
    // Package modules available from Lk.toml workspace/dependencies
    package_modules: HashMap<String, PathBuf>,
    missing_packages: HashSet<String>,
    proc_macro_providers: macro_system::ProcMacroProviders,
}
