#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::{Result, anyhow};

use super::Parser;
use crate::{
    expr::Expr,
    stmt::{Stmt, StmtParser},
    token::{ParseError, Span, Token},
};

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: None,
            prefix_mode: false,
        }
    }

    /// Create a parser with token spans for precise error reporting.
    pub fn new_with_spans(tokens: &'a [Token], spans: &'a [Span]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: Some(spans),
            prefix_mode: false,
        }
    }

    /// Parse `fn(params) => expr` style closure literal.
    pub(super) fn parse_fn_closure(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::Fn {
            return Err(anyhow!(self.err("Expected 'fn'")));
        }
        self.pos += 1;

        if self.eof() || self.tokens[self.pos] != Token::LParen {
            return Err(anyhow!(self.err("Expected '(' after 'fn'")));
        }
        self.pos += 1;

        let mut params = Vec::new();

        if !self.eof() && self.tokens[self.pos] != Token::RParen {
            loop {
                let param_name = if let Token::Id(name) = &self.tokens[self.pos] {
                    let name = name.clone();
                    self.pos += 1;
                    name
                } else {
                    return Err(anyhow!(self.err("Expected parameter name in fn literal")));
                };
                params.push(param_name);

                if !self.eof() && self.tokens[self.pos] == Token::Colon {
                    self.pos += 1;
                    self.skip_type_annotation()?;
                }

                if self.eof() {
                    return Err(anyhow!(self.err("Unexpected end in fn parameter list")));
                }

                match self.tokens[self.pos] {
                    Token::Comma => {
                        self.pos += 1;
                        if self.eof() {
                            return Err(anyhow!(self.err("Expected parameter after ',' in fn literal")));
                        }
                    }
                    Token::RParen => break,
                    _ => return Err(anyhow!(self.err("Expected ',' or ')' after parameter in fn literal"))),
                }
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::RParen {
            return Err(anyhow!(self.err("Expected ')' after fn parameters")));
        }
        self.pos += 1;

        if self.eof() || self.tokens[self.pos] != Token::Arrow {
            return Err(anyhow!(self.err("Expected '=>' after fn parameters")));
        }
        self.pos += 1;

        if self.eof() || !self.is_valid_expr_start() {
            return Err(anyhow!(self.err("Expected expression after '=>' in fn literal")));
        }

        let body = self.parse_expr()?;
        Ok(Expr::Closure {
            params,
            body: Box::new(body),
        })
    }

    /// Parse closure expression: `|param1, param2| expr`.
    pub(super) fn parse_closure(&mut self) -> Result<Expr> {
        self.pos += 1;
        let mut params = Vec::new();

        if !self.eof() && self.tokens[self.pos] != Token::Pipe {
            if let Token::Id(param_name) = &self.tokens[self.pos] {
                params.push(param_name.clone());
                self.pos += 1;
            } else {
                return Err(anyhow!(
                    self.err("Expected parameter name or '|' after opening '|' in closure")
                ));
            }

            while !self.eof() && self.tokens[self.pos] == Token::Comma {
                self.pos += 1;
                if let Token::Id(param_name) = &self.tokens[self.pos] {
                    params.push(param_name.clone());
                    self.pos += 1;
                } else {
                    return Err(anyhow!(self.err("Expected parameter name after comma in closure")));
                }
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::Pipe {
            return Err(anyhow!(self.err("Expected '|' to close parameter list in closure")));
        }
        self.pos += 1;

        if self.eof() || !self.is_valid_expr_start() {
            return Err(anyhow!(self.err("Expected expression after closure parameters")));
        }

        let body = if !self.eof() && self.tokens[self.pos] == Token::LBrace {
            self.parse_closure_block_expr()?
        } else {
            self.parse_expr()?
        };
        Ok(Expr::Closure {
            params,
            body: Box::new(body),
        })
    }

    fn parse_closure_block_expr(&mut self) -> Result<Expr> {
        self.pos += 1;
        let start = self.pos;
        let mut paren = 0usize;
        let mut bracket = 0usize;
        let mut brace = 0usize;
        while !self.eof() {
            match self.tokens[self.pos] {
                Token::LParen => {
                    paren += 1;
                    self.pos += 1;
                }
                Token::RParen => {
                    if paren == 0 {
                        return Err(anyhow!(self.err("Mismatched ')' in closure block")));
                    }
                    paren -= 1;
                    self.pos += 1;
                }
                Token::LBracket => {
                    bracket += 1;
                    self.pos += 1;
                }
                Token::RBracket => {
                    if bracket == 0 {
                        return Err(anyhow!(self.err("Mismatched ']' in closure block")));
                    }
                    bracket -= 1;
                    self.pos += 1;
                }
                Token::LBrace => {
                    brace += 1;
                    self.pos += 1;
                }
                Token::RBrace if brace == 0 => {
                    if paren != 0 || bracket != 0 {
                        return Err(anyhow!(self.err("Expected matching bracket before closure block end")));
                    }
                    break;
                }
                Token::RBrace => {
                    brace -= 1;
                    self.pos += 1;
                }
                _ => self.pos += 1,
            }
        }
        if self.eof() {
            return Err(anyhow!(self.err("Expected '}' to close closure block")));
        }
        let end = self.pos;
        self.pos += 1;

        let mut inner = Vec::with_capacity(end - start + 1);
        inner.extend_from_slice(&self.tokens[start..end]);
        if inner.is_empty() {
            return Ok(Expr::Literal(crate::val::LiteralVal::Nil));
        }
        if !matches!(inner.last(), Some(Token::Semicolon)) {
            inner.push(Token::Semicolon);
        }
        let mut stmt_parser = StmtParser::new(&inner);
        let program = stmt_parser.parse_program()?;
        let mut statements = program.statements;
        if let Some(last) = statements.last_mut()
            && let Stmt::Expr(expr) = last.as_ref()
        {
            let value = expr.clone();
            **last = Stmt::Return { value: Some(value) };
        }
        Ok(Expr::Block(statements))
    }

    /// Skip a type annotation after ':' in a parameter list.
    pub(super) fn skip_type_annotation(&mut self) -> Result<()> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected type after ':'")));
        }

        let mut consumed_any = false;
        let mut paren_depth = 0;
        let mut bracket_depth = 0;
        let mut angle_depth = 0;

        while !self.eof() {
            let token = &self.tokens[self.pos];
            match token {
                Token::Comma if paren_depth == 0 && bracket_depth == 0 && angle_depth == 0 => break,
                Token::RParen if paren_depth == 0 && bracket_depth == 0 && angle_depth == 0 => break,
                Token::LParen => {
                    paren_depth += 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::RParen => {
                    if paren_depth == 0 {
                        break;
                    }
                    paren_depth -= 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::LBracket => {
                    bracket_depth += 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::RBracket => {
                    if bracket_depth == 0 {
                        return Err(anyhow!(self.err("Unmatched ']' in type annotation")));
                    }
                    bracket_depth -= 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::Lt => {
                    angle_depth += 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::Gt => {
                    if angle_depth == 0 {
                        break;
                    }
                    angle_depth -= 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                _ => {
                    consumed_any = true;
                    self.pos += 1;
                }
            }
        }

        if !consumed_any {
            return Err(anyhow!(self.err("Expected type after ':'")));
        }

        Ok(())
    }

    pub(super) fn eof(&self) -> bool {
        self.pos >= self.len
    }

    pub(super) fn err(&self, msg: &str) -> String {
        let ctx = if let Some(token) = self.tokens.get(self.pos) {
            format!("found {:?}", token)
        } else {
            "found end of input".to_string()
        };
        format!("Syntax error: {} ({})", msg, ctx)
    }

    /// Recovering expression analysis: collect multiple parse errors across expression segments.
    pub fn recover_expression_errors(tokens: &'a [Token], spans: &'a [Span], input: &str) -> Vec<ParseError> {
        let mut errors = Vec::new();
        let len = tokens.len();
        let mut index = 0usize;

        fn is_hard_boundary(token: &Token) -> bool {
            matches!(
                token,
                Token::Comma | Token::Semicolon | Token::RParen | Token::RBracket | Token::RBrace | Token::Else
            )
        }

        fn is_soft_boundary(token: &Token) -> bool {
            matches!(
                token,
                Token::Eq
                    | Token::Ne
                    | Token::Gt
                    | Token::Lt
                    | Token::Ge
                    | Token::Le
                    | Token::In
                    | Token::And
                    | Token::Or
            )
        }

        while index < len {
            while index < len && is_hard_boundary(&tokens[index]) {
                index += 1;
            }
            if index >= len {
                break;
            }

            let segment_start = index;
            let mut cursor = index;
            let mut paren = 0;
            let mut bracket = 0;
            let mut brace = 0;
            while cursor < len {
                match &tokens[cursor] {
                    Token::LParen => paren += 1,
                    Token::RParen => {
                        if paren > 0 {
                            paren -= 1;
                        }
                        if paren == 0 && bracket == 0 && brace == 0 {
                            cursor += 1;
                            break;
                        }
                    }
                    Token::LBracket => bracket += 1,
                    Token::RBracket => {
                        if bracket > 0 {
                            bracket -= 1;
                        }
                        if paren == 0 && bracket == 0 && brace == 0 {
                            cursor += 1;
                            break;
                        }
                    }
                    Token::LBrace => brace += 1,
                    Token::RBrace => {
                        if brace > 0 {
                            brace -= 1;
                        }
                        if paren == 0 && bracket == 0 && brace == 0 {
                            break;
                        }
                    }
                    token if is_hard_boundary(token) => break,
                    token if is_soft_boundary(token) && paren == 0 && bracket == 0 && brace == 0 => break,
                    _ => {}
                }
                cursor += 1;
            }
            if cursor == segment_start {
                index = cursor + 1;
                continue;
            }

            let segment_tokens = &tokens[segment_start..cursor];
            let segment_spans = &spans[segment_start..cursor];
            if !segment_tokens.is_empty() {
                let mut parser = Parser::new_with_spans(segment_tokens, segment_spans);
                if let Err(error) = parser.parse_with_enhanced_errors(input) {
                    errors.push(error);
                }
            }

            index = cursor;
            if index < len && (is_soft_boundary(&tokens[index]) || is_hard_boundary(&tokens[index])) {
                index += 1;
            }
        }

        errors
    }

    /// Check if the current token can start a valid expression.
    pub(super) fn is_valid_expr_start(&self) -> bool {
        if self.eof() {
            return false;
        }

        matches!(
            self.tokens[self.pos],
            Token::Nil
                | Token::Bool(_)
                | Token::Int(_)
                | Token::Float(_)
                | Token::Str(_)
                | Token::Id(_)
                | Token::LBracket
                | Token::LBrace
                | Token::LParen
                | Token::Not
                | Token::BitNot
                | Token::Select
                | Token::Pipe
                | Token::Fn
        )
    }

    /// Check if the current token is an invalid separator.
    pub(super) fn is_invalid_separator(&self) -> bool {
        if self.eof() {
            return false;
        }

        matches!(self.tokens[self.pos], Token::Semicolon)
    }
}
