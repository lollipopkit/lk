use regex::Regex;
use ropey::Rope;
use tower_lsp::lsp_types::{Position, TextDocumentContentChangeEvent};

use lkr_core::token::{Span as CoreSpan, Token as CoreToken};

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
                if let Some((sig, doc)) = stdlib_func_hover(name) {
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

pub(crate) fn stdlib_func_hover(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "print" => Some(("print(fmt, ...args)", "Print to stdout without newline")),
        "println" => Some(("println(fmt, ...args)", "Print to stdout with newline")),
        "panic" => Some(("panic(message)", "Raise a runtime error")),
        "len" => Some(("len(collection)", "Return the length of a collection")),
        "type" => Some(("type(value)", "Return the runtime type of a value")),
        _ => None,
    }
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

pub(crate) fn find_call_before_cursor(line_prefix: &str) -> Option<(String, usize)> {
    let mut paren = 0i32;
    let bytes = line_prefix.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'(' => {
                if paren == 0 {
                    let mut j = i;
                    while j > 0 && (bytes[j - 1].is_ascii_alphanumeric() || bytes[j - 1] == b'_') {
                        j -= 1;
                    }
                    let name = &line_prefix[j..i];
                    let arg_count = bytes[i + 1..]
                        .split(|b| *b == b',')
                        .filter(|slice| slice.iter().any(|c| !c.is_ascii_whitespace()))
                        .count();
                    return Some((name.to_string(), arg_count));
                } else {
                    paren -= 1;
                }
            }
            b')' => paren += 1,
            _ => {}
        }
    }
    None
}

pub(crate) fn collect_named_keys_in_args(slice: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let bytes = slice.as_bytes();
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                paren += 1;
                i += 1;
            }
            b')' => {
                if paren > 0 {
                    paren -= 1;
                }
                i += 1;
            }
            b'{' => {
                brace += 1;
                i += 1;
            }
            b'}' => {
                if brace > 0 {
                    brace -= 1;
                }
                i += 1;
            }
            b'[' => {
                bracket += 1;
                i += 1;
            }
            b']' => {
                if bracket > 0 {
                    bracket -= 1;
                }
                i += 1;
            }
            _ => {
                if paren == 1 {
                    if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                        let start = i;
                        i += 1;
                        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                            i += 1;
                        }
                        let end = i;
                        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                            i += 1;
                        }
                        if i < bytes.len() && bytes[i] == b':' {
                            let name = &slice[start..end];
                            out.push(name.to_string());
                        }
                        continue;
                    }
                }
                i += 1;
            }
        }
    }
    out
}
