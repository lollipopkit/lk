use crate::{
    ast::Parser as ExprParser,
    expr::Expr,
    macro_system::{MacroExpandOptions, MacroTrace, ProcMacroOptions, expand_ast_macros, expand_macros, token_lexeme},
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
}

#[derive(Debug, Clone)]
pub struct SourceExpansion {
    pub tokens: Vec<Token>,
    pub spans: Vec<crate::token::Span>,
    pub trace: Vec<MacroTrace>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            expand_macros: true,
            macro_trace: false,
            recursion_limit: 128,
            base_dir: None,
        }
    }
}

pub fn parse_program_source(source: &str, options: ParseOptions) -> Result<Program, ParseError> {
    let expand_ast = options.expand_macros;
    let (tokens, spans) = tokenize_and_expand(source, options)?;
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser.parse_program_with_enhanced_errors(source)?;
    if expand_ast {
        expand_ast_macros(program, ProcMacroOptions::default())
    } else {
        Ok(program)
    }
}

pub fn parse_expr_source(source: &str, options: ParseOptions) -> Result<Expr, ParseError> {
    let (tokens, spans) = tokenize_and_expand(source, options)?;
    let mut parser = ExprParser::new_with_spans(&tokens, &spans);
    parser.parse_with_enhanced_errors(source)
}

pub fn expand_source(source: &str, options: ParseOptions) -> Result<SourceExpansion, ParseError> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source)?;
    if !options.expand_macros {
        return Ok(SourceExpansion {
            tokens,
            spans,
            trace: Vec::new(),
        });
    }
    let expanded = expand_macros(
        tokens,
        spans,
        MacroExpandOptions {
            recursion_limit: options.recursion_limit,
            trace: options.macro_trace,
            base_dir: options.base_dir,
        },
    )?;
    Ok(SourceExpansion {
        tokens: expanded.tokens,
        spans: expanded.spans,
        trace: expanded.trace,
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
