use crate::{
    ast::Parser,
    operator::{BinOp, UnaryOp},
    token::Tokenizer,
    typ::TypeChecker,
    val::{LiteralVal, Type},
};
use anyhow::Result;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::{
    collections::HashSet,
    fmt::{Debug, Display},
    sync::Arc,
};
/// Grammar (abridged):
/// exp     ::= paren
/// paren   ::= {'('} or {')'}
/// or      ::= and {'||' and}
/// and     ::= cmp {'&&' cmp}
/// cmp     ::= addsub {('<' | '>' | '<=' | '>=' | '!=' | '==') addsub}
/// addsub  ::= muldiv {('+' | '-') muldiv}
/// muldiv  ::= unary {('*' | '/' | '%') unary}
/// unary   ::= {'!'} postfix
/// postfix ::= primary { call | dot | opt_dot | opt_index | index }
/// primary ::= nil | false | true | int | float | string | template | list | map | var | paren
///            | closure | select | match
/// field   ::= id | int | string
/// list    ::= '[' [expr {',' expr}] ']'
/// map     ::= '{' [expr ':' expr {',' expr ':' expr}] '}'
///
/// Select case pattern for select statements
#[derive(Debug, Clone, PartialEq)]
pub enum SelectPattern {
    /// recv(channel) pattern with optional binding
    Recv {
        binding: Option<String>,
        channel: Box<Expr>,
    },
    /// send(channel, expr) pattern
    Send { channel: Box<Expr>, value: Box<Expr> },
}
/// Select case: case pattern => expr
#[derive(Debug, Clone, PartialEq)]
pub struct SelectCase {
    pub pattern: SelectPattern,
    pub guard: Option<Box<Expr>>, // Optional guard expression
    pub body: Box<Expr>,
}
/// Template string part: either a literal string or an interpolated expression
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateStringPart {
    /// String literal part
    Literal(String),
    /// Interpolated expression part (only ${expr} syntax)
    Expr(Box<Expr>),
}
impl Expr {
    // Enhanced formatting support has been removed. Only ${...} interpolation remains.
    // Default value-to-string conversion handled inline where needed.
}
/// Pattern matching pattern for match expressions
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Literal pattern: matches exact values (1, "hello", true)
    Literal(LiteralVal),
    /// Variable pattern: binds any value to a variable (x)
    Variable(String),
    /// Wildcard pattern: matches anything, no binding (_)
    Wildcard,
    /// Array/List destructuring pattern: [first, second, ..rest]
    List {
        patterns: Vec<Pattern>,
        rest: Option<String>, // Variable to bind rest of list
    },
    /// Map/Object destructuring pattern: {"key": pattern, "other": var}
    Map {
        patterns: Vec<(String, Pattern)>,
        rest: Option<String>, // Variable to bind remaining fields
    },
    /// Multiple patterns with | (pattern1 | pattern2)
    Or(Vec<Pattern>),
    /// Pattern with guard condition (pattern if guard_expr)
    Guard { pattern: Box<Pattern>, guard: Box<Expr> },
    /// Range pattern: 1..10, 'a'..='z'
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },
}
/// Match arm: pattern => expression
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Box<Expr>,
}
/// Details:
/// - No implicit context
///   + Identifiers must be defined in the lexical environment (e.g., via `let` in statements).
///   + There is no implicit runtime context lookup. Read with `std.read_to_string(std.stdin())` after `use { std } from io;` and parse with `json/yaml/toml` modules when needed.
/// - int / float are considered as `i64 / f64`.
/// - bool can be `true` or `false`.
/// - String
///   + can be wrapped with `""` or `''`.
///   + max length is 64.
/// - nil
///   + ONLY [Option::None] and [Result::Err] are `nil`.
///   + zero value of all types are NOT `nil`.
/// - list literals: `[1, 2, "hello"]`
/// - map literals: `{"key": "value", "count": 42}`
/// - Access literals: `[1, 2, 3].1`, `{"name": "Alice"}.name`
///
/// Examples:
/// - `{ "age": 20 }.age >= 18`
/// - `use { std } from io; use json; let data = json.parse(std.read_to_string(std.stdin())); data.user.name == "Alice"` (in statements)
/// - `[1, 2, 3]`
/// - `{"name": "John", "age": 30}`
/// - `[1, 2, 3].1`
/// - `{"name": "Alice"}.name`
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// expr == expr
    Bin(Box<Expr>, BinOp, Box<Expr>),
    /// !expr
    Unary(UnaryOp, Box<Expr>),
    /// cond ? then_expr : else_expr
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    /// expr && expr
    And(Box<Expr>, Box<Expr>),
    /// expr || expr
    Or(Box<Expr>, Box<Expr>),
    /// expr ?? expr (nullish coalescing)
    NullishCoalescing(Box<Expr>, Box<Expr>),
    /// expr.field
    Access(Box<Expr>, Box<Expr>),
    /// expr?.field (optional chaining)
    OptionalAccess(Box<Expr>, Box<Expr>),
    // (expr)
    Paren(Box<Expr>),
    /// [expr, expr, ...]
    List(Vec<Box<Expr>>),
    /// {expr: expr, expr: expr, ...}
    Map(Vec<(Box<Expr>, Box<Expr>)>),
    /// Struct literal: TypeName { field: expr, ... }
    StructLiteral {
        name: String,
        fields: Vec<(String, Box<Expr>)>,
    },
    /// Variable identifier
    Var(String),
    /// Function call: func_name(arg1, arg2, ...)
    Call(String, Vec<Box<Expr>>),
    /// Function call on expression: expr(arg1, arg2, ...)
    CallExpr(Box<Expr>, Vec<Box<Expr>>),
    /// Function call with named args: callee(positional..., name: expr, ...)
    CallNamed(Box<Expr>, Vec<Box<Expr>>, Vec<(String, Box<Expr>)>),
    /// Range expression: start..end with optional step: start..end..step
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,         // .. vs ..=
        step: Option<Box<Expr>>, // optional explicit step (positive or negative, non-zero)
    },
    /// select { case pattern => expr; ...; default => expr }
    Select {
        cases: Vec<SelectCase>,
        default_case: Option<Box<Expr>>,
    },
    /// Template string: `Hello ${name}!`
    TemplateString(Vec<TemplateStringPart>),
    /// Closure: |param1, param2| expr
    Closure {
        params: Vec<String>,
        body: Box<Expr>,
    },
    /// Expression-level block, primarily for multi-statement closure bodies.
    Block(Vec<Box<crate::stmt::Stmt>>),
    /// Match expression: match value { pattern => expr, ... }
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Literal(LiteralVal),
}
impl Expr {
    /// Get the identifier roots referenced by the expression.
    pub fn requested_ctx(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        self.collect_ctx_names(&mut names);
        names
    }
    /// Helper method to collect identifier roots recursively
    ///
    /// eg.: `user.props.(req.service).value && list` => `["user", "req", "list"]`
    fn collect_ctx_names(&self, names: &mut HashSet<String>) {
        match self {
            Expr::Conditional(c, t, e) => {
                c.collect_ctx_names(names);
                t.collect_ctx_names(names);
                e.collect_ctx_names(names);
            }
            // Removed '@' context access.
            Expr::Access(expr, field) => {
                expr.collect_ctx_names(names);
                field.collect_ctx_names(names);
            }
            Expr::OptionalAccess(expr, field) => {
                expr.collect_ctx_names(names);
                field.collect_ctx_names(names);
            }
            Expr::Bin(l, _, r) => {
                l.collect_ctx_names(names);
                r.collect_ctx_names(names);
            }
            Expr::Unary(_, expr) => {
                expr.collect_ctx_names(names);
            }
            Expr::And(l, r) | Expr::Or(l, r) | Expr::NullishCoalescing(l, r) => {
                l.collect_ctx_names(names);
                r.collect_ctx_names(names);
            }
            Expr::List(exprs) => {
                for expr in exprs {
                    expr.collect_ctx_names(names);
                }
            }
            Expr::Map(pairs) => {
                for (key, value) in pairs {
                    key.collect_ctx_names(names);
                    value.collect_ctx_names(names);
                }
            }
            Expr::Paren(expr) => {
                expr.collect_ctx_names(names);
            }
            // Variables contribute potential context roots
            Expr::Var(name) => {
                names.insert(name.clone());
            }
            // Function calls - collect from arguments
            Expr::Call(_, args) => {
                for arg in args {
                    arg.collect_ctx_names(names);
                }
            }
            Expr::CallExpr(expr, args) => {
                expr.collect_ctx_names(names);
                for arg in args {
                    arg.collect_ctx_names(names);
                }
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                callee.collect_ctx_names(names);
                for a in pos_args {
                    a.collect_ctx_names(names);
                }
                for (_n, e) in named_args {
                    e.collect_ctx_names(names);
                }
            }
            Expr::Range { start, end, step, .. } => {
                if let Some(s) = start {
                    s.collect_ctx_names(names);
                }
                if let Some(e) = end {
                    e.collect_ctx_names(names);
                }
                if let Some(st) = step {
                    st.collect_ctx_names(names);
                }
            }
            Expr::Select { cases, default_case } => {
                for case in cases {
                    match &case.pattern {
                        SelectPattern::Recv { channel, .. } => {
                            channel.collect_ctx_names(names);
                        }
                        SelectPattern::Send { channel, value } => {
                            channel.collect_ctx_names(names);
                            value.collect_ctx_names(names);
                        }
                    }
                    if let Some(guard) = &case.guard {
                        guard.collect_ctx_names(names);
                    }
                    case.body.collect_ctx_names(names);
                }
                if let Some(default_expr) = default_case {
                    default_expr.collect_ctx_names(names);
                }
            }
            Expr::TemplateString(parts) => {
                for part in parts {
                    match part {
                        TemplateStringPart::Literal(_) => {}
                        TemplateStringPart::Expr(expr) => {
                            expr.collect_ctx_names(names);
                        }
                    }
                }
            }
            Expr::Closure { params: _, body } => {
                body.collect_ctx_names(names);
            }
            Expr::Block(_) => {}
            Expr::Match { value, arms } => {
                value.collect_ctx_names(names);
                for arm in arms {
                    arm.body.collect_ctx_names(names);
                    // Collect from guard patterns if they contain identifier references
                    if let Pattern::Guard { guard, .. } = &arm.pattern {
                        guard.collect_ctx_names(names);
                    }
                }
            }
            Expr::StructLiteral { fields, .. } => {
                for (_k, v) in fields {
                    v.collect_ctx_names(names);
                }
            }
            // Only collect string values when they are actual identifier roots, not field names
            Expr::Literal(_) => {} // Receive operator: collect from inner expression
        }
    }
    /// Cached parsing: parse expression string and return a shared Arc<Expr>.
    pub fn parse_cached_arc(expression: &str) -> Result<Arc<Expr>> {
        use dashmap::mapref::entry::Entry;
        // Global static cache: Key is expression string, Value is parsed Expr wrapped in Arc
        static PARSE_CACHE: Lazy<DashMap<String, Arc<Expr>>> = Lazy::new(DashMap::new);
        // Fast read path
        if let Some(found) = PARSE_CACHE.get(expression) {
            return Ok(found.value().clone());
        }
        // Parse on miss, then insert with write lock
        let tokens = Tokenizer::tokenize(expression)?;
        let expr = Parser::new(&tokens).parse()?; // Constant folding happens in parser
        let expr_arc = Arc::new(expr);
        Ok(match PARSE_CACHE.entry(expression.to_string()) {
            Entry::Vacant(v) => {
                v.insert(expr_arc.clone());
                expr_arc
            }
            Entry::Occupied(o) => o.get().clone(),
        })
    }
    /// Constant folding: calculate pure constant sub-expressions as LiteralVal constants
    pub(crate) fn fold_constants(self) -> Expr {
        match self {
            Expr::Literal(_) => self, // Constant value, return directly
            Expr::Bin(l_box, op, r_box) => {
                // Recursively fold left and right sub-expressions
                let left = (*l_box).fold_constants();
                let right = (*r_box).fold_constants();
                // Try to calculate binary expression as constant
                if let (Expr::Literal(lval), Expr::Literal(rval)) = (&left, &right) {
                    if op.is_arith() {
                        if let Some(result_val) = fold_literal_arith(lval, &op, rval) {
                            return Expr::Literal(result_val);
                        }
                    } else if op.is_cmp()
                        && let Some(res_bool) = op.cmp_literals(lval, rval)
                    {
                        return Expr::Literal(LiteralVal::Bool(res_bool));
                    }
                    // Other cases (like type mismatch) don't fold, keep expression form
                }
                // Partial folding: left and right nodes already folded, but current node can't fold to constant
                Expr::Bin(Box::new(left), op, Box::new(right))
            }
            Expr::Conditional(c_box, t_box, e_box) => {
                let c = (*c_box).fold_constants();
                let t = (*t_box).fold_constants();
                let e = (*e_box).fold_constants();
                if let Expr::Literal(LiteralVal::Bool(b)) = c {
                    return if b { t } else { e };
                }
                Expr::Conditional(Box::new(c), Box::new(t), Box::new(e))
            }
            Expr::Unary(op, expr_box) => {
                let inner = (*expr_box).fold_constants();
                // Constant folding: !expr, if expr is boolean constant then calculate result
                if let Expr::Literal(LiteralVal::Bool(b)) = &inner {
                    return Expr::Literal(LiteralVal::Bool(!*b));
                }
                Expr::Unary(op, Box::new(inner))
            }
            Expr::And(e1_box, e2_box) => {
                let e1 = (*e1_box).fold_constants();
                // Short-circuit constant false: left side constant false, then entire AND is constant false
                if let Expr::Literal(LiteralVal::Bool(false)) = e1 {
                    return Expr::Literal(LiteralVal::Bool(false));
                }
                let e2 = (*e2_box).fold_constants();
                // Short-circuit constant true: left side constant true, then return right side expression result
                if let Expr::Literal(LiteralVal::Bool(true)) = e1 {
                    return e2;
                }
                // Both folded, if both are boolean constants then can further fold
                if let (Expr::Literal(LiteralVal::Bool(b1)), Expr::Literal(LiteralVal::Bool(b2))) = (&e1, &e2) {
                    return Expr::Literal(LiteralVal::Bool(*b1 && *b2));
                }
                Expr::And(Box::new(e1), Box::new(e2))
            }
            Expr::Or(e1_box, e2_box) => {
                let e1 = (*e1_box).fold_constants();
                if let Expr::Literal(LiteralVal::Bool(true)) = e1 {
                    // Left side constant true, OR expression is constant true
                    return Expr::Literal(LiteralVal::Bool(true));
                }
                let e2 = (*e2_box).fold_constants();
                if let Expr::Literal(LiteralVal::Bool(false)) = e1 {
                    // Left side constant false, OR result depends on right side
                    return e2;
                }
                if let (Expr::Literal(LiteralVal::Bool(b1)), Expr::Literal(LiteralVal::Bool(b2))) = (&e1, &e2) {
                    return Expr::Literal(LiteralVal::Bool(*b1 || *b2));
                }
                Expr::Or(Box::new(e1), Box::new(e2))
            }
            Expr::NullishCoalescing(e1_box, e2_box) => {
                let e1 = (*e1_box).fold_constants();
                // If left side is constant not nil, return it
                if let Expr::Literal(v) = &e1
                    && *v != LiteralVal::Nil
                {
                    return e1;
                }
                let e2 = (*e2_box).fold_constants();
                // If left side is constant nil, return right side
                if let Expr::Literal(LiteralVal::Nil) = e1 {
                    return e2;
                }
                Expr::NullishCoalescing(Box::new(e1), Box::new(e2))
            }
            // Removed '@' context access.
            Expr::Access(base_box, field_box) => {
                let base = (*base_box).fold_constants();
                let field = (*field_box).fold_constants();
                if let (Expr::Literal(base_val), Expr::Literal(field_val)) = (&base, &field) {
                    // Important: preserve Access when the field is a string literal so that
                    // subsequent call syntax (e.g. foo.bar()) can be intercepted for meta-method dispatch.
                    // This avoids turning `foo.bar` into a concrete value (e.g. Int), which would
                    // later cause `foo.bar()` to attempt calling a non-function value.
                    if field_val.as_str().is_some() {
                        return Expr::Access(Box::new(base.clone()), Box::new(field.clone()));
                    }
                    let _ = (base_val, field_val);
                }
                Expr::Access(Box::new(base), Box::new(field))
            }
            Expr::OptionalAccess(base_box, field_box) => {
                let base = (*base_box).fold_constants();
                let field = (*field_box).fold_constants();
                if let (Expr::Literal(base_val), Expr::Literal(field_val)) = (&base, &field) {
                    // Preserve OptionalAccess when field is a string literal to allow potential
                    // optional method-call sugar like `obj?.method()` to be handled later.
                    if field_val.as_str().is_some() {
                        return Expr::OptionalAccess(Box::new(base.clone()), Box::new(field.clone()));
                    }
                    if base_val == &LiteralVal::Nil {
                        return Expr::Literal(LiteralVal::Nil);
                    }
                    let _ = field_val;
                }
                Expr::OptionalAccess(Box::new(base), Box::new(field))
            }
            Expr::List(exprs) => {
                let folded_elems: Vec<Expr> = exprs.into_iter().map(|e| e.fold_constants()).collect();
                Expr::List(folded_elems.into_iter().map(Box::new).collect())
            }
            Expr::Map(pairs) => {
                let folded_pairs: Vec<(Box<Expr>, Box<Expr>)> = pairs
                    .into_iter()
                    .map(|(k, v)| (Box::new(k.fold_constants()), Box::new(v.fold_constants())))
                    .collect();
                Expr::Map(folded_pairs)
            }
            Expr::Paren(expr_box) => {
                // Keep parentheses structure, but fold internal expression
                Expr::Paren(Box::new((*expr_box).fold_constants()))
            }
            Expr::Var(name) => {
                // Variables can't be folded without environment
                Expr::Var(name)
            }
            Expr::Call(name, args) => {
                // Function calls can't be folded at compile time, but fold arguments
                let folded_args = args.into_iter().map(|a| Box::new(a.fold_constants())).collect();
                Expr::Call(name, folded_args)
            }
            Expr::CallExpr(expr, args) => {
                // Function calls can't be folded at compile time, but fold expression and arguments
                let folded_expr = Box::new(expr.fold_constants());
                let folded_args = args.into_iter().map(|a| Box::new(a.fold_constants())).collect();
                Expr::CallExpr(folded_expr, folded_args)
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                // Fold callee, positional and named arguments recursively
                let folded_callee = Box::new(callee.fold_constants());
                let folded_pos: Vec<Box<Expr>> = pos_args.into_iter().map(|a| Box::new(a.fold_constants())).collect();
                let folded_named: Vec<(String, Box<Expr>)> = named_args
                    .into_iter()
                    .map(|(n, e)| (n, Box::new(e.fold_constants())))
                    .collect();
                Expr::CallNamed(folded_callee, folded_pos, folded_named)
            }
            Expr::Range {
                start,
                end,
                inclusive,
                step,
            } => {
                // Range expressions with constant bounds can be folded
                let folded_start = start.map(|s| Box::new(s.fold_constants()));
                let folded_end = end.map(|e| Box::new(e.fold_constants()));
                let folded_step = step.map(|st| Box::new(st.fold_constants()));
                Expr::Range {
                    start: folded_start,
                    end: folded_end,
                    inclusive,
                    step: folded_step,
                }
            }
            Expr::Select { cases, default_case } => {
                let folded_cases = cases
                    .into_iter()
                    .map(|case| SelectCase {
                        pattern: match case.pattern {
                            SelectPattern::Recv { binding, channel } => SelectPattern::Recv {
                                binding,
                                channel: Box::new(channel.fold_constants()),
                            },
                            SelectPattern::Send { channel, value } => SelectPattern::Send {
                                channel: Box::new(channel.fold_constants()),
                                value: Box::new(value.fold_constants()),
                            },
                        },
                        guard: case.guard.map(|g| Box::new(g.fold_constants())),
                        body: Box::new(case.body.fold_constants()),
                    })
                    .collect();
                let folded_default = default_case.map(|d| Box::new(d.fold_constants()));
                Expr::Select {
                    cases: folded_cases,
                    default_case: folded_default,
                }
            }
            Expr::TemplateString(parts) => {
                // Template string constant folding: if all interpolated expressions are constants, fold to constant string
                let folded_parts: Vec<TemplateStringPart> = parts
                    .into_iter()
                    .map(|part| match part {
                        TemplateStringPart::Literal(s) => TemplateStringPart::Literal(s),
                        TemplateStringPart::Expr(expr) => {
                            let folded_expr = expr.fold_constants();
                            if let Expr::Literal(val) = folded_expr {
                                // Convert constant value to string
                                let str_val = match val.as_str() {
                                    Some(s) => s.to_string(),
                                    None => match val {
                                        LiteralVal::Int(i) => i.to_string(),
                                        LiteralVal::Float(f) => f.to_string(),
                                        LiteralVal::Bool(b) => b.to_string(),
                                        LiteralVal::Nil => "nil".to_string(),
                                        LiteralVal::String(_) => format!("{:?}", val),
                                        LiteralVal::ShortStr(_) => unreachable!("string handled above"),
                                    },
                                };
                                TemplateStringPart::Literal(str_val)
                            } else {
                                TemplateStringPart::Expr(Box::new(folded_expr))
                            }
                        }
                    })
                    .collect();
                // If all parts are literals, fold to a single string constant
                if folded_parts
                    .iter()
                    .all(|part| matches!(part, TemplateStringPart::Literal(_)))
                {
                    let result = folded_parts
                        .into_iter()
                        .map(|part| {
                            if let TemplateStringPart::Literal(s) = part {
                                s
                            } else {
                                unreachable!()
                            }
                        })
                        .collect::<String>();
                    return Expr::Literal(LiteralVal::from_str(&result));
                }
                Expr::TemplateString(folded_parts)
            }
            Expr::Closure { params, body } => {
                // Closures cannot be folded at compile time due to environment capture
                Expr::Closure {
                    params: params.clone(),
                    body: Box::new(body.fold_constants()),
                }
            }
            Expr::Block(statements) => Expr::Block(statements),
            Expr::Match { value, arms } => {
                // Match expressions cannot be fully folded without runtime evaluation
                // but we can fold the value and arm bodies
                let folded_value = Box::new(value.fold_constants());
                let folded_arms = arms
                    .into_iter()
                    .map(|arm| MatchArm {
                        pattern: arm.pattern, // Patterns contain runtime values, don't fold
                        body: Box::new(arm.body.fold_constants()),
                    })
                    .collect();
                Expr::Match {
                    value: folded_value,
                    arms: folded_arms,
                }
            }
            Expr::StructLiteral { name, fields } => {
                let mut new_fields: Vec<(String, Box<Expr>)> = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    new_fields.push((k, Box::new(v.fold_constants())));
                }
                Expr::StructLiteral {
                    name,
                    fields: new_fields,
                }
            }
        }
    }
}

