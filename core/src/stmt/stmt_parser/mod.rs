use crate::token::{Span, Token};

pub struct StmtParser<'a> {
    pub(crate) tokens: &'a [Token],
    pub(crate) pos: usize,
    pub(crate) len: usize,
    pub(crate) token_spans: Option<&'a [Span]>,
}

impl<'a> StmtParser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: None,
        }
    }

    pub fn new_with_spans(tokens: &'a [Token], spans: &'a [Span]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: Some(spans),
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
