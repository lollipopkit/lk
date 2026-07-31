use super::StmtParser;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
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

        if core::mem::discriminant(&self.tokens[self.pos]) != core::mem::discriminant(&expected) {
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

    /// The `: T` of a `let`, a parameter, or a field.
    ///
    /// The collecting and rendering live in [`crate::type_syntax`], shared with
    /// the closure parser — this had been the only copy until a lambda needed
    /// the same thing in a position that cannot reach this parser.
    pub(super) fn parse_type_annotation(&mut self) -> Result<Type> {
        let Some((ty, end)) =
            crate::type_syntax::parse_type_at(self.tokens, self.pos, crate::type_syntax::StopAt::Union)
        else {
            // Two different reports, told apart by whether anything
            // type-shaped was there at all — an empty position is a missing
            // annotation, a non-empty one is a bad type.
            // One rule, said the same way from every type position: a
            // Rust-shaped `fn(Int) -> Int` is the mis-spelling worth naming,
            // and which collector ran decides nothing about the message.
            if let Some(hint) = crate::type_syntax::function_type_hint(self.tokens, self.pos) {
                return Err(anyhow!(self.err(hint)));
            }
            let spelled = crate::type_syntax::spelling_at(self.tokens, self.pos, crate::type_syntax::StopAt::Union);
            return Err(anyhow!(if spelled.is_empty() {
                self.err("Expected type annotation")
            } else {
                self.err(&format!("Invalid type: {spelled}"))
            }));
        };
        self.pos = end;
        Ok(ty)
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

    pub(super) fn parse_inline_expr_until_named_delim(&mut self) -> Result<Expr> {
        self.parse_inline_expr_until_delim(false)
    }

    pub(super) fn parse_inline_expr_until_param_delim(&mut self) -> Result<Expr> {
        self.parse_inline_expr_until_delim(true)
    }

    fn parse_inline_expr_until_delim(&mut self, stop_at_rparen: bool) -> Result<Expr> {
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
                Token::RParen if paren > 0 => {
                    paren -= 1;
                    end_pos += 1;
                }
                Token::RParen if stop_at_rparen && paren == 0 && bracket == 0 && brace == 0 => {
                    break;
                }
                Token::RParen => end_pos += 1,
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
        let expr_spans = self.token_spans.map(|spans| &spans[start_pos..end_pos]);
        let mut expr_parser = if let Some(spans) = expr_spans {
            ExprParser::new_with_spans(expr_tokens, spans)
        } else {
            ExprParser::new(expr_tokens)
        };
        let expr = expr_parser.parse()?;
        self.pos = end_pos;
        Ok(expr)
    }

    pub(super) fn err(&self, msg: &str) -> String {
        let ctx = if let Some(c) = self.tokens.get(self.pos) {
            format!("found {:?}", c)
        } else {
            "found end of input".to_string()
        };
        format!("Syntax error: {} ({})", msg, ctx)
    }

    /// The span covering tokens `from..=to`.
    pub(super) fn span_covering(&self, from: usize, to: usize) -> Option<Span> {
        let spans = self.token_spans.as_ref()?;
        let start = spans.get(from)?;
        let end = spans.get(to.max(from))?;
        Some(Span::new(start.start.clone(), end.end.clone()))
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
        let pattern = ExprParser::parse_whole_pattern(pattern_tokens)?;

        // Update position
        self.pos = end_pos;

        Ok(pattern)
    }

    /// TODO(待删除): the second copy of [`crate::type_syntax::spelling`].
    ///
    /// `parse_type_annotation` now goes through the shared one; three positions
    /// still collect their own tokens with their own stop rules
    /// (`parse_inline_type_until_param_delim` and the two in `function.rs`) and
    /// render with this. Give each a `StopAt` variant and this goes away — one
    /// renderer, or the two will drift.
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
            Token::UInt(i) => format!("0x{i:X}"),
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
            Token::ColonColon => "::".to_string(),
            Token::Assign => "=".to_string(),
            Token::Pipe => "|".to_string(),
            Token::Question => "?".to_string(),
            Token::FnArrow => "->".to_string(),
            Token::Lt => "<".to_string(),
            Token::Gt => ">".to_string(),
            // Pointer types: `*u8`, `*mut u32`.
            Token::Mul => "*".to_string(),
            _ => format!("{:?}", token),
        }
    }
}