fn into_expr<S: AsRef<str>>(s: S) -> Result<Expr> {
    let tokens = Tokenizer::tokenize(s.as_ref())?;
    let expr = Parser::new(&tokens).parse()?;
    Ok(expr)
}
impl TryFrom<&str> for Expr {
    type Error = anyhow::Error;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        into_expr(value)
    }
}
impl TryFrom<String> for Expr {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        into_expr(value)
    }
}
impl Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expr::Bin(left, op, right) => write!(f, "{left} {op:?} {right}"),
            Expr::Unary(op, expr) => write!(f, "{op:?}{expr}"),
            Expr::Conditional(c, t, e) => write!(f, "{} ? {} : {}", c, t, e),
            Expr::And(left, right) => write!(f, "{left} && {right}"),
            Expr::Or(left, right) => write!(f, "{left} || {right}"),
            Expr::NullishCoalescing(left, right) => write!(f, "{left} ?? {right}"),
            // Removed '@' context access.
            Expr::Access(expr, field) => write!(f, "{}.{}", expr, field),
            Expr::OptionalAccess(expr, field) => write!(f, "{}?.{}", expr, field),
            Expr::List(exprs) => {
                let exprs: Vec<String> = exprs.iter().map(|e| e.to_string()).collect();
                write!(f, "[{}]", exprs.join(", "))
            }
            Expr::Map(pairs) => {
                let pairs: Vec<String> = pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", pairs.join(", "))
            }
            Expr::Paren(expr) => write!(f, "{expr}"),
            Expr::Var(name) => write!(f, "{}", name),
            Expr::Call(name, args) => {
                let args_str: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                write!(f, "{}({})", name, args_str.join(", "))
            }
            Expr::CallExpr(expr, args) => {
                let args_str: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                write!(f, "{}({})", expr, args_str.join(", "))
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                let mut parts: Vec<String> = Vec::new();
                parts.extend(pos_args.iter().map(|a| a.to_string()));
                parts.extend(named_args.iter().map(|(n, e)| format!("{}: {}", n, e)));
                write!(f, "{}({})", callee, parts.join(", "))
            }
            Expr::Range {
                start,
                end,
                inclusive,
                step,
            } => {
                let start_str = match start {
                    Some(s) => s.to_string(),
                    None => "".to_string(),
                };
                let end_str = match end {
                    Some(e) => e.to_string(),
                    None => "".to_string(),
                };
                let op = if *inclusive { "..=" } else { ".." };
                if let Some(st) = step {
                    write!(f, "{}{}{}..{}", start_str, op, end_str, st)
                } else {
                    write!(f, "{}{}{}", start_str, op, end_str)
                }
            }
            Expr::Select { cases, default_case } => {
                write!(f, "select {{")?;
                for (i, case) in cases.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "case ")?;
                    match &case.pattern {
                        SelectPattern::Recv { binding, channel } => {
                            if let Some(name) = binding {
                                write!(f, "{} <- recv({})", name, channel)?;
                            } else {
                                write!(f, "recv({})", channel)?;
                            }
                        }
                        SelectPattern::Send { channel, value } => write!(f, "{} <= send({})", channel, value)?,
                    }
                    if let Some(g) = &case.guard {
                        write!(f, " if {}", g)?;
                    }
                    write!(f, " => {}", case.body)?;
                }
                if let Some(default) = default_case {
                    if !cases.is_empty() {
                        write!(f, "; ")?;
                    }
                    write!(f, "default => {}", default)?;
                }
                write!(f, "}}")
            }
            Expr::TemplateString(parts) => {
                write!(f, "\"")?;
                for part in parts {
                    match part {
                        TemplateStringPart::Literal(s) => {
                            // Escape backslashes, quotes and dollar signs in literals
                            let escaped = s
                                .replace("\\", "\\\\")
                                .replace("\"", "\\\"")
                                .replace("$", "\\$")
                                .replace("{", "\\{")
                                .replace("}", "\\}");
                            write!(f, "{}", escaped)?;
                        }
                        TemplateStringPart::Expr(expr) => {
                            write!(f, "${{{}}}", expr)?;
                        }
                    }
                }
                write!(f, "\"")
            }
            Expr::Closure { params, body } => {
                let params_str = params.join(", ");
                write!(f, "|{}| {}", params_str, body)
            }
            Expr::Block(_) => write!(f, "{{ ... }}"),
            Expr::Match { value, arms } => {
                write!(f, "match {} {{", value)?;
                for (i, arm) in arms.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} => {}", arm.pattern, arm.body)?;
                }
                write!(f, "}}")
            }
            Expr::StructLiteral { name, fields } => {
                write!(f, "{} {{", name)?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Expr::Literal(val) => write!(f, "{}", val),
        }
    }
}

