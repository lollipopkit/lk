use super::{SourceToken, procedural::ProcMacroToken, token_lexeme};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
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
        let parsed = if let Some(token) = proc_token_kind_override(proc_token) {
            vec![token]
        } else {
            Tokenizer::tokenize(&proc_token.lexeme).map_err(|err| {
                ParseError::with_span(
                    format!(
                        "Procedural macro `{macro_name}` produced invalid token `{}`: {err}",
                        proc_token.lexeme
                    ),
                    span.clone(),
                )
            })?
        };
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

fn proc_token_kind_override(token: &ProcMacroToken) -> Option<Token> {
    match (token.kind.as_str(), token.lexeme.as_str()) {
        ("Or", "||") => Some(Token::Or),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_token(kind: &str, lexeme: &str) -> ProcMacroToken {
        ProcMacroToken {
            kind: kind.to_string(),
            lexeme: lexeme.to_string(),
            span: None,
        }
    }

    #[test]
    fn parse_proc_output_preserves_or_kind_for_context_sensitive_lexeme() {
        let (tokens, spans) =
            parse_proc_output_tokens("logic", &[proc_token("Or", "||")], &zero_span()).expect("parse proc output");

        assert_eq!(tokens, vec![Token::Or]);
        assert_eq!(spans, vec![zero_span()]);
    }
}
