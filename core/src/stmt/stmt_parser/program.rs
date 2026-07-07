use super::StmtParser;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    stmt::{Attribute, Program, Stmt},
    token::{ParseError, Position, Span, Token, offset_to_position},
};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    /// 解析整个程序
    pub fn parse_program(&mut self) -> Result<Program> {
        let mut statements = Vec::new();

        while !self.eof() {
            // 跳过空语句
            if self.tokens[self.pos] == Token::Semicolon {
                statements.push(Box::new(Stmt::Empty));
                self.pos += 1;
                continue;
            }

            statements.push(Box::new(self.parse_statement()?));
        }

        Program::new(statements)
    }

    /// Parse program with enhanced error reporting
    pub fn parse_program_with_enhanced_errors(&mut self, input: &str) -> core::result::Result<Program, ParseError> {
        let mut statements = Vec::new();

        while !self.eof() {
            // 跳过空语句
            if self.tokens[self.pos] == Token::Semicolon {
                statements.push(Box::new(Stmt::Empty));
                self.pos += 1;
                continue;
            }

            let stmt = match self.parse_statement() {
                Ok(s) => s,
                Err(err) => {
                    // Prefer precise token span if available; otherwise, fall back to offset estimation
                    if let Some(spans) = &self.token_spans
                        && self.pos < spans.len()
                    {
                        return Err(ParseError::with_span(err.to_string(), spans[self.pos].clone()));
                    }
                    let position = offset_to_position(
                        input,
                        if self.pos < self.tokens.len() && self.pos > 0 {
                            self.pos * input.len() / self.tokens.len().max(1)
                        } else {
                            input.len()
                        },
                    );
                    return Err(ParseError::with_position(err.to_string(), position));
                }
            };
            statements.push(Box::new(stmt));
        }

        Program::new(statements).map_err(|e| {
            // If we have more tokens at current position, use its span; otherwise fallback to start
            if let Some(spans) = &self.token_spans
                && self.pos < spans.len()
            {
                return ParseError::with_span(e.to_string(), spans[self.pos].clone());
            }
            ParseError::with_position(
                e.to_string(),
                Position {
                    line: 0,
                    column: 0,
                    offset: 0,
                },
            )
        })
    }

    /// Recovering parse: continue after errors using simple synchronization points to collect multiple errors.
    /// Returns a flat list of statements (without label map validation) and a list of parse errors with spans.
    pub fn parse_program_recovering_with_enhanced_errors(&mut self, input: &str) -> (Vec<Box<Stmt>>, Vec<ParseError>) {
        let mut statements = Vec::new();
        let mut errors = Vec::new();

        while !self.eof() {
            // Skip standalone semicolons
            if self.tokens[self.pos] == Token::Semicolon {
                statements.push(Box::new(Stmt::Empty));
                self.pos += 1;
                continue;
            }

            match self.parse_statement() {
                Ok(stmt) => statements.push(Box::new(stmt)),
                Err(err) => {
                    // Build precise error using token span if possible
                    let parse_err = if let Some(spans) = &self.token_spans {
                        let span = if self.pos < spans.len() {
                            spans[self.pos].clone()
                        } else if !spans.is_empty() {
                            // Fallback to last known span
                            spans[spans.len() - 1].clone()
                        } else {
                            // Ultimate fallback to end-of-input position
                            let pos = offset_to_position(input, input.len());
                            Span::single(pos)
                        };
                        ParseError::with_span(err.to_string(), span)
                    } else {
                        // Estimate position if spans unavailable
                        let position = offset_to_position(
                            input,
                            if self.pos < self.tokens.len() && self.pos > 0 {
                                self.pos * input.len() / self.tokens.len().max(1)
                            } else {
                                input.len()
                            },
                        );
                        ParseError::with_position(err.to_string(), position)
                    };
                    errors.push(parse_err);

                    // Error recovery: advance to next sync point to avoid infinite loop.
                    // Sync when encountering a ';' at depth 0 (consume it) or an '}' that likely closes the current block.
                    // Track simple nesting for (), [], {} to avoid syncing mid-expression.
                    if !self.eof() {
                        // Ensure we always advance at least one token to make progress
                        self.pos = (self.pos + 1).min(self.len);
                    }
                    let mut paren: i32 = 0;
                    let mut bracket: i32 = 0;
                    let mut brace: i32 = 0;
                    let mut seen_block: bool = false;
                    while !self.eof() {
                        match self.tokens[self.pos] {
                            Token::LParen => {
                                paren += 1;
                                self.pos += 1;
                            }
                            Token::RParen => {
                                if paren > 0 {
                                    paren -= 1;
                                }
                                self.pos += 1;
                            }
                            Token::LBracket => {
                                bracket += 1;
                                self.pos += 1;
                            }
                            Token::RBracket => {
                                if bracket > 0 {
                                    bracket -= 1;
                                }
                                self.pos += 1;
                            }
                            Token::LBrace => {
                                brace += 1;
                                seen_block = true;
                                self.pos += 1;
                            }
                            Token::RBrace => {
                                // If we have seen a block start and this '}' closes it (brace would go from 1->0),
                                // break here to avoid skipping the following statement.
                                if seen_block && brace == 1 && paren == 0 && bracket == 0 {
                                    self.pos += 1; // consume '}'
                                    break;
                                }
                                if brace > 0 {
                                    brace -= 1;
                                }
                                self.pos += 1;
                            }
                            Token::Semicolon if paren == 0 && bracket == 0 && brace == 0 => {
                                self.pos += 1; // consume ';'
                                break;
                            }
                            _ => {
                                self.pos += 1;
                            }
                        }
                    }
                }
            }
        }

        (statements, errors)
    }

    /// 解析单个语句
    pub fn parse_statement(&mut self) -> Result<Stmt> {
        if self.eof() {
            return Ok(Stmt::Empty);
        }

        match &self.tokens[self.pos] {
            Token::Hash => self.parse_attributed_stmt(),
            Token::Use => self.parse_import_stmt(),
            Token::If => self.parse_if_stmt(),
            Token::Try => self.parse_try_stmt(),
            Token::Go => self.parse_go_stmt(),
            Token::While => self.parse_while_stmt(),
            Token::For => self.parse_for_stmt(),
            Token::Struct => self.parse_struct_stmt(),
            Token::Type => self.parse_type_alias_stmt(),
            Token::Trait => self.parse_trait_stmt(),
            Token::Impl => self.parse_impl_stmt(),
            Token::Let => self.parse_let_stmt(),
            Token::Const => self.parse_const_stmt(),
            Token::Break => self.parse_break_stmt(),
            Token::Continue => self.parse_continue_stmt(),
            Token::Return => self.parse_return_stmt(),
            Token::Fn => self.parse_function_stmt(),
            Token::LBrace => self.parse_block_stmt(),
            Token::Id(id) => {
                // 优先解析短声明 `id := expr` 以避免与标签 `id:` 冲突
                if self.peek_ahead(1) == Some(&Token::Colon) && self.peek_ahead(2) == Some(&Token::Assign) {
                    self.parse_define_stmt_with_id(id.clone())
                } else if matches!(self.peek_ahead(1), Some(Token::LBracket | Token::Dot))
                    && let Some(stmt) = self.try_parse_access_assign_stmt_with_id(id.clone())?
                {
                    Ok(stmt)
                } else if self.peek_ahead(1) == Some(&Token::Assign) {
                    // 赋值 (id = expr;)
                    self.parse_assign_stmt_with_id(id.clone())
                } else if matches!(
                    self.peek_ahead(1),
                    Some(&Token::AddAssign)
                        | Some(&Token::SubAssign)
                        | Some(&Token::MulAssign)
                        | Some(&Token::DivAssign)
                        | Some(&Token::ModAssign)
                ) {
                    self.parse_compound_assign_stmt_with_id(id.clone())
                } else if self.peek_ahead(1) == Some(&Token::Colon) {
                    // Label + statement (id: stmt) is not yet supported; treat as expression fallback
                    self.parse_expr_stmt()
                } else {
                    self.parse_expr_stmt()
                }
            }
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_attributed_stmt(&mut self) -> Result<Stmt> {
        let attributes = self.parse_attributes()?;
        let item = self.parse_statement()?;
        if !is_attribute_item(&item) {
            return Err(anyhow!(self.err("Attributes can only be applied to item declarations")));
        }
        Ok(Stmt::Attributed {
            attributes,
            item: Box::new(item),
        })
    }

    pub(super) fn parse_attributes(&mut self) -> Result<Vec<Attribute>> {
        let mut attributes = Vec::new();
        while !self.eof() && self.tokens[self.pos] == Token::Hash {
            let hash_index = self.pos;
            self.pos += 1;
            self.expect_token(Token::LBracket)?;
            let mut tokens = Vec::new();
            let mut bracket_depth = 1i32;
            let mut close_index = self.pos;
            while !self.eof() {
                close_index = self.pos;
                match &self.tokens[self.pos] {
                    Token::LBracket => {
                        bracket_depth += 1;
                        tokens.push(self.tokens[self.pos].clone());
                        self.pos += 1;
                    }
                    Token::RBracket => {
                        bracket_depth -= 1;
                        self.pos += 1;
                        if bracket_depth == 0 {
                            break;
                        }
                        tokens.push(Token::RBracket);
                    }
                    token => {
                        tokens.push(token.clone());
                        self.pos += 1;
                    }
                }
            }
            if bracket_depth != 0 {
                return Err(anyhow!(self.err("Unclosed attribute; expected ']'")));
            }
            if tokens.is_empty() {
                return Err(anyhow!(self.err("Attribute cannot be empty")));
            }
            attributes.push(Attribute {
                tokens,
                span: self.attribute_span(hash_index, close_index),
            });
        }
        Ok(attributes)
    }

    fn attribute_span(&self, start: usize, end: usize) -> Option<Span> {
        let spans = self.token_spans?;
        let start_span = spans.get(start)?;
        let end_span = spans.get(end).unwrap_or(start_span);
        Some(Span::new(start_span.start.clone(), end_span.end.clone()))
    }
}

fn is_attribute_item(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Attributed { .. }
            | Stmt::Function { .. }
            | Stmt::Struct { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Trait { .. }
            | Stmt::Impl { .. }
    )
}
