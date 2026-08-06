#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::{Expr, MatchArm, Pattern, TemplateStringPart},
    operator::{BinOp, UnaryOp},
    token::{
        ParseError, Span, TemplateScanError, TemplateSegment, Token, Tokenizer, offset_to_position,
        split_template_string,
    },
    val::{LiteralVal, Type},
};
use anyhow::{Result, anyhow};

mod literals;
mod patterns;
mod support;

use support::BlockTail;

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    len: usize,
    token_spans: Option<&'a [Span]>,
    prefix_mode: bool,
    /// Monotonic id for parse-time desugars (`select`, postfix `!`), so
    /// nested instances don't shadow each other's synthesized locals.
    pub(super) desugar_counter: usize,
    /// Live nesting depth of `parse_expr`, bounded by [`MAX_PARSE_DEPTH`].
    ///
    /// Expression parsing is recursive descent, so nesting depth in the source
    /// is Rust stack depth. Without a bound, `((((…1…))))` overflows the stack
    /// and *aborts the process* — 500 levels was enough in a debug build. On a
    /// host that abort is at least clean (the guard page traps); on bare metal
    /// there is no guard page, so the same input silently walks off the stack
    /// into whatever is below it.
    pub(crate) depth: usize,
}

/// Cap on source nesting depth, shared by the expression and statement
/// parsers (see [`Parser::depth`] and `StmtParser::depth`).
///
/// Hand-written code does not approach this — the bound exists to turn a
/// pathological or hostile input into a syntax error instead of an abort.
///
/// One budget, not two. `if c { if c { … } }` alternates between the two
/// parsers, so a per-parser budget bounds neither: each crossing would hand
/// the next level a fresh allowance and the combined nesting would be
/// unbounded. Both parsers count into the same budget and seed it across
/// every crossing.
///
/// The value is set from measurement, not taste. One level of *source*
/// nesting costs about 18KiB of debug stack in a plain expression, because it
/// unwinds the whole precedence chain (`conditional` → `nullish` → `or` → …
/// → `postfix` → `primary` → `paren`) rather than one frame; a level that
/// crosses into the statement parser and back (`if`, `match`, a block) costs
/// several times that. The smallest stack this has to survive is a libtest
/// thread's 2MiB, which is where the cap is measured — `deeply_nested_*` in
/// `stmt_test.rs` and `ast_test.rs` are that measurement, and they abort the
/// whole test process rather than fail if the cap is ever raised past it.
#[cfg(feature = "std")]
pub(crate) const MAX_PARSE_DEPTH: usize = 64;
/// An MCU stack is kilobytes, not megabytes, so bare metal gets a tighter cap.
#[cfg(not(feature = "std"))]
pub(crate) const MAX_PARSE_DEPTH: usize = 16;

/// Nesting-budget exhaustion, kept distinguishable from an ordinary syntax
/// error.
///
/// A speculative parse treats a syntax error as "not this shape" and lets the
/// next candidate retry the same tokens — `try { … } catch e { }` is refused
/// by the expression parser and accepted by the statement parser, so that
/// retry is load-bearing. Budget exhaustion is not shape information: every
/// candidate fails it, and retrying each of them at every level doubles the
/// work per level. 256 nested `if`s did not finish in five minutes while the
/// two were indistinguishable.
#[derive(Debug)]
pub(crate) struct NestingTooDeep;

impl core::fmt::Display for NestingTooDeep {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "nesting too deep")
    }
}

impl core::error::Error for NestingTooDeep {}

struct StructLiteralParts {
    fields: Vec<(String, Box<Expr>)>,
    update_base: Option<Box<Expr>>,
}

/// Parser-local shape of one parsed `case` arm — consumed immediately by
/// `desugar_select`, never part of the public AST.
enum ParsedSelectArm {
    Recv { binding: Option<String>, channel: Expr },
    Send { channel: Expr, value: Expr },
}

struct ParsedSelectCase {
    arm: ParsedSelectArm,
    guard: Option<Expr>,
    body: Expr,
}

/// The name a parse-time desugar binds its temporary under.
///
/// `$` cannot appear in a source identifier — the lexer will not produce one —
/// which is why the internal builtins (`try$call`, `select$block`) already use
/// it. Minting every desugar local through here keeps the two halves from
/// drifting: the name nothing can collide with, and the name tools recognise
/// as *not the writer's*. They used to be `__unwrap0` / `__optcall0`, which a
/// program may legitimately spell, so no filter could tell them apart — and
/// the editor's outline listed them beside the real variables.
pub(crate) fn desugar_local(kind: &str, id: usize) -> String {
    format!("{kind}${id}")
}

/// Is `name` a local a desugar minted, rather than one the writer bound?
pub fn is_desugar_local(name: &str) -> bool {
    name.contains('$')
}

/// Build the desugared AST for `a?.m(args)`.
///
/// `Vec<Box<Expr>>` is the AST's own argument type, not a boxing choice made
/// here — see `Expr::CallExpr`.
///
/// `{ let t = a; t == nil ? nil : t.m(args) }` — the receiver is evaluated
/// once, and the call does not happen at all when it is nil.
#[allow(clippy::vec_box, reason = "the AST stores arguments as `Vec<Box<Expr>>`")]
fn desugar_optional_call(id: usize, receiver: Expr, field: Expr, args: Vec<Box<Expr>>) -> Expr {
    use crate::stmt::Stmt;

    let name = desugar_local("optcall", id);
    let binding = Box::new(Stmt::Let {
        pattern: Pattern::Variable(name.clone()),
        type_annotation: None,
        value: Box::new(receiver),
        span: None,
        is_const: false,
    });
    let call = Expr::CallExpr(
        Box::new(Expr::Access(Box::new(Expr::Var(name.clone())), Box::new(field))),
        args,
    );
    let check = Expr::Conditional(
        Box::new(Expr::Bin(
            Box::new(Expr::Var(name)),
            BinOp::Eq,
            Box::new(Expr::Literal(LiteralVal::Nil)),
        )),
        Box::new(Expr::Literal(LiteralVal::Nil)),
        Box::new(call),
    );
    Expr::Block(vec![binding, Box::new(Stmt::expr(Box::new(check)))])
}

