//! How a type is *spelled* in tokens.
//!
//! [`Type::parse`] takes a string, and the parsers hold tokens, so somebody has
//! to decide where a type annotation ends and render what it collected. That was
//! written once inside the statement parser — and then a lambda needed the same
//! thing (`|x: Int| -> Int { … }`), in a position where the statement parser
//! cannot be reached.
//!
//! It lives here rather than in `token` because it names [`Type`], and `token`
//! must not reach into `val` — that edge would drag `token` transitively into
//! the `val` ↔ `vm` cycle (see `docs/module-cycles.md`).
//!
//! What differs between the positions is only *where the type ends*, and that
//! is what [`StopAt`] says. The interesting one is `|`: at statement level it
//! separates the members of a union type, and between a lambda's `|`s it closes
//! the parameter list. So a union cannot be written directly in a lambda
//! parameter — parentheses do not help, because a parenthesised type is not part
//! of the grammar — and it goes through a `type` alias instead, which is a
//! second spelling of the same type.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use crate::token::{Token, token_lexeme};
use crate::val::Type;

/// What ends the annotation, beyond the tokens a type can never contain.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopAt {
    /// Statement level (`let x: T = …`, a parameter list, a field): `|` is a
    /// union separator and belongs to the type; `=` ends it.
    Union,
    /// A lambda parameter (`|x: T, y: U|`): a top-level `|` closes the list and
    /// a top-level `,` starts the next parameter.
    ClosureParam,
    /// A lambda's return type (`|x| -> T { … }`): a top-level `{` starts the
    /// body.
    ClosureReturn,
}

impl StopAt {
    /// Whether `token`, seen at nesting depth zero, ends the annotation.
    fn ends_here(self, token: &Token) -> bool {
        match self {
            StopAt::Union => matches!(token, Token::Assign),
            StopAt::ClosureParam => matches!(token, Token::Pipe | Token::Comma),
            StopAt::ClosureReturn => matches!(token, Token::LBrace),
        }
    }
}

/// Parses the type starting at `tokens[from]`, returning it and the index just
/// past it.
///
/// `None` when nothing type-shaped is there or the collected spelling is not a
/// type — the caller words the error, because "after `:`" and "after `->`" want
/// different ones.
pub(crate) fn parse_type_at(tokens: &[Token], from: usize, stop: StopAt) -> Option<(Type, usize)> {
    let (collected, end) = collect(tokens, from, stop);
    if collected.is_empty() {
        return None;
    }
    Type::parse(&spelling(&collected)).map(|ty| (ty, end))
}

/// How the annotation at `from` is spelled, for an error message.
///
/// The parse and the report have to agree on *what* was read, so the report
/// comes from the same collector rather than from a second guess at where the
/// type ended.
pub(crate) fn spelling_at(tokens: &[Token], from: usize, stop: StopAt) -> String {
    let (collected, _) = collect(tokens, from, stop);
    spelling(&collected)
}

/// The one mis-spelling worth naming: a Rust-shaped function type.
///
/// `fn(Int) -> Int` is what somebody coming from Rust writes, and it is not a
/// type here — `fn` introduces a *declaration*, and the type is `(Int) -> Int`.
/// Without this the two type positions answered differently and neither said
/// the rule: a parameter reported `Invalid type: Fn ( Int) -> Int` (a spelling
/// the program does not contain, from a collector that took the `fn` and then
/// could not parse it) and a `let` reported `Expected type annotation (found
/// Fn)` (from the collector that stops at `fn`, leaving nothing to name).
///
/// Deliberately *not* accepting `fn(…)` as a second spelling: one type, one
/// way to write it. Two spellings is the shape this codebase keeps removing.
pub(crate) fn function_type_hint(tokens: &[Token], from: usize) -> Option<&'static str> {
    matches!(tokens.get(from), Some(Token::Fn)).then_some(
        "a function type is written without `fn` — `(Int) -> Int`, not `fn(Int) -> Int`. \
         In this language `fn` introduces a declaration, never a type",
    )
}