fn fold_literal_arith(lhs: &LiteralVal, op: &BinOp, rhs: &LiteralVal) -> Option<LiteralVal> {
    match op {
        BinOp::Add => fold_literal_add(lhs, rhs),
        BinOp::Sub => fold_literal_numeric(lhs, rhs, |a, b| a - b, |a, b| a - b),
        BinOp::Mul => fold_literal_mul(lhs, rhs),
        BinOp::Div => fold_literal_div(lhs, rhs),
        BinOp::Mod => fold_literal_mod(lhs, rhs),
        _ => None,
    }
}

fn fold_literal_add(lhs: &LiteralVal, rhs: &LiteralVal) -> Option<LiteralVal> {
    match (lhs, rhs) {
        (LiteralVal::Int(a), LiteralVal::Int(b)) => Some(LiteralVal::Int(a + b)),
        (LiteralVal::Float(a), LiteralVal::Float(b)) => Some(LiteralVal::Float(a + b)),
        (LiteralVal::Float(a), LiteralVal::Int(b)) => Some(LiteralVal::Float(a + *b as f64)),
        (LiteralVal::Int(a), LiteralVal::Float(b)) => Some(LiteralVal::Float(*a as f64 + b)),
        (lhs, rhs) if lhs.as_str().is_some() && rhs.as_str().is_some() => Some(LiteralVal::concat_strings(
            lhs.as_str().expect("checked string"),
            rhs.as_str().expect("checked string"),
        )),
        (lhs, LiteralVal::Int(value)) if lhs.as_str().is_some() => {
            let mut buf = itoa::Buffer::new();
            Some(LiteralVal::concat_strings(
                lhs.as_str().expect("checked string"),
                buf.format(*value),
            ))
        }
        (lhs, LiteralVal::Float(value)) if lhs.as_str().is_some() => {
            let mut buf = ryu::Buffer::new();
            Some(LiteralVal::concat_strings(
                lhs.as_str().expect("checked string"),
                buf.format(*value),
            ))
        }
        (LiteralVal::Int(value), rhs) if rhs.as_str().is_some() => {
            let mut buf = itoa::Buffer::new();
            Some(LiteralVal::concat_strings(
                buf.format(*value),
                rhs.as_str().expect("checked string"),
            ))
        }
        (LiteralVal::Float(value), rhs) if rhs.as_str().is_some() => {
            let mut buf = ryu::Buffer::new();
            Some(LiteralVal::concat_strings(
                buf.format(*value),
                rhs.as_str().expect("checked string"),
            ))
        }
        _ => None,
    }
}

