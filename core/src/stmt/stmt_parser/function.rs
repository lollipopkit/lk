use super::StmtParser;
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

        // 解析函数名
        let name = if let Token::Id(id) = &self.tokens[self.pos] {
            let name = id.clone();
            self.pos += 1;
            name
        } else {
            return Err(anyhow!(self.err("Expected function name")));
        };

        // 解析参数列表
        self.expect_token(Token::LParen)?;
        let mut params: Vec<String> = Vec::new();
        let mut param_types: Vec<Option<Type>> = Vec::new();
        let mut named_params: Vec<NamedParamDecl> = Vec::new();
        let mut saw_named_block = false;

        while !self.eof() && self.tokens[self.pos] != Token::RParen {
            // 若遇到具名参数块，则解析之；具名块必须位于位置参数之后
            if self.tokens[self.pos] == Token::LBrace {
                if saw_named_block {
                    return Err(anyhow!(self.err("Duplicate named parameter block")));
                }
                saw_named_block = true;
                let named = self.parse_named_param_block()?;
                named_params.extend(named);
                // 允许块后跟逗号
                if !self.eof() && self.tokens[self.pos] == Token::Comma {
                    self.pos += 1;
                }
                // 继续循环以期待 ')' 结束
                continue;
            }

            if saw_named_block {
                return Err(anyhow!(
                    self.err("Positional parameters cannot follow named parameter block")
                ));
            }

            // 参数名
            let param_name = if let Token::Id(param) = &self.tokens[self.pos] {
                let p = param.clone();
                self.pos += 1;
                p
            } else {
                return Err(anyhow!(self.err("Expected parameter name or '{' for named block")));
            };

            // 可选的参数类型注解 `: Type`
            let mut parsed_type: Option<Type> = None;
            if !self.eof() && self.tokens[self.pos] == Token::Colon {
                self.pos += 1; // consume ':'
                let ty = self.parse_inline_type_until_param_delim()?;
                parsed_type = Some(ty);
            }

            params.push(param_name);
            param_types.push(parsed_type);

            // 分隔符：逗号或结束
            if !self.eof() && self.tokens[self.pos] == Token::Comma {
                self.pos += 1; // 继续下一个参数
            } else if !self.eof() && self.tokens[self.pos] == Token::RParen {
                // end of params
            } else if self.eof() {
                return Err(anyhow!(self.err("Unexpected end while parsing parameters")));
            } else {
                return Err(anyhow!(self.err("Expected ',' or ')' in parameter list")));
            }
        }

        self.expect_token(Token::RParen)?;

        // 可选的返回类型 `-> Type`
        let mut return_type: Option<Type> = None;
        if !self.eof() && self.tokens[self.pos] == Token::FnArrow {
            self.pos += 1; // consume '->'
            let ty = self.parse_inline_type_until_block_start()?;
            return_type = Some(ty);
        }

        // 解析函数体 (必须是块语句)
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

    /// 解析具名参数块：形如 `{a: T, b: ?U = default}`
    pub fn parse_named_param_block(&mut self) -> Result<Vec<NamedParamDecl>> {
        self.expect_token(Token::LBrace)?;
        let mut named_params: Vec<NamedParamDecl> = Vec::new();

        // 允许空块
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(named_params);
        }

        loop {
            // 名称
            let name = if let Token::Id(id) = &self.tokens[self.pos] {
                let n = id.clone();
                self.pos += 1;
                n
            } else {
                return Err(anyhow!(self.err("Expected identifier in named parameter block")));
            };

            // ':' 类型
            self.expect_token(Token::Colon)?;
            let ty = self.parse_inline_type_until_named_delim()?;

            // 可选默认值 `= expr`
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

            // 分隔符处理：逗号继续，右花括号结束
            if self.eof() {
                return Err(anyhow!(self.err("Unexpected end in named parameter block")));
            }
            match &self.tokens[self.pos] {
                Token::Comma => {
                    self.pos += 1;
                    // 允许尾随逗号：{a: T,}
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

    /// 将参数类型解析到 ',' 或 '}'（深度为 0）之前，不消耗分隔符
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
        Type::parse(&type_str).ok_or_else(|| anyhow!(self.err(&format!("Invalid type: {}", type_str))))
    }
}
