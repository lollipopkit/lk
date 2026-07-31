mod error;
mod lexer;

#[cfg(test)]
mod token_test;

pub use error::*;
pub use lexer::*;

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

/// One piece of a template string's content: plain text, or a `${…}` interior.
///
/// Both borrow from the content they were scanned out of, so reassembling a
/// template is a matter of writing the literals back verbatim and wrapping each
/// (possibly rewritten) expression in `${…}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSegment<'a> {
    Literal(&'a str),
    /// The text between `${` and its matching `}`, braces excluded.
    Expr(&'a str),
}

/// Split a `TemplateString` token's content into literal and `${…}` pieces.
///
/// One scanner, because there used to be two and they disagreed: the lexer
/// balances nested braces when deciding where an interpolation ends, and the
/// parser's copy did not, so `"${R {}}"` reached the parser as `R {` and the
/// struct-literal parser read past the end of its stream and panicked.
///
/// It is shared for a second reason now. A template is *one token* to the macro
/// expander, and its interior is only tokenized later, by the parser — so
/// nothing inside `${…}` was ever expanded or substituted. Teaching the expander
/// to look inside means it has to agree with the parser about where "inside"
/// starts and stops, and the way to guarantee that is to not have a second copy.
pub fn split_template_string(content: &str) -> Result<Vec<TemplateSegment<'_>>, TemplateScanError> {
    let mut segments = Vec::new();
    let mut literal_start = 0usize;
    let mut expr_start = None::<usize>;
    let mut depth = 0usize;

    let chars: Vec<(usize, char)> = content.char_indices().collect();
    let mut index = 0usize;
    while index < chars.len() {
        let (byte_pos, ch) = chars[index];
        match expr_start {
            Some(start) => {
                if ch == '{' {
                    depth += 1;
                } else if ch == '}' && depth > 0 {
                    depth -= 1;
                } else if ch == '}' {
                    segments.push(TemplateSegment::Expr(&content[start..byte_pos]));
                    expr_start = None;
                    literal_start = byte_pos + ch.len_utf8();
                }
                index += 1;
            }
            None if ch == '$' && index + 1 < chars.len() && chars[index + 1].1 == '{' => {
                if literal_start < byte_pos {
                    segments.push(TemplateSegment::Literal(&content[literal_start..byte_pos]));
                }
                index += 2;
                expr_start = Some(if index < chars.len() {
                    chars[index].0
                } else {
                    content.len()
                });
            }
            None => index += 1,
        }
    }

    if expr_start.is_some() {
        return Err(TemplateScanError::Unclosed);
    }
    if literal_start < content.len() {
        segments.push(TemplateSegment::Literal(&content[literal_start..]));
    }
    Ok(segments)
}