fn fold_literal_mul(lhs: &LiteralVal, rhs: &LiteralVal) -> Option<LiteralVal> {
    match (lhs, rhs) {
        (left, LiteralVal::Int(count)) if left.as_str().is_some() => {
            Some(repeat_literal_string(left.as_str()?, *count))
        }
        (LiteralVal::Int(count), right) if right.as_str().is_some() => {
            Some(repeat_literal_string(right.as_str()?, *count))
        }
        _ => fold_literal_numeric(lhs, rhs, |a, b| a * b, |a, b| a * b),
    }
}

fn repeat_literal_string(value: &str, count: i64) -> LiteralVal {
    if count <= 0 {
        LiteralVal::from_str("")
    } else {
        LiteralVal::from_str(&value.repeat(count as usize))
    }
}

fn fold_literal_div(lhs: &LiteralVal, rhs: &LiteralVal) -> Option<LiteralVal> {
    if literal_is_zero(rhs) {
        return None;
    }

    match (lhs, rhs) {
        (LiteralVal::Int(a), LiteralVal::Int(b)) => {
            let result = (*a as f64) / (*b as f64);
            if result.fract() == 0.0 {
                Some(LiteralVal::Int(result as i64))
            } else {
                Some(LiteralVal::Float(result))
            }
        }
        _ => fold_literal_numeric(lhs, rhs, |a, b| a / b, |a, b| a / b),
    }
}

