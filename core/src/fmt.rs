//! Source formatter shared by `lk fmt` and the LSP's `textDocument/formatting`.
//!
//! This is a **line re-indenter**, not a pretty-printer: it never moves code
//! between lines, so the only thing it can get wrong is the indent column. It
//! is driven by the tokenizer rather than by raw characters, which is what
//! makes it safe to run over a whole project unattended:
//!
//! - braces inside strings (`"}"`), raw strings and comments (`// }`) do not
//!   move the indent level, because they never surface as bracket tokens;
//! - lines inside a multi-line token (a raw string, an interpolated string with
//!   embedded newlines) or inside a block comment are emitted verbatim, so
//!   string contents and comment art survive byte-for-byte.
//!
//! A source that does not tokenize is rejected (`Err`) instead of being
//! rewritten on a guess — a formatter that mangles broken files is worse than
//! one that declines.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use crate::token::{ParseError, Span, Token, Tokenizer};

/// Layout knobs. Defaults match the language's own examples: 4 spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    /// Columns per indent level (or tabs per level when `use_tabs`).
    pub indent_width: usize,
    /// Indent with tab characters instead of spaces.
    pub use_tabs: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            indent_width: 4,
            use_tabs: false,
        }
    }
}

/// Format `input`, returning the formatted source. Idempotent:
/// `format_source(format_source(x)) == format_source(x)`.
pub fn format_source(input: &str, options: FormatOptions) -> Result<String, ParseError> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(input)?;
    Ok(render(input, &tokens, &spans, options))
}

/// Per-line layout facts derived from the token stream.
struct LineFacts {
    /// Emit the line verbatim (inside a multi-line string or block comment).
    protected: Vec<bool>,
    /// Net bracket depth change contributed by the line's tokens.
    delta: Vec<i32>,
    /// Closing brackets at the very start of the line, which dedent the line
    /// itself (`}`, `} else {`, `))` …).
    leading_closers: Vec<usize>,
    /// The line continues the previous one (`.map(…)` in a method chain), so it
    /// gets one extra level without changing the block depth.
    continuation: Vec<bool>,
}

/// Does a line starting with this token continue the previous line?
///
/// Only tokens that can never be a *prefix* operator qualify: `-x` and `*p` at
/// the start of a line are just as likely to be a fresh list element as a
/// continuation, and guessing wrong misaligns the element. (A leading `-2` is
/// not even a `Sub` token — the lexer folds the sign into the number.)
fn is_continuation_lead(token: &Token) -> bool {
    matches!(
        token,
        Token::Dot
            | Token::OptionalDot
            | Token::And
            | Token::Or
            | Token::NullishCoalescing
            | Token::Eq
            | Token::Ne
            | Token::Lt
            | Token::Gt
            | Token::Le
            | Token::Ge
    )
}

fn render(input: &str, tokens: &[Token], spans: &[Span], options: FormatOptions) -> String {
    let chars: Vec<char> = input.chars().collect();
    // 0-based line for every char offset, plus one entry past the end so the
    // scanners can index `chars.len()` without bounds checks.
    let mut line_of = Vec::with_capacity(chars.len() + 1);
    let mut line = 0usize;
    for &c in &chars {
        line_of.push(line);
        if c == '\n' {
            line += 1;
        }
    }
    line_of.push(line);
    let line_count = line + 1;

    let facts = collect_line_facts(&chars, &line_of, line_count, tokens, spans);
    emit(input, &facts, options)
}

