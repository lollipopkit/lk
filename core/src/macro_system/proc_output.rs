use super::{SourceToken, procedural::ProcMacroToken, token_lexeme};
use crate::token::{ParseError, Span, Token, Tokenizer};

pub(in crate::macro_system) fn source_tokens_from_proc_output(
    macro_name: &str,
    tokens: &[ProcMacroToken],
    fallback_span: &Span,
) -> Result<Vec<SourceToken>, ParseError> {
    let (tokens, spans) = parse_proc_output_tokens(macro_name, tokens, fallback_span)?;
    Ok(tokens
        .into_iter()
        .zip(spans)
        .map(|(token, span)| {
            let lexeme = token_lexeme(&token);
            SourceToken {
                token,
                span,
                lexeme,
                origins: Vec::new(),
            }
        })
        .collect())
}

pub(in crate::macro_system) fn parse_tokens_from_proc_output(
    macro_name: &str,
    tokens: &[ProcMacroToken],
    fallback_span: Option<&Span>,
) -> Result<(String, Vec<Token>, Vec<Span>), ParseError> {
    let fallback = fallback_span.cloned().unwrap_or_else(zero_span);
    let source = render_proc_macro_tokens(tokens);
    let (tokens, spans) = parse_proc_output_tokens(macro_name, tokens, &fallback)?;
    Ok((source, tokens, spans))
}

pub(in crate::macro_system) fn render_proc_macro_tokens(tokens: &[ProcMacroToken]) -> String {
    let mut output = String::new();
    let mut prev: Option<&str> = None;
    for token in tokens {
        if should_space_proc_tokens(prev, &token.lexeme) {
            output.push(' ');
        }
        output.push_str(&token.lexeme);
        prev = Some(&token.lexeme);
    }
    output
}

fn parse_proc_output_tokens(
    macro_name: &str,
    tokens: &[ProcMacroToken],
    fallback_span: &Span,
) -> Result<(Vec<Token>, Vec<Span>), ParseError> {
    let mut out_tokens = Vec::new();
    let mut out_spans = Vec::new();
    for proc_token in tokens {
        let span = proc_token
            .span
            .as_ref()
            .map(|span| span.to_span())
            .unwrap_or_else(|| fallback_span.clone());
        let parsed = Tokenizer::tokenize(&proc_token.lexeme).map_err(|err| {
            ParseError::with_span(
                format!(
                    "Procedural macro `{macro_name}` produced invalid token `{}`: {err}",
                    proc_token.lexeme
                ),
                span.clone(),
            )
        })?;
        if parsed.is_empty() {
            return Err(ParseError::with_span(
                format!("Procedural macro `{macro_name}` produced an empty token lexeme"),
                span,
            ));
        }
        for token in parsed {
            out_tokens.push(token);
            out_spans.push(span.clone());
        }
    }
    Ok((out_tokens, out_spans))
}

fn should_space_proc_tokens(prev: Option<&str>, current: &str) -> bool {
    let Some(prev) = prev else {
        return false;
    };
    if matches!(current, ")" | "}" | "]" | "," | ";" | "." | "::" | ":") {
        return false;
    }
    if matches!(prev, "(" | "{" | "[" | "." | "::" | "#") {
        return false;
    }
    true
}

fn zero_span() -> Span {
    use crate::token::Position;
    Span::new(Position::start(), Position::start())
}