fn fold_literal_mod(lhs: &LiteralVal, rhs: &LiteralVal) -> Option<LiteralVal> {
    if literal_is_zero(rhs) {
        return None;
    }
    fold_literal_numeric(lhs, rhs, |a, b| a % b, |a, b| a % b)
}

fn literal_is_zero(value: &LiteralVal) -> bool {
    match value {
        LiteralVal::Int(value) => *value == 0,
        LiteralVal::Float(value) => *value == 0.0,
        _ => false,
    }
}

fn fold_literal_numeric(
    lhs: &LiteralVal,
    rhs: &LiteralVal,
    int_op: fn(i64, i64) -> i64,
    float_op: fn(f64, f64) -> f64,
) -> Option<LiteralVal> {
    match (lhs, rhs) {
        (LiteralVal::Int(a), LiteralVal::Int(b)) => Some(LiteralVal::Int(int_op(*a, *b))),
        (LiteralVal::Float(a), LiteralVal::Float(b)) => Some(LiteralVal::Float(float_op(*a, *b))),
        (LiteralVal::Float(a), LiteralVal::Int(b)) => Some(LiteralVal::Float(float_op(*a, *b as f64))),
        (LiteralVal::Int(a), LiteralVal::Float(b)) => Some(LiteralVal::Float(float_op(*a as f64, *b))),
        _ => None,
    }
}

impl Expr {
    /// 静态类型检查表达式
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<Type> {
        type_checker.check_expr(self)
    }
}
