use super::StmtParser;
use crate::{ast::Parser as ExprParser, op::BinOp, stmt::Stmt, token::Token};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub fn parse_let_stmt(&mut self) -> Result<Stmt> {
        self.parse_binding_stmt(Token::Let, "let", false)
    }

    pub fn parse_const_stmt(&mut self) -> Result<Stmt> {
        self.parse_binding_stmt(Token::Const, "const", true)
    }

    fn parse_binding_stmt(&mut self, keyword: Token, keyword_str: &'static str, is_const: bool) -> Result<Stmt> {
        self.expect_token(keyword)?;

        // Parse pattern for binding statement until a top-level ':' (type annotation)
        // or '=' (assignment). Do NOT stop on ':' inside nested structures.
        let start_pos = self.pos;
        let mut end_pos = start_pos;
        let mut paren: i32 = 0;
        let mut bracket: i32 = 0;
        let mut brace: i32 = 0;

        while end_pos < self.len {
            match &self.tokens[end_pos] {
                Token::LParen => {
                    paren += 1;
                    end_pos += 1;
                }
                Token::RParen => {
                    if paren > 0 {
                        paren -= 1;
                    }
                    end_pos += 1;
                }
                Token::LBracket => {
                    bracket += 1;
                    end_pos += 1;
                }
                Token::RBracket => {
                    if bracket > 0 {
                        bracket -= 1;
                    }
                    end_pos += 1;
                }
                Token::LBrace => {
                    brace += 1;
                    end_pos += 1;
                }
                Token::RBrace => {
                    if brace > 0 {
                        brace -= 1;
                    }
                    end_pos += 1;
                }
                Token::Assign if paren == 0 && bracket == 0 && brace == 0 => {
                    break;
                }
                Token::Colon if paren == 0 && bracket == 0 && brace == 0 => {
                    break;
                }
                _ => {
                    end_pos += 1;
                }
            }
        }

        if end_pos == start_pos {
            return Err(anyhow!(self.err(&format!("Expected pattern after '{}'", keyword_str))));
        }

        // Use AST parser to parse the pattern
        let pattern_tokens = &self.tokens[start_pos..end_pos];
        let mut ast_parser = ExprParser::new(pattern_tokens);
        let pattern = ast_parser.parse_pattern()?;

        // Update position
        self.pos = end_pos;

        // Optional type annotation at top-level
        let type_annotation = if !self.eof() && self.tokens[self.pos] == Token::Colon {
            self.pos += 1; // consume ':'
            Some(self.parse_type_annotation()?)
        } else {
            None
        };

        self.expect_token(Token::Assign)?;

        let value = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;

        Ok(Stmt::Let {
            pattern,
            type_annotation,
            value: Box::new(value),
            span: self.current_span(),
            is_const,
        })
    }

    pub fn parse_assign_stmt_with_id(&mut self, name: String) -> Result<Stmt> {
        // 我们已经在parse_statement中匹配了Id，现在跳过它并继续解析赋值
        self.pos += 1; // 跳过已匹配的 Id token
        self.expect_token(Token::Assign)?;

        let value = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;

        Ok(Stmt::Assign {
            name,
            value: Box::new(value),
            span: self.current_span(),
        })
    }

    pub fn parse_compound_assign_stmt_with_id(&mut self, name: String) -> Result<Stmt> {
        // 我们已经在parse_statement中匹配了Id，现在跳过它并继续解析复合赋值
        self.pos += 1; // 跳过已匹配的 Id token

        // 获取复合赋值操作符
        let op = match &self.tokens[self.pos] {
            Token::AddAssign => BinOp::Add,
            Token::SubAssign => BinOp::Sub,
            Token::MulAssign => BinOp::Mul,
            Token::DivAssign => BinOp::Div,
            Token::ModAssign => BinOp::Mod,
            _ => return Err(anyhow!("Expected compound assignment operator")),
        };
        self.pos += 1; // 跳过复合赋值操作符

        let value = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;

        Ok(Stmt::CompoundAssign {
            name,
            op,
            value: Box::new(value),
            span: self.current_span(),
        })
    }

    pub fn parse_define_stmt_with_id(&mut self, name: String) -> Result<Stmt> {
        // consume Id (already peeked), ':' and '='
        self.pos += 1; // Id
        self.expect_token(Token::Colon)?;
        self.expect_token(Token::Assign)?;

        let value = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;
        Ok(Stmt::Define {
            name,
            value: Box::new(value),
        })
    }

    pub fn parse_break_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Break)?;
        self.expect_token(Token::Semicolon)?;
        Ok(Stmt::Break)
    }

    pub fn parse_continue_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Continue)?;
        self.expect_token(Token::Semicolon)?;
        Ok(Stmt::Continue)
    }

    pub fn parse_return_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Return)?;

        // 检查是否有返回值（如果下一个token不是分号，则有返回值）
        let value = if !self.eof() && self.tokens[self.pos] != Token::Semicolon {
            Some(Box::new(self.parse_expression()?))
        } else {
            None
        };

        self.expect_token(Token::Semicolon)?;

        Ok(Stmt::Return { value })
    }
}
