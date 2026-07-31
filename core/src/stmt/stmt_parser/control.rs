use super::StmtParser;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    ast::Parser as ExprParser,
    expr::Expr,
    stmt::{ForPattern, Stmt},
    token::Token,
};
use anyhow::{Result, anyhow, bail};

impl<'a> StmtParser<'a> {
    /// `go <expr>;` — Go-style fire-and-forget goroutine. Parse-time sugar
    /// (same treatment as try/catch → pcall): the operand is wrapped in a
    /// zero-param closure and handed to the `spawn` builtin, discarding the
    /// Task handle. Captures are snapshotted at spawn time (isolate
    /// semantics — see `spawn`); use `let t = spawn(|| …);` when the handle
    /// is needed.
    pub fn parse_go_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Go)?;
        let operand = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;
        let closure = Expr::Closure {
            params: Vec::new(),
            param_types: Vec::new(),
            return_type: None,
            body: Box::new(operand),
        };
        Ok(Stmt::expr(Box::new(Expr::Call(
            "spawn".to_string(),
            vec![Box::new(closure)],
        ))))
    }

    /// `try { … } catch e { … }` in statement position — the same node the
    /// expression parser builds, with its value discarded. `if` and `match` sit
    /// in statement position the same way; there is nothing here that a second
    /// AST node would say.
    pub fn parse_try_stmt(&mut self) -> Result<Stmt> {
        // A `try` that is the last thing here is a block's *tail*, so it is
        // parsed as the expression it is — same treatment as `if`, and for the
        // same reason: `try { try { … 1 } catch e { 2 } } catch e { 3 }` needs
        // the inner one to be a value. Anywhere else it stays a statement,
        // whose blocks are ordinary statement blocks.
        let keyword_pos = self.pos;
        if let Some(stmt) = self.try_parse_tail_expression_stmt(keyword_pos)? {
            return Ok(stmt);
        }
        self.expect_token(Token::Try)?;
        let Stmt::Block { statements: body } = self.parse_block_stmt()? else {
            bail!("`try` body must be a block");
        };
        self.expect_token(Token::Catch)?;
        let catch_var = match self.tokens.get(self.pos) {
            Some(Token::Id(name)) => {
                let name = name.clone();
                self.pos += 1;
                name
            }
            _ => bail!("expected an identifier after `catch`"),
        };
        let Stmt::Block { statements: handler } = self.parse_block_stmt()? else {
            bail!("`catch` body must be a block");
        };

        Ok(Stmt::expr(Box::new(Expr::Try {
            body,
            catch_var,
            handler,
        })))
    }

    /// 解析 if 语句
    pub fn parse_if_stmt(&mut self) -> Result<Stmt> {
        let keyword_pos = self.pos;
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
            // Regular `if`.
            //
            // When its branches are `{ … }`, this is the *expression* form —
            // the same construct, with the value discarded. Parsing it here
            // rather than as `Stmt::If` over statement-blocks is what makes
            // `if c { if d { 1 } else { 2 } } else { 3 }` work: a statement
            // block demands a `;` after every statement, so a branch whose
            // last line is the value it produces would not parse.
            //
            // The braceless forms (`if (c) return 1;`) have no block and no
            // value, and keep the statement path below.
            if let Some(stmt) = self.try_parse_if_expression_stmt(keyword_pos)? {
                return Ok(stmt);
            }

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

    /// Does the `if` at `keyword_pos` take a `{ … }` branch?
    ///
    /// Decided by scanning rather than by inspecting the parsed expression:
    /// the answer is needed *before* the expression exists, to choose which
    /// parser to run. (It also used to be that constant folding could delete
    /// the conditional outright — `if false { 1 } else { 2 }` came back as the
    /// surviving block. Folding no longer discards an unchecked branch, but
    /// the scan is still what decides.)
    fn if_branch_is_braced(&self, keyword_pos: usize) -> bool {
        let mut depth = 0i32;
        let mut index = keyword_pos + 1;
        while index < self.len {
            match &self.tokens[index] {
                Token::LParen | Token::LBracket => depth += 1,
                Token::RParen | Token::RBracket => depth -= 1,
                Token::LBrace if depth == 0 => return true,
                // A statement ends the search: `if (c) return 1;` has no block.
                Token::Semicolon if depth == 0 => return false,
                _ => {}
            }
            index += 1;
        }
        false
    }

    /// Parse a *trailing* keyword-led expression (`if …`, `try …`) as an
    /// expression statement, or answer `None` when it is not the last item.
    ///
    /// `keyword_pos` indexes the keyword token itself, since the expression
    /// parser has to see it. The sub-parser runs over a token slice and reports
    /// how much it consumed, so a `None` costs nothing: the caller is exactly
    /// where it was.
    fn try_parse_tail_expression_stmt(&mut self, keyword_pos: usize) -> Result<Option<Stmt>> {
        let tokens = &self.tokens[keyword_pos..];
        let spans = self.token_spans.map(|spans| &spans[keyword_pos..]);
        let mut parser = if let Some(spans) = spans {
            ExprParser::new_with_spans(tokens, spans)
        } else {
            ExprParser::new(tokens)
        };
        let Ok((expr, consumed)) = parser.parse_prefix() else {
            return Ok(None);
        };
        // Only when it is the *last* thing here: that is a block's tail, where
        // the value is what the block evaluates to. Anywhere else it is a
        // statement and has to stay one — its branches may `return`, `break` or
        // `continue`, and those lower as control flow out of the enclosing
        // function or loop, not as a value.
        let mut end = keyword_pos + consumed;
        if end < self.len && self.tokens[end] == Token::Semicolon {
            end += 1;
        }
        if end != self.len {
            return Ok(None);
        }
        self.pos = end;
        Ok(Some(Stmt::expr(Box::new(expr))))
    }

    /// Parse a *trailing* `if … { … }` as an expression statement, or answer
    /// `None` when this `if` is not the braced form or is not the last item.
    ///
    /// `keyword_pos` indexes the `if` token itself, since the expression parser
    /// has to see it. The sub-parser runs over a token slice and reports how
    /// much it consumed, so a `None` here costs nothing: the caller is exactly
    /// where it was.
    fn try_parse_if_expression_stmt(&mut self, keyword_pos: usize) -> Result<Option<Stmt>> {
        if !self.if_branch_is_braced(keyword_pos) {
            return Ok(None);
        }
        self.try_parse_tail_expression_stmt(keyword_pos)
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
            // Regular `while`.
            //
            // The condition stops at the top-level `{` that opens the body,
            // exactly as `if`'s does. Parentheses used to be *required* here
            // and optional there, for no reason either form could explain —
            // `while (i < 3) { … }` still parses, because a parenthesised
            // expression is an expression.
            let condition = strip_condition_parens(self.parse_expression_with_options(true)?);
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
        let mut pattern = self.parse_for_pattern()?;
        if !self.eof() && self.tokens[self.pos] == Token::Comma {
            let mut patterns = vec![pattern];
            while !self.eof() && self.tokens[self.pos] == Token::Comma {
                self.pos += 1;
                patterns.push(self.parse_for_pattern()?);
            }
            pattern = ForPattern::Tuple(patterns);
        }

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

/// A condition with its outer parentheses removed.
///
/// `Expr::Paren` carries no meaning — it exists so the formatter can print the
/// source back. But the loop analyses match on expression *shape*, and the
/// wrapper hides it: after `while` stopped requiring parentheses, the
/// still-legal `while (i < 3)` began parsing as `Paren(i < 3)` where it used
/// to be `i < 3`, and the constant `3` stopped being recognised as
/// loop-invariant — it became a loop-carried block parameter, reloaded every
/// iteration. Nothing was wrong with the answer, only with the code.
///
/// The `if` statement never had this because it consumed the parentheses as
/// tokens; stripping here restores that, for both.
fn strip_condition_parens(mut condition: Expr) -> Expr {
    while let Expr::Paren(inner) = condition {
        condition = *inner;
    }
    condition
}
