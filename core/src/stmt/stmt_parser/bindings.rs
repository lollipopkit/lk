use super::StmtParser;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{ast::Parser as ExprParser, expr::Expr, operator::BinOp, stmt::Stmt, token::Token, val::LiteralVal};
use anyhow::{Result, anyhow};

impl<'a> StmtParser<'a> {
    pub fn parse_let_stmt(&mut self) -> Result<Stmt> {
        self.parse_binding_stmt(Token::Let, "let", false)
    }

    pub fn parse_const_stmt(&mut self) -> Result<Stmt> {
        self.parse_binding_stmt(Token::Const, "const", true)
    }

    fn parse_binding_stmt(&mut self, keyword: Token, keyword_str: &'static str, is_const: bool) -> Result<Stmt> {
        let keyword_pos = self.pos;
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
        let pattern = ExprParser::parse_whole_pattern(pattern_tokens)?;

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
            // `let` keyword through the end of the pattern — the statement's own
            // position. This used to be `self.current_span()`, taken *after* the
            // whole statement was consumed: it named the token that follows, so
            // a `let`'s type error pointed at the next statement, and the last
            // statement in a file had no span at all.
            span: self.span_covering(keyword_pos, end_pos.saturating_sub(1)),
            is_const,
        })
    }

    pub fn parse_assign_stmt_with_id(&mut self, name: String) -> Result<Stmt> {
        // `parse_statement` already matched the `Id`; step over it.
        self.pos += 1;
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
        // `parse_statement` already matched the `Id`; step over it.
        self.pos += 1;

        // The bitwise operators are not `BinOp`s — `a & b` is a call to the
        // `__lk_bit_*` builtin, and giving them a second spelling in `BinOp`
        // would mean a second lowering, a second type rule, and two places for
        // them to disagree. So `a &= b` desugars to what `a = a & b` already
        // parses to.
        // `<<=` / `>>=` are three adjacent tokens, because the lexer never
        // emits a shift: `<<` is two `<`, so `<<=` is `<` then `<=`. Adjacency
        // is what tells them apart from `a < (b <= c)`, the same test
        // `Parser::peek_shift` makes for the shifts themselves.
        if let Some(builtin) = self.peek_shift_assign(self.pos) {
            self.pos += 2;
            let rhs = self.parse_expression()?;
            self.expect_token(Token::Semicolon)?;
            let value = Expr::Call(
                builtin.to_string(),
                vec![Box::new(Expr::Var(name.clone())), Box::new(rhs)],
            );
            return Ok(Stmt::Assign {
                name,
                value: Box::new(value),
                span: self.current_span(),
            });
        }
        if let Some(builtin) = bitwise_compound_builtin(&self.tokens[self.pos]) {
            self.pos += 1;
            let rhs = self.parse_expression()?;
            self.expect_token(Token::Semicolon)?;
            let value = Expr::Call(
                builtin.to_string(),
                vec![Box::new(Expr::Var(name.clone())), Box::new(rhs)],
            );
            return Ok(Stmt::Assign {
                name,
                value: Box::new(value),
                span: self.current_span(),
            });
        }

        let op = match &self.tokens[self.pos] {
            Token::AddAssign => BinOp::Add,
            Token::SubAssign => BinOp::Sub,
            Token::MulAssign => BinOp::Mul,
            Token::DivAssign => BinOp::Div,
            Token::ModAssign => BinOp::Mod,
            _ => return Err(anyhow!("Expected compound assignment operator")),
        };
        self.pos += 1;

        let value = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;

        Ok(Stmt::CompoundAssign {
            name,
            op,
            value: Box::new(value),
            span: self.current_span(),
        })
    }

    /// Where the last segment of an assignment target starts, given the target
    /// runs from `start` (the name) to `assign_pos` (the operator).
    ///
    /// `Some(i)` points at the `[` of a trailing `[key]` or at the `.` of a
    /// trailing `.field`. `None` means the tokens in between are not an access
    /// chain at all, which is this function's way of saying "not my statement".
    fn access_target_last_segment(&self, start: usize, assign_pos: usize) -> Option<usize> {
        if assign_pos <= start + 1 {
            return None;
        }
        if self.tokens.get(assign_pos - 1) == Some(&Token::RBracket) {
            // Back to the `[` that opens it, counting nested brackets so a key
            // that is itself an index (`m[ks[0]] = v`) finds the outer one.
            let mut depth = 0i32;
            let mut i = assign_pos - 1;
            loop {
                match self.tokens.get(i) {
                    Some(Token::RBracket) => depth += 1,
                    Some(Token::LBracket) => {
                        depth -= 1;
                        if depth == 0 {
                            return (i > start).then_some(i);
                        }
                    }
                    None => return None,
                    _ => {}
                }
                if i == start {
                    return None;
                }
                i -= 1;
            }
        }
        // `.field`, where the field is the token before the operator.
        if assign_pos >= start + 3
            && matches!(self.tokens.get(assign_pos - 2), Some(Token::Dot))
            && matches!(self.tokens.get(assign_pos - 1), Some(Token::Id(_) | Token::Str(_)))
        {
            return Some(assign_pos - 2);
        }
        None
    }

    pub fn try_parse_access_assign_stmt_with_id(&mut self, name: String) -> Result<Option<Stmt>> {
        let start = self.pos;
        let mut cursor = self.pos + 1;
        let mut bracket_depth = 0i32;
        let mut assign_pos = None;
        let mut assign_op = None;
        while cursor < self.len {
            match &self.tokens[cursor] {
                Token::LBracket => {
                    bracket_depth += 1;
                    cursor += 1;
                }
                Token::RBracket => {
                    bracket_depth -= 1;
                    cursor += 1;
                }
                Token::Assign
                | Token::AddAssign
                | Token::SubAssign
                | Token::MulAssign
                | Token::DivAssign
                | Token::ModAssign
                | Token::BitAndAssign
                | Token::BitOrAssign
                | Token::BitXorAssign
                    if bracket_depth == 0 =>
                {
                    assign_pos = Some(cursor);
                    assign_op = Some(self.tokens[cursor].clone());
                    break;
                }
                // `<<=` / `>>=`, which are two tokens (see `peek_shift_assign`).
                Token::Lt | Token::Gt if bracket_depth == 0 && self.peek_shift_assign(cursor).is_some() => {
                    assign_pos = Some(cursor);
                    assign_op = Some(self.tokens[cursor].clone());
                    break;
                }
                Token::Semicolon if bracket_depth == 0 => break,
                _ => cursor += 1,
            }
        }
        let Some(assign_pos) = assign_pos else {
            return Ok(None);
        };

        // Where the *last* segment of the target starts. A target is a chain —
        // `p.q.n`, `p.m["b"]`, `xs[0][1]` — and the store belongs to its last
        // step, applied to everything before it.
        //
        // This used to read the *first* segment and discard the rest: `p.m["b"]
        // = 2` became `p.m = 2` and `p.q.n = 5` became `p.q = 5`, both
        // silently, on both engines, and `lk check` had nothing to object to
        // when the field was `Any`. A map or a nested struct was destroyed by
        // an assignment that reads like an update.
        let Some(seg_start) = self.access_target_last_segment(start, assign_pos) else {
            return Ok(None);
        };
        let key = if self.tokens.get(seg_start) == Some(&Token::LBracket) {
            let mut parser = self.expr_parser(&self.tokens[seg_start + 1..assign_pos - 1], None);
            parser.parse()?
        } else {
            match self.tokens.get(seg_start + 1) {
                Some(Token::Id(field)) => Expr::Literal(LiteralVal::from_str(field.as_str())),
                Some(Token::Str(field)) => Expr::Literal(LiteralVal::from_str(field.as_str())),
                other => {
                    let found = other.map_or_else(|| "end of input".to_string(), crate::token::token_lexeme);
                    return Err(anyhow!(
                        self.err(&format!("Expected field name in assignment target, found `{found}`"))
                    ));
                }
            }
        };
        // Everything before the last segment. One token means the target is
        // `name<segment>`, which is the shape this function has always handled
        // and whose desugar re-binds the name; anything longer is a chain, and
        // the store lands on the container that chain names.
        let base_is_the_name = seg_start == start + 1;

        let shift_assign = self.peek_shift_assign(assign_pos);
        self.pos = assign_pos + if shift_assign.is_some() { 2 } else { 1 };
        let rhs = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;

        // The container the store lands on: the name itself for a one-segment
        // target, the chain before the last segment otherwise.
        let base = if base_is_the_name {
            Expr::Var(name.clone())
        } else {
            let mut parser = self.expr_parser(&self.tokens[start..seg_start], None);
            parser.parse()?
        };
        let current = Expr::Access(Box::new(base.clone()), Box::new(key.clone()));
        let value = match assign_op.expect("assignment operator found") {
            Token::Assign => rhs,
            Token::AddAssign => Expr::Bin(Box::new(current), BinOp::Add, Box::new(rhs)),
            Token::SubAssign => Expr::Bin(Box::new(current), BinOp::Sub, Box::new(rhs)),
            Token::MulAssign => Expr::Bin(Box::new(current), BinOp::Mul, Box::new(rhs)),
            Token::DivAssign => Expr::Bin(Box::new(current), BinOp::Div, Box::new(rhs)),
            Token::ModAssign => Expr::Bin(Box::new(current), BinOp::Mod, Box::new(rhs)),
            token => {
                let builtin = shift_assign
                    .or_else(|| bitwise_compound_builtin(&token))
                    .expect("assignment operator matched above");
                Expr::Call(builtin.to_string(), vec![Box::new(current), Box::new(rhs)])
            }
        };

        // A chain has no name to re-bind: the store mutates the container the
        // chain names, and every container in this language is a heap value, so
        // the change is visible through it. `p.m.set("b", 2)` — the spelling
        // that always worked — is the same operation.
        if !base_is_the_name {
            let setter = if self.tokens.get(seg_start) == Some(&Token::LBracket) {
                "__lk_set_index"
            } else {
                "__lk_set_field"
            };
            let store = Expr::CallExpr(
                Box::new(Expr::Var(setter.to_string())),
                vec![Box::new(base), Box::new(key), Box::new(value)],
            );
            return Ok(Some(Stmt::Expr {
                value: Box::new(store),
                span: self.current_span(),
            }));
        }
        if self.tokens.get(start + 1) == Some(&Token::LBracket) && matches!(key, Expr::Literal(LiteralVal::Int(_))) {
            let list_set = Expr::CallExpr(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("list".to_string())),
                    Box::new(Expr::Literal(LiteralVal::from_str("set"))),
                )),
                vec![Box::new(Expr::Var(name.clone())), Box::new(key), Box::new(value)],
            );
            let updated = Expr::Access(Box::new(list_set), Box::new(Expr::Literal(LiteralVal::Int(0))));
            Ok(Some(Stmt::Assign {
                name,
                value: Box::new(updated),
                span: self.current_span(),
            }))
        } else if self.tokens.get(start + 1) == Some(&Token::Dot) {
            let set_field = Expr::CallExpr(
                Box::new(Expr::Var("__lk_set_field".to_string())),
                vec![Box::new(Expr::Var(name.clone())), Box::new(key), Box::new(value)],
            );
            Ok(Some(Stmt::Assign {
                name,
                value: Box::new(set_field),
                span: self.current_span(),
            }))
        } else {
            let map_set = Expr::CallExpr(
                Box::new(Expr::Var("__lk_set_index".to_string())),
                vec![Box::new(Expr::Var(name)), Box::new(key), Box::new(value)],
            );
            Ok(Some(Stmt::expr(Box::new(map_set))))
        }
    }

    pub fn parse_define_stmt_with_id(&mut self, name: String) -> Result<Stmt> {
        // consume Id (already peeked), ':' and '='
        let name_pos = self.pos;
        self.pos += 1; // Id
        self.expect_token(Token::Colon)?;
        self.expect_token(Token::Assign)?;

        let value = self.parse_expression()?;
        self.expect_token(Token::Semicolon)?;
        Ok(Stmt::Define {
            name,
            value: Box::new(value),
            // The name alone: `x := v` has no annotation slot, so a hint goes
            // right after `x`, which is where the span ends.
            span: self.span_covering(name_pos, name_pos),
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

        // A `;` right here means the `return` carries no value.
        let value = if !self.eof() && self.tokens[self.pos] != Token::Semicolon {
            Some(Box::new(self.parse_expression()?))
        } else {
            None
        };

        self.expect_token(Token::Semicolon)?;

        Ok(Stmt::Return { value })
    }
}

/// The `__lk_bit_*` builtin a bitwise compound assignment desugars to.
fn bitwise_compound_builtin(token: &Token) -> Option<&'static str> {
    match token {
        Token::BitAndAssign => Some("__lk_bit_and"),
        Token::BitOrAssign => Some("__lk_bit_or"),
        Token::BitXorAssign => Some("__lk_bit_xor"),
        _ => None,
    }
}

impl<'a> super::StmtParser<'a> {
    /// `<<=` / `>>=` at `at`, as the `__lk_shl` / `__lk_shr` builtin.
    ///
    /// Two tokens (`Lt Le` / `Gt Ge`) that have to be *adjacent* in the source:
    /// `a < b <= c` is three tokens too, and only the spans tell them apart.
    pub(crate) fn peek_shift_assign(&self, at: usize) -> Option<&'static str> {
        let builtin = match (self.tokens.get(at)?, self.tokens.get(at + 1)?) {
            (Token::Lt, Token::Le) => "__lk_shl",
            (Token::Gt, Token::Ge) => "__lk_shr",
            _ => return None,
        };
        if let Some(spans) = &self.token_spans
            && let (Some(first), Some(second)) = (spans.get(at), spans.get(at + 1))
            && first.end.offset != second.start.offset
        {
            return None;
        }
        Some(builtin)
    }
}