/// Build the desugared AST for a postfix `!` unwrap (see `parse_postfix`).
fn desugar_unwrap(id: usize, operand: Expr) -> Expr {
    use crate::stmt::Stmt;

    let name = desugar_local("unwrap", id);
    let binding = Box::new(Stmt::Let {
        pattern: Pattern::Variable(name.clone()),
        type_annotation: None,
        value: Box::new(operand),
        span: None,
        is_const: false,
    });
    let check = Expr::Conditional(
        Box::new(Expr::Bin(
            Box::new(Expr::Var(name.clone())),
            BinOp::Eq,
            Box::new(Expr::Literal(LiteralVal::Nil)),
        )),
        Box::new(Expr::Call(
            "error".to_string(),
            vec![Box::new(Expr::Literal(LiteralVal::from_str("unwrap of nil value")))],
        )),
        Box::new(Expr::Var(name)),
    );
    Expr::Block(vec![binding, Box::new(Stmt::expr(Box::new(check)))])
}

/// Build the desugared AST for a parsed `select` (see `parse_select` for the
/// full shape and the semantics it pins).
fn desugar_select(id: usize, cases: Vec<ParsedSelectCase>, default_case: Option<Expr>) -> Expr {
    use crate::stmt::Stmt;

    fn let_stmt(name: String, value: Expr) -> Box<Stmt> {
        Box::new(Stmt::Let {
            pattern: Pattern::Variable(name),
            type_annotation: None,
            value: Box::new(value),
            span: None,
            is_const: false,
        })
    }
    fn int_lit(value: i64) -> Expr {
        Expr::Literal(LiteralVal::Int(value))
    }
    fn bool_lit(value: bool) -> Expr {
        Expr::Literal(LiteralVal::Bool(value))
    }
    fn nil_lit() -> Expr {
        Expr::Literal(LiteralVal::Nil)
    }
    fn index(expr: Expr, at: i64) -> Expr {
        Expr::Access(Box::new(expr), Box::new(int_lit(at)))
    }

    let has_default = default_case.is_some();
    let mut statements: Vec<Box<Stmt>> = Vec::new();
    let mut types: Vec<Box<Expr>> = Vec::with_capacity(cases.len());
    let mut channels: Vec<Box<Expr>> = Vec::with_capacity(cases.len());
    let mut values: Vec<Box<Expr>> = Vec::with_capacity(cases.len());
    let mut guards: Vec<Box<Expr>> = Vec::with_capacity(cases.len());
    let mut arms: Vec<(Option<String>, Expr)> = Vec::with_capacity(cases.len());

    // Channel operands, send values, and guards evaluate eagerly, in source
    // order (Go's rule), into synthesized locals.
    for (i, case) in cases.into_iter().enumerate() {
        let channel_name = format!("{}_ch_{i}", desugar_local("select", id));
        let (kind, binding) = match case.arm {
            ParsedSelectArm::Recv { binding, channel } => {
                statements.push(let_stmt(channel_name.clone(), channel));
                values.push(Box::new(nil_lit()));
                (0, binding)
            }
            ParsedSelectArm::Send { channel, value } => {
                statements.push(let_stmt(channel_name.clone(), channel));
                let value_name = format!("{}_v_{i}", desugar_local("select", id));
                statements.push(let_stmt(value_name.clone(), value));
                values.push(Box::new(Expr::Var(value_name)));
                (1, None)
            }
        };
        let guard_name = format!("{}_g_{i}", desugar_local("select", id));
        // Normalize any truthy guard to a real Bool — `select$block` treats
        // non-Bool guard entries as disabled.
        let guard_value = match case.guard {
            Some(guard) => Expr::Conditional(Box::new(guard), Box::new(bool_lit(true)), Box::new(bool_lit(false))),
            None => bool_lit(true),
        };
        statements.push(let_stmt(guard_name.clone(), guard_value));
        types.push(Box::new(int_lit(kind)));
        channels.push(Box::new(Expr::Var(channel_name)));
        guards.push(Box::new(Expr::Var(guard_name)));
        arms.push((binding, case.body));
    }

    let result_name = format!("{}_r", desugar_local("select", id));
    statements.push(let_stmt(
        result_name.clone(),
        Expr::Call(
            "select$block".to_string(),
            vec![
                Box::new(Expr::List(types)),
                Box::new(Expr::List(channels)),
                Box::new(Expr::List(values)),
                Box::new(Expr::List(guards)),
                Box::new(bool_lit(has_default)),
            ],
        ),
    ));

    // Innermost-out conditional chain over the fired arm's index; a recv
    // binding is a plain let over `r[2][1]` (payload = [ok, value]) scoped to
    // its own arm block.
    let mut dispatch = nil_lit();
    for (i, (binding, body)) in arms.into_iter().enumerate().rev() {
        let arm_body = match binding {
            Some(name) => Expr::Block(vec![
                let_stmt(name, index(index(Expr::Var(result_name.clone()), 2), 1)),
                Box::new(Stmt::expr(Box::new(body))),
            ]),
            None => body,
        };
        dispatch = Expr::Conditional(
            Box::new(Expr::Bin(
                Box::new(index(Expr::Var(result_name.clone()), 1)),
                BinOp::Eq,
                Box::new(int_lit(i as i64)),
            )),
            Box::new(arm_body),
            Box::new(dispatch),
        );
    }
    let top = Expr::Conditional(
        Box::new(index(Expr::Var(result_name), 0)),
        Box::new(default_case.unwrap_or_else(nil_lit)),
        Box::new(dispatch),
    );
    statements.push(Box::new(Stmt::expr(Box::new(top))));
    Expr::Block(statements)
}

