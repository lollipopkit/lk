use std::collections::BTreeSet;

use crate::{
    expr::{Expr, MatchArm, Pattern, SelectCase, SelectPattern, TemplateStringPart},
    stmt::{ForPattern, Stmt},
};

pub(crate) struct FreeVarCollector {
    scopes: Vec<BTreeSet<String>>,
    free: BTreeSet<String>,
}

impl FreeVarCollector {
    pub(crate) fn new() -> Self {
        Self {
            scopes: vec![BTreeSet::new()],
            free: BTreeSet::new(),
        }
    }

    pub(crate) fn declare<S: Into<String>>(&mut self, name: S) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.into());
        }
    }

    pub(crate) fn with_scope<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Self),
    {
        self.scopes.push(BTreeSet::new());
        f(self);
        self.scopes.pop();
    }

    pub(crate) fn mark_use(&mut self, name: &str) {
        if !self.is_local(name) {
            self.free.insert(name.to_string());
        }
    }

    pub(crate) fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(name))
    }

    pub(crate) fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Block { statements } => {
                self.with_scope(|collector| {
                    for s in statements {
                        collector.visit_stmt(s);
                    }
                });
            }
            Stmt::Let { pattern, value, .. } => {
                self.visit_expr(value);
                self.bind_pattern(pattern);
            }
            Stmt::Assign { name, value, .. } => {
                self.visit_expr(value);
                self.mark_use(name);
            }
            Stmt::CompoundAssign { name, value, .. } => {
                self.visit_expr(value);
                self.mark_use(name);
            }
            Stmt::Define { name, value } => {
                self.visit_expr(value);
                self.declare(name);
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                self.visit_expr(condition);
                self.with_scope(|collector| collector.visit_stmt(then_stmt));
                if let Some(else_stmt) = else_stmt {
                    self.with_scope(|collector| collector.visit_stmt(else_stmt));
                }
            }
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => {
                self.visit_expr(value);
                self.with_scope(|collector| {
                    collector.bind_pattern(pattern);
                    collector.visit_stmt(then_stmt);
                });
                if let Some(else_stmt) = else_stmt {
                    self.with_scope(|collector| collector.visit_stmt(else_stmt));
                }
            }
            Stmt::While { condition, body } => {
                self.visit_expr(condition);
                self.with_scope(|collector| collector.visit_stmt(body));
            }
            Stmt::WhileLet { pattern, value, body } => {
                self.visit_expr(value);
                self.with_scope(|collector| {
                    collector.bind_pattern(pattern);
                    collector.visit_stmt(body);
                });
            }
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                self.visit_expr(iterable);
                self.with_scope(|collector| {
                    collector.bind_for_pattern(pattern);
                    collector.visit_stmt(body);
                });
            }
            Stmt::Return { value } => {
                if let Some(expr) = value {
                    self.visit_expr(expr);
                }
            }
            Stmt::Expr(expr) => {
                self.visit_expr(expr);
            }
            Stmt::Function { name, .. } => {
                self.declare(name);
            }
            Stmt::Struct { name, .. } | Stmt::TypeAlias { name, .. } | Stmt::Trait { name, .. } => {
                self.declare(name);
            }
            Stmt::Impl { methods, .. } => {
                for method in methods {
                    self.visit_stmt(method);
                }
            }
            Stmt::Import(_) | Stmt::Break | Stmt::Continue | Stmt::Empty => {}
        }
    }

    pub(crate) fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Bin(l, _, r) | Expr::And(l, r) | Expr::Or(l, r) | Expr::NullishCoalescing(l, r) => {
                self.visit_expr(l);
                self.visit_expr(r);
            }
            Expr::Unary(_, e) | Expr::Paren(e) => self.visit_expr(e),
            Expr::Conditional(c, t, e) => {
                self.visit_expr(c);
                self.visit_expr(t);
                self.visit_expr(e);
            }
            Expr::Access(base, field) | Expr::OptionalAccess(base, field) => {
                self.visit_expr(base);
                self.visit_expr(field);
            }
            Expr::List(items) => {
                for item in items {
                    self.visit_expr(item);
                }
            }
            Expr::Map(entries) => {
                for (k, v) in entries {
                    self.visit_expr(k);
                    self.visit_expr(v);
                }
            }
            Expr::StructLiteral { fields, .. } => {
                for (_, expr) in fields {
                    self.visit_expr(expr);
                }
            }
            Expr::Var(name) => self.mark_use(name),
            Expr::Call(name, args) => {
                self.mark_use(name);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            Expr::CallExpr(callee, args) => {
                self.visit_expr(callee);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            Expr::CallNamed(callee, positional, named) => {
                self.visit_expr(callee);
                for arg in positional {
                    self.visit_expr(arg);
                }
                for (_, expr) in named {
                    self.visit_expr(expr);
                }
            }
            Expr::Range { start, end, step, .. } => {
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
                if let Some(st) = step {
                    self.visit_expr(st);
                }
            }
            Expr::Select { cases, default_case } => {
                for case in cases {
                    self.visit_select_case(case);
                }
                if let Some(default) = default_case {
                    self.visit_expr(default);
                }
            }
            Expr::TemplateString(parts) => {
                for part in parts {
                    if let TemplateStringPart::Expr(expr) = part {
                        self.visit_expr(expr);
                    }
                }
            }
            Expr::Closure { .. } => {}
            Expr::Match { value, arms } => {
                self.visit_expr(value);
                for MatchArm { pattern, body } in arms {
                    self.with_scope(|collector| {
                        collector.bind_pattern(pattern);
                        collector.visit_expr(body);
                    });
                }
            }
            Expr::Val(_) => {}
        }
    }

    fn visit_select_case(&mut self, case: &SelectCase) {
        self.with_scope(|collector| {
            match &case.pattern {
                SelectPattern::Recv { binding, channel } => {
                    collector.visit_expr(channel);
                    if let Some(name) = binding {
                        collector.declare(name);
                    }
                }
                SelectPattern::Send { channel, value } => {
                    collector.visit_expr(channel);
                    collector.visit_expr(value);
                }
            }
            if let Some(guard) = &case.guard {
                collector.visit_expr(guard);
            }
            collector.visit_expr(&case.body);
        });
    }

    fn bind_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Variable(name) => {
                self.declare(name);
            }
            Pattern::List { patterns, rest } => {
                for p in patterns {
                    self.bind_pattern(p);
                }
                if let Some(rest_name) = rest {
                    self.declare(rest_name);
                }
            }
            Pattern::Map { patterns, rest } => {
                for (_, p) in patterns {
                    self.bind_pattern(p);
                }
                if let Some(rest_name) = rest {
                    self.declare(rest_name);
                }
            }
            Pattern::Or(alternatives) => {
                let mut names = BTreeSet::new();
                for alt in alternatives {
                    let mut inner = FreeVarCollector::new();
                    inner.bind_pattern(alt);
                    if let Some(scope) = inner.scopes.last() {
                        names.extend(scope.iter().cloned());
                    }
                }
                for name in names {
                    self.declare(name);
                }
            }
            Pattern::Guard { pattern, guard } => {
                self.bind_pattern(pattern);
                self.visit_expr(guard);
            }
            Pattern::Range { start, end, .. } => {
                self.visit_expr(start);
                self.visit_expr(end);
            }
            Pattern::Literal(_) | Pattern::Wildcard => {}
        }
    }

    fn bind_for_pattern(&mut self, pattern: &ForPattern) {
        match pattern {
            ForPattern::Variable(name) => {
                self.declare(name);
            }
            ForPattern::Ignore => {}
            ForPattern::Tuple(patterns) => {
                for pat in patterns {
                    self.bind_for_pattern(pat);
                }
            }
            ForPattern::Array { patterns, rest } => {
                for pat in patterns {
                    self.bind_for_pattern(pat);
                }
                if let Some(rest_name) = rest {
                    self.declare(rest_name);
                }
            }
            ForPattern::Object(fields) => {
                for (name, pat) in fields {
                    self.declare(name);
                    self.bind_for_pattern(pat);
                }
            }
        }
    }

    pub(crate) fn into_sorted_vec(self) -> Vec<String> {
        self.free.into_iter().collect()
    }
}
