mod error;
mod lexer;

#[cfg(test)]
mod token_test;

pub use error::*;
pub use lexer::*;

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

/// The source text a token was written as.
///
/// Lives here rather than in the macro system: it is a property of `Token`
/// itself, and having it there made `stmt` depend on `macro_system` purely to
/// print a token — a dependency cycle (`macro_system` parses `stmt` patterns)
/// that blocked separating the two.
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
        // `lk macro expand` shows.
        Token::UInt(value) => alloc::format!("0x{value:X}"),
        Token::Float(value) => value.to_string(),
        Token::Bool(value) => value.to_string(),
        Token::Id(value) => value.clone(),
    }
}
