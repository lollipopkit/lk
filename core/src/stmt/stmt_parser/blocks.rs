use super::StmtParser;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{ast::Parser as ExprParser, expr::Expr, stmt::Stmt, token::Token};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub fn parse_block_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::LBrace)?;

        let mut statements = Vec::new();
        while !self.eof() && self.tokens[self.pos] != Token::RBrace {
            // 跳过空语句
            if self.tokens[self.pos] == Token::Semicolon {
                statements.push(Box::new(Stmt::Empty));
                self.pos += 1;
                continue;
            }

            let stmt = self.parse_statement()?;
            statements.push(Box::new(stmt));
        }

        self.expect_token(Token::RBrace)?;

        Ok(Stmt::Block { statements })
    }

    pub fn parse_expr_stmt(&mut self) -> Result<Stmt> {
        // Taken before the expression is parsed, and ended before the `;`: this
        // is the one statement whose span exists so that an *argument* type
        // error has somewhere to point. `f("x");` used to report the mistake
        // with no position at all, because a bare call is a `Stmt::Expr` and
        // that was the only variant carrying none.
        let start_pos = self.pos;
        let expr = self.parse_statement_expression()?;
        // An expression that *ends in a block* needs no `;`.
        //
        // `if c { … }` never did, because it was a statement. `match x { … }`
        // and `unsafe { … }` are expressions, so they did — the same shape on
        // the page, one of them demanding punctuation the other refuses.
        // Ending in `}` is the whole rule, so a construct added later inherits
        // it instead of joining the exception list.
        let ends_in_block = self.pos > 0 && self.tokens[self.pos - 1] == Token::RBrace;
        if ends_in_block {
            if !self.eof() && self.tokens[self.pos] == Token::Semicolon {
                self.pos += 1;
            }
        } else {
            self.expect_token(Token::Semicolon)?;
        }
        Ok(Stmt::Expr {
            value: Box::new(expr),
            span: self.span_covering(start_pos, self.pos.saturating_sub(1)),
        })
    }

    pub fn parse_expression(&mut self) -> Result<Expr> {
        self.parse_expression_with_options(false)
    }

    pub fn parse_expression_with_options(&mut self, stop_at_for_loop_body: bool) -> Result<Expr> {
        self.parse_expression_slice(stop_at_for_loop_body, false)
    }

    /// The expression of an expression *statement*.
    ///
    /// Differs from [`Self::parse_expression`] in one way: a leading braced
    /// construct ends the expression at its closing `}`, because that is where
    /// the statement ends. As an operand it must not — `return match x { … }
    /// == nil;` compares the match's value, and stopping at the brace would
    /// silently drop the comparison.
    fn parse_statement_expression(&mut self) -> Result<Expr> {
        self.parse_expression_slice(false, true)
    }

    fn parse_expression_slice(&mut self, stop_at_for_loop_body: bool, end_at_block: bool) -> Result<Expr> {
        // 找到表达式的结束位置
        let start_pos = self.pos;
        let mut depth = 0;
        let mut end_pos = start_pos;
        // `if` is an expression, so a top-level `else` can belong to the
        // expression being sliced rather than to an enclosing `if` *statement*.
        // Count the unmatched `if`s seen so far and hand the `else` to the
        // nearest one; only a genuinely dangling `else` ends the slice, which
        // is what this used to assume unconditionally.
        let mut unmatched_ifs = 0usize;
        // An expression that *begins* with a braced construct also *ends* at
        // that construct's closing `}` when it stands as a statement:
        // `match x { … } println("next");` is two statements, not one
        // expression with leftover tokens. Conditions never reach this — they
        // stop at the `{` that opens the body (`stop_at_for_loop_body`).
        let starts_with_block_expr = end_at_block
            && matches!(
                self.tokens.get(start_pos),
                Some(Token::Match | Token::Unsafe | Token::If)
            );

        while end_pos < self.len {
            let token = &self.tokens[end_pos];

            match token {
                Token::LBrace if depth == 0 && stop_at_for_loop_body => {
                    break; // for循环体的开始
                }
                Token::LParen | Token::LBrace | Token::LBracket => {
                    depth += 1;
                    end_pos += 1;
                }
                Token::RParen => {
                    if depth == 0 {
                        break; // 条件表达式的结束
                    }
                    depth -= 1;
                    end_pos += 1;
                }
                Token::RBrace => {
                    if depth == 0 {
                        break; // 块的结束
                    }
                    depth -= 1;
                    end_pos += 1;
                    // The construct this expression opened with just closed.
                    // An `else` may still follow an `if`; nothing else can.
                    if depth == 0 && starts_with_block_expr && !matches!(self.tokens.get(end_pos), Some(Token::Else)) {
                        break;
                    }
                }
                Token::RBracket => {
                    depth -= 1;
                    end_pos += 1;
                }
                Token::Semicolon if depth == 0 => {
                    break;
                }
                Token::If if depth == 0 => {
                    unmatched_ifs += 1;
                    end_pos += 1;
                }
                Token::Else if depth == 0 => {
                    if unmatched_ifs == 0 {
                        break;
                    }
                    unmatched_ifs -= 1;
                    end_pos += 1;
                }
                _ => {
                    end_pos += 1;
                }
            }
        }

        if end_pos == start_pos {
            // In a `while`/`for` header the only expression that can start with
            // `{` is a map literal, and this `{` is the body's. Say the way out
            // rather than only that this is wrong — the same wording `if` and
            // `match` use (`ast::Parser::parse_header_expr_before_brace`).
            if stop_at_for_loop_body && matches!(self.tokens.get(start_pos), Some(Token::LBrace)) {
                return Err(anyhow!(self.err(
                    "Expected an expression before '{': a `{` here opens the body, \
                     so a map literal must be parenthesised — `({…}) { … }`"
                )));
            }
            return Err(anyhow!(self.err("Expected expression")));
        }

        // 使用表达式解析器解析这部分 tokens
        let expr_tokens = &self.tokens[start_pos..end_pos];
        let expr_spans = self.token_spans.map(|spans| &spans[start_pos..end_pos]);
        let mut expr_parser = if let Some(spans) = expr_spans {
            ExprParser::new_with_spans(expr_tokens, spans)
        } else {
            ExprParser::new(expr_tokens)
        };
        let expr = expr_parser.parse()?;

        // 更新位置
        self.pos = end_pos;

        Ok(expr)
    }
}
