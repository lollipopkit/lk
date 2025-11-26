use crate::rt::{SelectOperation, with_runtime};
use crate::{
    ast::Parser,
    op::{BinOp, UnaryOp},
    stmt::Stmt,
    token::Tokenizer,
    typ::TypeChecker,
    val::{ClosureCapture, ClosureInit, ClosureValue, ObjectValue, Type, Val, methods::find_method_for_val},
    vm::VmContext,
};
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectCase {
    pub pattern: SelectPattern,
    pub guard: Option<Box<Expr>>, // Optional guard expression
    pub body: Box<Expr>,
}
/// Template string part: either a literal string or an interpolated expression
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    /// Literal pattern: matches exact values (1, "hello", true)
    Literal(Val),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Box<Expr>,
}
impl std::fmt::Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pattern::Literal(val) => write!(f, "{}", val),
            Pattern::Variable(name) => write!(f, "{}", name),
            Pattern::Wildcard => write!(f, "_"),
            Pattern::List { patterns, rest } => {
                write!(f, "[")?;
                for (i, pattern) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", pattern)?;
                }
                if let Some(rest_name) = rest {
                    if !patterns.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "..{}", rest_name)?;
                }
                write!(f, "]")
            }
            Pattern::Map { patterns, rest } => {
                write!(f, "{{")?;
                for (i, (key, pattern)) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "\"{}\": {}", key, pattern)?;
                }
                if let Some(rest_name) = rest {
                    if !patterns.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "..{}", rest_name)?;
                }
                write!(f, "}}")
            }
            Pattern::Or(patterns) => {
                for (i, pattern) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}", pattern)?;
                }
                Ok(())
            }
            Pattern::Guard { pattern, guard } => {
                write!(f, "{} if {}", pattern, guard)
            }
            Pattern::Range { start, end, inclusive } => {
                let op = if *inclusive { "..=" } else { ".." };
                write!(f, "{}{}{}", start, op, end)
            }
        }
    }
}
impl Pattern {
    /// Check if this pattern matches a value, returning bindings if it matches
    /// Returns Ok(Some(bindings)) on match, Ok(None) on no match, Err on error
    pub fn matches(&self, value: &Val, ctx: Option<&VmContext>) -> Result<Option<Vec<(String, Val)>>> {
        let mut bindings = Vec::new();
        if self.matches_impl(value, &mut bindings, ctx)? {
            Ok(Some(bindings))
        } else {
            Ok(None)
        }
    }
    fn matches_impl(&self, value: &Val, bindings: &mut Vec<(String, Val)>, ctx: Option<&VmContext>) -> Result<bool> {
        match self {
            Pattern::Literal(pattern_val) => Ok(value == pattern_val),
            Pattern::Variable(name) => {
                bindings.push((name.clone(), value.clone()));
                Ok(true)
            }
            Pattern::Wildcard => Ok(true),
            Pattern::List { patterns, rest } => {
                let list_items: Vec<Val> = match value {
                    Val::List(list) => (*list).to_vec(),
                    Val::Str(s) => {
                        // Convert string to list of character strings for destructuring
                        s.chars().map(|c| Val::Str(c.to_string().into())).collect::<Vec<_>>()
                    }
                    _ => return Ok(false),
                };
                // Check if we have enough elements for non-rest patterns
                if patterns.len() > list_items.len() && rest.is_none() {
                    return Ok(false);
                }
                // Match each pattern against corresponding list element
                for (i, pattern) in patterns.iter().enumerate() {
                    if i >= list_items.len() {
                        return Ok(false);
                    }
                    if !pattern.matches_impl(&list_items[i], bindings, ctx)? {
                        return Ok(false);
                    }
                }
                // Bind rest elements if specified
                if let Some(rest_name) = rest {
                    let rest_items: Vec<Val> = list_items.iter().skip(patterns.len()).cloned().collect();
                    bindings.push((rest_name.clone(), Val::List(Arc::from(rest_items))));
                } else if patterns.len() != list_items.len() {
                    // No rest pattern but lengths don't match
                    return Ok(false);
                }
                Ok(true)
            }
            Pattern::Map { patterns, rest } => {
                if let Val::Map(map) = value {
                    let map_ref = map.as_ref();
                    // Match each pattern against corresponding map field
                    for (key, pattern) in patterns {
                        if let Some(field_val) = map_ref.get(key.as_str()) {
                            if !pattern.matches_impl(field_val, bindings, ctx)? {
                                return Ok(false);
                            }
                        } else {
                            return Ok(false); // Required key not found
                        }
                    }
                    // Bind remaining fields if specified
                    if let Some(rest_name) = rest {
                        let matched_keys: HashSet<&str> = patterns.iter().map(|(k, _)| k.as_str()).collect();
                        let rest_map: HashMap<String, Val> = map_ref
                            .iter()
                            .filter(|(k, _)| !matched_keys.contains(k.as_ref()))
                            .map(|(k, v)| (k.to_string(), v.clone()))
                            .collect();
                        bindings.push((rest_name.clone(), rest_map.into()));
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Pattern::Or(patterns) => {
                for pattern in patterns {
                    let mut temp_bindings = Vec::new();
                    if pattern.matches_impl(value, &mut temp_bindings, ctx)? {
                        bindings.extend(temp_bindings);
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Pattern::Guard { pattern, guard } => {
                let mut temp_bindings = Vec::new();
                if pattern.matches_impl(value, &mut temp_bindings, ctx)? {
                    // Evaluate guard in provided VmContext with temporary bindings
                    if let Some(_ctx_ref) = ctx {
                        let mut temp_ctx = _ctx_ref.clone();
                        temp_ctx.push_scope();
                        for (n, v) in &temp_bindings {
                            temp_ctx.set(n.clone(), v.clone());
                        }
                        let guard_result = guard.eval_with_ctx(&mut temp_ctx)?;
                        temp_ctx.pop_scope();
                        if let Val::Bool(true) = guard_result {
                            bindings.extend(temp_bindings);
                            Ok(true)
                        } else {
                            Ok(false)
                        }
                    } else if !temp_bindings.is_empty() {
                        Err(anyhow!("Guard conditions with bindings require evaluation context"))
                    } else {
                        let guard_result = if let Some(mut ctx_ref) = ctx.cloned() {
                            guard.eval_with_ctx(&mut ctx_ref)?
                        } else {
                            return Err(anyhow!("Guard evaluation requires context"));
                        }; // degenerate case, no bindings
                        if let Val::Bool(true) = guard_result {
                            Ok(true)
                        } else {
                            Ok(false)
                        }
                    }
                } else {
                    Ok(false)
                }
            }
            Pattern::Range { start, end, inclusive } => {
                if let Some(mut ctx_ref) = ctx.cloned() {
                    let start_val = start.eval_with_ctx(&mut ctx_ref)?;
                    let end_val = end.eval_with_ctx(&mut ctx_ref)?;
                    match (value, &start_val, &end_val) {
                        (Val::Int(v), Val::Int(s), Val::Int(e)) => {
                            if *inclusive {
                                Ok(*v >= *s && *v <= *e)
                            } else {
                                Ok(*v >= *s && *v < *e)
                            }
                        }
                        (Val::Float(v), Val::Float(s), Val::Float(e)) => {
                            if *inclusive {
                                Ok(*v >= *s && *v <= *e)
                            } else {
                                Ok(*v >= *s && *v < *e)
                            }
                        }
                        _ => Ok(false),
                    }
                } else {
                    Err(anyhow!("Range pattern evaluation requires context"))
                }
            }
        }
    }
}
/// Details:
/// - No implicit context
///   + Identifiers must be defined in the lexical environment (e.g., via `let` in statements).
///   + There is no implicit runtime context lookup. Read with `io.read()` and parse with `json/yaml/toml` modules when needed.
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
/// - `import json; let data = json.parse(io.read()); data.user.name == "Alice"` (in statements)
/// - `[1, 2, 3]`
/// - `{"name": "John", "age": 30}`
/// - `[1, 2, 3].1`
/// - `{"name": "Alice"}.name`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Match expression: match value { pattern => expr, ... }
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Val(Val),
}
impl Expr {
    pub fn eval(&self) -> Result<Val> {
        let mut ctx = VmContext::new();
        self.eval_with_ctx(&mut ctx)
    }

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
            // legacy '@' context access removed
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
            Expr::Val(_) => {} // Receive operator: collect from inner expression
        }
    }
    /// Cached parsing: parse expression string and return a shared Arc<Expr>.
    /// Use `parse_cached` if you need an owned `Expr` value.
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
    /// Backwards-compatible helper that returns an owned `Expr` by cloning
    /// the shared cached AST. Prefer `parse_cached_arc` for performance.
    pub fn parse_cached(expression: &str) -> Result<Expr> {
        Ok(Self::parse_cached_arc(expression)?.as_ref().clone())
    }
    /// Constant folding: calculate pure constant sub-expressions as Val constants
    pub(crate) fn fold_constants(self) -> Expr {
        match self {
            Expr::Val(_) => self, // Constant value, return directly
            Expr::Bin(l_box, op, r_box) => {
                // Recursively fold left and right sub-expressions
                let left = (*l_box).fold_constants();
                let right = (*r_box).fold_constants();
                // Try to calculate binary expression as constant
                if let (Expr::Val(lval), Expr::Val(rval)) = (&left, &right) {
                    if op.is_arith() {
                        // Arithmetic operation constant folding
                        let result = match op {
                            BinOp::Add => (lval as &Val) + (rval as &Val),
                            BinOp::Sub => (lval as &Val) - (rval as &Val),
                            BinOp::Mul => (lval as &Val) * (rval as &Val),
                            BinOp::Div => (lval as &Val) / (rval as &Val),
                            BinOp::Mod => (lval as &Val) % (rval as &Val),
                            _ => unreachable!(),
                        };
                        if let Ok(result_val) = result {
                            return Expr::Val(result_val);
                        }
                    } else if op.is_cmp() {
                        // Comparison/contains operation constant folding
                        if let Ok(res_bool) = op.cmp(lval, rval) {
                            return Expr::Val(Val::Bool(res_bool));
                        }
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
                if let Expr::Val(Val::Bool(b)) = c {
                    return if b { t } else { e };
                }
                Expr::Conditional(Box::new(c), Box::new(t), Box::new(e))
            }
            Expr::Unary(op, expr_box) => {
                let inner = (*expr_box).fold_constants();
                // Constant folding: !expr, if expr is boolean constant then calculate result
                if let Expr::Val(Val::Bool(b)) = &inner {
                    return Expr::Val(Val::Bool(!*b));
                }
                Expr::Unary(op, Box::new(inner))
            }
            Expr::And(e1_box, e2_box) => {
                let e1 = (*e1_box).fold_constants();
                // Short-circuit constant false: left side constant false, then entire AND is constant false
                if let Expr::Val(Val::Bool(false)) = e1 {
                    return Expr::Val(Val::Bool(false));
                }
                let e2 = (*e2_box).fold_constants();
                // Short-circuit constant true: left side constant true, then return right side expression result
                if let Expr::Val(Val::Bool(true)) = e1 {
                    return e2;
                }
                // Both folded, if both are boolean constants then can further fold
                if let (Expr::Val(Val::Bool(b1)), Expr::Val(Val::Bool(b2))) = (&e1, &e2) {
                    return Expr::Val(Val::Bool(*b1 && *b2));
                }
                Expr::And(Box::new(e1), Box::new(e2))
            }
            Expr::Or(e1_box, e2_box) => {
                let e1 = (*e1_box).fold_constants();
                if let Expr::Val(Val::Bool(true)) = e1 {
                    // Left side constant true, OR expression is constant true
                    return Expr::Val(Val::Bool(true));
                }
                let e2 = (*e2_box).fold_constants();
                if let Expr::Val(Val::Bool(false)) = e1 {
                    // Left side constant false, OR result depends on right side
                    return e2;
                }
                if let (Expr::Val(Val::Bool(b1)), Expr::Val(Val::Bool(b2))) = (&e1, &e2) {
                    return Expr::Val(Val::Bool(*b1 || *b2));
                }
                Expr::Or(Box::new(e1), Box::new(e2))
            }
            Expr::NullishCoalescing(e1_box, e2_box) => {
                let e1 = (*e1_box).fold_constants();
                // If left side is constant not nil, return it
                if let Expr::Val(v) = &e1
                    && *v != Val::Nil
                {
                    return e1;
                }
                let e2 = (*e2_box).fold_constants();
                // If left side is constant nil, return right side
                if let Expr::Val(Val::Nil) = e1 {
                    return e2;
                }
                Expr::NullishCoalescing(Box::new(e1), Box::new(e2))
            }
            // legacy '@' context access removed
            Expr::Access(base_box, field_box) => {
                let base = (*base_box).fold_constants();
                let field = (*field_box).fold_constants();
                if let (Expr::Val(base_val), Expr::Val(field_val)) = (&base, &field) {
                    // Important: preserve Access when the field is a string literal so that
                    // subsequent call syntax (e.g. foo.bar()) can be intercepted for meta-method dispatch.
                    // This avoids turning `foo.bar` into a concrete value (e.g. Int), which would
                    // later cause `foo.bar()` to attempt calling a non-function value.
                    if matches!(field_val, Val::Str(_)) {
                        return Expr::Access(Box::new(base.clone()), Box::new(field.clone()));
                    }
                    // For non-string fields (e.g. numeric indices), fold direct access where possible
                    if let Some(res_val) = base_val.access(field_val) {
                        return Expr::Val(res_val);
                    } else {
                        return Expr::Val(Val::Nil);
                    }
                }
                Expr::Access(Box::new(base), Box::new(field))
            }
            Expr::OptionalAccess(base_box, field_box) => {
                let base = (*base_box).fold_constants();
                let field = (*field_box).fold_constants();
                if let (Expr::Val(base_val), Expr::Val(field_val)) = (&base, &field) {
                    // Preserve OptionalAccess when field is a string literal to allow potential
                    // optional method-call sugar like `obj?.method()` to be handled later.
                    if matches!(field_val, Val::Str(_)) {
                        return Expr::OptionalAccess(Box::new(base.clone()), Box::new(field.clone()));
                    }
                    // Direct access to constant structure with optional chaining
                    if base_val == &Val::Nil {
                        return Expr::Val(Val::Nil);
                    }
                    if let Some(res_val) = base_val.access(field_val) {
                        return Expr::Val(res_val);
                    } else {
                        return Expr::Val(Val::Nil);
                    }
                }
                Expr::OptionalAccess(Box::new(base), Box::new(field))
            }
            Expr::List(exprs) => {
                // List constant folding: if all elements are constants then fold to one Val::List
                let folded_elems: Vec<Expr> = exprs.into_iter().map(|e| e.fold_constants()).collect();
                if folded_elems.iter().all(|e| matches!(e, Expr::Val(_))) {
                    // Extract all constant values as new list elements
                    let const_vals: Vec<Val> = folded_elems
                        .into_iter()
                        .map(|e| if let Expr::Val(v) = e { v } else { unreachable!() })
                        .collect();
                    return Expr::Val(Val::List(Arc::from(const_vals)));
                }
                Expr::List(folded_elems.into_iter().map(Box::new).collect())
            }
            Expr::Map(pairs) => {
                // Map constant folding: if all keys and values are constants, then construct constant Map
                let folded_pairs: Vec<(Box<Expr>, Box<Expr>)> = pairs
                    .into_iter()
                    .map(|(k, v)| (Box::new(k.fold_constants()), Box::new(v.fold_constants())))
                    .collect();
                if folded_pairs
                    .iter()
                    .all(|(k, v)| matches!(&**k, Expr::Val(_)) && matches!(&**v, Expr::Val(_)))
                {
                    let mut const_map = HashMap::with_capacity(folded_pairs.len());
                    for (k_expr, v_expr) in &folded_pairs {
                        if let (Expr::Val(k_val), Expr::Val(v_val)) = (&**k_expr, &**v_expr) {
                            // Convert key to string (only allow basic type keys)
                            let key_str = match k_val {
                                Val::Str(s) => s.as_ref().to_string(),
                                Val::Int(i) => i.to_string(),
                                Val::Float(f) => f.to_string(),
                                Val::Bool(b) => b.to_string(),
                                _ => {
                                    // Map key must be basic type, if Nil/List/Map appears, don't fold entire Map
                                    return Expr::Map(folded_pairs);
                                }
                            };
                            const_map.insert(key_str, v_val.clone());
                        }
                    }
                    return Expr::Val(Val::from(const_map));
                }
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
                            if let Expr::Val(val) = folded_expr {
                                // Convert constant value to string
                                let str_val = match val {
                                    Val::Str(s) => s.as_ref().to_string(),
                                    Val::Int(i) => i.to_string(),
                                    Val::Float(f) => f.to_string(),
                                    Val::Bool(b) => b.to_string(),
                                    Val::Nil => "nil".to_string(),
                                    Val::List(l) => format!("{:?}", l),
                                    Val::Map(m) => format!("{:?}", m),
                                    Val::Object(_) => format!("{:?}", val),
                                    Val::Task(_) => format!("{:?}", val),
                                    Val::Channel(_) => format!("{:?}", val),
                                    Val::Stream(_) | Val::StreamCursor { .. } => format!("{:?}", val),
                                    Val::Iterator(_) => "[Iterator]".to_string(),
                                    Val::MutationGuard(_) => "[MutationGuard]".to_string(),
                                    Val::Closure(_) => "[Closure]".to_string(),
                                    Val::RustFunction(_) | Val::RustFunctionNamed(_) => "[Function]".to_string(),
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
                    return Expr::Val(Val::Str(Arc::from(result)));
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
impl TryInto<Val> for &Expr {
    type Error = anyhow::Error;
    fn try_into(self) -> Result<Val> {
        match self {
            Expr::Val(val) => Ok(val.clone()), // Clone necessary as eval returns owned Val
            _ => {
                let msg = format!("Can't convert Expr::{:?} to Val", self);
                Err(anyhow!(msg))
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
            // legacy '@' context access removed
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
            Expr::Val(val) => write!(f, "{}", val),
        }
    }
}
impl From<Val> for Expr {
    fn from(val: Val) -> Self {
        Expr::Val(val)
    }
}
impl Expr {
    /// 静态类型检查表达式
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<Type> {
        type_checker.check_expr(self)
    }
    /// 使用 VmContext 进行表达式求值（统一接口）
    ///
    /// 渐进收敛：优先使用 ctx 语义处理常见分支；未覆盖的场景回退到旧实现。
    pub fn eval_with_ctx(&self, ctx: &mut VmContext) -> Result<Val> {
        match self {
            // 变量解析：查找顺序
            // 1) 本地/全局作用域
            // 2) 导入上下文符号（import 语句）
            // 3) 模块解析器注册的内置函数（如测试环境中的 spawn/chan）
            Expr::Var(name) => {
                if let Some(v) = ctx.get(name).cloned() {
                    Ok(v)
                } else if let Some(v) = ctx.import_context().get_symbol(name) {
                    Ok(v.clone())
                } else if let Some(v) = ctx.resolver().get_builtin(name) {
                    Ok(v.clone())
                } else {
                    Err(anyhow!("Undefined variable: {}", name))
                }
            }
            // 括号表达式
            Expr::Paren(expr) => expr.eval_with_ctx(ctx),
            // 字面量列表
            Expr::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for e in items {
                    out.push(e.eval_with_ctx(ctx)?);
                }
                Ok(Val::List(Arc::from(out)))
            }
            // 字面量 Map（键统一为字符串）
            Expr::Map(pairs) => {
                let mut map = HashMap::with_capacity(pairs.len());
                for (k, v) in pairs {
                    let key_val = k.eval_with_ctx(ctx)?;
                    let val_val = v.eval_with_ctx(ctx)?;
                    let key_str = match key_val {
                        Val::Str(s) => s.as_ref().to_string(),
                        Val::Int(i) => i.to_string(),
                        Val::Float(f) => f.to_string(),
                        Val::Bool(b) => b.to_string(),
                        other => return Err(anyhow!("Map key must be primitive, got: {:?}", other)),
                    };
                    map.insert(key_str, val_val);
                }
                Ok(Val::from(map))
            }
            // 结构体字面量（字段求值）
            Expr::StructLiteral { name, fields } => {
                let mut hm = HashMap::with_capacity(fields.len());
                for (k, vexpr) in fields {
                    hm.insert(k.clone(), vexpr.eval_with_ctx(ctx)?);
                }
                Ok(Val::Object(Arc::new(ObjectValue {
                    type_name: name.clone().into(),
                    fields: Arc::new(hm),
                })))
            }
            // 二元运算（使用已有 BinOp::eval_vals 以统一算术与比较语义）
            Expr::Bin(l, op, r) => {
                let lval = l.eval_with_ctx(ctx)?;
                let rval = r.eval_with_ctx(ctx)?;
                // 直接复用 BinOp 上的值级运算实现，覆盖：
                // - 算术：+ - * / %
                // - 比较：== != < <= > >= in
                op.eval_vals(&lval, &rval)
            }
            // 逻辑与/或/空合并（短路）
            Expr::And(l, r) => {
                let lv = l.eval_with_ctx(ctx)?;
                let is_truthy = !matches!(lv, Val::Bool(false) | Val::Nil);
                if is_truthy {
                    r.eval_with_ctx(ctx)
                } else {
                    Ok(Val::Bool(false))
                }
            }
            Expr::Or(l, r) => {
                let lv = l.eval_with_ctx(ctx)?;
                let is_truthy = !matches!(lv, Val::Bool(false) | Val::Nil);
                if is_truthy {
                    Ok(Val::Bool(true))
                } else {
                    r.eval_with_ctx(ctx)
                }
            }
            Expr::NullishCoalescing(l, r) => {
                let lv = l.eval_with_ctx(ctx)?;
                if matches!(lv, Val::Nil) {
                    r.eval_with_ctx(ctx)
                } else {
                    Ok(lv)
                }
            }
            // 模板字符串
            Expr::TemplateString(parts) => {
                let mut s = String::new();
                for p in parts {
                    match p {
                        TemplateStringPart::Literal(t) => s.push_str(t),
                        TemplateStringPart::Expr(e) => s.push_str(&e.eval_with_ctx(ctx)?.display_string(Some(ctx))),
                    }
                }
                Ok(Val::Str(Arc::from(s)))
            }
            // 属性访问 / 下标访问
            Expr::Access(expr, field) => {
                let val = expr.eval_with_ctx(ctx)?;
                let field_val = field.eval_with_ctx(ctx)?;
                Ok(val.access(&field_val).unwrap_or(Val::Nil))
            }
            // 可选访问
            Expr::OptionalAccess(expr, field) => {
                let val = expr.eval_with_ctx(ctx)?;
                if matches!(val, Val::Nil) {
                    return Ok(Val::Nil);
                }
                let field_val = field.eval_with_ctx(ctx)?;
                Ok(val.access(&field_val).unwrap_or(Val::Nil))
            }
            // 函数调用：按名称
            Expr::Call(func_name, args) => {
                // Resolve callee with same fallback strategy as variable lookup
                let func_val = if let Some(v) = ctx.get(func_name).cloned() {
                    v
                } else if let Some(v) = ctx.import_context().get_symbol(func_name) {
                    v.clone()
                } else if let Some(v) = ctx.resolver().get_builtin(func_name) {
                    v.clone()
                } else {
                    return Err(anyhow!("Undefined function: {}", func_name));
                };
                let mut argv = Vec::with_capacity(args.len());
                for a in args {
                    argv.push(a.eval_with_ctx(ctx)?);
                }
                func_val.call(&argv, ctx)
            }
            // 函数调用：通用 callee 表达式（含方法糖）
            Expr::CallExpr(callee, args) => {
                // 方法糖：obj.method(...)
                if let Expr::Access(obj_expr, field_expr) = callee.as_ref() {
                    let obj_val = obj_expr.eval_with_ctx(ctx)?;
                    let field_val = field_expr.eval_with_ctx(ctx)?;
                    if let Val::Str(method_name) = field_val {
                        // 1) 直接属性可调用；若属性是非函数且无实参调用，则返回该属性值（如 list.len() -> list.len）
                        if let Some(prop_val) = obj_val.access(&Val::Str(method_name.clone())) {
                            match prop_val {
                                Val::Closure(_) | Val::RustFunction(_) | Val::RustFunctionNamed(_) => {
                                    let mut argv = Vec::with_capacity(args.len());
                                    for a in args {
                                        argv.push(a.eval_with_ctx(ctx)?);
                                    }
                                    return prop_val.call(&argv, ctx);
                                }
                                other => {
                                    if args.is_empty() {
                                        return Ok(other);
                                    }
                                    // fall through to meta-method lookup for non-empty args
                                }
                            }
                        }
                        // 2) 检查 trait 实现 - 先评估参数再检查 trait 以避免借用冲突
                        let mut full_args = Vec::with_capacity(args.len() + 1);
                        full_args.push(obj_val.clone());
                        for a in args {
                            full_args.push(a.eval_with_ctx(ctx)?);
                        }

                        // 优先检查 trait 实现
                        if let Some(tc) = ctx.type_checker() {
                            let obj_type = obj_val.dispatch_type();
                            if let Some(method_val) = tc.registry().get_method(&obj_type, method_name.as_ref()) {
                                return method_val.clone().call(&full_args, ctx);
                            }
                        }

                        // 回退到内置方法注册
                        if let Some(func) = find_method_for_val(&obj_val, method_name.as_ref()) {
                            let func_val = Val::RustFunction(func);
                            return func_val.call(&full_args, ctx);
                        }

                        return Err(anyhow!("{} has no method '{}'", obj_val.type_name(), method_name));
                    }
                }
                // 普通 callee
                let callee_val = callee.eval_with_ctx(ctx)?;
                match callee_val {
                    Val::Closure(_) | Val::RustFunction(_) | Val::RustFunctionNamed(_) => {
                        let mut argv = Vec::with_capacity(args.len());
                        for a in args {
                            argv.push(a.eval_with_ctx(ctx)?);
                        }
                        callee_val.call(&argv, ctx)
                    }
                    other => Err(anyhow!("{} is not a function", other.type_name())),
                }
            }
            // 具名参数调用（含方法糖）
            Expr::CallNamed(callee, pos_args, named_args) => {
                // 方法糖：obj.method(...)
                if let Expr::Access(obj_expr, field_expr) = callee.as_ref() {
                    let obj_val = obj_expr.eval_with_ctx(ctx)?;
                    let field_val = field_expr.eval_with_ctx(ctx)?;
                    if let Val::Str(method_name) = field_val {
                        // 1) 直接属性可调用
                        if let Some(prop_val) = obj_val.access(&Val::Str(method_name.clone())) {
                            match prop_val {
                                Val::Closure(_) | Val::RustFunctionNamed(_) => {
                                    let mut pos = Vec::with_capacity(pos_args.len());
                                    for a in pos_args {
                                        pos.push(a.eval_with_ctx(ctx)?);
                                    }
                                    let mut named: Vec<(String, Val)> = Vec::with_capacity(named_args.len());
                                    for (n, e) in named_args {
                                        named.push((n.clone(), e.eval_with_ctx(ctx)?));
                                    }
                                    return prop_val.call_named(&pos, &named, ctx);
                                }
                                Val::RustFunction(_) => {
                                    if !named_args.is_empty() {
                                        return Err(anyhow!("Named arguments are not supported for native functions"));
                                    }
                                    let mut argv = Vec::with_capacity(pos_args.len());
                                    for a in pos_args {
                                        argv.push(a.eval_with_ctx(ctx)?);
                                    }
                                    return prop_val.call(&argv, ctx);
                                }
                                other => {
                                    if pos_args.is_empty() && named_args.is_empty() {
                                        return Ok(other);
                                    }
                                    // fall through to meta-method lookup otherwise
                                }
                            }
                        }
                        // 2) 检查 trait 实现 - 先评估参数避免借用冲突（仅支持非具名）
                        if !named_args.is_empty() {
                            // trait 方法不支持具名参数，先检查避免评估工作
                            if let Some(tc) = ctx.type_checker() {
                                let obj_type = obj_val.dispatch_type();
                                if tc.registry().get_method(&obj_type, method_name.as_ref()).is_some() {
                                    return Err(anyhow!("Named arguments are not supported for trait methods"));
                                }
                            }
                        }

                        // 评估所有参数
                        let mut full_args = Vec::with_capacity(pos_args.len() + 1);
                        full_args.push(obj_val.clone());
                        for a in pos_args {
                            full_args.push(a.eval_with_ctx(ctx)?);
                        }

                        // 检查 trait 实现
                        if let Some(tc) = ctx.type_checker() {
                            let obj_type = obj_val.dispatch_type();
                            if let Some(method_val) = tc.registry().get_method(&obj_type, method_name.as_ref()) {
                                return method_val.clone().call(&full_args, ctx);
                            }
                        }

                        // 回退到内置方法
                        if let Some(func) = find_method_for_val(&obj_val, method_name.as_ref()) {
                            let func_val = Val::RustFunction(func);
                            return func_val.call(&full_args, ctx);
                        }

                        return Err(anyhow!("{} has no method '{}'", obj_val.type_name(), method_name));
                    }
                }
                // 普通 callee
                let callee_val = callee.eval_with_ctx(ctx)?;
                let mut pos = Vec::with_capacity(pos_args.len());
                for a in pos_args {
                    pos.push(a.eval_with_ctx(ctx)?);
                }
                let mut named: Vec<(String, Val)> = Vec::with_capacity(named_args.len());
                for (n, e) in named_args {
                    named.push((n.clone(), e.eval_with_ctx(ctx)?));
                }
                callee_val.call_named(&pos, &named, ctx)
            }
            // 范围表达式：生成列表值
            Expr::Range {
                start,
                end,
                inclusive,
                step,
            } => {
                let start_val = match start {
                    Some(expr) => expr.eval_with_ctx(ctx)?,
                    None => Val::Int(0),
                };
                let end_val = match end {
                    Some(expr) => expr.eval_with_ctx(ctx)?,
                    None => return Err(anyhow!("Open-ended ranges not supported in for loops")),
                };
                let step_val = match step {
                    Some(expr) => Some(expr.eval_with_ctx(ctx)?),
                    None => None,
                };
                match (start_val, end_val, step_val) {
                    (Val::Int(s), Val::Int(e), None) => {
                        let range: Vec<Val> = if *inclusive {
                            (s..=e).map(Val::Int).collect()
                        } else {
                            (s..e).map(Val::Int).collect()
                        };
                        Ok(Val::List(range.into()))
                    }
                    (Val::Int(mut i), Val::Int(e), Some(Val::Int(st))) => {
                        if st == 0 {
                            return Err(anyhow!("Range step cannot be zero"));
                        }
                        let mut out: Vec<Val> = Vec::new();
                        if st > 0 {
                            if *inclusive {
                                while i <= e {
                                    out.push(Val::Int(i));
                                    i += st;
                                }
                            } else {
                                while i < e {
                                    out.push(Val::Int(i));
                                    i += st;
                                }
                            }
                        } else if *inclusive {
                            while i >= e {
                                out.push(Val::Int(i));
                                i += st;
                            }
                        } else {
                            while i > e {
                                out.push(Val::Int(i));
                                i += st;
                            }
                        }
                        Ok(Val::List(out.into()))
                    }
                    (_, _, Some(_)) => Err(anyhow!("Range step must be an integer")),
                    _ => Err(anyhow!("Range bounds must be integers")),
                }
            }
            // select 表达式
            Expr::Select { cases, default_case } => {
                let mut select_op = SelectOperation::new();
                let mut bindings: Vec<Option<String>> = Vec::with_capacity(cases.len());
                for (idx, case) in cases.iter().enumerate() {
                    // guard
                    if let Some(g) = &case.guard {
                        match g.eval_with_ctx(ctx)? {
                            Val::Bool(true) => {}
                            Val::Bool(false) => {
                                bindings.push(None);
                                continue;
                            }
                            other => {
                                return Err(anyhow!("Select guard must be Bool, got {:?}", other));
                            }
                        }
                    }
                    match &case.pattern {
                        SelectPattern::Recv { binding, channel } => {
                            let channel_val = channel.eval_with_ctx(ctx)?;
                            let channel_id = if let Val::Channel(channel) = channel_val {
                                channel.id
                            } else {
                                return Err(anyhow!("recv() target is not a channel"));
                            };
                            select_op.add_recv(idx, channel_id);
                            bindings.push(binding.clone());
                        }
                        SelectPattern::Send { channel, value } => {
                            let channel_val = channel.eval_with_ctx(ctx)?;
                            let value_val = value.eval_with_ctx(ctx)?;
                            let channel_id = if let Val::Channel(channel) = channel_val {
                                channel.id
                            } else {
                                return Err(anyhow!("send() target is not a channel"));
                            };
                            select_op.add_send(idx, channel_id, value_val);
                            bindings.push(None);
                        }
                    }
                }
                let has_default = default_case.is_some();
                if select_op.is_empty() && !has_default {
                    return Ok(Val::Nil);
                }
                let select_result = with_runtime(|runtime| runtime.block_on(select_op.execute(runtime, has_default)))?;
                if select_result.is_default {
                    if let Some(default_expr) = default_case {
                        return default_expr.eval_with_ctx(ctx);
                    }
                    return Ok(Val::Nil);
                }
                let case_index = select_result
                    .case_index
                    .ok_or_else(|| anyhow!("Select returned no case index"))?;
                let selected_case = cases
                    .get(case_index)
                    .ok_or_else(|| anyhow!("Invalid select case index"))?;
                if let Some(name) = bindings.get(case_index).cloned().flatten()
                    && let Some((_ok, payload)) = select_result.recv_payload
                {
                    ctx.push_scope();
                    ctx.set(name, payload);
                    let result = selected_case.body.eval_with_ctx(ctx);
                    ctx.pop_scope();
                    return result;
                }
                selected_case.body.eval_with_ctx(ctx)
            }
            // 匹配表达式
            Expr::Match { value, arms } => {
                let match_val = value.eval_with_ctx(ctx)?;
                for arm in arms {
                    if let Some(bindings) = Pattern::matches(&arm.pattern, &match_val, Some(ctx))? {
                        ctx.push_scope();
                        for (name, val) in bindings {
                            ctx.set(name, val);
                        }
                        let result = arm.body.eval_with_ctx(ctx);
                        ctx.pop_scope();
                        return result;
                    }
                }
                Err(anyhow!("No pattern matched in match expression"))
            }
            // 一元运算
            Expr::Unary(op, expr) => {
                let val = expr.eval_with_ctx(ctx)?;
                op.eval_val(&val)
            }
            // 条件表达式
            Expr::Conditional(cond, then_expr, else_expr) => {
                let cv = cond.eval_with_ctx(ctx)?;
                match cv {
                    Val::Bool(true) => then_expr.eval_with_ctx(ctx),
                    Val::Bool(false) => else_expr.eval_with_ctx(ctx),
                    _ => Err(anyhow!("Ternary condition must be Bool, got: {:?}", cv)),
                }
            }
            // 闭包表达式
            Expr::Closure { params, body } => {
                let stmt = Stmt::Expr(body.clone());
                Ok(Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
                    params: Arc::new(params.clone()),
                    named_params: Arc::new(Vec::new()),
                    body: Arc::new(stmt),
                    env: Arc::new(VmContext::new()),
                    upvalues: Arc::new(Vec::new()),
                    captures: ClosureCapture::empty(),
                    capture_specs: Arc::new(Vec::new()),
                    default_funcs: Arc::new(Vec::new()),
                    debug_name: None,
                    debug_location: None,
                }))))
            }
            // 字面量值
            Expr::Val(val) => Ok(val.clone()),
        }
    }
}
