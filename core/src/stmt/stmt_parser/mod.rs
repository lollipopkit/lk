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
}

impl<'a> StmtParser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: None,
            in_member_body: false,
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