impl<'a> Parser<'a> {
    pub fn parse(&mut self) -> Result<Expr> {
        if self.eof() {
            return Ok(Expr::Literal(LiteralVal::Nil));
        }

        let exp = self.parse_expr()?;

        if !self.eof() {
            return Err(anyhow!(self.err("Unexpected tokens at end")));
        }

        // All sub-expressions parsed, apply constant folding optimization
        Ok(exp.fold_constants())
    }

    pub fn parse_prefix(&mut self) -> Result<(Expr, usize)> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected expression")));
        }
        let previous_prefix_mode = self.prefix_mode;
        self.prefix_mode = true;
        let result = self.parse_expr();
        self.prefix_mode = previous_prefix_mode;
        let exp = result?;
        Ok((exp.fold_constants(), self.pos))
    }

    /// Parse with enhanced error information that includes position
    pub fn parse_with_enhanced_errors(&mut self, input: &str) -> core::result::Result<Expr, ParseError> {
        if self.eof() {
            return Ok(Expr::Literal(LiteralVal::Nil));
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

    /// Runs `parse` one level deeper, refusing to go past [`MAX_PARSE_DEPTH`].
    ///
    /// Every recursive descent that can nest without bound has to go through
    /// here, not just `parse_expr`: prefix operators recurse into themselves
    /// (`!!!…x`) and `match` arms recurse into `parse_conditional` directly,
    /// so bounding only `parse_expr` left both able to overflow the stack.
    fn deeper<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.depth >= MAX_PARSE_DEPTH {
            return Err(anyhow::Error::new(NestingTooDeep).context(self.err("Expression nesting too deep")));
        }
        self.depth += 1;
        // Decremented on the error path too — a bounded parse that fails must
        // not leave the counter raised for whatever the caller tries next.
        let parsed = parse(self);
        self.depth -= 1;
        parsed
    }

    /// A parser over a token sub-slice that continues *this* parser's depth
    /// budget. A nested parse is still nesting even when it gets its own
    /// `Parser`, so starting the sub-parser back at zero would hand a
    /// deeply-nested construct an unbounded budget one slice at a time.
    fn sub_parser<'b>(&self, tokens: &'b [Token]) -> Parser<'b> {
        let mut parser = Parser::new(tokens);
        parser.depth = self.depth;
        parser
    }

    fn sub_parser_with_spans<'b>(&self, tokens: &'b [Token], spans: &'b [Span]) -> Parser<'b> {
        let mut parser = Parser::new_with_spans(tokens, spans);
        parser.depth = self.depth;
        parser
    }

    /// Every nested expression form routes back through here.
    fn parse_expr(&mut self) -> Result<Expr> {
        self.deeper(Self::parse_conditional)
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
        let mut expr = self.parse_bit_or()?;
        while !self.eof() {
            match self.tokens[self.pos] {
                Token::And => {
                    self.pos += 1;
                    let right = self.parse_bit_or()?;
                    expr = Expr::And(Box::new(expr), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// `expr | expr`
    fn parse_bit_or(&mut self) -> Result<Expr> {
        let mut expr = self.parse_bit_and()?;
        while !self.eof() && self.tokens[self.pos] == Token::Pipe {
            self.pos += 1;
            let right = self.parse_bit_and()?;
            expr = Self::builtin_call("__lk_bit_or", vec![expr, right]);
        }
        Ok(expr)
    }

    /// `expr & expr`
    fn parse_bit_and(&mut self) -> Result<Expr> {
        let mut expr = self.parse_cmp()?;
        while !self.eof() && self.tokens[self.pos] == Token::BitAnd {
            self.pos += 1;
            let right = self.parse_cmp()?;
            expr = Self::builtin_call("__lk_bit_and", vec![expr, right]);
        }
        Ok(expr)
    }

    /// `expr << expr` / `expr >> expr`
    ///
    /// Shifts are two adjacent comparison tokens rather than tokens of their
    /// own, because the lexer has no way to tell `List<List<Int>>` from a right
    /// shift — Rust has the same problem and splits the token back apart in
    /// type position. Recognising the pair *here*, in the expression grammar,
    /// means the type parser never sees anything new: it goes on consuming one
    /// `>` at a time, and no generic annotation can be broken by this.
    ///
    /// Adjacency is required when spans are available, so `a < < b` is not
    /// silently a shift. Between comparison and addition, as in Rust: `a + b <<
    /// c` shifts the sum, and `a << b == c` compares the shift.
    fn parse_shift(&mut self) -> Result<Expr> {
        let mut expr = self.parse_add_sub()?;
        while let Some(builtin) = self.peek_shift() {
            self.pos += 2;
            let right = self.parse_add_sub()?;
            expr = Self::builtin_call(builtin, vec![expr, right]);
        }
        Ok(expr)
    }

    /// `<<` or `>>` at the cursor, as a pair of adjacent tokens.
    fn peek_shift(&self) -> Option<&'static str> {
        if self.pos + 1 >= self.len {
            return None;
        }
        let builtin = match (&self.tokens[self.pos], &self.tokens[self.pos + 1]) {
            (Token::Lt, Token::Lt) => "__lk_shl",
            (Token::Gt, Token::Gt) => "__lk_shr",
            _ => return None,
        };
        if let Some(spans) = self.token_spans
            && let (Some(first), Some(second)) = (spans.get(self.pos), spans.get(self.pos + 1))
            && first.end.offset != second.start.offset
        {
            return None;
        }
        Some(builtin)
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
        let mut expr = self.parse_shift()?;

        if !self.eof() && (self.tokens[self.pos] == Token::Range || self.tokens[self.pos] == Token::RangeInclusive) {
            let inclusive = self.tokens[self.pos] == Token::RangeInclusive;
            self.pos += 1; // consume '..' or '..='

            // Check if there's an end expression
            let end = if !self.eof() && !self.is_range_terminator() {
                Some(Box::new(self.parse_shift()?))
            } else {
                None
            };

            // Optional explicit step indicated by another '..'
            let step = if !self.eof() && self.tokens[self.pos] == Token::Range {
                self.pos += 1; // consume '..'
                if self.eof() || self.is_range_terminator() {
                    return Err(anyhow!(self.err("Expected step expression after '..'")));
                }
                Some(Box::new(self.parse_shift()?))
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
        let mut expr = self.parse_cast()?;
        while !self.eof() {
            let op = match self.tokens[self.pos] {
                Token::Mul => BinOp::Mul,
                Token::Div => BinOp::Div,
                Token::Mod => BinOp::Mod,
                _ => break,
            };
            self.pos += 1;
            let right = self.parse_cast()?;
            expr = Expr::Bin(Box::new(expr), op, Box::new(right));
        }
        Ok(expr)
    }

    /// `unsafe { … }`.
    ///
    /// The braces are required — there is no bare `unsafe expr` form — so the
    /// region a reader has to audit is always delimited, and the parse never
    /// has to guess how far the marker reaches.
    fn parse_unsafe_block(&mut self) -> Result<Expr> {
        self.pos += 1;
        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            return Err(anyhow!(self.err("Expecting '{' after 'unsafe'")));
        }
        let block = self.parse_brace_block(support::BlockTail::Value)?;
        Ok(Expr::Unsafe(Box::new(block)))
    }

    /// The type after `as`.
    ///
    /// Only a bare type name, not the full annotation grammar the statement
    /// parser handles: a cast target is `u8` or `Int`, never `Map<K, V>` or a
    /// function type. Keeping it narrow avoids having to disambiguate `<` here
    /// from a comparison, which is exactly the ambiguity that makes C-style
    /// casts hard to parse.
    fn parse_cast_target(&mut self) -> Result<Type> {
        // A pointer prefix: `as *mut u32`. `*` after `as` is unambiguous —
        // a type position never holds a multiplication.
        if matches!(self.tokens.get(self.pos), Some(Token::Mul)) {
            self.pos += 1;
            let mutable = matches!(self.tokens.get(self.pos), Some(Token::Id(name)) if name == "mut");
            if mutable {
                self.pos += 1;
            }
            let pointee = self.parse_cast_target()?;
            return Ok(Type::Ptr {
                pointee: Box::new(pointee),
                mutable,
            });
        }
        let Some(Token::Id(name)) = self.tokens.get(self.pos) else {
            return Err(anyhow!(self.err("Expecting a type name after 'as'")));
        };
        let Some(ty) = Type::parse(name) else {
            let msg = alloc::format!("Unknown type '{name}' after 'as'");
            return Err(anyhow!(self.err(&msg)));
        };
        self.pos += 1;
        Ok(ty)
    }

    /// - `expr as T`
    ///
    /// Binds tighter than the binary operators and looser than unary, so
    /// `a * b as u8` is `a * (b as u8)` and `!x as u8` is `(!x) as u8` —
    /// the same precedence Rust gives it. Left-associative: `x as u8 as u32`
    /// is `(x as u8) as u32`, which is how a double conversion is written.
    fn parse_cast(&mut self) -> Result<Expr> {
        let mut expr = self.parse_unary()?;
        while !self.eof() && self.tokens[self.pos] == Token::As {
            self.pos += 1;
            let ty = self.parse_cast_target()?;
            expr = Expr::Cast(Box::new(expr), ty);
        }
        Ok(expr)
    }

    /// - `!expr`
    /// - `-expr`
    /// - `expr`
    fn parse_unary(&mut self) -> Result<Expr> {
        if self.eof() {
            return Err(anyhow!(self.err("Expected expression")));
        }
        let token = &self.tokens[self.pos];
        match token {
            // `-expr`.
            //
            // The lexer already folds a *literal* `-5` into `Int(-5)` where it
            // can tell an operand is expected, which is why the language got
            // this far without a negation operator at all: `-5` worked and
            // `-x` was a syntax error everywhere, with `0 - x` as the
            // workaround. That lexer path stays — it is the only thing that can
            // spell `-9223372036854775808`, whose magnitude does not fit in an
            // `i64` — so this arm folds literals the same way to keep the two
            // routes producing identical code.
            Token::Sub => {
                self.pos += 1;
                let expr = self.deeper(Self::parse_unary)?;
                Ok(match expr {
                    Expr::Literal(LiteralVal::Int(value)) => Expr::Literal(LiteralVal::Int(-value)),
                    Expr::Literal(LiteralVal::Float(value)) => Expr::Literal(LiteralVal::Float(-value)),
                    other => Expr::Unary(UnaryOp::Neg, Box::new(other)),
                })
            }
            Token::Not => {
                self.pos += 1;
                let expr = self.deeper(Self::parse_unary)?;
                Ok(Expr::Unary(UnaryOp::Not, Box::new(expr)))
            }
            Token::BitNot => {
                self.pos += 1;
                let expr = self.deeper(Self::parse_unary)?;
                Ok(Self::builtin_call("__lk_bit_not", vec![expr]))
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
                    } else if self.eof() || self.tokens[self.pos] != Token::RParen {
                        return Err(anyhow!(self.err("Expected ',' or ')' in function call")));
                    }
                }

                if self.eof() || self.tokens[self.pos] != Token::RParen {
                    return Err(anyhow!(self.err("Expected ')' to close function call")));
                }
                self.pos += 1; // skip ')'

                // `a?.m(args)` is a *call*, and `OptionalAccess` is a read:
                // the compiler lowers it as an index, so `s?.len()` indexed the
                // string with the string `"len"` and failed at runtime with
                // "String index must be Int" — on the one operator that exists
                // for values which may be nil.
                //
                // Rewritten here into the conditional it means, the way postfix
                // `!` is. The checker and the compiler then see ordinary
                // constructs, and the result is `T?` because one branch is nil
                // — the rule every other maybe-missing branch follows.
                let optional_receiver = matches!((&expr, saw_named), (Expr::OptionalAccess(_, _), false));
                if optional_receiver {
                    let Expr::OptionalAccess(receiver, field) = expr else {
                        unreachable!("checked just above");
                    };
                    let id = self.desugar_counter;
                    self.desugar_counter += 1;
                    expr = desugar_optional_call(id, *receiver, *field, pos_args);
                } else if saw_named {
                    expr = Expr::CallNamed(Box::new(expr), pos_args, named_args);
                } else {
                    expr = Expr::CallExpr(Box::new(expr), pos_args);
                }
            } else if !self.eof() && self.tokens[self.pos] == Token::LBrace {
                // Possible struct literal: only allowed immediately after a variable name
                if let Expr::Var(name) = &expr {
                    expr = self.parse_struct_literal_after_name(name.clone())?;
                } else if self.prefix_mode {
                    break;
                } else if let Some(literal) = self.parse_qualified_struct_literal(&expr)? {
                    expr = literal;
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
                // `a?.m(args)` is a *call*, and `OptionalAccess` is a read: the
                // compiler lowers it as an index, so `s?.len()` indexed the
                // string with the string `"len"` and failed at runtime with
                // "String index must be Int" — on the one operator that exists
                // for values which may be nil.
                //
                // Desugared here, the way postfix `!` is, into the conditional
                // it means. Both the checker and the compiler then see ordinary
                // constructs: the result is `T?` because one branch is nil,
                // which is the rule every other maybe-missing branch follows.
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
            } else if !self.eof() && self.tokens[self.pos] == Token::Not && !self.macro_invocation_follows(&expr) {
                // Postfix `!` — Swift-style force unwrap, parse-time sugar:
                // `expr!` ⇒ `{ let __unwrap{n} = expr;
                //              __unwrap{n} == nil ? error("unwrap of nil value")
                //                                 : __unwrap{n} }`
                // Raises a catchable error on nil, evaluates to the value
                // otherwise. Two boundaries: a `!` that continues a *macro
                // name* is a macro invocation, not an unwrap (see
                // [`Parser::macro_invocation_follows`]), and the lexer greedily
                // takes `!=` as Ne, so `x!== 1` is a parse error — write
                // `x! == 1`.
                self.pos += 1;
                self.desugar_counter += 1;
                expr = desugar_unwrap(self.desugar_counter, expr);
            } else if !self.eof() && self.tokens[self.pos] == Token::Not && self.macro_invocation_follows(&expr) {
                // A macro invocation that reached the parser is one macro
                // expansion left alone, and expansion runs first — so no macro
                // of this name is defined. Saying that beats what the fall
                // through said: `nope!()` left the `!` unconsumed and reported
                // "Unexpected tokens at end (found Not)", which names a token
                // the program does not contain and no macro at all.
                let Expr::Var(name) = &expr else {
                    unreachable!("macro_invocation_follows only answers true for a bare name");
                };
                let msg = alloc::format!(
                    "no macro named `{name}` is defined — `{name}!(…)` is a macro invocation, \
                     and to call an unwrapped value write `({name}!)(…)`"
                );
                return Err(anyhow!(self.err(&msg)));
            } else {
                break; // No more postfix operations
            }
        }

        Ok(expr)
    }

    /// Whether the `!` at the cursor continues a **macro invocation** rather
    /// than being a postfix unwrap.
    ///
    /// A macro name is an identifier, so only a bare name can be one:
    /// `name!(…)`, `name![…]`, `name!{…}`. The test used to be the open
    /// delimiter alone, which made `m["a"]![0]` — unwrap a map read, then index
    /// it — a "macro invocation reached the parser" error, for a spelling no
    /// macro could ever have. `xs[0]![0]` likewise. The workaround was to
    /// parenthesise or split the line, for an expression with no ambiguity in
    /// it at all.
    ///
    /// To *call* an unwrapped value bound to a bare name, parenthesise:
    /// `(f!)(…)`. That one really is ambiguous, and the name goes to the macro.
    fn macro_invocation_follows(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Var(_))
            && matches!(
                self.tokens.get(self.pos + 1),
                Some(Token::LParen | Token::LBracket | Token::LBrace)
            )
    }

    /// Parse struct fields: '{ id: expr, ... }'
    fn parse_struct_literal_after_name(&mut self, name: String) -> Result<Expr> {
        let parts = self.parse_struct_fields()?;
        if let Some(base) = parts.update_base {
            let overlay = Expr::Map(
                parts
                    .fields
                    .into_iter()
                    .map(|(key, value)| (Box::new(Expr::Literal(LiteralVal::from_str(&key))), value))
                    .collect(),
            );
            let fields = Self::builtin_call("__lk_merge_fields", vec![*base, overlay]);
            Ok(Self::builtin_call(
                "__lk_make_struct",
                vec![Expr::Literal(LiteralVal::from_str(&name)), fields],
            ))
        } else {
            Ok(Expr::StructLiteral {
                name,
                fields: parts.fields,
            })
        }
    }

    /// `module.Type { field: value, … }` — the constructor call it desugars to.
    ///
    /// The object has to be built *by the module that declares the type*: a
    /// type's identity carries its defining module (`val::TypeScope`), and one
    /// built here would not be the type `impl … for Type` was registered
    /// against. So this becomes `module.Type$new(field: value, …)`, the
    /// constructor `stmt::struct_ctors` puts beside every `struct` — an
    /// ordinary cross-module call, which runs *there*.
    ///
    /// Named arguments, so the call site needs to know nothing about the
    /// declaration: the same `field: value` pairs the literal is written with,
    /// and a missing or misspelled one is the constructor's own arity error.
    ///
    /// `None` when the receiver is not a qualified name (`m.f() { … }`, a map
    /// field followed by a block) — the caller then reports its own error.
    fn parse_qualified_struct_literal(&mut self, expr: &Expr) -> Result<Option<Expr>> {
        let Expr::Access(module, field) = expr else {
            return Ok(None);
        };
        let Expr::Var(_) = module.as_ref() else {
            return Ok(None);
        };
        let Expr::Literal(name) = field.as_ref() else {
            return Ok(None);
        };
        let Some(type_name) = name.as_str() else {
            return Ok(None);
        };
        // A type, by the same spelling rule the rest of the language uses.
        if !type_name.starts_with(char::is_uppercase) {
            return Ok(None);
        }
        let parts = self.parse_struct_fields()?;
        if parts.update_base.is_some() {
            return Err(anyhow!(self.err(
                "`..base` update syntax needs the type's own module: build the value there and update it here"
            )));
        }
        let callee = Expr::Access(
            module.clone(),
            Box::new(Expr::Literal(LiteralVal::from_str(
                &crate::stmt::struct_ctors::constructor_name(type_name),
            ))),
        );
        Ok(Some(Expr::CallNamed(Box::new(callee), Vec::new(), parts.fields)))
    }

    fn parse_struct_fields(&mut self) -> Result<StructLiteralParts> {
        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            return Err(anyhow!(self.err("Expecting '{' to start struct literal")));
        }
        self.pos += 1;

        let mut fields: Vec<(String, Box<Expr>)> = Vec::new();
        let mut update_base: Option<Box<Expr>> = None;

        // Allow empty struct: Type {}
        if !self.eof() && self.tokens[self.pos] == Token::RBrace {
            self.pos += 1;
            return Ok(StructLiteralParts { fields, update_base });
        }

        loop {
            // The token stream can end here — a struct literal inside a string
            // interpolation is cut at the first `}`, so `"${R {}}"` arrives as
            // `R {` and nothing more. Reading past the end panicked the parser;
            // an unterminated literal is a syntax error like any other.
            if self.eof() {
                return Err(anyhow!(self.err("Unexpected end in struct literal fields")));
            }
            if self.tokens[self.pos] == Token::Range {
                if update_base.is_some() {
                    return Err(anyhow!(self.err("Duplicate struct update base")));
                }
                self.pos += 1;
                if self.eof() || !self.is_valid_expr_start() {
                    return Err(anyhow!(self.err("Expected expression after '..' in struct update")));
                }
                update_base = Some(Box::new(self.parse_expr()?));

                if self.eof() {
                    return Err(anyhow!(self.err("Unexpected end in struct literal fields")));
                }
                match &self.tokens[self.pos] {
                    Token::Comma => {
                        self.pos += 1;
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
                        return Err(anyhow!(self.err("Expected ',' or '}' in struct literal")));
                    }
                }
            }

            // Field name. A keyword names one unambiguously here — a struct
            // literal's `{ … }` holds `name: value` pairs and nothing else.
            let key = if let Token::Id(id) = &self.tokens[self.pos] {
                let k = id.clone();
                self.pos += 1;
                k
            } else if let Some(word) = crate::token::keyword_as_name(&self.tokens[self.pos]) {
                self.pos += 1;
                word.to_string()
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

        Ok(StructLiteralParts { fields, update_base })
    }

    fn builtin_call(name: &str, args: Vec<Expr>) -> Expr {
        Expr::Call(name.to_string(), args.into_iter().map(Box::new).collect())
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
                Ok(Expr::Literal(LiteralVal::Nil))
            }
            Token::Bool(b) => {
                self.pos += 1;
                Ok(Expr::Literal(LiteralVal::Bool(*b)))
            }
            Token::Int(i) => {
                self.pos += 1;
                Ok(Expr::Literal(LiteralVal::Int(*i)))
            }
            // A radix literal that needs all 64 bits *is* a `u64`, so it is
            // parsed as one — the same expression `0x8000_0000_0000_0000 as u64`
            // builds, which already worked and is what the error used to tell
            // people to write.
            //
            // Saying it here rather than relaxing the range check is what keeps
            // `let y: u8 = -1` refused: the carrier cannot tell those two apart,
            // and by this point the token still can.
            Token::UInt { value, .. } => {
                let value = *value;
                self.pos += 1;
                Ok(Expr::Cast(
                    Box::new(Expr::Literal(LiteralVal::Int(value as i64))),
                    crate::val::Type::MachineInt(crate::val::IntKind::U64),
                ))
            }
            Token::Float(f) => {
                self.pos += 1;
                Ok(Expr::Literal(LiteralVal::Float(*f)))
            }
            Token::Str(s) => {
                self.pos += 1;
                Ok(Expr::Literal(LiteralVal::from_str(s.as_str())))
            }
            Token::TemplateString(content) => {
                self.pos += 1;
                self.parse_template_string_content(content)
            }
            Token::LBracket => self.parse_list(),
            // `{` opens a map *or* a block — see `brace_opens_a_block`.
            Token::LBrace if self.brace_opens_a_block() => self.parse_brace_block(BlockTail::Value),
            Token::LBrace => self.parse_map(),
            Token::Select => self.parse_select(),
            Token::Unsafe => self.parse_unsafe_block(),
            Token::Match => self.parse_match(),
            Token::If => self.parse_if_expr(),
            Token::Try => self.parse_try_expr(),
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

    /// Parse a `select` expression and desugar it at parse time — the same
    /// treatment `try`/`catch` gets (→ `pcall`): there is no `Expr::Select`
    /// node. The cases lower onto the `select$block` runtime builtin (whose
    /// `$` name is untokenizable, so user code can't collide with it) plus
    /// ordinary let/list/conditional AST, so the resolver, type checker, VM
    /// compiler, and AOT all handle `select` with zero dedicated code:
    ///
    /// ```text
    /// select { case v <- recv(ch) if g => b0; case send(ch2, x) => b1; default => d }
    /// ⇓
    /// {
    ///     let __select{n}_ch_0 = ch;   let __select{n}_g_0 = g ? true : false;
    ///     let __select{n}_ch_1 = ch2;  let __select{n}_v_1 = x;  let __select{n}_g_1 = true;
    ///     let __select{n}_r = select$block([0, 1], [ch_0, ch_1], [nil, v_1], [g_0, g_1], true);
    ///     __select{n}_r[0] ? d
    ///         : __select{n}_r[1] == 0 ? { let v = __select{n}_r[2][1]; b0 }
    ///         : __select{n}_r[1] == 1 ? b1
    ///         : nil
    /// }
    /// ```
    ///
    /// Semantics pinned by this shape: channel operands, send values, and
    /// guards evaluate eagerly in source order (like Go); guards normalize to
    /// Bool via the conditional (any truthy value enables the arm) and are
    /// evaluated *outside* the recv binding's scope (the binding doesn't
    /// exist until an arm is chosen); a recv binding gets the received
    /// *value* (`nil` once the channel is closed — the Go zero-value
    /// analogue); with no `default` and no ready arm the call blocks the
    /// thread, and with no arms *and* no default it evaluates to `nil`.
    /// `{n}` is a per-parser counter so nested selects don't shadow each
    /// other's synthesized locals.
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

                    default_case = Some(expr);
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

        self.desugar_counter += 1;
        Ok(desugar_select(self.desugar_counter, cases, default_case))
    }

    /// Parse match expression: match value { pattern => expr, ... }
    /// The expression between a keyword and its `{ … }` — a `match` scrutinee
    /// or an `if` condition.
    ///
    /// Parsed from a token slice that stops at the first *top-level* `{`,
    /// because postfix parsing would otherwise read `match x {` as the struct
    /// literal `x { … }`. Leaves `self.pos` on that brace; the caller consumes
    /// it. The cost is that a struct literal cannot be written bare in this
    /// position — `if Point { x: 1 } == p` needs parentheses — which is the
    /// same trade Rust makes, for the same reason.
    fn parse_header_expr_before_brace(&mut self, keyword: &str) -> Result<Box<Expr>> {
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
                Token::LBrace if paren == 0 && bracket == 0 => break,
                _ => i += 1,
            }
        }

        if i == start_pos {
            // The only expression that can start with `{` is a map literal, and
            // here `{` is the body's — so say which way out there is rather
            // than only that this is wrong.
            let msg = alloc::format!(
                "Expected an expression before '{{' in {keyword}: a `{{` here opens the body, \
                 so a map literal must be parenthesised — `{keyword} ({{…}}) {{ … }}`"
            );
            return Err(anyhow!(self.err(&msg)));
        }

        let value_tokens = &self.tokens[start_pos..i];
        let value_spans = self.token_spans.map(|sp| &sp[start_pos..i]);
        let mut sub = if let Some(spans) = value_spans {
            self.sub_parser_with_spans(value_tokens, spans)
        } else {
            self.sub_parser(value_tokens)
        };
        let value = Box::new(sub.parse_expr()?);
        self.pos = i;

        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            let msg = alloc::format!("Expecting '{{' after the {keyword} expression");
            return Err(anyhow!(self.err(&msg)));
        }
        Ok(value)
    }

    /// `try { … } catch e { … }`, which is an expression like `if` and `match`.
    ///
    /// The value is the body's trailing expression, or the handler's when the
    /// body raised — the same rule `if` uses for its two branches, including
    /// "a branch that ends in a statement yields nil". `let r = try { … }
    /// catch e { … };` used to be a syntax error, so the way to get a value out
    /// was to declare a `nil` first and assign into it from both halves, or to
    /// wrap the whole thing in a function and `return` twice.
    ///
    /// Statement position parses through here too (`StmtParser::parse_try_stmt`
    /// wraps the result in `Stmt::Expr`), so there is one node, one type rule
    /// and one lowering — as with `if`, whose statement form is not a second
    /// implementation either.
    fn parse_try_expr(&mut self) -> Result<Expr> {
        self.pos += 1; // 'try'
        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            return Err(anyhow!(self.err("Expected '{' after `try`")));
        }
        let Expr::Block(body) = self.parse_brace_block(BlockTail::Value)? else {
            return Err(anyhow!(self.err("`try` body must be a block")));
        };
        if self.eof() || self.tokens[self.pos] != Token::Catch {
            return Err(anyhow!(self.err("Expected `catch` after the `try` block")));
        }
        self.pos += 1;
        let catch_var = match self.tokens.get(self.pos) {
            Some(Token::Id(name)) => {
                let name = name.clone();
                self.pos += 1;
                name
            }
            _ => return Err(anyhow!(self.err("Expected an identifier after `catch`"))),
        };
        if self.eof() || self.tokens[self.pos] != Token::LBrace {
            return Err(anyhow!(self.err("Expected '{' after the `catch` binding")));
        }
        let Expr::Block(handler) = self.parse_brace_block(BlockTail::Value)? else {
            return Err(anyhow!(self.err("`catch` body must be a block")));
        };
        Ok(Expr::Try {
            body,
            catch_var,
            handler,
        })
    }

    /// `if cond { … } else { … }` in *expression* position.
    ///
    /// `match` has always been an expression here; `if` was not, so
    /// `let a = match c { … };` worked and `let a = if c { … } else { … };`
    /// was a syntax error, with the C-style ternary as the only way to choose
    /// a value — the very operator a language whose `if` is an expression does
    /// not need. Both now lower through the same node: `Expr::Conditional`
    /// over two `Expr::Block`s, each evaluating to its last expression.
    ///
    /// A missing `else` yields `nil`, as does a branch whose block ends in a
    /// statement rather than an expression.
    fn parse_if_expr(&mut self) -> Result<Expr> {
        self.pos += 1; // 'if'
        let condition = self.parse_header_expr_before_brace("if")?;
        let then_block = self.parse_brace_block(BlockTail::Value)?;
        let else_expr = if !self.eof() && self.tokens[self.pos] == Token::Else {
            self.pos += 1;
            if self.eof() {
                return Err(anyhow!(self.err("Expected a block or 'if' after 'else'")));
            }
            match self.tokens[self.pos] {
                Token::If => self.deeper(Self::parse_if_expr)?,
                Token::LBrace => self.parse_brace_block(BlockTail::Value)?,
                _ => return Err(anyhow!(self.err("Expected a block or 'if' after 'else'"))),
            }
        } else {
            Expr::Literal(LiteralVal::Nil)
        };
        Ok(Expr::Conditional(condition, Box::new(then_block), Box::new(else_expr)))
    }

    fn parse_match(&mut self) -> Result<Expr> {
        if self.tokens[self.pos] != Token::Match {
            let msg = format!("Expecting 'match', found {:?}", self.tokens[self.pos]);
            return Err(anyhow!(self.err(&msg)));
        }
        self.pos += 1;

        let value = self.parse_header_expr_before_brace("match")?;
        self.pos += 1;

        let mut arms = Vec::new();

        while !self.eof() && self.tokens[self.pos] != Token::RBrace {
            // Parse pattern
            let pattern = self.parse_pattern()?;

            if self.eof() || self.tokens[self.pos] != Token::Arrow {
                return Err(anyhow!(self.err("Expecting '=>' after pattern")));
            }
            self.pos += 1;

            // Parse body expression. Through `parse_expr`, not
            // `parse_conditional`: the arm body is where `match` nests into
            // itself, so it has to be counted.
            //
            // A `{` here opens a *block*, as it does after a closure's
            // parameters — `1 => { work(); }` was a syntax error before,
            // because postfix parsing read the brace as a map literal and then
            // found statements inside it. The cost is that an arm whose value
            // really is a map needs parentheses (`_ => ({"k": 1})`), which is
            // the trade Rust makes for the same reason.
            let body = Box::new(if !self.eof() && self.tokens[self.pos] == Token::LBrace {
                self.parse_brace_block(BlockTail::Value)?
            } else {
                self.parse_expr()?
            });

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

    /// Parse template string content from a TemplateString token
    ///
    /// Where the `${…}` boundaries are is [`split_template_string`]'s answer, not
    /// this function's: the scan used to live here as a second copy of the
    /// lexer's, and the two disagreed about nested braces (`"${R {}}"` cut at the
    /// first `}` and the struct-literal parser then read past the end of its
    /// stream). The macro expander now needs the same answer, which makes a
    /// shared scanner the only way to keep three readers of one syntax honest.
    fn parse_template_string_content(&mut self, content: &str) -> Result<Expr> {
        let segments = split_template_string(content)
            .map_err(|TemplateScanError::Unclosed| anyhow!(self.err("Unclosed template expression")))?;
        let mut parts = Vec::new();
        for segment in segments {
            match segment {
                TemplateSegment::Literal(text) => parts.push(TemplateStringPart::Literal(text.to_string())),
                TemplateSegment::Expr("") => {}
                TemplateSegment::Expr(text) => {
                    let expr_tokens = Tokenizer::tokenize_enhanced(text)
                        .map_err(|e| anyhow!(self.err(&format!("Failed to parse template expression: {e}"))))?;
                    if expr_tokens.is_empty() {
                        continue;
                    }
                    let mut expr_parser = self.sub_parser(&expr_tokens);
                    let expr = expr_parser
                        .parse_expr()
                        .map_err(|e| anyhow!(self.err(&format!("Failed to parse template expression: {e}"))))?;
                    parts.push(TemplateStringPart::Expr(Box::new(expr)));
                }
            }
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
                // A comma here is almost always somebody writing a tuple. The
                // language has none — `Tuple<A, B>` is a *type*, and the value
                // it describes is a list — so "Expecting ')'" left the reader
                // to guess what to write instead.
                let msg = if !self.eof() && self.tokens[self.pos] == Token::Comma {
                    "there is no tuple literal — a value with several elements is a list, written `[a, b]`. \
                     (`Tuple<A, B>` is a type for exactly that, not a second kind of value)"
                        .to_string()
                } else {
                    format!(
                        "Expecting ')', found {:?}",
                        if self.eof() {
                            &Token::Nil
                        } else {
                            &self.tokens[self.pos]
                        }
                    )
                };
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
        let mut segments: Vec<Expr> = Vec::new();
        let mut saw_spread = false;

        // Handle empty list
        if !self.eof() && self.tokens[self.pos] == Token::RBracket {
            self.pos += 1;
            return Ok(Expr::List(elements));
        }

        while !self.eof() && self.tokens[self.pos] != Token::RBracket {
            if self.tokens[self.pos] == Token::Range {
                saw_spread = true;
                if !elements.is_empty() {
                    segments.push(Expr::List(core::mem::take(&mut elements)));
                }
                self.pos += 1;
                if self.eof() || !self.is_valid_expr_start() {
                    return Err(anyhow!(self.err("Expected expression after '..' in list spread")));
                }
                segments.push(self.parse_expr()?);
            } else {
                if !self.is_valid_expr_start() {
                    let msg = format!("Invalid list element start: {:?}", self.tokens[self.pos]);
                    return Err(anyhow!(self.err(&msg)));
                }
                elements.push(Box::new(self.parse_expr()?));
            }

            match self.tokens.get(self.pos) {
                Some(Token::Comma) => {
                    self.pos += 1;
                    if !self.eof() && self.tokens[self.pos] == Token::RBracket {
                        break;
                    }
                }
                Some(Token::RBracket) => break,
                Some(token) => {
                    let msg = if self.is_invalid_separator() {
                        format!("Invalid separator in list: {:?}. Use ',' to separate elements", token)
                    } else {
                        format!("Expecting ',' or ']', found {:?}", token)
                    };
                    return Err(anyhow!(self.err(&msg)));
                }
                None => break,
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

        if !saw_spread {
            return Ok(Expr::List(elements));
        }
        if !elements.is_empty() {
            segments.push(Expr::List(elements));
        }
        let mut iter = segments.into_iter();
        let Some(first) = iter.next() else {
            return Ok(Expr::List(Vec::new()));
        };
        Ok(iter.fold(first, |left, right| {
            Expr::Bin(Box::new(left), BinOp::Add, Box::new(right))
        }))
    }
}
