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

/// Does a line starting with these tokens continue the previous line?
///
/// The rule is "a token that can never *start* an expression", and it is
/// checked rather than assumed: of the infix operators only `-` can, because
/// `-x` is negation. `+x`, `*x`, `/x`, `%x` and `&x` are all syntax errors, so
/// a line beginning with one is a continuation and nothing else. (A leading
/// `-2` is not even a `Sub` token — the lexer folds the sign into the number.)
///
/// The doc comment here used to also exclude `*` on the grounds that `*p` was
/// "just as likely to be a fresh list element". There is no such element: `*`
/// has no prefix form. Left alone, every wrapped `a * b`, `a + b` and `a & b`
/// was dedented to statement level.
///
/// `|` is the one genuinely ambiguous lead, because it opens a lambda. That is
/// decided by looking at the rest of the line: `|x|` and `|x, y|` are a
/// parameter list, anything else is a bitwise or.
fn is_continuation_lead(line: &[Token]) -> bool {
    match line.first() {
        Some(
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
            | Token::Add
            | Token::Mul
            | Token::Div
            | Token::Mod
            | Token::BitAnd
            | Token::BitXor,
        ) => true,
        Some(Token::Pipe) => !opens_lambda_params(&line[1..]),
        _ => false,
    }
}

/// `x|`, `x, y|`, or `|` — the parameter list of a lambda whose `|` has already
/// been consumed. Anything else means the leading `|` was a bitwise or.
fn opens_lambda_params(rest: &[Token]) -> bool {
    let mut expect_name = true;
    for token in rest {
        match token {
            Token::Pipe => return true,
            Token::Id(_) if expect_name => expect_name = false,
            Token::Comma if !expect_name => expect_name = true,
            _ => return false,
        }
    }
    false
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
    /// Enough to see `|a, b, c|` — a longer parameter list on a wrapped line is
    /// not worth a heap allocation per line to catch.
    const LEAD_TOKENS: usize = 8;
    let mut line_lead: Vec<Vec<Token>> = vec![Vec::new(); line_count];

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

        // The first few tokens of each line, so the continuation test can look
        // past the lead — `|` needs the rest of a lambda's parameter list to
        // tell itself from a bitwise or.
        let lead = &mut line_lead[start];
        if lead.len() < LEAD_TOKENS {
            lead.push(token.clone());
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

    for (idx, lead) in line_lead.iter().enumerate() {
        facts.continuation[idx] = is_continuation_lead(lead);
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
    let mut out = String::with_capacity(input.len() + 16);
    let mut indent: i32 = 0;
    let mut lines: Vec<&str> = input.split('\n').collect();
    // `split` yields a trailing "" for a file that ends in a newline; that is
    // the terminator, not a line.
    if input.ends_with('\n') {
        lines.pop();
    }

    // Preserve the file's line ending instead of silently converting a CRLF
    // checkout to LF — read from a line the file actually *ends*, not from
    // whether "\r\n" appears anywhere in it. A `"a\r\nb"` inside a string
    // literal is content, and letting it decide would rewrite every line of an
    // LF file. Protected lines are skipped for the same reason, and the last
    // line only counts when something terminates it.
    let terminated = if input.ends_with('\n') {
        lines.len()
    } else {
        lines.len().saturating_sub(1)
    };
    let eol = (0..terminated)
        .find(|idx| !facts.protected.get(*idx).copied().unwrap_or(false))
        .map(|idx| if lines[idx].ends_with('\r') { "\r\n" } else { "\n" })
        .unwrap_or("\n");

    // The emitted level chosen for each bracket depth. Two lines at the *same*
    // depth must land in the same column, which is the one thing a re-indenter
    // cannot get wrong and still be called one.
    //
    // This used to be `depth.min(prev_level + 1)` — a per-line clamp meant to
    // stop a line that opens several brackets from jumping two levels. It also
    // made the column depend on how many lines had been emitted since, so a
    // list wrapped after a two-bracket open climbed a staircase:
    //
    // ```lk
    // uart_write([110, 97, 116,     // depth 0 -> level 0, opens `(` and `[`
    //     32, 116, 104,             // depth 2, prev 0 -> level 1
    //         102, 105, 98]);       // depth 2, prev 1 -> level 2   <- same depth!
    // ```
    //
    // Recording the level per depth keeps the "one level at a time" rule (a
    // multi-bracket open assigns *one* new level to every depth it skipped)
    // without letting position in the file decide the column.
    let mut depth_levels: Vec<usize> = vec![0];

    for (idx, raw_line) in lines.iter().enumerate() {
        let raw = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if facts.protected[idx] {
            out.push_str(raw);
        } else {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let depth = (indent - facts.leading_closers[idx] as i32).max(0) as usize;
                // A depth first seen on this line takes one level past the
                // deepest one already assigned — `unless!(cond {` or `foo(bar(`
                // skips a depth nobody wrote anything at, so both new depths
                // share that one level. Coming back out drops the levels that
                // are no longer open.
                depth_levels.truncate(depth + 1);
                if depth_levels.len() <= depth {
                    let level = depth_levels.last().copied().unwrap_or(0) + 1;
                    depth_levels.resize(depth + 1, level);
                }
                let level = depth_levels[depth] + usize::from(facts.continuation[idx]);
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
