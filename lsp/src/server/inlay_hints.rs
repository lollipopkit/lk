use regex::Regex;
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};

pub fn compute_inlay_hints(content: &str, range: Range) -> Vec<InlayHint> {
    compute_inlay_hints_with_margin(content, range, 3)
}

pub(crate) fn compute_inlay_hints_with_margin(content: &str, range: Range, margin_lines: usize) -> Vec<InlayHint> {
    let mut defs: HashMap<String, Vec<String>> = HashMap::new();
    let mut def_positions: HashSet<usize> = HashSet::new();
    if let Ok(re) = Regex::new(r"(?m)\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)") {
        for caps in re.captures_iter(content) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str().to_string();
                let name_start = name_match.start();
                def_positions.insert(name_start);

                let params_str = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let params: Vec<String> = params_str
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.split(':').next().unwrap_or("").trim().to_string())
                    .collect();
                if !name.is_empty() {
                    defs.insert(name, params);
                }
            }
        }
    }
    defs.entry("print".to_string())
        .or_insert_with(|| vec!["fmt".into(), "...args".into()]);
    defs.entry("println".to_string())
        .or_insert_with(|| vec!["fmt".into(), "...args".into()]);
    defs.entry("panic".to_string())
        .or_insert_with(|| vec!["message".into()]);

    let mut line_starts: Vec<usize> = vec![0];
    for (i, b) in content.as_bytes().iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let within_range = |ofs: usize| -> bool {
        let mut line = 0usize;
        for (idx, start) in line_starts.iter().enumerate() {
            if *start > ofs {
                break;
            }
            line = idx;
        }
        let line_u = line as u32;
        line_u >= range.start.line && line_u <= range.end.line
    };

    let mut hints = Vec::new();
    let bytes = content.as_bytes();
    let total_lines = line_starts.len();
    let start_line = range.start.line as usize;
    let end_line = range.end.line as usize;
    let scan_start_line = start_line.saturating_sub(margin_lines);
    let scan_end_line = (end_line + margin_lines).min(total_lines.saturating_sub(1));
    let scan_start_byte = line_starts.get(scan_start_line).copied().unwrap_or(0);
    let scan_end_byte = if scan_end_line + 1 < total_lines {
        line_starts[scan_end_line + 1]
    } else {
        content.len()
    };

    let mut i = scan_start_byte;
    let mut in_string: Option<u8> = None;
    let mut in_line_comment = false;
    while i < bytes.len() && i < scan_end_byte {
        if bytes[i] == b'\n' {
            in_line_comment = false;
            i += 1;
            continue;
        }
        if in_line_comment {
            i += 1;
            continue;
        }
        if in_string.is_none() && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if let Some(q) = in_string {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
                continue;
            }
            if bytes[i] == q {
                in_string = None;
                i += 1;
                continue;
            }
            i += 1;
            continue;
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }

        if i >= scan_end_byte {
            break;
        }
        if !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            i += 1;
            continue;
        }
        let name_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        let name_end = i;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'(' {
            continue;
        }

        let name = std::str::from_utf8(&bytes[name_start..name_end])
            .unwrap_or("")
            .to_string();
        if def_positions.contains(&name_start) {
            continue;
        }
        let params = defs.get(&name).cloned().unwrap_or_default();
        let mut depth = 0i32;
        let mut arg_start = i + 1;
        let mut arg_index = 0usize;
        let mut pos = i + 1;
        let mut next_string: Option<u8> = None;
        let mut next_comment = false;
        while pos < bytes.len() {
            if bytes[pos] == b'\n' {
                next_comment = false;
            }
            if next_comment {
                pos += 1;
                continue;
            }
            if next_string.is_none() && pos + 1 < bytes.len() && bytes[pos] == b'/' && bytes[pos + 1] == b'/' {
                next_comment = true;
                pos += 2;
                continue;
            }
            if let Some(q) = next_string {
                if bytes[pos] == b'\\' {
                    pos = (pos + 2).min(bytes.len());
                    continue;
                }
                if bytes[pos] == q {
                    next_string = None;
                    pos += 1;
                    continue;
                }
                pos += 1;
                continue;
            } else if bytes[pos] == b'"' || bytes[pos] == b'\'' {
                next_string = Some(bytes[pos]);
                pos += 1;
                continue;
            }

            match bytes[pos] as char {
                '(' | '[' | '{' => {
                    depth += 1;
                }
                ')' => {
                    if depth == 0 {
                        let (hint_pos, ok) = first_sig_pos(content, arg_start, pos);
                        if ok && arg_index < params.len() && within_range(hint_pos) {
                            hints.push(make_param_hint(&params[arg_index], hint_pos, &line_starts));
                        }
                        i = name_end;
                        break;
                    } else {
                        depth -= 1;
                    }
                }
                ',' => {
                    if depth == 0 {
                        let (hint_pos, ok) = first_sig_pos(content, arg_start, pos);
                        if ok && arg_index < params.len() && within_range(hint_pos) {
                            hints.push(make_param_hint(&params[arg_index], hint_pos, &line_starts));
                        }
                        arg_index += 1;
                        arg_start = pos + 1;
                    }
                }
                _ => {}
            }
            pos += 1;
        }
    }
    hints
}

fn first_sig_pos(content: &str, start: usize, end: usize) -> (usize, bool) {
    let slice = &content[start..end];
    let mut acc = 0usize;
    for ch in slice.chars() {
        if !ch.is_whitespace() {
            return (start + acc, true);
        }
        acc += ch.len_utf8();
    }
    (start, false)
}

fn make_param_hint(param: &str, ofs: usize, line_starts: &[usize]) -> InlayHint {
    let mut line = 0usize;
    for (idx, start) in line_starts.iter().enumerate() {
        if *start > ofs {
            break;
        }
        line = idx;
    }
    let col = ofs - line_starts[line];
    InlayHint {
        position: Position::new(line as u32, col as u32),
        label: InlayHintLabel::from(format!("{}:", param)),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: Some(false),
        data: None,
    }
}
