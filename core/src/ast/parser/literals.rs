#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::{Result, anyhow};

use crate::{ast::parser::Parser, expr::Expr, token::Token, val::LiteralVal};

impl<'a> Parser<'a> {
    /// Parse map literal: `{key: value, key: value, ...}`
    pub(super) fn parse_map(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::LBrace {
            let msg = format!("Expecting '{{', found {:?}", self.tokens[self.pos]);
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        let mut pairs = Vec::new();

        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(Expr::Map(pairs));
        }

        if !self.eof() {
            if !self.is_valid_expr_start() {
                let msg = format!("Invalid map key start: {:?}", self.tokens[self.pos]);
                return Err(anyhow!(self.err(&msg)));
            }

            let key = Box::new(self.parse_map_key()?);
            self.expect_map_colon()?;
            self.ensure_map_value_start()?;

            let value = Box::new(self.parse_expr()?);
            pairs.push((key, value));

            while !self.eof() {
                match self.tokens[self.pos] {
                    Token::Comma => {
                        self.pos += 1;
                        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
                            break;
                        }

                        if self.eof() || !self.is_valid_expr_start() {
                            return Err(anyhow!(self.err("Invalid map key after comma")));
                        }

                        let key = Box::new(self.parse_map_key()?);
                        self.expect_map_colon()?;
                        self.ensure_map_value_start()?;

                        let value = Box::new(self.parse_expr()?);
                        pairs.push((key, value));
                    }
                    Token::RBrace => break,
                    _ => {
                        let msg = format!("Expecting ',' or '}}', found {:?}", self.tokens[self.pos]);
                        return Err(anyhow!(self.err(&msg)));
                    }
                }
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::RBrace {
            return Err(anyhow!(self.err("Expecting '}'")));
        }
        self.pos += 1;

        Ok(Expr::Map(pairs))
    }

    fn parse_map_key(&mut self) -> Result<Expr> {
        if self.pos + 1 < self.len
            && matches!(self.tokens[self.pos + 1], Token::Colon)
            && let Token::Id(id) = &self.tokens[self.pos]
        {
            let key = Expr::Literal(LiteralVal::from_str(id.as_str()));
            self.pos += 1;
            return Ok(key);
        }
        self.parse_expr()
    }

    fn expect_map_colon(&mut self) -> Result<()> {
        if self.eof() || self.tokens[self.pos] != Token::Colon {
            return Err(anyhow!(self.err("Expecting ':'")));
        }
        self.pos += 1;
        Ok(())
    }

    fn ensure_map_value_start(&self) -> Result<()> {
        if self.eof() || !self.is_valid_expr_start() {
            return Err(anyhow!(self.err("Invalid map value after ':'")));
        }
        Ok(())
    }

    /// Parse field name for .field and ?.field access - treats IDs as string literals
    pub(super) fn parse_field_name(&mut self) -> Result<Expr> {
        match &self.tokens[self.pos] {
            Token::Id(id) => {
                let expr = Expr::Literal(LiteralVal::from_str(id.as_str()));
                self.pos += 1;
                Ok(expr)
            }
            Token::Str(s) => {
                let expr = Expr::Literal(LiteralVal::from_str(s.as_str()));
                self.pos += 1;
                Ok(expr)
            }
            Token::Int(i) => {
                let expr = Expr::Literal(LiteralVal::Int(*i));
                self.pos += 1;
                Ok(expr)
            }
            _ => {
                let msg = format!("Invalid field name: {:?}", &self.tokens[self.pos]);
                Err(anyhow!(self.err(&msg)))
            }
        }
    }
}
