use super::StmtParser;
use crate::{stmt::Stmt, token::Token, val::Type};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub fn parse_struct_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Struct)?;

        // 名称
        let name = if let Token::Id(id) = &self.tokens[self.pos] {
            let n = id.clone();
            self.pos += 1;
            n
        } else {
            return Err(anyhow!(self.err("Expected struct name after 'struct'")));
        };

        // 字段块
        self.expect_token(Token::LBrace)?;
        let mut fields: Vec<(String, Option<Type>)> = Vec::new();

        // 允许空结构体
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(Stmt::Struct { name, fields });
        }

        loop {
            // 字段名
            let field_name = if let Token::Id(id) = &self.tokens[self.pos] {
                let s = id.clone();
                self.pos += 1;
                s
            } else {
                return Err(anyhow!(self.err("Expected field name in struct")));
            };

            // ':' 类型（可选；未注解视为 Any）
            let mut ty: Option<Type> = None;
            if !self.eof() && self.tokens[self.pos] == Token::Colon {
                self.pos += 1; // consume ':'
                // 复用具名参数的类型解析（直至 ',' 或 '}'）
                let parsed = self.parse_inline_type_until_named_delim()?;
                ty = Some(parsed);
            }

            fields.push((field_name, ty));

            if self.eof() {
                return Err(anyhow!(self.err("Unexpected end in struct fields")));
            }
            match &self.tokens[self.pos] {
                Token::Comma => {
                    self.pos += 1;
                    // 允许尾随逗号
                    if !self.eof() && self.tokens[self.pos] == Token::RBrace {
                        self.pos += 1;
                        break;
                    }
                }
                Token::RBrace => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(anyhow!(self.err("Expected ',' or '}' in struct fields"))),
            }
        }

        Ok(Stmt::Struct { name, fields })
    }

    /// 解析 trait 语句：trait Name { fn method(params[: type]...) [-> type]; ... }
    pub fn parse_trait_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Trait)?;

        // trait 名称
        let name = if let Token::Id(id) = &self.tokens[self.pos] {
            let n = id.clone();
            self.pos += 1;
            n
        } else {
            return Err(anyhow!(self.err("Expected trait name after 'trait'")));
        };

        self.expect_token(Token::LBrace)?;

        let mut methods: Vec<(String, Type)> = Vec::new();

        // 允许空 trait
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(Stmt::Trait { name, methods });
        }

        while !self.eof() && self.tokens[self.pos] != Token::RBrace {
            // 每个方法声明以 fn 开始
            self.expect_token(Token::Fn)?;

            // 方法名
            let mname = if let Token::Id(id) = &self.tokens[self.pos] {
                let m = id.clone();
                self.pos += 1;
                m
            } else {
                return Err(anyhow!(self.err("Expected method name in trait")));
            };

            // 参数列表（仅用于签名）
            self.expect_token(Token::LParen)?;
            let mut param_types: Vec<Type> = Vec::new();
            while !self.eof() && self.tokens[self.pos] != Token::RParen {
                // 参数名
                if let Token::Id(_param_name) = &self.tokens[self.pos] {
                    self.pos += 1; // consume name
                } else {
                    return Err(anyhow!(self.err("Expected parameter name in trait method")));
                }
                // 可选类型注解
                let mut pty: Type = Type::Any;
                if !self.eof() && self.tokens[self.pos] == Token::Colon {
                    self.pos += 1; // ':'
                    pty = self.parse_inline_type_until_param_delim()?;
                }
                param_types.push(pty);
                // 分隔符
                if !self.eof() && self.tokens[self.pos] == Token::Comma {
                    self.pos += 1;
                } else if !self.eof() && self.tokens[self.pos] == Token::RParen {
                    // ok
                } else if self.eof() {
                    return Err(anyhow!(self.err("Unexpected end in trait method parameters")));
                } else {
                    return Err(anyhow!(self.err("Expected ',' or ')' in trait method parameters")));
                }
            }
            self.expect_token(Token::RParen)?;

            // 可选返回类型
            let mut ret_ty: Type = Type::Any;
            if !self.eof() && self.tokens[self.pos] == Token::FnArrow {
                self.pos += 1; // '->'
                ret_ty = self.parse_inline_type_until_semicolon()?;
                // parse_inline_type_until_semicolon stops before ';'
                self.expect_token(Token::Semicolon)?;
            } else {
                // 末尾分号（无返回类型时）
                self.expect_token(Token::Semicolon)?;
            }

            let fun_ty = Type::Function {
                params: param_types,
                named_params: Vec::new(),
                return_type: Box::new(ret_ty),
            };
            methods.push((mname, fun_ty));
        }

        self.expect_token(Token::RBrace)?;
        Ok(Stmt::Trait { name, methods })
    }

    /// 解析 impl 语句：impl Trait for Type { fn method(...) { ... } }
    pub fn parse_impl_stmt(&mut self) -> Result<Stmt> {
        self.expect_token(Token::Impl)?;

        // trait 名称
        let trait_name = if let Token::Id(id) = &self.tokens[self.pos] {
            let n = id.clone();
            self.pos += 1;
            n
        } else {
            return Err(anyhow!(self.err("Expected trait name after 'impl'")));
        };

        // 'for'
        if self.eof() || self.tokens[self.pos] != Token::For {
            return Err(anyhow!(self.err("Expected 'for' in impl statement")));
        }
        self.pos += 1;

        // 目标类型（直到 '{'）
        let target_type = self.parse_inline_type_until_block_start()?;

        self.expect_token(Token::LBrace)?;

        let mut methods: Vec<Stmt> = Vec::new();

        // 允许空 impl
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(Stmt::Impl {
                trait_name,
                target_type,
                methods,
            });
        }

        while !self.eof() && self.tokens[self.pos] != Token::RBrace {
            // 只允许方法定义（fn）
            if self.tokens[self.pos] != Token::Fn {
                return Err(anyhow!(self.err("Expected 'fn' in impl block")));
            }
            let m = self.parse_function_stmt()?;
            methods.push(m);
        }

        self.expect_token(Token::RBrace)?;

        Ok(Stmt::Impl {
            trait_name,
            target_type,
            methods,
        })
    }
}
