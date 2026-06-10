use crate::{
    ast::Parser as ExprParser,
    expr::Expr,
    macro_system::{
        MacroExpandOptions, MacroTokenOrigin, MacroTrace, ProcMacroDependency, ProcMacroDependencyRecorder,
        ProcMacroOptions, ProcMacroProviders, expand_ast_macros, expand_macros, token_lexeme,
    },
    stmt::{Program, StmtParser},
    token::{ParseError, Token, Tokenizer},
};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ParseOptions {
    pub expand_macros: bool,
    pub macro_trace: bool,
    pub recursion_limit: usize,
    pub base_dir: Option<PathBuf>,
    pub macro_features: Vec<String>,
    pub proc_macro_providers: ProcMacroProviders,
}

#[derive(Debug, Clone)]
pub struct SourceExpansion {
    pub tokens: Vec<Token>,
    pub spans: Vec<crate::token::Span>,
    pub origins: Vec<MacroTokenOrigin>,
    pub trace: Vec<MacroTrace>,
    pub proc_macro_dependencies: Vec<ProcMacroDependency>,
}

#[derive(Debug, Clone)]
pub struct ProgramExpansion {
    pub source: SourceExpansion,
    pub program: Program,
    pub ast_expanded: bool,
    pub proc_macro_dependencies: Vec<ProcMacroDependency>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            expand_macros: true,
            macro_trace: false,
            recursion_limit: 128,
            base_dir: None,
            macro_features: Vec::new(),
            proc_macro_providers: ProcMacroProviders::default(),
        }
    }
}

pub fn parse_program_source(source: &str, options: ParseOptions) -> Result<Program, ParseError> {
    Ok(expand_program_source(source, options)?.program)
}

pub fn expand_program_source(source: &str, options: ParseOptions) -> Result<ProgramExpansion, ParseError> {
    let expand_ast = options.expand_macros;
    let dependency_recorder = ProcMacroDependencyRecorder::default();
    let proc_macro_options = ProcMacroOptions {
        features: options.macro_features.clone(),
        providers: options.proc_macro_providers.clone(),
        dependency_recorder: dependency_recorder.clone(),
        ..ProcMacroOptions::default()
    };
    let source_expansion = expand_source_with_recorder(source, options, dependency_recorder.clone())?;
    let mut parser = StmtParser::new_with_spans(&source_expansion.tokens, &source_expansion.spans);
    let parsed_program = parser
        .parse_program_with_enhanced_errors(source)
        .map_err(|error| enrich_parse_error_with_macro_origins(error, &source_expansion))?;
    let program = if expand_ast {
        expand_ast_macros(parsed_program.clone(), proc_macro_options)
    } else {
        Ok(parsed_program.clone())
    }?;
    Ok(ProgramExpansion {
        ast_expanded: program != parsed_program,
        source: source_expansion,
        program,
        proc_macro_dependencies: dependency_recorder.dependencies(),
    })
}

pub fn parse_expr_source(source: &str, options: ParseOptions) -> Result<Expr, ParseError> {
    let expanded = expand_source(source, options)?;
    let mut parser = ExprParser::new_with_spans(&expanded.tokens, &expanded.spans);
    parser
        .parse_with_enhanced_errors(source)
        .map_err(|error| enrich_parse_error_with_macro_origins(error, &expanded))
}

pub fn expand_source(source: &str, options: ParseOptions) -> Result<SourceExpansion, ParseError> {
    expand_source_with_recorder(source, options, ProcMacroDependencyRecorder::default())
}

fn expand_source_with_recorder(
    source: &str,
    options: ParseOptions,
    dependency_recorder: ProcMacroDependencyRecorder,
) -> Result<SourceExpansion, ParseError> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source)?;
    if !options.expand_macros {
        return Ok(SourceExpansion {
            tokens,
            spans,
            origins: Vec::new(),
            trace: Vec::new(),
            proc_macro_dependencies: Vec::new(),
        });
    }
    let expanded = expand_macros(
        tokens,
        spans,
        MacroExpandOptions {
            recursion_limit: options.recursion_limit,
            trace: options.macro_trace,
            base_dir: options.base_dir,
            proc_macro_providers: options.proc_macro_providers,
            proc_macro_features: options.macro_features,
            proc_macro_dependency_recorder: dependency_recorder.clone(),
        },
    )?;
    Ok(SourceExpansion {
        tokens: expanded.tokens,
        spans: expanded.spans,
        origins: expanded.origins,
        trace: expanded.trace,
        proc_macro_dependencies: expanded.proc_macro_dependencies,
    })
}

fn enrich_parse_error_with_macro_origins(error: ParseError, expansion: &SourceExpansion) -> ParseError {
    if error.message.contains("Macro origin stack:") {
        return error;
    }
    let Some(origin) = error
        .span
        .as_ref()
        .and_then(|span| origin_for_span(&expansion.origins, span))
    else {
        return error;
    };
    if origin.frames.is_empty() {
        return error;
    }

    let mut message = error.message;
    message.push_str("\nMacro origin stack:");
    for frame in origin.frames.iter().rev() {
        message.push_str(&format!(
            "\n  token `{}` from {} of `{}` at {}",
            origin.lexeme,
            frame.kind.as_str(),
            frame.macro_name,
            frame.call_span
        ));
    }
    match error.span {
        Some(span) => ParseError::with_span(message, span),
        None => ParseError::new(message),
    }
}

fn origin_for_span<'a>(origins: &'a [MacroTokenOrigin], span: &crate::token::Span) -> Option<&'a MacroTokenOrigin> {
    origins.iter().find(|origin| origin.span == *span).or_else(|| {
        origins
            .iter()
            .find(|origin| origin.span.start.offset == span.start.offset)
    })
}

pub fn render_tokens(tokens: &[Token]) -> String {
    let mut output = String::new();
    let mut prev: Option<&Token> = None;
    for token in tokens {
        let lexeme = token_lexeme(token);
        if should_insert_space(prev, token) {
            output.push(' ');
        }
        output.push_str(&lexeme);
        prev = Some(token);
    }
    output
}

pub fn render_program(program: &Program) -> String {
    let mut output = String::new();
    for stmt in &program.statements {
        output.push_str(&stmt.to_string());
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    output.trim_end().to_string()
}

pub fn tokenize_and_expand(
    source: &str,
    options: ParseOptions,
) -> Result<(Vec<crate::token::Token>, Vec<crate::token::Span>), ParseError> {
    let expanded = expand_source(source, options)?;
    Ok((expanded.tokens, expanded.spans))
}

fn should_insert_space(prev: Option<&Token>, current: &Token) -> bool {
    let Some(prev) = prev else {
        return false;
    };
    if matches!(
        current,
        Token::RParen
            | Token::RBrace
            | Token::RBracket
            | Token::Comma
            | Token::Semicolon
            | Token::Dot
            | Token::ColonColon
    ) {
        return false;
    }
    if matches!(
        prev,
        Token::LParen | Token::LBrace | Token::LBracket | Token::Dot | Token::ColonColon
    ) {
        return false;
    }
    true
}
