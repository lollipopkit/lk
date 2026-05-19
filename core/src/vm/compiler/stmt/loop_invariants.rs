use std::collections::HashSet;

use crate::{
    expr::Expr,
    stmt::{ForPattern, Stmt},
    val::Val,
};

use super::FunctionBuilder;

impl FunctionBuilder {
    pub(super) fn expr_is_loop_cacheable(expr: &Expr) -> bool {
        match expr {
            Expr::Val(_) | Expr::Var(_) => true,
            Expr::Paren(inner) | Expr::Unary(_, inner) => Self::expr_is_loop_cacheable(inner),
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right) => {
                Self::expr_is_loop_cacheable(left) && Self::expr_is_loop_cacheable(right)
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                Self::expr_is_loop_cacheable(condition)
                    && Self::expr_is_loop_cacheable(then_expr)
                    && Self::expr_is_loop_cacheable(else_expr)
            }
            _ => false,
        }
    }

    fn collect_for_pattern_names(pattern: &ForPattern, out: &mut HashSet<String>) {
        match pattern {
            ForPattern::Variable(name) => {
                out.insert(name.clone());
            }
            ForPattern::Ignore => {}
            ForPattern::Tuple(patterns) | ForPattern::Array { patterns, rest: None } => {
                for pattern in patterns {
                    Self::collect_for_pattern_names(pattern, out);
                }
            }
            ForPattern::Array {
                patterns,
                rest: Some(rest),
            } => {
                for pattern in patterns {
                    Self::collect_for_pattern_names(pattern, out);
                }
                out.insert(rest.clone());
            }
            ForPattern::Object(entries) => {
                for (_, pattern) in entries {
                    Self::collect_for_pattern_names(pattern, out);
                }
            }
        }
    }

    fn expr_is_immutable_literal(expr: &Expr) -> bool {
        match expr {
            Expr::Val(value) => matches!(
                value,
                Val::Nil | Val::Bool(_) | Val::Float(_) | Val::Str(_) | Val::ShortStr(_)
            ),
            Expr::Paren(inner) => Self::expr_is_immutable_literal(inner),
            _ => false,
        }
    }

    fn expr_worth_loop_hoisting(expr: &Expr) -> bool {
        match expr {
            Expr::Bin(_, _, _)
            | Expr::Unary(_, _)
            | Expr::Conditional(_, _, _)
            | Expr::And(_, _)
            | Expr::Or(_, _)
            | Expr::NullishCoalescing(_, _) => true,
            Expr::Paren(inner) => Self::expr_worth_loop_hoisting(inner),
            _ => Self::expr_is_immutable_literal(expr),
        }
    }

    pub(super) fn collect_loop_invariant_exprs_from_expr(
        &self,
        expr: &Expr,
        loop_names: &HashSet<String>,
        body: &Stmt,
        out: &mut Vec<Expr>,
    ) {
        if Self::expr_worth_loop_hoisting(expr) && Self::expr_is_loop_cacheable(expr) {
            let names = expr.requested_ctx();
            let safe_literal = names.is_empty() && Self::expr_is_immutable_literal(expr);
            let safe_named = !names.is_empty()
                && names.iter().all(|name| {
                    !loop_names.contains(name) && self.lookup(name).is_some() && !Self::stmt_assigns_name(body, name)
                });
            if safe_literal || safe_named {
                if !out.iter().any(|existing| existing == expr) {
                    out.push(expr.clone());
                }
                return;
            }
        }

        match expr {
            Expr::Paren(inner) | Expr::Unary(_, inner) => {
                self.collect_loop_invariant_exprs_from_expr(inner, loop_names, body, out);
            }
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right)
            | Expr::Access(left, right)
            | Expr::OptionalAccess(left, right) => {
                self.collect_loop_invariant_exprs_from_expr(left, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_expr(right, loop_names, body, out);
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                self.collect_loop_invariant_exprs_from_expr(condition, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_expr(then_expr, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_expr(else_expr, loop_names, body, out);
            }
            Expr::List(items) => {
                for item in items {
                    self.collect_loop_invariant_exprs_from_expr(item, loop_names, body, out);
                }
            }
            Expr::Map(pairs) => {
                for (key, value) in pairs {
                    self.collect_loop_invariant_exprs_from_expr(key, loop_names, body, out);
                    self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                }
            }
            Expr::Range { start, end, step, .. } => {
                if let Some(expr) = start {
                    self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                }
                if let Some(expr) = end {
                    self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                }
                if let Some(expr) = step {
                    self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                }
            }
            Expr::TemplateString(parts) => {
                for part in parts {
                    if let crate::expr::TemplateStringPart::Expr(expr) = part {
                        self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                    }
                }
            }
            Expr::Call(_, args) => {
                for arg in args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
            }
            Expr::CallExpr(callee, args) => {
                self.collect_loop_invariant_exprs_from_expr(callee, loop_names, body, out);
                for arg in args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                self.collect_loop_invariant_exprs_from_expr(callee, loop_names, body, out);
                for arg in pos_args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
                for (_, arg) in named_args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
            }
            Expr::Match { value, arms } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                for arm in arms {
                    self.collect_loop_invariant_exprs_from_expr(&arm.body, loop_names, body, out);
                }
            }
            Expr::StructLiteral { fields, .. } => {
                for (_, value) in fields {
                    self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                }
            }
            Expr::Val(_) | Expr::Var(_) | Expr::Closure { .. } | Expr::Block(_) | Expr::Select { .. } => {}
        }
    }

    pub(super) fn collect_loop_invariant_exprs_from_stmt(
        &self,
        stmt: &Stmt,
        loop_names: &HashSet<String>,
        body: &Stmt,
        out: &mut Vec<Expr>,
    ) {
        match stmt {
            Stmt::Block { statements } => {
                for statement in statements {
                    self.collect_loop_invariant_exprs_from_stmt(statement, loop_names, body, out);
                }
            }
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Define { value, .. }
            | Stmt::Expr(value)
            | Stmt::Return { value: Some(value) }
            | Stmt::CompoundAssign { value, .. } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                self.collect_loop_invariant_exprs_from_expr(condition, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(then_stmt, loop_names, body, out);
                if let Some(else_stmt) = else_stmt {
                    self.collect_loop_invariant_exprs_from_stmt(else_stmt, loop_names, body, out);
                }
            }
            Stmt::While {
                condition,
                body: nested_body,
            } => {
                self.collect_loop_invariant_exprs_from_expr(condition, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(nested_body, loop_names, body, out);
            }
            Stmt::For {
                iterable,
                body: nested_body,
                ..
            } => {
                self.collect_loop_invariant_exprs_from_expr(iterable, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(nested_body, loop_names, body, out);
            }
            Stmt::IfLet {
                value,
                then_stmt,
                else_stmt,
                ..
            } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(then_stmt, loop_names, body, out);
                if let Some(else_stmt) = else_stmt {
                    self.collect_loop_invariant_exprs_from_stmt(else_stmt, loop_names, body, out);
                }
            }
            Stmt::WhileLet {
                value, body: then_stmt, ..
            } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(then_stmt, loop_names, body, out);
            }
            Stmt::Function { .. } | Stmt::Impl { .. } => {}
            Stmt::Return { value: None }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Import { .. }
            | Stmt::Struct { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Trait { .. }
            | Stmt::Empty => {}
        }
    }

    pub(super) fn collect_loop_invariant_exprs(&self, pattern: &ForPattern, body: &Stmt) -> Vec<Expr> {
        let mut loop_names = HashSet::new();
        Self::collect_for_pattern_names(pattern, &mut loop_names);
        let mut exprs = Vec::new();
        self.collect_loop_invariant_exprs_from_stmt(body, &loop_names, body, &mut exprs);
        exprs
    }
}
