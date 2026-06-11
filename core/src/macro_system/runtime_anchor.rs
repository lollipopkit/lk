use std::collections::BTreeSet;

use crate::token::{Position, Span, Token};

use super::{
    MacroRegistry, SourceToken,
    imports::{self, MacroRuntimeAnchorSource},
    token_lexeme,
};

pub(super) fn rewrite_anchor_runtime_refs(tokens: Vec<SourceToken>, registry: &MacroRegistry) -> Vec<SourceToken> {
    let local_anchor = imports::local_macro_crate_anchor();
    let mut used_runtime_anchors = BTreeSet::new();
    let mut rewritten = Vec::with_capacity(tokens.len());
    let mut index = 0usize;
    while index < tokens.len() {
        if let Some(anchor) = crate_anchor_path_at(&tokens, index) {
            if anchor == local_anchor {
                index += 2;
                continue;
            }
            if registry.runtime_anchors.contains_key(anchor) {
                used_runtime_anchors.insert(anchor.to_string());
                rewritten.push(tokens[index].clone());
                rewritten.push(runtime_anchor_dot(&tokens[index + 1]));
                index += 2;
                continue;
            }
        }
        rewritten.push(tokens[index].clone());
        index += 1;
    }

    if used_runtime_anchors.is_empty() {
        return rewritten;
    }

    let mut output = Vec::new();
    let span = rewritten
        .first()
        .or_else(|| tokens.first())
        .map(|token| token.span.clone())
        .unwrap_or_else(|| Span::single(Position::start()));
    for anchor in used_runtime_anchors {
        let Some(source) = registry.runtime_anchors.get(&anchor) else {
            continue;
        };
        output.extend(runtime_import_tokens(&anchor, source, &span));
    }
    output.extend(rewritten);
    output
}

fn crate_anchor_path_at(tokens: &[SourceToken], index: usize) -> Option<&str> {
    let Token::Id(anchor) = &tokens.get(index)?.token else {
        return None;
    };
    if !anchor.starts_with("__lk_macro_crate_") {
        return None;
    }
    if matches!(tokens.get(index + 1).map(|token| &token.token), Some(Token::ColonColon)) {
        Some(anchor)
    } else {
        None
    }
}

fn runtime_import_tokens(anchor: &str, source: &MacroRuntimeAnchorSource, span: &Span) -> Vec<SourceToken> {
    let source_token = match source {
        MacroRuntimeAnchorSource::File(path) => Token::Str(path.clone()),
        MacroRuntimeAnchorSource::Module(name) => Token::Id(name.clone()),
    };
    [
        Token::Use,
        Token::Mul,
        Token::As,
        Token::Id(anchor.to_string()),
        Token::From,
        source_token,
        Token::Semicolon,
    ]
    .into_iter()
    .map(|token| source_token_with_span(token, span.clone()))
    .collect()
}

fn runtime_anchor_dot(colon_colon: &SourceToken) -> SourceToken {
    let mut dot = colon_colon.clone();
    dot.token = Token::Dot;
    dot.lexeme = token_lexeme(&dot.token);
    dot
}

fn source_token_with_span(token: Token, span: Span) -> SourceToken {
    let lexeme = token_lexeme(&token);
    SourceToken {
        token,
        span,
        lexeme,
        origins: Vec::new(),
    }
}