fn collect_line_facts(
    chars: &[char],
    line_of: &[usize],
    line_count: usize,
    tokens: &[Token],
    spans: &[Span],
) -> LineFacts {
    let mut facts = LineFacts {
        protected: vec![false; line_count],
        delta: vec![0i32; line_count],
        leading_closers: vec![0usize; line_count],
        continuation: vec![false; line_count],
    };
    // Still counting leading closers on this line (cleared by the first token
    // that is not a closing bracket).
    let mut counting = vec![true; line_count];
    let mut seen_token = vec![false; line_count];

    for (token, span) in tokens.iter().zip(spans.iter()) {
        let start = (span.start.line as usize).saturating_sub(1);
        let end = (span.end.line as usize).saturating_sub(1);
        if start >= line_count {
            continue;
        }
        // Continuation lines of a multi-line token are string content.
        for flag in facts
            .protected
            .iter_mut()
            .take(end.min(line_count - 1) + 1)
            .skip(start + 1)
        {
            *flag = true;
        }

        if !seen_token[start] {
            seen_token[start] = true;
            facts.continuation[start] = is_continuation_lead(token);
        }

        let delta = match token {
            Token::LParen | Token::LBrace | Token::LBracket => 1,
            Token::RParen | Token::RBrace | Token::RBracket => -1,
            _ => 0,
        };
        facts.delta[start] += delta;
        if counting[start] {
            if delta < 0 {
                facts.leading_closers[start] += 1;
            } else {
                counting[start] = false;
            }
        }
    }

    // Everything between two tokens is whitespace or a comment: block comments
    // are the only thing there that can span lines.
    let mut cursor = 0usize;
    for span in spans {
        if span.start.offset > cursor {
            protect_block_comments(
                chars,
                cursor,
                span.start.offset,
                line_of,
                line_count,
                &mut facts.protected,
            );
        }
        cursor = cursor.max(span.end.offset);
    }
    protect_block_comments(chars, cursor, chars.len(), line_of, line_count, &mut facts.protected);

    facts
}

/// Mark the continuation lines of every block comment in `chars[from..to)`.
/// The opening line is left alone so `/* … */` blocks indent with their code.
fn protect_block_comments(
    chars: &[char],
    from: usize,
    to: usize,
    line_of: &[usize],
    line_count: usize,
    protected: &mut [bool],
) {
    let to = to.min(chars.len());
    let mut i = from;
    while i < to {
        if chars[i] == '/' && i + 1 < to && chars[i + 1] == '/' {
            while i < to && chars[i] != '\n' {
                i += 1;
            }
        } else if chars[i] == '/' && i + 1 < to && chars[i + 1] == '*' {
            let start_line = line_of[i];
            // The lexer allows nested block comments; match that here.
            let mut depth = 1usize;
            i += 2;
            while i < to && depth > 0 {
                if chars[i] == '/' && i + 1 < to && chars[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && i + 1 < to && chars[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let end_line = line_of[i.min(chars.len())];
            for flag in protected
                .iter_mut()
                .take(end_line.min(line_count - 1) + 1)
                .skip(start_line + 1)
            {
                *flag = true;
            }
        } else {
            i += 1;
        }
    }
}

fn emit(input: &str, facts: &LineFacts, options: FormatOptions) -> String {
    // Preserve the file's dominant line ending instead of silently converting
    // a CRLF checkout to LF.
    let eol = if input.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(input.len() + 16);
    let mut indent: i32 = 0;
    let mut lines: Vec<&str> = input.split('\n').collect();
    // `split` yields a trailing "" for a file that ends in a newline; that is
    // the terminator, not a line.
    if input.ends_with('\n') {
        lines.pop();
    }

    for (idx, raw_line) in lines.iter().enumerate() {
        let raw = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if facts.protected[idx] {
            out.push_str(raw);
        } else {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let level =
                    (indent - facts.leading_closers[idx] as i32).max(0) as usize + usize::from(facts.continuation[idx]);
                push_indent(&mut out, level, options);
                out.push_str(trimmed);
            }
        }
        out.push_str(eol);
        indent = (indent + facts.delta[idx]).max(0);
    }

    // Exactly one trailing newline. Trailing blank lines cannot be inside a
    // string (it would be unterminated), so trimming them is always safe.
    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    if !out.is_empty() {
        out.push_str(eol);
    }
    out
}

fn push_indent(out: &mut String, level: usize, options: FormatOptions) {
    let unit = if options.use_tabs { '\t' } else { ' ' };
    let width = if options.use_tabs {
        1
    } else {
        options.indent_width.clamp(1, 16)
    };
    for _ in 0..level * width {
        out.push(unit);
    }
}

#[cfg(test)]
mod fmt_test;
