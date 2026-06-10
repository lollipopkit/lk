use super::{LkAnalyzer, MAX_TOKENS_PER_DOC, MAX_TOKENS_PER_RANGE};
use serde::Serialize;
use std::collections::BTreeMap;
use tower_lsp::lsp_types::{Range, SemanticToken};

const SEMANTIC_TOKEN_TYPE_COUNT: u32 = 11;
const SEMANTIC_TOKEN_MODIFIER_COUNT: u32 = 4;
const KEYWORD_IDX: u32 = 1;
const VARIABLE_IDX: u32 = 2;
const OPERATOR_IDX: u32 = 6;

#[derive(Debug, Clone, Serialize)]
pub struct SemanticTokenValidationSummary {
    pub valid: bool,
    pub token_count: usize,
    pub errors: Vec<String>,
    pub token_type_counts: BTreeMap<u32, usize>,
    pub modifier_bitset_counts: BTreeMap<u32, usize>,
    pub max_line: u32,
    pub max_start: u32,
    pub max_length: u32,
}

fn semantic_keyword_token(identifier: &str) -> Option<u32> {
    match identifier {
        "if" | "else" | "while" | "for" | "in" | "fn" | "return" | "break" | "continue" | "use" | "from" | "as"
        | "match" | "case" | "default" | "select" | "type" | "trait" | "impl" | "true" | "false" | "nil" | "spawn"
        | "chan" | "send" | "recv" | "export" | "macro_rules" => Some(KEYWORD_IDX),
        _ => None,
    }
}

fn semantic_operator_len(chars: &[char], index: usize) -> Option<usize> {
    let current = *chars.get(index)?;
    let next = chars.get(index + 1).copied();
    let third = chars.get(index + 2).copied();

    match (current, next, third) {
        ('.', Some('.'), Some('=')) => Some(3),
        ('=', Some('='), _)
        | (':', Some(':'), _)
        | ('!', Some('='), _)
        | ('<', Some('='), _)
        | ('>', Some('='), _)
        | ('&', Some('&'), _)
        | ('|', Some('|'), _)
        | ('-', Some('>'), _)
        | ('=', Some('>'), _)
        | ('<', Some('-'), _)
        | ('?', Some('?'), _)
        | ('?', Some('.'), _)
        | ('.', Some('.'), _)
        | ('+', Some('='), _)
        | ('-', Some('='), _)
        | ('*', Some('='), _)
        | ('/', Some('='), _)
        | ('%', Some('='), _) => Some(2),
        ('|', _, _) => Some(1),
        _ => None,
    }
}

