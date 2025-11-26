use super::StmtParser;
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
        let expr = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;
        Ok(Stmt::Expr(Box::new(expr)))
    }

    pub fn parse_expression(&mut self) -> Result<Expr> {
        self.parse_expression_with_options(false)
    }

    pub fn parse_expression_with_options(&mut self, stop_at_for_loop_body: bool) -> Result<Expr> {
        // 找到表达式的结束位置
        let start_pos = self.pos;
        let mut depth = 0;
        let mut end_pos = start_pos;

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
                }
                Token::RBracket => {
                    depth -= 1;
                    end_pos += 1;
                }
                Token::Semicolon if depth == 0 => {
                    break;
                }
                Token::Else if depth == 0 => {
                    break;
                }
                _ => {
                    end_pos += 1;
                }
            }
        }

        if end_pos == start_pos {
            return Err(anyhow!(self.err("Expected expression")));
        }

        // 使用表达式解析器解析这部分 tokens
        let expr_tokens = &self.tokens[start_pos..end_pos];
        let mut expr_parser = ExprParser::new(expr_tokens);
        let expr = expr_parser.parse()?;

        // 更新位置
        self.pos = end_pos;

        Ok(expr)
    }
}
