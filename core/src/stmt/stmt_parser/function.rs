use super::StmtParser;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::Expr,
    stmt::{NamedParamDecl, Stmt},
    token::Token,
    val::Type,
};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub fn parse_function_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Fn)?;

        let name = if let Token::Id(id) = &self.tokens[self.pos] {
            let name = id.clone();
            self.pos += 1;
            name
        } else if let Some(word) = self
            .in_member_body
            .then(|| crate::token::keyword_as_name(&self.tokens[self.pos]))
            .flatten()
        {
            // A method name, not a global one — reached through `.`, so a
            // keyword says it unambiguously (`db.select()`).
            self.pos += 1;
            word.to_string()
        } else if let Some(word) = crate::token::keyword_as_name(&self.tokens[self.pos]) {
            // Refused, but in the language's words: the reader needs to know it
            // is a keyword and where one *is* allowed.
            return Err(anyhow!(self.err(&alloc::format!(
                "`{word}` is a keyword, so it cannot name a top-level function — a call to one is a bare name, where `{word}(…)` could not be told from the `{word}` statement. It *can* name a method or a field"
            ))));
        } else {
            return Err(anyhow!(self.err("Expected function name")));
        };

        self.expect_token(Token::LParen)?;
        let mut params: Vec<String> = Vec::new();
        let mut param_types: Vec<Option<Type>> = Vec::new();
        let mut named_params: Vec<NamedParamDecl> = Vec::new();
        let mut saw_named_block = false;
        let mut saw_default_positional = false;

        while !self.eof() && self.tokens[self.pos] != Token::RParen {
            // The named-parameter block, which must follow the positional
            // parameters.
            if self.tokens[self.pos] == Token::LBrace {
                if saw_named_block {
                    return Err(anyhow!(self.err("Duplicate named parameter block")));
                }
                saw_named_block = true;
                let named = self.parse_named_param_block()?;
                named_params.extend(named);
                // A comma may follow the block.
                if !self.eof() && self.tokens[self.pos] == Token::Comma {
                    self.pos += 1;
                }
                continue;
            }

            if saw_named_block {
                return Err(anyhow!(
                    self.err("Positional parameters cannot follow named parameter block")
                ));
            }

            let param_name = if let Token::Id(param) = &self.tokens[self.pos] {
                let p = param.clone();
                self.pos += 1;
                p
            } else {
                return Err(anyhow!(self.err("Expected parameter name or '{' for named block")));
            };

            let mut parsed_type: Option<Type> = None;
            if !self.eof() && self.tokens[self.pos] == Token::Colon {
                self.pos += 1; // consume ':'
                let ty = self.parse_inline_type_until_param_delim()?;
                parsed_type = Some(ty);
            }

            if !self.eof() && self.tokens[self.pos] == Token::Assign {
                saw_default_positional = true;
                self.pos += 1;
                let default_expr = self.parse_inline_expr_until_param_delim()?;
                named_params.push(NamedParamDecl {
                    name: param_name,
                    type_annotation: Some(parsed_type.unwrap_or(Type::Any)),
                    default: Some(default_expr),
                });
            } else {
                if saw_default_positional {
                    return Err(anyhow!(self.err(
                        "Required positional parameters cannot follow default positional parameters"
                    )));
                }
                params.push(param_name);
                param_types.push(parsed_type);
            }

            if !self.eof() && self.tokens[self.pos] == Token::Comma {
                self.pos += 1;
            } else if !self.eof() && self.tokens[self.pos] == Token::RParen {
                // end of params
            } else if self.eof() {
                return Err(anyhow!(self.err("Unexpected end while parsing parameters")));
            } else {
                return Err(anyhow!(self.err("Expected ',' or ')' in parameter list")));
            }
        }

        self.expect_token(Token::RParen)?;

        let mut return_type: Option<Type> = None;
        if !self.eof() && self.tokens[self.pos] == Token::FnArrow {
            self.pos += 1; // consume '->'
            let ty = self.parse_inline_type_until_block_start()?;
            return_type = Some(ty);
        }

        // The body has to be a block.
        let body = Box::new(self.parse_block_stmt()?);

        Ok(Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            return_type,
            body,
        })
    }

    /// Parses a named-parameter block: `{a: T, b: ?U = default}`.
    pub fn parse_named_param_block(&mut self) -> Result<Vec<NamedParamDecl>> {
        self.expect_token(Token::LBrace)?;
        let mut named_params: Vec<NamedParamDecl> = Vec::new();

        // An empty block is allowed.
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(named_params);
        }

        loop {
            let name = if let Token::Id(id) = &self.tokens[self.pos] {
                let n = id.clone();
                self.pos += 1;
                n
            } else {
                return Err(anyhow!(self.err("Expected identifier in named parameter block")));
            };

            self.expect_token(Token::Colon)?;
            let ty = self.parse_inline_type_until_named_delim()?;

            let mut default_expr: Option<Expr> = None;
            if !self.eof() && self.tokens[self.pos] == Token::Assign {
                self.pos += 1; // consume '='
                let expr = self.parse_inline_expr_until_named_delim()?;
                default_expr = Some(expr);
            }

            named_params.push(NamedParamDecl {
                name,
                type_annotation: Some(ty),
                default: default_expr,
            });

            if self.eof() {
                return Err(anyhow!(self.err("Unexpected end in named parameter block")));
            }
            match &self.tokens[self.pos] {
                Token::Comma => {
                    self.pos += 1;
                    // A trailing comma is allowed.
                    if !self.eof() && self.tokens[self.pos] == Token::RBrace {
                        self.pos += 1;
                        break;
                    }
                    continue;
                }
                Token::RBrace => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(anyhow!(self.err("Expected ',' or '}' in named parameter block")));
                }
            }
        }

        Ok(named_params)
    }

    /// Parses a parameter type up to the ',' or '}' at depth 0, leaving the
    /// separator unconsumed.
    pub fn parse_inline_type_until_named_delim(&mut self) -> Result<Type> {
        let start_pos = self.pos;
        let mut tokens: Vec<&Token> = Vec::new();
        let mut paren: i32 = 0;
        let mut bracket: i32 = 0;
        let mut brace: i32 = 0;
        let mut angle: i32 = 0;

        while !self.eof() {
            let t = &self.tokens[self.pos];
            match t {
                Token::LParen => {
                    paren += 1;
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::RParen => {
                    if paren > 0 {
                        paren -= 1;
                    }
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::LBracket => {
                    bracket += 1;
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::RBracket => {
                    if bracket > 0 {
                        bracket -= 1;
                    }
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::LBrace => {
                    brace += 1;
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::RBrace => {
                    if brace == 0 {
                        break;
                    }
                    brace -= 1;
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::Lt => {
                    angle += 1;
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::Gt => {
                    if angle > 0 {
                        angle -= 1;
                    }
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::Comma if paren == 0 && bracket == 0 && brace == 0 && angle == 0 => {
                    break;
                }
                Token::Assign if paren == 0 && bracket == 0 && brace == 0 && angle == 0 => {
                    break;
                }
                _ => {
                    tokens.push(t);
                    self.pos += 1;
                }
            }
        }

        if tokens.is_empty() {
            self.pos = start_pos;
            return Err(anyhow!(self.err("Expected type annotation")));
        }

        let type_str = self.tokens_to_type_string(&tokens);
        if let Some(ty) = Type::parse(&type_str) {
            return Ok(ty);
        }
        // Rewind to the type's first token before reporting: the collector
        // stopped at whatever ended the annotation, and both the `found …`
        // context and the span come from the position — so without this the
        // message pointed at the `,` or the `)` that is not the problem.
        self.pos = start_pos;
        let message = crate::type_syntax::function_type_hint(self.tokens, start_pos)
            .map_or_else(|| alloc::format!("Invalid type: {type_str}"), String::from);
        Err(anyhow!(self.err(&message)))
    }
}