impl LkAnalyzer {
    /// Generate semantic tokens for LK code (optimized version)
    pub fn generate_semantic_tokens(&self, content: &str) -> Vec<SemanticToken> {
        // Early return for empty content
        if content.trim().is_empty() {
            return Vec::new();
        }

        // Use the existing, working implementation but with optimizations
        let mut tokens: Vec<SemanticToken> = Vec::new();
        let mut line_number = 0;

        // Define the legend indices (must match the legend in main.rs)
        const COMMENT_IDX: u32 = 0;
        const FUNCTION_IDX: u32 = 3;
        const STRING_IDX: u32 = 4;
        const NUMBER_IDX: u32 = 5;

        let lines: Vec<&str> = content.lines().collect();

        // Track multi-line block comments
        let mut in_block_comment = false;

        for line in lines {
            let mut char_index = 0;
            let chars: Vec<char> = line.chars().collect();
            let len = chars.len();

            while char_index < len {
                let c = chars[char_index];

                // Skip whitespace
                if c.is_whitespace() {
                    char_index += 1;
                    continue;
                }

                // Handle block comments spanning multiple lines
                if in_block_comment {
                    // Search for end of block comment '*/' in the current line
                    let mut j = char_index;
                    let mut end_found = false;
                    while j + 1 < len {
                        if chars[j] == '*' && chars[j + 1] == '/' {
                            // Emit from current position to the end of '*/'
                            let comment_len = (j + 2) - char_index;
                            tokens.push(self.create_token(line_number, char_index, comment_len, COMMENT_IDX, 0));
                            // Move past '*/' and exit block comment
                            char_index = j + 2;
                            in_block_comment = false;
                            end_found = true;
                            break;
                        }
                        j += 1;
                    }
                    if !end_found {
                        // Entire rest of line is comment
                        tokens.push(self.create_token(line_number, char_index, len - char_index, COMMENT_IDX, 0));
                        // Proceed to next line still inside block comment
                        break;
                    }
                    // Continue scanning the remainder of the line after closing the block comment
                    continue;
                }

                // Handle line comments: // ...
                if c == '/' && char_index + 1 < len && chars[char_index + 1] == '/' {
                    let comment_start = char_index;
                    // Everything to end of line is a comment
                    tokens.push(self.create_token(line_number, comment_start, len - comment_start, COMMENT_IDX, 0));
                    break;
                }

                // Handle block comment start: /* ... */
                if c == '/' && char_index + 1 < len && chars[char_index + 1] == '*' {
                    let comment_start = char_index;
                    // Look for closing */ on the same line first
                    let mut j = char_index + 2;
                    let mut closed_here = false;
                    while j + 1 < len {
                        if chars[j] == '*' && chars[j + 1] == '/' {
                            // Found end on the same line
                            let comment_len = (j + 2) - comment_start;
                            tokens.push(self.create_token(line_number, comment_start, comment_len, COMMENT_IDX, 0));
                            char_index = j + 2;
                            closed_here = true;
                            break;
                        }
                        j += 1;
                    }
                    if !closed_here {
                        // Rest of line is comment; continue block comment on next lines
                        tokens.push(self.create_token(line_number, comment_start, len - comment_start, COMMENT_IDX, 0));
                        in_block_comment = true;
                        break;
                    }
                    // Continue scanning after end of block comment on same line
                    continue;
                }

                // Handle hash-style comments (# ...).
                if c == '#' {
                    let comment_start = char_index;
                    // Everything to end of line is a comment
                    tokens.push(self.create_token(line_number, comment_start, len - comment_start, COMMENT_IDX, 0));
                    break;
                }

                // Handle strings
                if c == '"' || c == '\'' {
                    let string_start = char_index;
                    let quote_char = c;
                    char_index += 1;

                    while char_index < len && chars[char_index] != quote_char {
                        if chars[char_index] == '\\' && char_index + 1 < len {
                            char_index += 2;
                        } else {
                            char_index += 1;
                        }
                    }

                    if char_index < len && chars[char_index] == quote_char {
                        char_index += 1;
                    }

                    tokens.push(self.create_token(line_number, string_start, char_index - string_start, STRING_IDX, 0));
                    continue;
                }

                // Handle numbers
                if c.is_ascii_digit() {
                    let num_start = char_index;
                    while char_index < len && (chars[char_index].is_ascii_digit() || chars[char_index] == '.') {
                        char_index += 1;
                    }

                    tokens.push(self.create_token(line_number, num_start, char_index - num_start, NUMBER_IDX, 0));
                    continue;
                }

                // Handle identifiers and keywords (and detect function calls)
                if c.is_alphabetic() || c == '_' {
                    let ident_start = char_index;
                    while char_index < len && (chars[char_index].is_alphanumeric() || chars[char_index] == '_') {
                        char_index += 1;
                    }

                    let identifier: String = chars[ident_start..char_index].iter().collect();

                    // Let TextMate scopes drive declaration keyword colors so themes
                    // can render `let`/`const` consistently with Rust-style grammars.
                    if matches!(identifier.as_str(), "let" | "const" | "_") {
                        continue;
                    }

                    // Check for keywords
                    let mut token_idx = semantic_keyword_token(&identifier).unwrap_or(VARIABLE_IDX);

                    // If next non-whitespace char is '(' or '!', treat as function/macro identifier.
                    if token_idx == VARIABLE_IDX {
                        let mut j = char_index;
                        while j < len && chars[j].is_whitespace() {
                            j += 1;
                        }
                        if j < len && matches!(chars[j], '(' | '!') {
                            token_idx = FUNCTION_IDX;
                        }
                    }

                    tokens.push(self.create_token(line_number, ident_start, char_index - ident_start, token_idx, 0));
                    continue;
                }

                if let Some(op_len) = semantic_operator_len(&chars, char_index) {
                    tokens.push(self.create_token(line_number, char_index, op_len, OPERATOR_IDX, 0));
                    char_index += op_len;
                    continue;
                }

                // Skip other operators and punctuation to reduce token density
                if "+-*/%,;(){}[]@.$#".contains(c) {
                    char_index += 1;
                    continue;
                }

                char_index += 1;
            }

            line_number += 1;
            // Stop early if token budget is exceeded
            if tokens.len() >= MAX_TOKENS_PER_DOC {
                break;
            }
        }

        // Convert absolute positions to delta-encoded positions required by LSP
        let mut result: Vec<SemanticToken> = Vec::with_capacity(tokens.len());
        let mut prev_line: u32 = 0;
        let mut prev_start: u32 = 0;
        let mut first = true;

        for t in tokens.into_iter() {
            let line = t.delta_line; // stored absolute line
            let start = t.delta_start; // stored absolute start
            let delta_line = if first { line } else { line.saturating_sub(prev_line) };
            let delta_start = if first || delta_line != 0 {
                start
            } else {
                start.saturating_sub(prev_start)
            };

            result.push(SemanticToken {
                delta_line,
                delta_start,
                length: t.length,
                token_type: t.token_type,
                token_modifiers_bitset: t.token_modifiers_bitset,
            });

            prev_line = line;
            prev_start = start;
            first = false;
        }

        result
    }