/// Write segments back out as template content, inverse of [`split_template_string`].
pub fn join_template_segments(segments: &[TemplateSegment<'_>]) -> String {
    let mut out = String::new();
    for segment in segments {
        match segment {
            TemplateSegment::Literal(text) => out.push_str(text),
            TemplateSegment::Expr(text) => {
                out.push_str("${");
                out.push_str(text);
                out.push('}');
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateScanError {
    Unclosed,
}

/// The word a keyword token spells, when it may stand in for an identifier.
///
/// Keywords are reserved *everywhere*, which is more than the grammar needs: a
/// **member** is always reached through `.` or declared inside a `struct` /
/// `impl` / `trait` body, and none of those positions can start a statement. So
/// `db.select()`, `parser.match(x)` and `struct Row { type: String }` were
/// syntax errors for no reason a reader could act on.
///
/// The value literals (`true`, `false`, `nil`) are deliberately absent: they are
/// values, not keywords, and `p.nil` reads as nothing.
///
/// A *top-level* `fn` keeps the restriction — a call to it is a bare name in
/// expression position, where `select(1)` and `select { … }` would have to be
/// told apart.
pub fn keyword_as_name(token: &Token) -> Option<&'static str> {
    Some(match token {
        Token::In => "in",
        Token::If => "if",
        Token::Else => "else",
        Token::While => "while",
        Token::Let => "let",
        Token::Const => "const",
        Token::Break => "break",
        Token::Continue => "continue",
        Token::Defer => "defer",
        Token::Return => "return",
        Token::Fn => "fn",
        Token::Use => "use",
        Token::From => "from",
        Token::As => "as",
        Token::For => "for",
        Token::Go => "go",
        Token::Match => "match",
        Token::Unsafe => "unsafe",
        Token::Try => "try",
        Token::Catch => "catch",
        Token::Select => "select",
        Token::Case => "case",
        Token::Default => "default",
        Token::Type => "type",
        Token::Struct => "struct",
        Token::Trait => "trait",
        Token::Impl => "impl",
        _ => return None,
    })
}

/// The source text a token was written as.
///
/// Lives here rather than in the macro system: it is a property of `Token`
/// itself, and having it there made `stmt` depend on `macro_system` purely to
/// print a token — a dependency cycle (`macro_system` parses `stmt` patterns)
/// that blocked separating the two.
pub fn token_lexeme(token: &Token) -> String {
    match token {
        Token::LParen => "(".to_string(),
        Token::RParen => ")".to_string(),
        Token::LBrace => "{".to_string(),
        Token::RBrace => "}".to_string(),
        Token::LBracket => "[".to_string(),
        Token::RBracket => "]".to_string(),
        Token::Dot => ".".to_string(),
        Token::ColonColon => "::".to_string(),
        Token::OptionalDot => "?.".to_string(),
        Token::Colon => ":".to_string(),
        Token::Comma => ",".to_string(),
        Token::Semicolon => ";".to_string(),
        Token::Dollar => "$".to_string(),
        Token::Hash => "#".to_string(),
        Token::At => "@".to_string(),
        Token::Defer => "defer".to_string(),
        Token::Assign => "=".to_string(),
        Token::AddAssign => "+=".to_string(),
        Token::SubAssign => "-=".to_string(),
        Token::MulAssign => "*=".to_string(),
        Token::DivAssign => "/=".to_string(),
        Token::ModAssign => "%=".to_string(),
        Token::Nil => "nil".to_string(),
        Token::Eq => "==".to_string(),
        Token::Ne => "!=".to_string(),
        Token::Gt => ">".to_string(),
        Token::Lt => "<".to_string(),
        Token::Ge => ">=".to_string(),
        Token::Le => "<=".to_string(),
        Token::In => "in".to_string(),
        Token::And => "&&".to_string(),
        Token::Or => "||".to_string(),
        Token::BitAnd => "&".to_string(),
        Token::BitNot => "~".to_string(),
        Token::Not => "!".to_string(),
        Token::Add => "+".to_string(),
        Token::Sub => "-".to_string(),
        Token::Mul => "*".to_string(),
        Token::Div => "/".to_string(),
        Token::Mod => "%".to_string(),
        Token::Arrow => "=>".to_string(),
        Token::LeftArrow => "<-".to_string(),
        Token::NullishCoalescing => "??".to_string(),
        Token::Range => "..".to_string(),
        Token::RangeInclusive => "..=".to_string(),
        Token::If => "if".to_string(),
        Token::Else => "else".to_string(),
        Token::While => "while".to_string(),
        Token::Let => "let".to_string(),
        Token::Const => "const".to_string(),
        Token::Break => "break".to_string(),
        Token::Continue => "continue".to_string(),
        Token::Return => "return".to_string(),
        Token::Fn => "fn".to_string(),
        Token::For => "for".to_string(),
        Token::Match => "match".to_string(),
        Token::Try => "try".to_string(),
        Token::Unsafe => "unsafe".to_string(),
        Token::Catch => "catch".to_string(),
        Token::Case => "case".to_string(),
        Token::Default => "default".to_string(),
        Token::Select => "select".to_string(),
        Token::Go => "go".to_string(),
        Token::Use => "use".to_string(),
        Token::From => "from".to_string(),
        Token::As => "as".to_string(),
        Token::Type => "type".to_string(),
        Token::Struct => "struct".to_string(),
        Token::Trait => "trait".to_string(),
        Token::Impl => "impl".to_string(),
        Token::Pipe => "|".to_string(),
        Token::Question => "?".to_string(),
        Token::FnArrow => "->".to_string(),
        Token::Str(value) => format!("\"{}\"", value.escape_default()),
        Token::TemplateString(value) => format!("\"{}\"", value.escape_default()),
        Token::Int(value) => value.to_string(),
        // Printed back at the radix it was written at: the decimal spelling of
        // a 64-bit mask is not what anyone wrote, and this text is what
        // `lk macro expand` shows. (`Token::Int` does *not* keep its radix, so
        // a mask that fits in an `i64` still comes back in decimal — see the
        // note on `Token::UInt`.)
        Token::UInt { value, radix } => render_radix(*value, *radix),
        Token::Float(value) => value.to_string(),
        Token::Bool(value) => value.to_string(),
        Token::Id(value) => value.clone(),
    }
}

/// A `u64` literal written back at the radix it was written at.
///
/// The separators a programmer used (`0x3F20_0000`) are not recoverable — the
/// lexer drops them — so this is the digits without them, which is the closest
/// this can get without keeping the lexeme itself.
pub fn render_radix(value: u64, radix: u32) -> alloc::string::String {
    match radix {
        16 => alloc::format!("0x{value:X}"),
        8 => alloc::format!("0o{value:o}"),
        2 => alloc::format!("0b{value:b}"),
        _ => alloc::format!("{value}"),
    }
}
