use regex::Regex;
use ropey::Rope;
use tower_lsp::lsp_types::{Position, TextDocumentContentChangeEvent};

use lk_core::token::{Span as CoreSpan, Token as CoreToken};

// Convert LSP UTF-16 position to Rope char index (scalar values), clamped to the end of the line.
pub(crate) fn position_to_char_idx(text: &Rope, pos: Position) -> usize {
    let line_idx = pos.line as usize;
    if line_idx >= text.len_lines() {
        return text.len_chars();
    }
    let line_start_char = text.line_to_char(line_idx);
    let line_slice = text.line(line_idx);
    let target_utf16 = pos.character as usize;

    if let Some(s) = line_slice.as_str() {
        if s.is_ascii() {
            let len_chars = s.len();
            let clamped = target_utf16.min(len_chars);
            return line_start_char + clamped;
        }
    }

    let mut seen_utf16 = 0usize;
    let mut chars_in_line = 0usize;
    for ch in line_slice.chars() {
        let u16_len = ch.len_utf16();
        if seen_utf16 + u16_len > target_utf16 {
            break;
        }
        seen_utf16 += u16_len;
        chars_in_line += 1;
        if seen_utf16 == target_utf16 {
            break;
        }
    }
    line_start_char + chars_in_line
}

// Apply incremental LSP changes to a rope buffer.
pub(crate) fn apply_incremental_change_rope(text: &mut Rope, change: &TextDocumentContentChangeEvent) {
    if let Some(range) = &change.range {
        let start_char = position_to_char_idx(text, range.start);
        let end_char = position_to_char_idx(text, range.end);
        let (s, e) = if start_char <= end_char {
            (start_char, end_char)
        } else {
            (end_char, start_char)
        };
        if s != e {
            text.remove(s..e);
        }
        if !change.text.is_empty() {
            text.insert(s, &change.text);
        }
    } else {
        *text = Rope::from_str(&change.text);
    }
}

// Find token covering an absolute character offset using half-open [start,end) spans.
pub(crate) fn find_token_at_offset(
    spans: &[CoreSpan],
    tokens: &[CoreToken],
    offset: usize,
) -> Option<(usize, CoreToken)> {
    for (i, span) in spans.iter().enumerate() {
        if offset >= span.start.offset && offset < span.end.offset {
            return Some((i, tokens[i].clone()));
        }
    }
    None
}

// Build a concise hover string for a token.
pub(crate) fn describe_token_hover(tokens: &[CoreToken], _spans: &[CoreSpan], idx: usize) -> String {
    use CoreToken as T;
    let tok = &tokens[idx];

    match tok {
        T::Id(name) => {
            let is_call = tokens.get(idx + 1).map(|t| matches!(t, T::LParen)).unwrap_or(false);
            if is_call {
                if let Some((sig, doc)) = stdlib_func_hover(tokens, idx) {
                    format!("{}\n{}", sig, doc)
                } else {
                    format!("Function call: {}(…)", name)
                }
            } else {
                format!("Identifier: {}", name)
            }
        }
        T::Str(s) => format!("String literal: \"{}\"", s),
        T::Int(n) => format!("Integer literal: {}", n),
        T::Float(n) => format!("Float literal: {}", n),
        T::Bool(true) => "Boolean literal: true".to_string(),
        T::Bool(false) => "Boolean literal: false".to_string(),
        T::Nil => "Nil literal".to_string(),
        T::LBrace => "Block start `{`".to_string(),
        T::RBrace => "Block end `}`".to_string(),
        T::LParen => "Group start `(`".to_string(),
        T::RParen => "Group end `)`".to_string(),
        T::LBracket => "List literal start `[`".to_string(),
        T::RBracket => "List literal end `]`".to_string(),
        T::Assign => "Assignment `=`".to_string(),
        T::Colon => "Separator `:`".to_string(),
        T::Comma => "Comma`,`".to_string(),
        T::Dot => "Member access `.`".to_string(),
        T::Arrow => "Function arrow `=>`".to_string(),
        _ => format!("Token: {:?}", tok),
    }
}

pub(crate) fn stdlib_func_hover(tokens: &[CoreToken], idx: usize) -> Option<(String, String)> {
    let path = dotted_token_path(tokens, idx)?;
    let catalog = lk_stdlib::stdlib_catalog();
    if path.len() == 1 {
        let global = catalog.global(path[0])?;
        let signature = global.signature.clone().unwrap_or_else(|| global.detail.clone());
        let docs = global.docs.clone().unwrap_or_else(|| "LK stdlib global".to_string());
        return Some((signature, docs));
    }
    let export = catalog.export_path(&path)?;
    let signature = export.signature.clone().unwrap_or_else(|| export.detail.clone());
    let docs = export.docs.clone().unwrap_or_else(|| "LK stdlib export".to_string());
    Some((signature, docs))
}

fn dotted_token_path(tokens: &[CoreToken], idx: usize) -> Option<Vec<&str>> {
    let CoreToken::Id(name) = tokens.get(idx)? else {
        return None;
    };
    let mut path = vec![name.as_str()];
    let mut cursor = idx;
    while cursor >= 2 {
        if !matches!(tokens.get(cursor - 1), Some(CoreToken::Dot)) {
            break;
        }
        let Some(CoreToken::Id(parent)) = tokens.get(cursor - 2) else {
            break;
        };
        path.push(parent.as_str());
        cursor -= 2;
    }
    path.reverse();
    Some(path)
}

pub(crate) fn infer_call_at_position(content: &str, position: Position) -> (String, Option<usize>) {
    let rope = Rope::from_str(content);
    let char_idx = position_to_char_idx(&rope, position);
    let line_start = rope.try_char_to_line(char_idx).unwrap_or(0);
    let line_text = rope.line(line_start).to_string();
    let line_start_char = rope.line_to_char(line_start);
    let within_line = char_idx.saturating_sub(line_start_char);
    let line_prefix: String = line_text.chars().take(within_line).collect();

    if let Ok(re) = Regex::new(r"([A-Za-z_]\w*)\s*\(([A-Za-z0-9_,\s]*)$") {
        if let Some(caps) = re.captures(&line_prefix) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            let args_slice = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let count = args_slice.split(',').filter(|s| !s.trim().is_empty()).count();
            return (name, Some(count.saturating_sub(1)));
        }
    }
    (String::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdlib_hover_uses_generated_signature_and_docs() {
        let tokens = vec![
            CoreToken::Id("env".to_string()),
            CoreToken::Dot,
            CoreToken::Id("get".to_string()),
            CoreToken::LParen,
        ];

        let (signature, docs) = stdlib_func_hover(&tokens, 2).expect("env.get hover");

        assert_eq!(signature, "env.get(key: String) -> String?");
        assert_eq!(docs, "Returns an environment variable, or nil if it is not set.");
    }
}