    /// Generate semantic tokens for a specific LSP range (best-effort).
    /// Note: range is interpreted using UTF-16 columns per LSP spec.
    pub fn generate_semantic_tokens_in_range(&self, content_slice: &str, range: Range) -> Vec<SemanticToken> {
        // Helper to convert UTF-16 column to char index for a single line
        fn utf16_to_char_idx(line: &str, utf16_col: u32) -> usize {
            let mut seen = 0usize;
            for (i, ch) in line.chars().enumerate() {
                let w = ch.len_utf16();
                if seen + w > utf16_col as usize {
                    return i;
                }
                seen += w;
                if seen == utf16_col as usize {
                    return i + 1;
                }
            }
            line.chars().count()
        }

        let start_line_abs = range.start.line as usize;
        let _end_line_abs = range.end.line as usize;
        let start_utf16 = range.start.character;
        let end_utf16 = range.end.character;

        // We'll first collect tokens with absolute positions, then convert to LSP delta encoding
        let mut tokens: Vec<SemanticToken> = Vec::new();

        // Define the legend indices (must match the legend in main.rs)
        const COMMENT_IDX: u32 = 0;
        const FUNCTION_IDX: u32 = 3;
        const STRING_IDX: u32 = 4;
        const NUMBER_IDX: u32 = 5;

        let lines: Vec<&str> = content_slice.lines().collect();
        if lines.is_empty() {
            return Vec::new();
        }
        let first_local = 0usize;
        let last_local = lines.len().saturating_sub(1);

        // Track multi-line block comments inside the processed window only
        let mut in_block_comment = false;

        for (local_idx, line) in lines.iter().enumerate() {
            let line_number = (start_line_abs + local_idx) as u32;
            let mut char_index = 0usize;
            let chars: Vec<char> = line.chars().collect();
            let len = chars.len();

            // Compute char bounds for clamping tokens on boundary lines
            let start_char_bound = if local_idx == first_local {
                utf16_to_char_idx(line, start_utf16)
            } else {
                0
            };
            let end_char_bound = if local_idx == last_local {
                utf16_to_char_idx(line, end_utf16).max(start_char_bound)
            } else {
                len
            };

            while char_index < len {
                let c = chars[char_index];

                // Skip whitespace
                if c.is_whitespace() {
                    char_index += 1;
                    continue;
                }

                // Handle block comments spanning multiple lines
                if in_block_comment {
                    // Search for end of block comment '*/' in the current line
                    let mut j = char_index;
                    while j + 1 < len {
                        if chars[j] == '*' && chars[j + 1] == '/' {
                            // emit block until here if within bounds
                            let start = char_index.max(start_char_bound);
                            let length = if j + 2 > start {
                                (j + 2).saturating_sub(start)
                            } else {
                                0
                            };
                            if length > 0 && start < end_char_bound {
                                let capped_len = length.min(end_char_bound.saturating_sub(start));
                                tokens.push(self.create_token(line_number, start, capped_len, COMMENT_IDX, 0));
                            }
                            char_index = j + 2;
                            in_block_comment = false;
                            break;
                        }
                        j += 1;
                    }
                    if in_block_comment {
                        // whole rest of line is a comment
                        let start = char_index.max(start_char_bound);
                        if start < end_char_bound {
                            let capped_len = end_char_bound - start;
                            tokens.push(self.create_token(line_number, start, capped_len, COMMENT_IDX, 0));
                        }
                        break;
                    }
                    continue;
                }

                // Line comments
                if c == '/' && char_index + 1 < len && chars[char_index + 1] == '/' {
                    let start = char_index.max(start_char_bound);
                    if start < end_char_bound {
                        let capped_len = end_char_bound - start;
                        tokens.push(self.create_token(line_number, start, capped_len, COMMENT_IDX, 0));
                    }
                    break;
                }
                // Block comment start
                if c == '/' && char_index + 1 < len && chars[char_index + 1] == '*' {
                    in_block_comment = true;
                    let start = char_index.max(start_char_bound);
                    if start < end_char_bound {
                        let capped_len = (char_index + 2).saturating_sub(start);
                        if capped_len > 0 {
                            tokens.push(self.create_token(line_number, start, capped_len, COMMENT_IDX, 0));
                        }
                    }
                    char_index += 2;
                    continue;
                }

                // Strings (single quoted or double quoted)
                if c == '"' || c == '\'' {
                    let mut j = char_index + 1;
                    while j < len {
                        if chars[j] == c && chars[j - 1] != '\\' {
                            break;
                        }
                        j += 1;
                    }
                    let end = if j < len { j + 1 } else { len };
                    let start = char_index.max(start_char_bound);
                    if start < end_char_bound {
                        let capped_len_total = end.saturating_sub(start);
                        if capped_len_total > 0 {
                            let capped_len = capped_len_total.min(end_char_bound.saturating_sub(start));
                            tokens.push(self.create_token(line_number, start, capped_len, STRING_IDX, 0));
                        }
                    }
                    char_index = end;
                    continue;
                }

                // Numbers
                if c.is_ascii_digit() {
                    let mut j = char_index + 1;
                    while j < len && (chars[j].is_ascii_digit() || chars[j] == '.') {
                        j += 1;
                    }
                    let start = char_index.max(start_char_bound);
                    if start < end_char_bound {
                        let capped_len_total = j.saturating_sub(start);
                        if capped_len_total > 0 {
                            let capped_len = capped_len_total.min(end_char_bound.saturating_sub(start));
                            tokens.push(self.create_token(line_number, start, capped_len, NUMBER_IDX, 0));
                        }
                    }
                    char_index = j;
                    continue;
                }

                // Identifiers and keywords (and detect function calls)
                if c.is_ascii_alphabetic() || c == '_' {
                    let ident_start = char_index;
                    let mut j = char_index + 1;
                    while j < len && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                        j += 1;
                    }
                    let slice: &str = &line[ident_start..j];
                    // Let TextMate scopes drive declaration keyword colors so themes
                    // can render `let`/`const` consistently with Rust-style grammars.
                    if matches!(slice, "let" | "const" | "_") {
                        char_index = j;
                        continue;
                    }

                    let mut token_idx = semantic_keyword_token(slice).unwrap_or(VARIABLE_IDX);
                    // Detect function or macro call by peeking next non-whitespace char.
                    if token_idx == VARIABLE_IDX {
                        let mut k = j;
                        while k < len && chars[k].is_whitespace() {
                            k += 1;
                        }
                        if k < len && matches!(chars[k], '(' | '!') {
                            token_idx = FUNCTION_IDX;
                        }
                    }
                    let start = ident_start.max(start_char_bound);
                    if start < end_char_bound {
                        let capped_len_total = j.saturating_sub(start);
                        if capped_len_total > 0 {
                            let capped_len = capped_len_total.min(end_char_bound.saturating_sub(start));
                            tokens.push(self.create_token(line_number, start, capped_len, token_idx, 0));
                        }
                    }
                    char_index = j;
                    continue;
                }

                if let Some(op_len) = semantic_operator_len(&chars, char_index) {
                    let start = char_index.max(start_char_bound);
                    if start < end_char_bound {
                        let capped_len_total = (char_index + op_len).saturating_sub(start);
                        if capped_len_total > 0 {
                            let capped_len = capped_len_total.min(end_char_bound.saturating_sub(start));
                            tokens.push(self.create_token(line_number, start, capped_len, OPERATOR_IDX, 0));
                        }
                    }
                    char_index += op_len;
                    continue;
                }

                // Skip other operators and punctuation to reduce token density
                if "+-*/%,;(){}[]@.".contains(c) {
                    char_index += 1;
                    continue;
                }

                char_index += 1;
            }
            // Stop early if range token budget is exceeded
            if tokens.len() >= MAX_TOKENS_PER_RANGE {
                break;
            }
        }

