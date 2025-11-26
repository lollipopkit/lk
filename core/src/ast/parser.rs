use crate::{
    expr::{Expr, MatchArm, Pattern, SelectCase, SelectPattern, TemplateStringPart},
    op::{BinOp, UnaryOp},
    token::{ParseError, Span, Token, Tokenizer, offset_to_position},
    val::Val,
};
use anyhow::{Result, anyhow};
use std::sync::Arc;

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    len: usize,
    token_spans: Option<&'a [Span]>,
}

impl<'a> Parser<'a> {
    pub fn parse(&mut self) -> Result<Expr> {
        if self.eof() {
            return Ok(Expr::Val(Val::Nil));
        }

        let exp = self.parse_expr()?;

        if !self.eof() {
            return Err(anyhow!(self.err("Unexpected tokens at end")));
        }

        // All sub-expressions parsed, apply constant folding optimization
        Ok(exp.fold_constants())
    }

    /// Parse with enhanced error information that includes position
    pub fn parse_with_enhanced_errors(&mut self, input: &str) -> std::result::Result<Expr, ParseError> {
        if self.eof() {
            return Ok(Expr::Val(Val::Nil));
        }

        let exp = match self.parse_expr() {
            Ok(expr) => expr,
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

        if !self.eof() {
            if let Some(spans) = &self.token_spans
                && self.pos < spans.len()
            {
                return Err(ParseError::with_span(
                    "Unexpected tokens at end".to_string(),
                    spans[self.pos].clone(),
                ));
            }
            let position = offset_to_position(
                input,
                if self.pos < self.tokens.len() {
                    self.pos * input.len() / self.tokens.len().max(1)
                } else {
                    input.len()
                },
            );
            return Err(ParseError::with_position(
                "Unexpected tokens at end".to_string(),
                position,
            ));
        }

        // All sub-expressions parsed, apply constant folding optimization
        Ok(exp.fold_constants())
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_conditional()
    }

    /// - `cond ? then : else` (ternary conditional)
    ///   Right-associative; precedence lower than nullish coalescing/or/and.
    fn parse_conditional(&mut self) -> Result<Expr> {
        let mut expr = self.parse_nullish_coalescing()?;
        if !self.eof() && self.tokens[self.pos] == Token::Question {
            // consume '?'
            self.pos += 1;

            // parse then branch as a full expression (it will naturally stop before ':')
            let then_expr = self.parse_expr()?;

            // expect ':'
            if self.eof() || self.tokens[self.pos] != Token::Colon {
                return Err(anyhow!(self.err("Expected ':' in ternary expression")));
            }
            self.pos += 1; // consume ':'

            // parse else branch (allow nesting: right-associative)
            let else_expr = self.parse_expr()?;

            expr = Expr::Conditional(Box::new(expr), Box::new(then_expr), Box::new(else_expr));
        }
        Ok(expr)
    }

    /// - `expr ?? expr` (nullish coalescing)
    fn parse_nullish_coalescing(&mut self) -> Result<Expr> {
        let mut expr = self.parse_or()?;
        while !self.eof() {
            match self.tokens[self.pos] {
                Token::NullishCoalescing => {
                    self.pos += 1;
                    let right = self.parse_or()?;
                    expr = Expr::NullishCoalescing(Box::new(expr), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// - `expr || expr`
    fn parse_or(&mut self) -> Result<Expr> {
        let mut expr = self.parse_and()?;
        while !self.eof() {
            match self.tokens[self.pos] {
                Token::Or => {
                    self.pos += 1;
                    let right = self.parse_and()?;
                    expr = Expr::Or(Box::new(expr), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// `expr && expr`
    fn parse_and(&mut self) -> Result<Expr> {
        let mut expr = self.parse_cmp()?;
        while !self.eof() {
            match self.tokens[self.pos] {
                Token::And => {
                    self.pos += 1;
                    let right = self.parse_cmp()?;
                    expr = Expr::And(Box::new(expr), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// - `expr == expr`
    /// - `expr != expr`
    ///   ...
    fn parse_cmp(&mut self) -> Result<Expr> {
        let mut expr = self.parse_range()?;
        while !self.eof() {
            let op = match self.tokens[self.pos] {
                Token::Eq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                Token::Gt => BinOp::Gt,
                Token::Lt => BinOp::Lt,
                Token::Ge => BinOp::Ge,
                Token::Le => BinOp::Le,
                Token::In => BinOp::In,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_range()?;
            expr = Expr::Bin(Box::new(expr), op, Box::new(right));
        }
        Ok(expr)
    }

    /// - `expr..expr` (range)
    /// - `expr..=expr` (inclusive range)
    /// - `expr..expr..step` (explicit step)
    /// - `expr..=expr..step` (inclusive with explicit step)
    fn parse_range(&mut self) -> Result<Expr> {
        let mut expr = self.parse_add_sub()?;

        if !self.eof() && (self.tokens[self.pos] == Token::Range || self.tokens[self.pos] == Token::RangeInclusive) {
            let inclusive = self.tokens[self.pos] == Token::RangeInclusive;
            self.pos += 1; // consume '..' or '..='

            // Check if there's an end expression
            let end = if !self.eof() && !self.is_range_terminator() {
                Some(Box::new(self.parse_add_sub()?))
            } else {
                None
            };

            // Optional explicit step indicated by another '..'
            let step = if !self.eof() && self.tokens[self.pos] == Token::Range {
                self.pos += 1; // consume '..'
                if self.eof() || self.is_range_terminator() {
                    return Err(anyhow!(self.err("Expected step expression after '..'")));
                }
                Some(Box::new(self.parse_add_sub()?))
            } else {
                None
            };

            expr = Expr::Range {
                start: Some(Box::new(expr)),
                end,
                inclusive,
                step,
            };
        }

        Ok(expr)
    }

    /// Check if the current token terminates a range expression
    fn is_range_terminator(&self) -> bool {
        if self.eof() {
            return true;
        }
        matches!(
            self.tokens[self.pos],
            Token::RParen | Token::RBrace | Token::RBracket | Token::Comma | Token::Semicolon | Token::In
        )
    }

    /// - `expr + expr`
    /// - `expr - expr`
    fn parse_add_sub(&mut self) -> Result<Expr> {
        let mut expr = self.parse_mul_div()?;
        while !self.eof() {
            let op = match self.tokens[self.pos] {
                Token::Add => BinOp::Add,
                Token::Sub => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_mul_div()?;
            expr = Expr::Bin(Box::new(expr), op, Box::new(right));
        }
        Ok(expr)
    }

    /// - `expr * expr`
    /// - `expr / expr`
    fn parse_mul_div(&mut self) -> Result<Expr> {
        let mut expr = self.parse_unary()?;
        while !self.eof() {
            let op = match self.tokens[self.pos] {
                Token::Mul => BinOp::Mul,
                Token::Div => BinOp::Div,
                Token::Mod => BinOp::Mod,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_unary()?;
            expr = Expr::Bin(Box::new(expr), op, Box::new(right));
        }
        Ok(expr)
    }

    /// - `!expr`
    /// - `expr`
    fn parse_unary(&mut self) -> Result<Expr> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected expression")));
        }
        let token = &self.tokens[self.pos];
        match token {
            Token::Not => {
                self.pos += 1;
                let expr = self.parse_unary()?;
                Ok(Expr::Unary(UnaryOp::Not, Box::new(expr)))
            }
            _ => self.parse_postfix(),
        }
    }

    /// - `primary`
    /// - `primary.field`
    /// - `primary.field.field`
    /// - `primary[expr]`
    /// - `func_name(args)`
    /// - `TypeName { field: expr, ... }` (struct literal)
    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;

        loop {
            if !self.eof() && self.tokens[self.pos] == Token::LParen {
                // Function call
                self.pos += 1; // skip '('

                let mut pos_args: Vec<Box<Expr>> = Vec::new();
                let mut named_args: Vec<(String, Box<Expr>)> = Vec::new();
                let mut saw_named = false;

                // Parse arguments
                while !self.eof() && self.tokens[self.pos] != Token::RParen {
                    // Named argument?  ident ':' expr
                    if let Token::Id(name) = &self.tokens[self.pos]
                        && (self.pos + 1) < self.len
                        && self.tokens[self.pos + 1] == Token::Colon
                    {
                        saw_named = true;
                        let key = name.clone();
                        self.pos += 2; // consume ident and ':'
                        // parse value expr
                        let val_expr = Box::new(self.parse_expr()?);
                        named_args.push((key, val_expr));
                    } else {
                        if saw_named {
                            return Err(anyhow!(self.err("Positional arguments cannot follow named arguments")));
                        }
                        pos_args.push(Box::new(self.parse_expr()?));
                    }

                    if !self.eof() && self.tokens[self.pos] == Token::Comma {
                        self.pos += 1;
                    } else if self.tokens[self.pos] != Token::RParen {
                        return Err(anyhow!(self.err("Expected ',' or ')' in function call")));
                    }
                }

                if self.eof() || self.tokens[self.pos] != Token::RParen {
                    return Err(anyhow!(self.err("Expected ')' to close function call")));
                }
                self.pos += 1; // skip ')'

                if saw_named {
                    expr = Expr::CallNamed(Box::new(expr), pos_args, named_args);
                } else {
                    expr = Expr::CallExpr(Box::new(expr), pos_args);
                }
            } else if !self.eof() && self.tokens[self.pos] == Token::LBrace {
                // Possible struct literal: only allowed immediately after a variable name
                if let Expr::Var(name) = &expr {
                    let fields = self.parse_struct_fields()?;
                    expr = Expr::StructLiteral {
                        name: name.clone(),
                        fields,
                    };
                } else {
                    // If not a simple Var before '{', treat as error to avoid ambiguity with blocks
                    return Err(anyhow!(self.err(
                        "Unexpected '{' after expression; did you mean a struct literal like Type { ... }?"
                    )));
                }
            } else if !self.eof() && self.tokens[self.pos] == Token::Dot {
                // Dot access
                self.pos += 1;

                if self.eof() {
                    return Err(anyhow!(self.err("Expecting field after '.'")));
                }

                let field = self.parse_field_name()?;
                expr = Expr::Access(Box::new(expr), Box::new(field));
            } else if !self.eof() && self.tokens[self.pos] == Token::OptionalDot {
                // Optional dot access (?.)
                self.pos += 1;
                if self.eof() {
                    return Err(anyhow!(self.err("Expecting field after '?.'")));
                }
                let field = self.parse_field_name()?;
                // Optional access is only supported on regular expressions, not @ expressions
                expr = Expr::OptionalAccess(Box::new(expr), Box::new(field));
            } else if !self.eof()
                && self.tokens[self.pos] == Token::Question
                && (self.pos + 1) < self.len
                && self.tokens[self.pos + 1] == Token::LBracket
            {
                // Optional bracket access: expr?[expr]
                // consume '?' and '['
                self.pos += 2;

                // Parse index expression
                if self.eof() || !self.is_valid_expr_start() {
                    let msg = format!(
                        "Invalid index/key after '?[', {:?}",
                        if self.eof() {
                            &Token::Nil
                        } else {
                            &self.tokens[self.pos]
                        }
                    );
                    return Err(anyhow!(self.err(&msg)));
                }
                let index_expr = Box::new(self.parse_expr()?);

                // Expect closing ']'
                if self.eof() || self.tokens[self.pos] != Token::RBracket {
                    let msg = format!(
                        "Expecting ']' to close optional index, found {:?}",
                        if self.eof() {
                            &Token::Nil
                        } else {
                            &self.tokens[self.pos]
                        }
                    );
                    return Err(anyhow!(self.err(&msg)));
                }
                self.pos += 1; // skip ']'

                expr = Expr::OptionalAccess(Box::new(expr), index_expr);
            } else if !self.eof() && self.tokens[self.pos] == Token::LBracket {
                // Bracket indexing: expr[expr]
                // Consume '['
                self.pos += 1;

                // Expect an expression inside brackets
                if self.eof() || !self.is_valid_expr_start() {
                    let msg = format!(
                        "Invalid index/key after '[', {:?}",
                        if self.eof() {
                            &Token::Nil
                        } else {
                            &self.tokens[self.pos]
                        }
                    );
                    return Err(anyhow!(self.err(&msg)));
                }
                let index_expr = Box::new(self.parse_expr()?);

                // Expect closing ']'
                if self.eof() || self.tokens[self.pos] != Token::RBracket {
                    let msg = format!(
                        "Expecting ']' to close index, found {:?}",
                        if self.eof() {
                            &Token::Nil
                        } else {
                            &self.tokens[self.pos]
                        }
                    );
                    return Err(anyhow!(self.err(&msg)));
                }
                self.pos += 1; // skip ']'

                // Build bracket Access
                expr = Expr::Access(Box::new(expr), index_expr);
            } else {
                break; // No more postfix operations
            }
        }

        Ok(expr)
    }

    /// Parse struct fields: '{ id: expr, ... }'
    fn parse_struct_fields(&mut self) -> Result<Vec<(String, Box<Expr>)>> {
        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            return Err(anyhow!(self.err("Expecting '{' to start struct literal")));
        }
        self.pos += 1;

        let mut fields: Vec<(String, Box<Expr>)> = Vec::new();

        // Allow empty struct: Type {}
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(fields);
        }

        loop {
            // Field name must be identifier
            let key = if let Token::Id(id) = &self.tokens[self.pos] {
                let k = id.clone();
                self.pos += 1;
                k
            } else {
                return Err(anyhow!(self.err("Expected identifier as struct field name")));
            };

            // ':'
            if self.eof() || self.tokens[self.pos] != Token::Colon {
                return Err(anyhow!(self.err("Expected ':' after struct field name")));
            }
            self.pos += 1;

            // Value expression
            let val = self.parse_expr()?;
            fields.push((key, Box::new(val)));

            if self.eof() {
                return Err(anyhow!(self.err("Unexpected end in struct literal fields")));
            }
            match &self.tokens[self.pos] {
                Token::Comma => {
                    self.pos += 1;
                    // Allow trailing comma before '}'
                    if !self.eof() && self.tokens[self.pos] == Token::RBrace {
                        self.pos += 1;
                        break;
                    }
                }
                Token::RBrace => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(anyhow!(self.err("Expected ',' or '}' in struct literal")));
                }
            }
        }

        Ok(fields)
    }

    /// - `nil`
    /// - `true`
    /// - `false`
    /// - `1`
    /// - `1.2`
    /// - `"str"`
    /// - `[...]`
    /// - `{...}`
    fn parse_primary(&mut self) -> Result<Expr> {
        if self.eof() {
            return Err(anyhow!(self.err("Unexpected end of input")));
        }

        match &self.tokens[self.pos] {
            Token::Nil => {
                self.pos += 1;
                Ok(Expr::Val(Val::Nil))
            }
            Token::Bool(b) => {
                self.pos += 1;
                Ok(Expr::Val(Val::Bool(*b)))
            }
            Token::Int(i) => {
                self.pos += 1;
                Ok(Expr::Val(Val::Int(*i)))
            }
            Token::Float(f) => {
                self.pos += 1;
                Ok(Expr::Val(Val::Float(*f)))
            }
            Token::Str(s) => {
                self.pos += 1;
                Ok(Expr::Val(Val::Str(Arc::from(s.as_str()))))
            }
            Token::TemplateString(content) => {
                self.pos += 1;
                self.parse_template_string_content(content)
            }
            Token::LBracket => self.parse_list(),
            Token::LBrace => self.parse_map(),
            Token::Select => self.parse_select(),
            Token::Match => self.parse_match(),
            Token::LParen => self.parse_paren(),
            Token::Fn => self.parse_fn_closure(),
            Token::Pipe => self.parse_closure(),
            Token::Id(id) => {
                let expr = Expr::Var(id.clone());
                self.pos += 1;
                Ok(expr)
            }
            _ => {
                let msg = format!("Unexpected token: {:?}", self.tokens[self.pos]);
                Err(anyhow!(self.err(&msg)))
            }
        }
    }

    /// Parse select expression: select { case ...; default ... }
    fn parse_select(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::Select {
            let msg = format!("Expecting 'select', found {:?}", self.tokens[self.pos]);
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            return Err(anyhow!(self.err("Expecting '{' after 'select'")));
        }
        self.pos += 1;

        let mut cases = Vec::new();
        let mut default_case = None;

        while !self.eof() && self.tokens[self.pos] != Token::RBrace {
            match &self.tokens[self.pos] {
                Token::Case => {
                    self.pos += 1;
                    let case = self.parse_select_case()?;
                    cases.push(case);
                }
                Token::Default => {
                    self.pos += 1;
                    if self.eof() || self.tokens[self.pos] != Token::Arrow {
                        return Err(anyhow!(self.err("Expecting '=>' after 'default'")));
                    }
                    self.pos += 1;

                    if self.eof() || !self.is_valid_expr_start() {
                        return Err(anyhow!(self.err("Expecting expression after 'default =>'")));
                    }

                    let expr = self.parse_expr()?;

                    // Semicolon is optional for the last case
                    if !self.eof() && self.tokens[self.pos] == Token::Semicolon {
                        self.pos += 1;
                    }

                    default_case = Some(Box::new(expr));
                }
                Token::Semicolon => {
                    self.pos += 1; // Skip semicolons between cases
                }
                _ => {
                    let msg = format!("Unexpected token in select: {:?}", self.tokens[self.pos]);
                    return Err(anyhow!(self.err(&msg)));
                }
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::RBrace {
            return Err(anyhow!(self.err("Expecting '}' to close select statement")));
        }
        self.pos += 1;

        Ok(Expr::Select { cases, default_case })
    }

    /// Parse match expression: match value { pattern => expr, ... }
    fn parse_match(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::Match {
            let msg = format!("Expecting 'match', found {:?}", self.tokens[self.pos]);
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        // Parse the value to match against, stopping before the opening '{'
        // to avoid consuming it as a struct literal in postfix parsing.
        let start_pos = self.pos;
        let mut i = self.pos;
        let mut paren: i32 = 0;
        let mut bracket: i32 = 0;
        while i < self.len {
            match &self.tokens[i] {
                Token::LParen => {
                    paren += 1;
                    i += 1;
                }
                Token::RParen => {
                    if paren > 0 {
                        paren -= 1;
                    }
                    i += 1;
                }
                Token::LBracket => {
                    bracket += 1;
                    i += 1;
                }
                Token::RBracket => {
                    if bracket > 0 {
                        bracket -= 1;
                    }
                    i += 1;
                }
                Token::LBrace if paren == 0 && bracket == 0 => {
                    break; // stop before '{' that begins match arms
                }
                _ => i += 1,
            }
        }

        if i == start_pos {
            return Err(anyhow!(self.err("Expected value before '{' in match expression")));
        }

        let value_tokens = &self.tokens[start_pos..i];
        let value_spans = self.token_spans.map(|sp| &sp[start_pos..i]);
        let mut sub = if let Some(spans) = value_spans {
            Parser::new_with_spans(value_tokens, spans)
        } else {
            Parser::new(value_tokens)
        };
        let value = Box::new(sub.parse_expr()?);
        self.pos = i;

        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            return Err(anyhow!(self.err("Expecting '{' after match value")));
        }
        self.pos += 1;

        let mut arms = Vec::new();

        while !self.eof() && self.tokens[self.pos] != Token::RBrace {
            // Parse pattern
            let pattern = self.parse_pattern()?;

            if self.eof() || self.tokens[self.pos] != Token::Arrow {
                return Err(anyhow!(self.err("Expecting '=>' after pattern")));
            }
            self.pos += 1;

            // Parse body expression
            let body = Box::new(self.parse_conditional()?);

            arms.push(MatchArm { pattern, body });

            // Handle optional comma/semicolon between arms
            if !self.eof() && (self.tokens[self.pos] == Token::Comma || self.tokens[self.pos] == Token::Semicolon) {
                self.pos += 1;
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::RBrace {
            return Err(anyhow!(self.err("Expecting '}' to close match expression")));
        }
        self.pos += 1;

        if arms.is_empty() {
            return Err(anyhow!(self.err("Match expression must have at least one arm")));
        }

        Ok(Expr::Match { value, arms })
    }

    /// Parse a pattern for match expressions
    pub fn parse_pattern(&mut self) -> Result<Pattern> {
        self.parse_or_pattern()
    }

    /// Parse OR pattern: pattern1 | pattern2
    fn parse_or_pattern(&mut self) -> Result<Pattern> {
        let mut patterns = vec![self.parse_guard_pattern()?];

        while !self.eof() && self.tokens[self.pos] == Token::Pipe {
            self.pos += 1; // Skip |
            patterns.push(self.parse_guard_pattern()?);
        }

        if patterns.len() == 1 {
            Ok(patterns.into_iter().next().unwrap())
        } else {
            Ok(Pattern::Or(patterns))
        }
    }

    /// Parse pattern with optional guard: pattern if expr
    fn parse_guard_pattern(&mut self) -> Result<Pattern> {
        let pattern = self.parse_primary_pattern()?;

        if !self.eof() && self.tokens[self.pos] == Token::If {
            self.pos += 1; // Skip if
            let guard = Box::new(self.parse_conditional()?);
            Ok(Pattern::Guard {
                pattern: Box::new(pattern),
                guard,
            })
        } else {
            Ok(pattern)
        }
    }

    /// Parse primary pattern: literals, variables, destructuring
    fn parse_primary_pattern(&mut self) -> Result<Pattern> {
        if self.eof() {
            return Err(anyhow!(self.err("Unexpected end of input in pattern")));
        }

        match &self.tokens[self.pos] {
            // Literal patterns
            Token::Int(i) => {
                let start_val = *i;
                self.pos += 1;

                // Check if this is a range pattern
                if !self.eof()
                    && (self.tokens[self.pos] == Token::Range || self.tokens[self.pos] == Token::RangeInclusive)
                {
                    let inclusive = self.tokens[self.pos] == Token::RangeInclusive;
                    self.pos += 1;
                    let end_expr = Box::new(self.parse_conditional()?);

                    Ok(Pattern::Range {
                        start: Box::new(Expr::Val(Val::Int(start_val))),
                        end: end_expr,
                        inclusive,
                    })
                } else {
                    Ok(Pattern::Literal(Val::Int(start_val)))
                }
            }
            Token::Float(f) => {
                let start_val = *f;
                self.pos += 1;
                // Check if this is a range pattern
                if !self.eof()
                    && (self.tokens[self.pos] == Token::Range || self.tokens[self.pos] == Token::RangeInclusive)
                {
                    let inclusive = self.tokens[self.pos] == Token::RangeInclusive;
                    self.pos += 1;
                    let end_expr = Box::new(self.parse_conditional()?);
                    Ok(Pattern::Range {
                        start: Box::new(Expr::Val(Val::Float(start_val))),
                        end: end_expr,
                        inclusive,
                    })
                } else {
                    Ok(Pattern::Literal(Val::Float(start_val)))
                }
            }
            Token::Str(s) => {
                let val = Val::Str(Arc::from(s.clone()));
                self.pos += 1;
                Ok(Pattern::Literal(val))
            }
            Token::Bool(b) => {
                let val = Val::Bool(*b);
                self.pos += 1;
                Ok(Pattern::Literal(val))
            }
            Token::Nil => {
                self.pos += 1;
                Ok(Pattern::Literal(Val::Nil))
            }

            // Wildcard pattern
            Token::Id(name) if name == "_" => {
                self.pos += 1;
                Ok(Pattern::Wildcard)
            }

            // Variable pattern
            Token::Id(name) => {
                let name = name.clone();
                self.pos += 1;
                Ok(Pattern::Variable(name))
            }

            // List pattern: [pattern1, pattern2, ..rest]
            Token::LBracket => {
                self.pos += 1; // Skip [
                let mut patterns = Vec::new();
                let mut rest = None;

                while !self.eof() && self.tokens[self.pos] != Token::RBracket {
                    if self.tokens[self.pos] == Token::Range {
                        // Rest pattern: ..rest
                        self.pos += 1; // Skip ..
                        if let Token::Id(rest_name) = &self.tokens[self.pos] {
                            rest = Some(rest_name.clone());
                            self.pos += 1;
                        } else {
                            return Err(anyhow!(self.err("Expecting identifier after '..' in list pattern")));
                        }
                        break;
                    } else {
                        patterns.push(self.parse_pattern()?);

                        if !self.eof() && self.tokens[self.pos] == Token::Comma {
                            self.pos += 1; // Skip comma
                        }
                    }
                }

                if self.eof() || self.tokens[self.pos] != Token::RBracket {
                    return Err(anyhow!(self.err("Expecting ']' to close list pattern")));
                }
                self.pos += 1;

                Ok(Pattern::List { patterns, rest })
            }

            // Map pattern: {"key": pattern, "other": var, ..rest}
            Token::LBrace => {
                self.pos += 1; // Skip {
                let mut patterns = Vec::new();
                let mut rest = None;

                while !self.eof() && self.tokens[self.pos] != Token::RBrace {
                    if self.tokens[self.pos] == Token::Range {
                        // Rest pattern: ..rest
                        self.pos += 1; // Skip ..
                        if let Token::Id(rest_name) = &self.tokens[self.pos] {
                            rest = Some(rest_name.clone());
                            self.pos += 1;
                        } else {
                            return Err(anyhow!(self.err("Expecting identifier after '..' in map pattern")));
                        }
                        break;
                    } else {
                        // Parse key: pattern
                        let key = match &self.tokens[self.pos] {
                            Token::Str(s) => s.clone(),
                            Token::Id(s) => s.clone(),
                            _ => {
                                return Err(anyhow!(self.err("Expecting string or identifier as map key")));
                            }
                        };
                        self.pos += 1;

                        if self.eof() || self.tokens[self.pos] != Token::Colon {
                            return Err(anyhow!(self.err("Expecting ':' after map key in pattern")));
                        }
                        self.pos += 1;

                        let pattern = self.parse_pattern()?;
                        patterns.push((key, pattern));

                        if !self.eof() && self.tokens[self.pos] == Token::Comma {
                            self.pos += 1; // Skip comma
                        }
                    }
                }

                if self.eof() || self.tokens[self.pos] != Token::RBrace {
                    return Err(anyhow!(self.err("Expecting '}' to close map pattern")));
                }
                self.pos += 1;

                Ok(Pattern::Map { patterns, rest })
            }

            // Unknown pattern
            _ => {
                let msg = format!("Unexpected token in pattern: {:?}", self.tokens[self.pos]);
                Err(anyhow!(self.err(&msg)))
            }
        }
    }

    /// Parse a select case: case pattern [if guard] => expr;
    fn parse_select_case(&mut self) -> Result<SelectCase> {
        // Parse optional binding for recv pattern (identifier <- ...)
        if self.eof() {
            return Err(anyhow!(self.err("Expecting pattern after 'case'")));
        }
        let mut binding: Option<String> = None;
        if let Token::Id(name) = &self.tokens[self.pos]
            && self.pos + 1 < self.len
            && matches!(self.tokens[self.pos + 1], Token::LeftArrow | Token::Le)
        {
            let identifier = name.clone();
            self.pos += 2; // consume identifier and arrow token
            if identifier != "_" {
                binding = Some(identifier);
            }
        }

        if self.eof() {
            return Err(anyhow!(self.err("Expecting pattern after binding")));
        }

        // Parse pattern
        let pattern = if matches!(&self.tokens[self.pos], Token::Id(name) if name == "recv") {
            let binding_value = binding;
            self.pos += 1;
            if self.eof() || self.tokens[self.pos] != Token::LParen {
                return Err(anyhow!(self.err("Expecting '(' after 'recv' in case pattern")));
            }
            self.pos += 1;

            let channel = self.parse_expr()?;

            if self.eof() || self.tokens[self.pos] != Token::RParen {
                return Err(anyhow!(self.err("Expecting ')' after channel in recv pattern")));
            }
            self.pos += 1;

            SelectPattern::Recv {
                binding: binding_value,
                channel: Box::new(channel),
            }
        } else if matches!(&self.tokens[self.pos], Token::Id(name) if name == "send") {
            if binding.is_some() {
                return Err(anyhow!(self.err("Send pattern does not support bindings")));
            }
            self.pos += 1;
            if self.eof() || self.tokens[self.pos] != Token::LParen {
                return Err(anyhow!(self.err("Expecting '(' after 'send' in case pattern")));
            }
            self.pos += 1;

            let channel = self.parse_expr()?;

            if self.eof() || self.tokens[self.pos] != Token::Comma {
                return Err(anyhow!(self.err("Expecting ',' after channel in send pattern")));
            }
            self.pos += 1;

            let value = self.parse_expr()?;

            if self.eof() || self.tokens[self.pos] != Token::RParen {
                return Err(anyhow!(self.err("Expecting ')' after value in send pattern")));
            }
            self.pos += 1;

            SelectPattern::Send {
                channel: Box::new(channel),
                value: Box::new(value),
            }
        } else {
            let msg = format!("Unexpected pattern token: {:?}", self.tokens[self.pos]);
            return Err(anyhow!(self.err(&msg)));
        };

        // Optional guard: `if <expr>`
        let guard = if !self.eof() && self.tokens[self.pos] == Token::If {
            self.pos += 1;
            if self.eof() || !self.is_valid_expr_start() {
                return Err(anyhow!(self.err("Expecting guard expression after 'if'")));
            }
            let g = self.parse_expr()?;
            Some(Box::new(g))
        } else {
            None
        };

        // Parse arrow
        if self.eof() || self.tokens[self.pos] != Token::Arrow {
            return Err(anyhow!(self.err("Expecting '=>' after pattern")));
        }
        self.pos += 1;

        // Parse body expression
        if self.eof() || !self.is_valid_expr_start() {
            return Err(anyhow!(self.err("Expecting expression after '=>'")));
        }
        let body = self.parse_expr()?;

        // Semicolon is optional for the last case
        if !self.eof() && self.tokens[self.pos] == Token::Semicolon {
            self.pos += 1;
        }

        Ok(SelectCase {
            pattern,
            guard,
            body: Box::new(body),
        })
    }

    /// Parse template string content from a TemplateString token
    fn parse_template_string_content(&mut self, content: &str) -> Result<Expr> {
        let mut parts = Vec::new();
        let mut current_literal = String::new();
        let mut in_expr = false;
        let mut expr_start = 0;
        let mut pos = 0;

        while pos < content.len() {
            let c = content.chars().nth(pos).unwrap();

            if in_expr {
                if c == '}' {
                    // End of ${...} expression
                    let expr_content = &content[expr_start..pos];
                    if !expr_content.is_empty() {
                        let expr_tokens = match Tokenizer::tokenize_enhanced(expr_content) {
                            Ok(tokens) => tokens,
                            Err(e) => {
                                return Err(anyhow!(
                                    self.err(&format!("Failed to parse template expression: {}", e))
                                ));
                            }
                        };

                        if !expr_tokens.is_empty() {
                            let mut expr_parser = Parser::new(&expr_tokens);
                            match expr_parser.parse_expr() {
                                Ok(expr) => parts.push(TemplateStringPart::Expr(Box::new(expr))),
                                Err(e) => {
                                    return Err(anyhow!(
                                        self.err(&format!("Failed to parse template expression: {}", e))
                                    ));
                                }
                            }
                        }
                    }
                    in_expr = false;
                    pos += 1; // skip the '}'
                } else {
                    pos += 1;
                }
            } else if c == '$' && pos + 1 < content.len() && content.chars().nth(pos + 1) == Some('{') {
                // Start of original ${expr} syntax
                pos += 2; // skip '${'

                // Push the current literal if not empty
                if !current_literal.is_empty() {
                    parts.push(TemplateStringPart::Literal(std::mem::take(&mut current_literal)));
                }

                in_expr = true;
                expr_start = pos;
            } else {
                current_literal.push(c);
                pos += 1;
            }
        }

        // Push any remaining literal content
        if !current_literal.is_empty() {
            parts.push(TemplateStringPart::Literal(current_literal));
        }

        // If we're still in an expression, it's an error
        if in_expr {
            return Err(anyhow!(self.err("Unclosed template expression")));
        }

        Ok(Expr::TemplateString(parts))
    }

    /// - `(expr)`
    /// - `expr`
    fn parse_paren(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] == Token::LParen {
            self.pos += 1;
            let expr = self.parse_expr()?;
            if self.eof() || self.tokens[self.pos] != Token::RParen {
                let msg = format!(
                    "Expecting ')', found {:?}",
                    if self.eof() {
                        &Token::Nil
                    } else {
                        &self.tokens[self.pos]
                    }
                );
                return Err(anyhow!(self.err(&msg)));
            }
            self.pos += 1;
            Ok(Expr::Paren(Box::new(expr)))
        } else {
            match &self.tokens[self.pos] {
                Token::Id(id) => {
                    let expr = Expr::Var(id.clone());
                    self.pos += 1;
                    Ok(expr)
                }
                Token::Select => self.parse_select(),
                Token::Match => self.parse_match(),
                _ => {
                    let msg = format!("Unexpected token: {:?}", self.tokens[self.pos]);
                    Err(anyhow!(self.err(&msg)))
                }
            }
        }
    }

    /// Parse list literal: `[expr, expr, ...]`
    fn parse_list(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::LBracket {
            let msg = format!("Expecting '[', found {:?}", self.tokens[self.pos]);
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        let mut elements = Vec::new();

        // Handle empty list
        if !self.eof() && self.tokens[self.pos] == Token::RBracket {
            self.pos += 1;
            return Ok(Expr::List(elements));
        }

        // Parse first element
        if !self.eof() {
            if !self.is_valid_expr_start() {
                let msg = format!("Invalid list element start: {:?}", self.tokens[self.pos]);
                return Err(anyhow!(self.err(&msg)));
            }

            elements.push(Box::new(self.parse_expr()?));

            // Parse remaining elements
            while !self.eof() {
                match self.tokens[self.pos] {
                    Token::Comma => {
                        self.pos += 1;
                        // Handle trailing comma
                        if !self.eof() && self.tokens[self.pos] == Token::RBracket {
                            break;
                        }

                        if self.eof() || !self.is_valid_expr_start() {
                            let msg = format!(
                                "Invalid list element after comma: {:?}",
                                if self.eof() {
                                    &Token::Nil
                                } else {
                                    &self.tokens[self.pos]
                                }
                            );
                            return Err(anyhow!(self.err(&msg)));
                        }

                        elements.push(Box::new(self.parse_expr()?));
                    }
                    Token::RBracket => break,
                    _ => {
                        let msg = if self.is_invalid_separator() {
                            format!(
                                "Invalid separator in list: {:?}. Use ',' to separate elements",
                                self.tokens[self.pos]
                            )
                        } else {
                            format!("Expecting ',' or ']', found {:?}", self.tokens[self.pos])
                        };
                        return Err(anyhow!(self.err(&msg)));
                    }
                }
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::RBracket {
            let msg = format!(
                "Expecting ']', found {:?}",
                if self.eof() {
                    &Token::Nil
                } else {
                    &self.tokens[self.pos]
                }
            );
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        Ok(Expr::List(elements))
    }

    /// Parse map literal: `{key: value, key: value, ...}`
    fn parse_map(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::LBrace {
            let msg = format!("Expecting '{{', found {:?}", self.tokens[self.pos]);
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        let mut pairs = Vec::new();

        // Handle empty map
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(Expr::Map(pairs));
        }

        // Parse first key-value pair
        if !self.eof() {
            if !self.is_valid_expr_start() {
                let msg = format!("Invalid map key start: {:?}", self.tokens[self.pos]);
                return Err(anyhow!(self.err(&msg)));
            }

            let key = Box::new(self.parse_expr()?);

            if self.eof() || self.tokens[self.pos] != Token::Colon {
                let msg = format!(
                    "Expecting ':', found {:?}",
                    if self.eof() {
                        &Token::Nil
                    } else {
                        &self.tokens[self.pos]
                    }
                );
                return Err(anyhow!(self.err(&msg)));
            }
            self.pos += 1;

            if self.eof() || !self.is_valid_expr_start() {
                let msg = format!(
                    "Invalid map value after ':', {:?}",
                    if self.eof() {
                        &Token::Nil
                    } else {
                        &self.tokens[self.pos]
                    }
                );
                return Err(anyhow!(self.err(&msg)));
            }

            let value = Box::new(self.parse_expr()?);
            pairs.push((key, value));

            // Parse remaining pairs
            while !self.eof() {
                match self.tokens[self.pos] {
                    Token::Comma => {
                        self.pos += 1;
                        // Handle trailing comma
                        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
                            break;
                        }

                        if self.eof() || !self.is_valid_expr_start() {
                            let msg = format!(
                                "Invalid map key after comma: {:?}",
                                if self.eof() {
                                    &Token::Nil
                                } else {
                                    &self.tokens[self.pos]
                                }
                            );
                            return Err(anyhow!(self.err(&msg)));
                        }

                        let key = Box::new(self.parse_expr()?);

                        if self.eof() || self.tokens[self.pos] != Token::Colon {
                            let msg = format!(
                                "Expecting ':', found {:?}",
                                if self.eof() {
                                    &Token::Nil
                                } else {
                                    &self.tokens[self.pos]
                                }
                            );
                            return Err(anyhow!(self.err(&msg)));
                        }
                        self.pos += 1;

                        if self.eof() || !self.is_valid_expr_start() {
                            let msg = format!(
                                "Invalid map value after ':', {:?}",
                                if self.eof() {
                                    &Token::Nil
                                } else {
                                    &self.tokens[self.pos]
                                }
                            );
                            return Err(anyhow!(self.err(&msg)));
                        }

                        let value = Box::new(self.parse_expr()?);
                        pairs.push((key, value));
                    }
                    Token::RBrace => break,
                    _ => {
                        let msg = format!("Expecting ',' or '}}', found {:?}", self.tokens[self.pos]);
                        return Err(anyhow!(self.err(&msg)));
                    }
                }
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::RBrace {
            let msg = format!(
                "Expecting '}}', found {:?}",
                if self.eof() {
                    &Token::Nil
                } else {
                    &self.tokens[self.pos]
                }
            );
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        Ok(Expr::Map(pairs))
    }

    /// Parse field name for .field and ?.field access - treats IDs as string literals
    fn parse_field_name(&mut self) -> Result<Expr> {
        match &self.tokens[self.pos] {
            Token::Id(id) => {
                // For field access, treat identifiers as literal strings
                let expr = Expr::Val(Val::Str(Arc::from(id.as_str())));
                self.pos += 1;
                Ok(expr)
            }
            Token::Str(s) => {
                let expr = Expr::Val(Val::Str(Arc::from(s.as_str())));
                self.pos += 1;
                Ok(expr)
            }
            Token::Int(i) => {
                let expr = Expr::Val(Val::Int(*i));
                self.pos += 1;
                Ok(expr)
            }
            _ => {
                let msg = format!("Invalid field name: {:?}", &self.tokens[self.pos]);
                Err(anyhow!(self.err(&msg)))
            }
        }
    }

    // legacy '@' syntax fully removed; no parse_at/at-specific field access remain

    /// Check if the current token can start a valid expression
    fn is_valid_expr_start(&self) -> bool {
        if self.eof() {
            return false;
        }

        matches!(
            self.tokens[self.pos],
            Token::Nil
                | Token::Bool(_)
                | Token::Int(_)
                | Token::Float(_)
                | Token::Str(_)
                | Token::Id(_)
                | Token::LBracket
                | Token::LBrace
                | Token::LParen
                | Token::Not
                | Token::Select
                | Token::Pipe
                | Token::Fn
        )
    }

    /// Check if the current token is an invalid separator
    fn is_invalid_separator(&self) -> bool {
        if self.eof() {
            return false;
        }

        matches!(self.tokens[self.pos], Token::Semicolon)
    }

    /// Recovering expression analysis: collect multiple parse errors across expression segments
    /// without building a final AST. Uses shallow segmentation on common boundaries to surface
    /// multiple issues within a single line/chunk.
    pub fn recover_expression_errors(tokens: &'a [Token], spans: &'a [Span], input: &str) -> Vec<ParseError> {
        let mut errors = Vec::new();
        let len = tokens.len();
        let mut i = 0usize;

        // Track depth for (), [], {} to decide boundaries at depth 0
        let mut paren: i32;
        let mut bracket: i32;
        let mut brace: i32;

        fn is_hard_boundary(tok: &Token) -> bool {
            matches!(
                tok,
                Token::Comma | Token::Semicolon | Token::RParen | Token::RBracket | Token::RBrace | Token::Else
            )
        }

        fn is_soft_boundary(tok: &Token) -> bool {
            matches!(
                tok,
                Token::Eq
                    | Token::Ne
                    | Token::Gt
                    | Token::Lt
                    | Token::Ge
                    | Token::Le
                    | Token::In
                    | Token::And
                    | Token::Or
            )
        }

        while i < len {
            // Skip immediate boundaries to avoid empty segments
            while i < len && is_hard_boundary(&tokens[i]) {
                i += 1;
            }
            if i >= len {
                break;
            }

            // Determine a segment [i, j)
            let seg_start = i;
            let mut j = i;
            paren = 0;
            bracket = 0;
            brace = 0;
            while j < len {
                match &tokens[j] {
                    Token::LParen => {
                        paren += 1;
                        j += 1;
                    }
                    Token::RParen => {
                        if paren > 0 {
                            paren -= 1;
                        }
                        if paren == 0 && bracket == 0 && brace == 0 {
                            j += 1;
                            break;
                        }
                        j += 1;
                    }
                    Token::LBracket => {
                        bracket += 1;
                        j += 1;
                    }
                    Token::RBracket => {
                        if bracket > 0 {
                            bracket -= 1;
                        }
                        if paren == 0 && bracket == 0 && brace == 0 {
                            j += 1;
                            break;
                        }
                        j += 1;
                    }
                    Token::LBrace => {
                        brace += 1;
                        j += 1;
                    }
                    Token::RBrace => {
                        if brace > 0 {
                            brace -= 1;
                        }
                        if paren == 0 && bracket == 0 && brace == 0 {
                            break;
                        }
                        j += 1;
                    }
                    t if is_hard_boundary(t) => {
                        break;
                    }
                    t if is_soft_boundary(t) && paren == 0 && bracket == 0 && brace == 0 => {
                        break;
                    }
                    _ => {
                        j += 1;
                    }
                }
            }
            if j == seg_start {
                i = j + 1;
                continue;
            }

            // Attempt to parse the segment
            let seg_tokens = &tokens[seg_start..j];
            let seg_spans = &spans[seg_start..j];
            if !seg_tokens.is_empty() {
                let mut p = Parser::new_with_spans(seg_tokens, seg_spans);
                match p.parse_with_enhanced_errors(input) {
                    Ok(_) => {}
                    Err(e) => errors.push(e),
                }
            }

            // Advance to next segment; if current position is at a soft boundary, skip it
            i = j;
            if i < len && (is_soft_boundary(&tokens[i]) || is_hard_boundary(&tokens[i])) {
                i += 1;
            }
        }

        errors
    }
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: None,
        }
    }

    /// Create a parser with token spans for precise error reporting
    pub fn new_with_spans(tokens: &'a [Token], spans: &'a [Span]) -> Self {
        let len = tokens.len();
        Self {
            tokens,
            pos: 0,
            len,
            token_spans: Some(spans),
        }
    }

    /// Parse `fn(params) => expr` style closure literal.
    fn parse_fn_closure(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::Fn {
            return Err(anyhow!(self.err("Expected 'fn'")));
        }
        self.pos += 1; // consume 'fn'

        if self.eof() || self.tokens[self.pos] != Token::LParen {
            return Err(anyhow!(self.err("Expected '(' after 'fn'")));
        }
        self.pos += 1; // consume '('

        let mut params = Vec::new();

        if !self.eof() && self.tokens[self.pos] != Token::RParen {
            loop {
                let param_name = if let Token::Id(name) = &self.tokens[self.pos] {
                    let n = name.clone();
                    self.pos += 1;
                    n
                } else {
                    return Err(anyhow!(self.err("Expected parameter name in fn literal")));
                };
                params.push(param_name);

                if !self.eof() && self.tokens[self.pos] == Token::Colon {
                    self.pos += 1; // consume ':'
                    self.skip_type_annotation()?;
                }

                if self.eof() {
                    return Err(anyhow!(self.err("Unexpected end in fn parameter list")));
                }

                match self.tokens[self.pos] {
                    Token::Comma => {
                        self.pos += 1; // consume ','
                        if self.eof() {
                            return Err(anyhow!(self.err("Expected parameter after ',' in fn literal")));
                        }
                        continue;
                    }
                    Token::RParen => break,
                    _ => {
                        return Err(anyhow!(self.err("Expected ',' or ')' after parameter in fn literal")));
                    }
                }
            }
        }

        if self.eof() || self.tokens[self.pos] != Token::RParen {
            return Err(anyhow!(self.err("Expected ')' after fn parameters")));
        }
        self.pos += 1; // consume ')'

        if self.eof() || self.tokens[self.pos] != Token::Arrow {
            return Err(anyhow!(self.err("Expected '=>' after fn parameters")));
        }
        self.pos += 1; // consume '=>'

        if self.eof() || !self.is_valid_expr_start() {
            return Err(anyhow!(self.err("Expected expression after '=>' in fn literal")));
        }

        let body = self.parse_expr()?;

        Ok(Expr::Closure {
            params,
            body: Box::new(body),
        })
    }

    /// Parse closure expression: |param1, param2| expr
    fn parse_closure(&mut self) -> Result<Expr> {
        self.pos += 1; // Consume the opening '|'

        // Parse parameters
        let mut params = Vec::new();

        // Check if there are any parameters
        if !self.eof() && self.tokens[self.pos] != Token::Pipe {
            // Parse first parameter
            if let Token::Id(param_name) = &self.tokens[self.pos] {
                params.push(param_name.clone());
                self.pos += 1;
            } else {
                return Err(anyhow!(
                    self.err("Expected parameter name or '|' after opening '|' in closure")
                ));
            }

            // Parse additional parameters separated by commas
            while !self.eof() && self.tokens[self.pos] == Token::Comma {
                self.pos += 1; // Consume comma

                if let Token::Id(param_name) = &self.tokens[self.pos] {
                    params.push(param_name.clone());
                    self.pos += 1;
                } else {
                    return Err(anyhow!(self.err("Expected parameter name after comma in closure")));
                }
            }
        }

        // Expect closing '|'
        if self.eof() || self.tokens[self.pos] != Token::Pipe {
            return Err(anyhow!(self.err("Expected '|' to close parameter list in closure")));
        }
        self.pos += 1; // Consume closing '|'

        // Parse closure body
        if self.eof() || !self.is_valid_expr_start() {
            return Err(anyhow!(self.err("Expected expression after closure parameters")));
        }

        let body = self.parse_expr()?;

        Ok(Expr::Closure {
            params,
            body: Box::new(body),
        })
    }

    /// Skip a type annotation after ':' in a parameter list.
    fn skip_type_annotation(&mut self) -> Result<()> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected type after ':'")));
        }

        let mut consumed_any = false;
        let mut paren_depth = 0;
        let mut bracket_depth = 0;
        let mut angle_depth = 0;

        while !self.eof() {
            let token = &self.tokens[self.pos];
            match token {
                Token::Comma if paren_depth == 0 && bracket_depth == 0 && angle_depth == 0 => break,
                Token::RParen if paren_depth == 0 && bracket_depth == 0 && angle_depth == 0 => break,
                Token::LParen => {
                    paren_depth += 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::RParen => {
                    if paren_depth == 0 {
                        break;
                    }
                    paren_depth -= 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::LBracket => {
                    bracket_depth += 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::RBracket => {
                    if bracket_depth == 0 {
                        return Err(anyhow!(self.err("Unmatched ']' in type annotation")));
                    }
                    bracket_depth -= 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::Lt => {
                    angle_depth += 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                Token::Gt => {
                    if angle_depth == 0 {
                        break;
                    }
                    angle_depth -= 1;
                    consumed_any = true;
                    self.pos += 1;
                }
                _ => {
                    consumed_any = true;
                    self.pos += 1;
                }
            }
        }

        if !consumed_any {
            return Err(anyhow!(self.err("Expected type after ':'")));
        }

        Ok(())
    }

    fn eof(&self) -> bool {
        self.pos >= self.len
    }

    fn err(&self, msg: &str) -> String {
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
}
