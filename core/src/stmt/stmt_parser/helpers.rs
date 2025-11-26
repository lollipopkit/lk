use super::StmtParser;
use crate::{
    ast::Parser as ExprParser,
    expr::{Expr, Pattern},
    token::{Span, Token},
    val::Type,
};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub(super) fn eof(&self) -> bool {
        self.pos >= self.len
    }

    pub(super) fn expect_token(&mut self, expected: Token) -> Result<()> {
        if self.eof() {
            return Err(anyhow!(
                self.err(&format!("Expected {:?}, found end of input", expected))
            ));
        }

        if std::mem::discriminant(&self.tokens[self.pos]) != std::mem::discriminant(&expected) {
            return Err(anyhow!(
                self.err(&format!("Expected {:?}, found {:?}", expected, self.tokens[self.pos]))
            ));
        }

        self.pos += 1;
        Ok(())
    }

    pub(super) fn peek_ahead(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset)
    }

    pub(super) fn expect_id(&mut self) -> Result<String> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected identifier")));
        }

        match &self.tokens[self.pos] {
            Token::Id(id) => {
                let id = id.clone();
                self.pos += 1;
                Ok(id)
            }
            _ => Err(anyhow!(self.err("Expected identifier"))),
        }
    }

    pub(super) fn parse_type_annotation(&mut self) -> Result<Type> {
        let mut type_tokens = Vec::new();

        // Collect tokens that make up the type annotation until we hit a token that can't be part of a type
        while !self.eof() {
            match &self.tokens[self.pos] {
                Token::Id(_)
                | Token::Lt
                | Token::Gt
                | Token::Comma
                | Token::LParen
                | Token::RParen
                | Token::Arrow
                | Token::Question
                | Token::Pipe
                | Token::LBracket
                | Token::RBracket => {
                    type_tokens.push(&self.tokens[self.pos]);
                    self.pos += 1;
                }
                _ => break,
            }
        }

        if type_tokens.is_empty() {
            return Err(anyhow!(self.err("Expected type annotation")));
        }

        let type_str = self.tokens_to_type_string(&type_tokens);
        let parsed_type = Type::parse(&type_str);
        parsed_type.ok_or_else(|| anyhow!(self.err(&format!("Invalid type: {}", type_str))))
    }

    pub(super) fn parse_inline_type_until_param_delim(&mut self) -> Result<Type> {
        let start_pos = self.pos;
        let mut tokens: Vec<&Token> = Vec::new();
        let mut paren: i32 = 0;
        let mut bracket: i32 = 0;
        let mut angle: i32 = 0;
        let mut guard: usize = 0;

        while !self.eof() {
            guard += 1;
            if guard > 1000 {
                break;
            }
            let t = &self.tokens[self.pos];
            match t {
                Token::LParen => {
                    paren += 1;
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::RParen => {
                    if paren == 0 && bracket == 0 && angle == 0 {
                        break;
                    }
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
                Token::Comma if paren == 0 && bracket == 0 && angle == 0 => {
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

    pub(super) fn parse_inline_type_until_semicolon(&mut self) -> Result<Type> {
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
                Token::RBrace if brace > 0 => {
                    brace -= 1;
                    tokens.push(t);
                    self.pos += 1;
                }
                Token::Semicolon if paren == 0 && bracket == 0 && brace == 0 && angle == 0 => {
                    break;
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
                _ => {
                    tokens.push(t);
                    self.pos += 1;
                }
            }
        }

        if tokens.is_empty() {
            self.pos = start_pos;
            return Err(anyhow!(self.err("Expected return type before ';'")));
        }

        let type_str = self.tokens_to_type_string(&tokens);
        Type::parse(&type_str).ok_or_else(|| anyhow!(self.err(&format!("Invalid type: {}", type_str))))
    }

    pub(super) fn parse_inline_type_until_block_start(&mut self) -> Result<Type> {
        let start_pos = self.pos;
        let mut tokens: Vec<&Token> = Vec::new();
        let mut paren: i32 = 0;
        let mut bracket: i32 = 0;
        let mut angle: i32 = 0;

        while !self.eof() {
            let t = &self.tokens[self.pos];
            match t {
                Token::LBrace if paren == 0 && bracket == 0 && angle == 0 => {
                    break;
                }
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
                _ => {
                    tokens.push(t);
                    self.pos += 1;
                }
            }
        }

        if tokens.is_empty() {
            self.pos = start_pos;
            return Err(anyhow!(self.err("Expected type after '->'")));
        }

        let type_str = self.tokens_to_type_string(&tokens);
        Type::parse(&type_str).ok_or_else(|| anyhow!(self.err(&format!("Invalid type: {}", type_str))))
    }

    pub(super) fn parse_inline_expr_until_named_delim(&mut self) -> Result<Expr> {
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
                Token::RBrace if brace > 0 => {
                    brace -= 1;
                    end_pos += 1;
                }
                Token::Comma | Token::RBrace if paren == 0 && bracket == 0 && brace == 0 => {
                    break;
                }
                _ => end_pos += 1,
            }
        }

        if end_pos == start_pos {
            return Err(anyhow!(self.err("Expected expression for default value")));
        }

        let expr_tokens = &self.tokens[start_pos..end_pos];
        let mut expr_parser = ExprParser::new(expr_tokens);
        let expr = expr_parser.parse()?;
        self.pos = end_pos;
        Ok(expr)
    }

    pub(super) fn err(&self, msg: &str) -> String {
        let r_idx = if self.pos + 5 < self.len {
            self.pos + 5
        } else {
            self.len
        };
        let l_idx = self.pos.saturating_sub(5);
        let r_idx = if r_idx > self.len { self.len } else { r_idx };
        let chars = &self.tokens[l_idx..r_idx];
        let chars: Vec<_> = chars.iter().collect();
        let c = self.tokens.get(self.pos);
        let ctx = if let Some(c) = c {
            format!("'{:?}' at index {}, near '{:?}'", c, self.pos, chars)
        } else {
            format!("at end, near '{:?}'", chars)
        };
        format!("Syntax error: {} ({})", msg, ctx)
    }

    pub(super) fn current_span(&self) -> Option<Span> {
        if let Some(spans) = &self.token_spans {
            if self.pos < spans.len() {
                Some(spans[self.pos].clone())
            } else {
                None
            }
        } else {
            None
        }
    }

    pub(super) fn parse_pattern(&mut self) -> Result<Pattern> {
        // Find the end of the pattern by looking for the '=' token
        let start_pos = self.pos;
        let mut end_pos = start_pos;
        let mut depth = 0;

        while end_pos < self.len {
            match &self.tokens[end_pos] {
                Token::LParen | Token::LBrace | Token::LBracket => {
                    depth += 1;
                    end_pos += 1;
                }
                Token::RParen | Token::RBrace | Token::RBracket => {
                    depth -= 1;
                    end_pos += 1;
                }
                Token::Assign if depth == 0 => {
                    break; // Found the '=' at top level, pattern ends here
                }
                _ => {
                    end_pos += 1;
                }
            }
        }

        if end_pos == start_pos {
            return Err(anyhow!(self.err("Expected pattern before '='")));
        }

        // Use AST parser to parse the pattern
        let pattern_tokens = &self.tokens[start_pos..end_pos];
        let mut ast_parser = ExprParser::new(pattern_tokens);
        let pattern = ast_parser.parse_pattern()?;

        // Update position
        self.pos = end_pos;

        Ok(pattern)
    }

    pub(super) fn tokens_to_type_string(&self, tokens: &[&Token]) -> String {
        let mut result = String::new();

        for (i, token) in tokens.iter().enumerate() {
            if i > 0 {
                match token {
                    Token::Pipe => result.push_str(" | "),
                    Token::Lt => result.push('<'),
                    Token::Gt | Token::Comma | Token::RParen | Token::RBracket | Token::RBrace => {
                        result.push_str(&self.token_to_string(token));
                    }
                    _ => {
                        if !matches!(tokens.get(i - 1), Some(Token::Lt)) {
                            result.push(' ');
                        }
                        result.push_str(&self.token_to_string(token));
                    }
                }
            } else {
                result.push_str(&self.token_to_string(token));
            }
        }

        result
    }

    pub(super) fn token_to_string(&self, token: &Token) -> String {
        match token {
            Token::Id(name) => name.clone(),
            Token::Str(s) => format!("\"{}\"", s),
            Token::Int(i) => i.to_string(),
            Token::Float(f) => f.to_string(),
            Token::Bool(b) => b.to_string(),
            Token::LParen => "(".to_string(),
            Token::RParen => ")".to_string(),
            Token::LBrace => "{".to_string(),
            Token::RBrace => "}".to_string(),
            Token::LBracket => "[".to_string(),
            Token::RBracket => "]".to_string(),
            Token::Comma => ",".to_string(),
            Token::Colon => ":".to_string(),
            Token::Pipe => "|".to_string(),
            Token::Question => "?".to_string(),
            Token::FnArrow => "->".to_string(),
            Token::Lt => "<".to_string(),
            Token::Gt => ">".to_string(),
            _ => format!("{:?}", token),
        }
    }
}