        // Convert absolute positions to delta-encoded positions required by LSP
        let mut result: Vec<SemanticToken> = Vec::with_capacity(tokens.len());
        let mut prev_line: u32 = 0;
        let mut prev_start: u32 = 0;
        let mut first = true;
        for t in tokens.into_iter() {
            let line = t.delta_line;
            let start = t.delta_start;
            let delta_line = if first { line } else { line.saturating_sub(prev_line) };
            let delta_start = if first || delta_line != 0 {
                start
            } else {
                start.saturating_sub(prev_start)
            };
            result.push(SemanticToken {
                delta_line,
                delta_start,
                length: t.length,
                token_type: t.token_type,
                token_modifiers_bitset: t.token_modifiers_bitset,
            });
            prev_line = line;
            prev_start = start;
            first = false;
        }
        result
    }

    pub fn validate_semantic_tokens(&self, content: &str, tokens: &[SemanticToken]) -> SemanticTokenValidationSummary {
        let line_lengths: Vec<u32> = content
            .lines()
            .map(|line| line.chars().map(|ch| ch.len_utf16() as u32).sum())
            .collect();
        let allowed_modifier_bits = if SEMANTIC_TOKEN_MODIFIER_COUNT == 32 {
            u32::MAX
        } else {
            (1u32 << SEMANTIC_TOKEN_MODIFIER_COUNT) - 1
        };

        let mut errors = Vec::new();
        let mut token_type_counts = BTreeMap::new();
        let mut modifier_bitset_counts = BTreeMap::new();
        let mut line = 0u32;
        let mut start = 0u32;
        let mut max_line = 0u32;
        let mut max_start = 0u32;
        let mut max_length = 0u32;

        for (idx, token) in tokens.iter().enumerate() {
            if idx == 0 {
                line = token.delta_line;
                start = token.delta_start;
            } else if token.delta_line == 0 {
                start = start.saturating_add(token.delta_start);
            } else {
                line = line.saturating_add(token.delta_line);
                start = token.delta_start;
            }

            *token_type_counts.entry(token.token_type).or_insert(0) += 1;
            *modifier_bitset_counts.entry(token.token_modifiers_bitset).or_insert(0) += 1;
            max_line = max_line.max(line);
            max_start = max_start.max(start);
            max_length = max_length.max(token.length);

            if token.length == 0 {
                errors.push(format!("token {idx}: length must be greater than 0"));
            }
            if token.token_type >= SEMANTIC_TOKEN_TYPE_COUNT {
                errors.push(format!(
                    "token {idx}: token_type {} exceeds legend size {}",
                    token.token_type, SEMANTIC_TOKEN_TYPE_COUNT
                ));
            }
            if token.token_modifiers_bitset & !allowed_modifier_bits != 0 {
                errors.push(format!(
                    "token {idx}: modifier bitset {} contains undeclared modifiers",
                    token.token_modifiers_bitset
                ));
            }

            let Some(line_len) = line_lengths.get(line as usize).copied() else {
                errors.push(format!("token {idx}: line {line} is outside document"));
                continue;
            };
            if start > line_len {
                errors.push(format!(
                    "token {idx}: start {start} exceeds line {line} length {line_len}"
                ));
                continue;
            }
            if start.saturating_add(token.length) > line_len {
                errors.push(format!(
                    "token {idx}: range {}..{} exceeds line {line} length {line_len}",
                    start,
                    start.saturating_add(token.length)
                ));
            }
        }

        SemanticTokenValidationSummary {
            valid: errors.is_empty(),
            token_count: tokens.len(),
            errors,
            token_type_counts,
            modifier_bitset_counts,
            max_line,
            max_start,
            max_length,
        }
    }

    fn create_token(
        &self,
        line: u32,
        start_char: usize,
        length: usize,
        token_type_idx: u32,
        modifiers: u32,
    ) -> SemanticToken {
        SemanticToken {
            delta_line: line,                  // line number (0-based)
            delta_start: start_char as u32,    // start character (0-based)
            length: length as u32,             // token length
            token_type: token_type_idx,        // token type index
            token_modifiers_bitset: modifiers, // token modifiers
        }
    }
}