/// The tokens making up a type annotation, and where it ends.
///
/// Nesting is tracked so a delimiter *inside* the type does not end it:
/// `Map<String, List<Int>>` holds commas, and `(Int) -> Int` holds parentheses.
fn collect(tokens: &[Token], from: usize, stop: StopAt) -> (Vec<&Token>, usize) {
    let mut out: Vec<&Token> = Vec::new();
    let mut pos = from;
    let (mut paren, mut bracket, mut brace, mut angle) = (0i32, 0i32, 0i32, 0i32);
    while pos < tokens.len() {
        let nested = paren > 0 || bracket > 0 || brace > 0 || angle > 0;
        if !nested && stop.ends_here(&tokens[pos]) {
            break;
        }
        match &tokens[pos] {
            Token::LParen => paren += 1,
            Token::RParen if paren > 0 => paren -= 1,
            Token::LBracket => bracket += 1,
            Token::RBracket if bracket > 0 => bracket -= 1,
            Token::LBrace => brace += 1,
            Token::RBrace if brace > 0 => brace -= 1,
            Token::Lt => angle += 1,
            Token::Gt => angle = angle.saturating_sub(1),
            // `*` starts a pointer type (`*u8`, `*mut u32`). It is the same
            // token as multiplication, but a type position never contains one,
            // so there is nothing to disambiguate.
            Token::Id(_)
            | Token::Comma
            | Token::Colon
            | Token::ColonColon
            | Token::Assign
            | Token::FnArrow
            | Token::Question
            | Token::Mul
            | Token::Pipe => {}
            _ => break,
        }
        out.push(&tokens[pos]);
        pos += 1;
    }
    (out, pos)
}

/// Renders collected tokens as the string [`Type::parse`] reads.
///
/// Spacing matters only where it separates identifiers; `<`, `>`, `,` and the
/// closers attach to what precedes them, and `|` gets spaces because that is
/// how a union prints.
/// The one renderer for a type's written form. `pub(crate)` because the
/// statement parser's three token-collecting positions render with it too —
/// they used to have their own copy, whose token table was missing every
/// keyword, so a type spelling holding one came out as the *Debug* name:
/// `fn(Int) -> Int` read `Fn(Int) -> Int`, `nil` read `Nil`.
pub(crate) fn spelling(tokens: &[&Token]) -> String {
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        if i == 0 {
            out.push_str(&token_lexeme(token));
            continue;
        }
        match token {
            Token::Pipe => out.push_str(" | "),
            Token::Lt => out.push('<'),
            Token::Gt | Token::Comma | Token::RParen | Token::RBracket | Token::RBrace => {
                out.push_str(&token_lexeme(token));
            }
            _ => {
                if !matches!(tokens.get(i - 1), Some(Token::Lt)) {
                    out.push(' ');
                }
                out.push_str(&token_lexeme(token));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Tokenizer;

    fn ty(src: &str, stop: StopAt) -> Option<(Type, usize)> {
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        parse_type_at(&tokens, 0, stop)
    }

    #[test]
    fn nesting_does_not_end_the_annotation() {
        let (parsed, end) = ty("Map<String, List<Int>>", StopAt::Union).expect("a nested type");
        assert_eq!(parsed.display(), "Map<String, List<Int>>");
        assert_eq!(end, 9, "the whole spelling is consumed");
    }

    /// The one interesting difference between the positions.
    #[test]
    fn pipe_is_a_union_at_statement_level_and_a_delimiter_in_a_lambda() {
        let (union, _) = ty("Int | String", StopAt::Union).expect("a union");
        assert_eq!(union.display(), "Int | String");
        // Between a lambda's `|`s the same token closes the parameter list, so
        // the type is just `Int` and the caller resumes at the `|`.
        let (single, end) = ty("Int | String", StopAt::ClosureParam).expect("a single type");
        assert_eq!(single.display(), "Int");
        assert_eq!(end, 1);
        // Which is why a union in a lambda parameter goes through a `type`
        // alias: parentheses are not a way to group a type here.
        assert!(ty("(Int | String)", StopAt::ClosureParam).is_none());
    }

    /// Each position ends where its grammar says, and nesting is not the end.
    #[test]
    fn each_position_ends_where_its_grammar_says() {
        // A parameter ends at the comma starting the next one…
        let (first, end) = ty("Int, b: String", StopAt::ClosureParam).expect("the first parameter");
        assert_eq!(first.display(), "Int");
        assert_eq!(end, 1);
        // …but not at a comma *inside* the type.
        let (nested, _) = ty("Map<String, Int>, b: String", StopAt::ClosureParam).expect("a nested comma");
        assert_eq!(nested.display(), "Map<String, Int>");
        // A return type ends where the body opens.
        let (ret, end) = ty("Int { return 1; }", StopAt::ClosureReturn).expect("a return type");
        assert_eq!(ret.display(), "Int");
        assert_eq!(end, 1);
    }

    #[test]
    fn a_function_type_is_a_type() {
        let (parsed, _) = ty("(Int, Int) -> String", StopAt::Union).expect("a function type");
        assert_eq!(parsed.display(), "(Int, Int) -> String");
    }

    #[test]
    fn nothing_type_shaped_is_none() {
        assert!(ty("{", StopAt::Union).is_none());
        assert!(ty("+", StopAt::Union).is_none());
        // An unknown *name*, on the other hand, is a type: user-declared
        // structs and traits arrive here as plain identifiers.
        let (named, _) = ty("Nonesuch", StopAt::Union).expect("a named type");
        assert_eq!(named.display(), "Nonesuch");
    }
}
