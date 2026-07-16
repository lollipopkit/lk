use super::*;

impl<'a> Parser<'a> {
    /// Parse a pattern for match expressions
    pub fn parse_pattern(&mut self) -> Result<Pattern> {
        self.parse_or_pattern()
    }

    pub fn parse_pattern_prefix(&mut self) -> Result<(Pattern, usize)> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected pattern")));
        }
        let pattern = self.parse_pattern()?;
        Ok((pattern, self.pos))
    }

    /// Parse OR pattern: pattern1 | pattern2
    pub(super) fn parse_or_pattern(&mut self) -> Result<Pattern> {
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
    pub(super) fn parse_guard_pattern(&mut self) -> Result<Pattern> {
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
    pub(super) fn parse_primary_pattern(&mut self) -> Result<Pattern> {
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
                        start: Box::new(Expr::Literal(LiteralVal::Int(start_val))),
                        end: end_expr,
                        inclusive,
                    })
                } else {
                    Ok(Pattern::Literal(LiteralVal::Int(start_val)))
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
                        start: Box::new(Expr::Literal(LiteralVal::Float(start_val))),
                        end: end_expr,
                        inclusive,
                    })
                } else {
                    Ok(Pattern::Literal(LiteralVal::Float(start_val)))
                }
            }
            Token::Str(s) => {
                let val = LiteralVal::from_str(s.as_str());
                self.pos += 1;
                Ok(Pattern::Literal(val))
            }
            Token::Bool(b) => {
                let val = LiteralVal::Bool(*b);
                self.pos += 1;
                Ok(Pattern::Literal(val))
            }
            Token::Nil => {
                self.pos += 1;
                Ok(Pattern::Literal(LiteralVal::Nil))
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
    pub(super) fn parse_select_case(&mut self) -> Result<ParsedSelectCase> {
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
        let arm = if matches!(&self.tokens[self.pos], Token::Id(name) if name == "recv") {
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

            ParsedSelectArm::Recv {
                binding: binding_value,
                channel,
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

            ParsedSelectArm::Send { channel, value }
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
            Some(g)
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

        Ok(ParsedSelectCase { arm, guard, body })
    }
}
