use super::StmtParser;
use crate::{
    stmt::{ForPattern, Stmt},
    token::Token,
};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    /// 解析 if 语句
    pub fn parse_if_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::If)?;

        // Check if this is an "if let" statement
        if !self.eof() && self.tokens[self.pos] == Token::Let {
            self.pos += 1; // consume 'let'

            // Parse the pattern
            let pattern = self.parse_pattern()?;

            // Expect '='
            self.expect_token(Token::Assign)?;

            // Parse the value expression (stop at LBrace for if let body)
            let value = self.parse_expression_with_options(true)?;

            // Parse then statement (no parentheses for if let)
            let then_stmt = Box::new(self.parse_statement()?);

            // Parse optional else statement
            let else_stmt = if !self.eof() && self.tokens[self.pos] == Token::Else {
                self.pos += 1;
                Some(Box::new(self.parse_statement()?))
            } else {
                None
            };

            Ok(Stmt::IfLet {
                pattern,
                value: Box::new(value),
                then_stmt,
                else_stmt,
            })
        } else {
            // Regular if statement
            let condition = if !self.eof() && self.tokens[self.pos] == Token::LParen {
                // Standard form: if (cond) stmt
                self.pos += 1; // consume '('
                let cond = self.parse_expression()?;
                self.expect_token(Token::RParen)?;
                cond
            } else {
                // Also support: if cond { ... } (without parentheses)
                // Stop parsing the condition at '{' when at top-level
                self.parse_expression_with_options(true)?
            };

            let then_stmt = Box::new(self.parse_statement()?);

            let else_stmt = if !self.eof() && self.tokens[self.pos] == Token::Else {
                self.pos += 1;
                Some(Box::new(self.parse_statement()?))
            } else {
                None
            };

            Ok(Stmt::If {
                condition: Box::new(condition),
                then_stmt,
                else_stmt,
            })
        }
    }

    /// 解析 while 语句
    pub fn parse_while_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::While)?;

        // Check if this is a "while let" statement
        if !self.eof() && self.tokens[self.pos] == Token::Let {
            self.pos += 1; // consume 'let'

            // Parse the pattern
            let pattern = self.parse_pattern()?;

            // Expect '='
            self.expect_token(Token::Assign)?;

            // Parse the value expression (stop at LBrace for while let body)
            let value = self.parse_expression_with_options(true)?;

            // Parse body statement (no parentheses for while let)
            let body = Box::new(self.parse_statement()?);

            Ok(Stmt::WhileLet {
                pattern,
                value: Box::new(value),
                body,
            })
        } else {
            // Regular while statement
            self.expect_token(Token::LParen)?;

            let condition = self.parse_expression()?;

            self.expect_token(Token::RParen)?;
            let body = Box::new(self.parse_statement()?);

            Ok(Stmt::While {
                condition: Box::new(condition),
                body,
            })
        }
    }

    /// 解析 for 语句
    pub fn parse_for_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::For)?; // 消费 'for'

        // 解析模式 (变量名或解构)
        let pattern = self.parse_for_pattern()?;

        self.expect_token(Token::In)?; // 消费 'in'

        // 解析可迭代表达式 - 在for循环中遇到LBrace时停止
        let iterable = self.parse_expression_with_options(true)?;

        // 解析循环体
        let body = Box::new(self.parse_statement()?);

        Ok(Stmt::For {
            pattern,
            iterable: Box::new(iterable),
            body,
        })
    }

    /// 解析 for 循环的模式
    pub fn parse_for_pattern(&mut self) -> Result<ForPattern> {
        match &self.tokens[self.pos] {
            // 忽略模式: _
            Token::Id(name) if name == "_" => {
                self.pos += 1;
                Ok(ForPattern::Ignore)
            }
            // 简单变量: identifier
            Token::Id(name) => {
                let var_name = name.clone();
                self.pos += 1;
                Ok(ForPattern::Variable(var_name))
            }
            // 元组模式: (a, b, c)
            Token::LParen => {
                self.pos += 1; // 消费 '('
                let mut patterns = Vec::new();

                // 处理空元组 ()
                if !self.eof() && self.tokens[self.pos] == Token::RParen {
                    self.pos += 1;
                    return Ok(ForPattern::Tuple(patterns));
                }

                loop {
                    patterns.push(self.parse_for_pattern()?);

                    if self.eof() {
                        return Err(anyhow!(self.err("Expected ')' in tuple pattern")));
                    }

                    match &self.tokens[self.pos] {
                        Token::Comma => {
                            self.pos += 1; // 消费 ','
                            // 允许尾随逗号: (a, b,)
                            if !self.eof() && self.tokens[self.pos] == Token::RParen {
                                break;
                            }
                            continue;
                        }
                        Token::RParen => break,
                        _ => return Err(anyhow!(self.err("Expected ',' or ')' in tuple pattern"))),
                    }
                }

                self.pos += 1; // 消费 ')'
                Ok(ForPattern::Tuple(patterns))
            }
            // 数组模式: [a, b] 或 [a, b, ..rest]
            Token::LBracket => {
                self.pos += 1; // 消费 '['
                let mut patterns = Vec::new();
                let mut rest = None;

                // 处理空数组 []
                if !self.eof() && self.tokens[self.pos] == Token::RBracket {
                    self.pos += 1;
                    return Ok(ForPattern::Array { patterns, rest });
                }

                loop {
                    // 检查剩余模式 ..
                    if !self.eof() && self.tokens[self.pos] == Token::Range {
                        self.pos += 1; // 消费 '..'

                        // 可选的剩余变量名
                        if !self.eof()
                            && let Token::Id(name) = &self.tokens[self.pos]
                        {
                            rest = Some(name.clone());
                            self.pos += 1;
                        }

                        // 剩余模式后不能再有其他模式
                        if self.eof() {
                            return Err(anyhow!(self.err("Expected ']' after rest pattern")));
                        }

                        match &self.tokens[self.pos] {
                            Token::RBracket => break,
                            Token::Comma => {
                                self.pos += 1;
                                if !self.eof() && self.tokens[self.pos] == Token::RBracket {
                                    break;
                                } else {
                                    return Err(anyhow!(self.err("No patterns allowed after rest pattern")));
                                }
                            }
                            _ => {
                                return Err(anyhow!(self.err("Expected ']' or ',' after rest pattern")));
                            }
                        }
                    } else {
                        patterns.push(self.parse_for_pattern()?);
                    }

                    if self.eof() {
                        return Err(anyhow!(self.err("Expected ']' in array pattern")));
                    }

                    match &self.tokens[self.pos] {
                        Token::Comma => {
                            self.pos += 1; // 消费 ','
                            // 允许尾随逗号: [a, b,]
                            if !self.eof() && self.tokens[self.pos] == Token::RBracket {
                                break;
                            }
                            continue;
                        }
                        Token::RBracket => break,
                        _ => return Err(anyhow!(self.err("Expected ',' or ']' in array pattern"))),
                    }
                }

                self.pos += 1; // 消费 ']'
                Ok(ForPattern::Array { patterns, rest })
            }
            // 对象模式: {"k1": v1, "k2": v2}
            Token::LBrace => {
                self.pos += 1; // 消费 '{'
                let mut entries: Vec<(String, ForPattern)> = Vec::new();

                // 处理空对象 {}
                if !self.eof() && self.tokens[self.pos] == Token::RBrace {
                    self.pos += 1;
                    return Ok(ForPattern::Object(entries));
                }

                loop {
                    if self.eof() {
                        return Err(anyhow!(self.err("Expected string key in object pattern")));
                    }

                    // 键必须是字符串字面量
                    let key = if let Token::Str(s) = &self.tokens[self.pos] {
                        let k = s.clone();
                        self.pos += 1;
                        k
                    } else {
                        return Err(anyhow!(self.err("Expected string key in object pattern")));
                    };

                    // 冒号
                    self.expect_token(Token::Colon)?;

                    // 值部分可以是任意 for 模式（变量、_、元组、数组、嵌套对象等）
                    let value_pattern = self.parse_for_pattern()?;

                    entries.push((key, value_pattern));

                    if self.eof() {
                        return Err(anyhow!(self.err("Expected '}' in object pattern")));
                    }

                    match &self.tokens[self.pos] {
                        Token::Comma => {
                            self.pos += 1; // 继续解析下一个键值
                            // 允许尾随逗号
                            if !self.eof() && self.tokens[self.pos] == Token::RBrace {
                                break;
                            }
                            continue;
                        }
                        Token::RBrace => break,
                        _ => {
                            return Err(anyhow!(self.err("Expected ',' or '}' in object pattern")));
                        }
                    }
                }

                self.pos += 1; // 消费 '}'
                Ok(ForPattern::Object(entries))
            }
            _ => Err(anyhow!(self.err("Expected pattern after 'for'"))),
        }
    }
}
