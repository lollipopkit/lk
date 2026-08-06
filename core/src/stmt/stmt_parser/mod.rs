use crate::token::{Span, Token};

pub struct StmtParser<'a> {
    pub(crate) tokens: &'a [Token],
    pub(crate) pos: usize,
    pub(crate) len: usize,
    pub(crate) token_spans: Option<&'a [Span]>,
    /// Inside an `impl` or `trait` body, where a `fn` declares a **member**.
    ///
    /// A member is only ever reached through `.`, so a keyword names one
    /// unambiguously. A *top-level* `fn` keeps the restriction: a call to it is
    /// a bare name in expression position, where `select(1)` and `select { … }`
    /// would have to be told apart.
    pub(crate) in_member_body: bool,
    /// Live nesting depth of `parse_statement`, bounded by
    /// [`MAX_STMT_DEPTH`].
    ///
    /// The statement twin of `ast::parser::Parser::depth`. Statement parsing is
    /// recursive descent too — `if { if { … } }` is one Rust frame per level —
    /// and *every consumer downstream inherits the depth*: the type checker
    /// walks the same tree, and it is the one that ran out first.
    /// `lk check` on 170 nested `if`s aborted the process with
    /// `fatal runtime error: stack overflow` (exit 134), no line to blame,
    /// while parsing the same file alone succeeded.
    pub(crate) depth: usize,
}

/// Cap on live `parse_statement` frames (see [`StmtParser::depth`]).
///
/// **One level of source nesting costs two frames**: `if cond { … }` is a
/// statement, and so is the block it takes as its body. So this bounds source
/// nesting at half its value — 24 levels with the number below, measured that
/// way rather than assumed.
///
/// Set from measurement, like [`crate::ast::parser::MAX_EXPR_DEPTH`]. A debug
/// `lk check` (8MiB main stack) aborts between 160 and 170 *source* levels, so
/// one level costs roughly 50KiB across parse and check. A libtest thread gets
/// 2MiB, putting its ceiling near 41 source levels; 24 sits under that with
/// room to spare.
///
/// The deepest brace nesting in this repository's own `.lk` corpus — counting
/// the `fn`/`impl`/`struct` levels too — is **6**.
#[cfg(feature = "std")]
pub const MAX_STMT_DEPTH: usize = 48;
/// An MCU stack is kilobytes, not megabytes (same reasoning as the expression
/// cap's bare-metal value). Eight source levels.
#[cfg(not(feature = "std"))]
pub const MAX_STMT_DEPTH: usize = 16;

impl<'a> StmtParser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: None,
            in_member_body: false,
            depth: 0,
        }
    }

    pub fn new_with_spans(tokens: &'a [Token], spans: &'a [Span]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: Some(spans),
            in_member_body: false,
            depth: 0,
        }
    }
}

mod bindings;
mod blocks;
mod control;
mod declarations;
mod function;
mod helpers;
mod imports;
mod program;
