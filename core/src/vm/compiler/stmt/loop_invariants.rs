use std::collections::HashSet;

use crate::{
    expr::Expr,
    stmt::{ForPattern, Stmt},
    val::{Type, Val},
};

use super::FunctionBuilder;

impl FunctionBuilder {
    pub(super) fn lookup_loop_invariant_let(&self, name: &str) -> Option<u16> {
        self.loop_invariant_let_regs
            .iter()
            .rev()
            .find_map(|(candidate, reg)| (candidate == name).then_some(*reg))
    }

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
                Val::Nil | Val::Bool(_) | Val::Int(_) | Val::Float(_) | Val::Str(_) | Val::ShortStr(_)
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

    fn expr_names_stable_in_loop(&self, expr: &Expr, loop_names: &HashSet<String>, body: &Stmt) -> bool {
        expr.requested_ctx().iter().all(|name| {
            !loop_names.contains(name)
                && (self.lookup(name).is_some() || self.lookup_const(name).is_some())
                && !Self::stmt_mutates_name(body, name)
        })
    }

    fn call_expr_safe_to_loop_hoist(&self, expr: &Expr, loop_names: &HashSet<String>, body: &Stmt) -> bool {
        let Expr::CallExpr(callee, args) = expr else {
            return false;
        };
        let Expr::Access(receiver, method) = callee.as_ref() else {
            return false;
        };
        let Expr::Val(method_value) = method.as_ref() else {
            return false;
        };
        let Some(method_name) = method_value.as_str() else {
            return false;
        };

        let stable_args = args
            .iter()
            .all(|arg| self.expr_names_stable_in_loop(arg, loop_names, body));
        if !stable_args {
            return false;
        }

        if args.is_empty() && method_name == "len" {
            return self.expr_names_stable_in_loop(receiver, loop_names, body)
                && matches!(
                    self.expr_value_fact(receiver),
                    Some(Type::String | Type::List(_) | Type::Map(_, _))
                );
        }

        if args.len() == 1
            && matches!(method_name, "starts_with" | "contains")
            && matches!(args[0].as_ref(), Expr::Val(value) if value.as_str().is_some())
        {
            return self.expr_names_stable_in_loop(receiver, loop_names, body)
                && self.expr_known_string_in_loop(receiver);
        }

        if args.len() == 1 && matches!(method_name, "get" | "has") {
            return self.expr_names_stable_in_loop(receiver, loop_names, body)
                && (self.known_map_expr(receiver).is_some() || self.known_list_expr(receiver).is_some());
        }

        if args.len() == 2
            && matches!(method_name, "get" | "has")
            && matches!(receiver.as_ref(), Expr::Var(name) if matches!(name.as_str(), "map" | "list") && self.lookup(name).is_none())
        {
            return self.expr_names_stable_in_loop(args[0].as_ref(), loop_names, body)
                && (self.known_map_expr(args[0].as_ref()).is_some()
                    || self.known_list_expr(args[0].as_ref()).is_some());
        }

        false
    }

    fn expr_known_string_in_loop(&self, expr: &Expr) -> bool {
        if self.expr_value_fact(expr) == Some(Type::String) {
            return true;
        }
        match expr {
            Expr::Val(value) => value.as_str().is_some(),
            Expr::Var(name) => self.const_env.get(name).is_some_and(|value| value.as_str().is_some()),
            Expr::Paren(inner) => self.expr_known_string_in_loop(inner),
            _ => false,
        }
    }

    fn expr_mutates_name(expr: &Expr, target: &str) -> bool {
        match expr {
            Expr::CallExpr(callee, args) => {
                if let Expr::Access(receiver, method) = callee.as_ref()
                    && let Expr::Val(method_value) = method.as_ref()
                    && matches!(method_value.as_str(), Some("set" | "push"))
                {
                    if matches!(receiver.as_ref(), Expr::Var(name) if name == target) {
                        return true;
                    }
                    if matches!(receiver.as_ref(), Expr::Var(name) if matches!(name.as_str(), "map" | "list"))
                        && matches!(args.first().map(|arg| arg.as_ref()), Some(Expr::Var(name)) if name == target)
                    {
                        return true;
                    }
                }
                Self::expr_mutates_name(callee, target) || args.iter().any(|arg| Self::expr_mutates_name(arg, target))
            }
            Expr::Call(_, args) => args.iter().any(|arg| Self::expr_mutates_name(arg, target)),
            Expr::CallNamed(callee, pos_args, named_args) => {
                Self::expr_mutates_name(callee, target)
                    || pos_args.iter().any(|arg| Self::expr_mutates_name(arg, target))
                    || named_args.iter().any(|(_, arg)| Self::expr_mutates_name(arg, target))
            }
            Expr::Paren(inner) | Expr::Unary(_, inner) => Self::expr_mutates_name(inner, target),
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right)
            | Expr::Access(left, right)
            | Expr::OptionalAccess(left, right) => {
                Self::expr_mutates_name(left, target) || Self::expr_mutates_name(right, target)
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                Self::expr_mutates_name(condition, target)
                    || Self::expr_mutates_name(then_expr, target)
                    || Self::expr_mutates_name(else_expr, target)
            }
            Expr::List(items) => items.iter().any(|item| Self::expr_mutates_name(item, target)),
            Expr::Map(pairs) => pairs
                .iter()
                .any(|(key, value)| Self::expr_mutates_name(key, target) || Self::expr_mutates_name(value, target)),
            Expr::Range { start, end, step, .. } => start
                .iter()
                .chain(end.iter())
                .chain(step.iter())
                .any(|expr| Self::expr_mutates_name(expr, target)),
            Expr::TemplateString(parts) => parts.iter().any(|part| match part {
                crate::expr::TemplateStringPart::Literal(_) => false,
                crate::expr::TemplateStringPart::Expr(expr) => Self::expr_mutates_name(expr, target),
            }),
            Expr::Match { value, arms } => {
                Self::expr_mutates_name(value, target)
                    || arms.iter().any(|arm| Self::expr_mutates_name(&arm.body, target))
            }
            Expr::StructLiteral { fields, .. } => {
                fields.iter().any(|(_, value)| Self::expr_mutates_name(value, target))
            }
            Expr::Val(_) | Expr::Var(_) | Expr::Closure { .. } | Expr::Block(_) | Expr::Select { .. } => false,
        }
    }

    pub(super) fn stmt_mutates_name(stmt: &Stmt, target: &str) -> bool {
        if Self::stmt_assigns_name(stmt, target) {
            return true;
        }
        match stmt {
            Stmt::Block { statements } => statements.iter().any(|stmt| Self::stmt_mutates_name(stmt, target)),
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Define { value, .. }
            | Stmt::Expr(value)
            | Stmt::Return { value: Some(value) }
            | Stmt::CompoundAssign { value, .. } => Self::expr_mutates_name(value, target),
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                Self::expr_mutates_name(condition, target)
                    || Self::stmt_mutates_name(then_stmt, target)
                    || else_stmt
                        .as_deref()
                        .is_some_and(|branch| Self::stmt_mutates_name(branch, target))
            }
            Stmt::While { condition, body } => {
                Self::expr_mutates_name(condition, target) || Self::stmt_mutates_name(body, target)
            }
            Stmt::For { iterable, body, .. } => {
                Self::expr_mutates_name(iterable, target) || Self::stmt_mutates_name(body, target)
            }
            Stmt::IfLet {
                value,
                then_stmt,
                else_stmt,
                ..
            } => {
                Self::expr_mutates_name(value, target)
                    || Self::stmt_mutates_name(then_stmt, target)
                    || else_stmt
                        .as_deref()
                        .is_some_and(|branch| Self::stmt_mutates_name(branch, target))
            }
            Stmt::WhileLet { value, body, .. } => {
                Self::expr_mutates_name(value, target) || Self::stmt_mutates_name(body, target)
            }
            Stmt::Function { body, .. } => Self::stmt_mutates_name(body, target),
            Stmt::Impl { methods, .. } => methods.iter().any(|method| Self::stmt_mutates_name(method, target)),
            Stmt::Return { value: None }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Import { .. }
            | Stmt::Struct { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Trait { .. }
            | Stmt::Empty => false,
        }
    }

    fn stmt_reassigns_name(stmt: &Stmt, target: &str) -> bool {
        match stmt {
            Stmt::Block { statements } => statements.iter().any(|stmt| Self::stmt_reassigns_name(stmt, target)),
            Stmt::Assign { name, .. } | Stmt::CompoundAssign { name, .. } => name == target,
            Stmt::If {
                then_stmt, else_stmt, ..
            }
            | Stmt::IfLet {
                then_stmt, else_stmt, ..
            } => {
                Self::stmt_reassigns_name(then_stmt, target)
                    || else_stmt
                        .as_deref()
                        .is_some_and(|branch| Self::stmt_reassigns_name(branch, target))
            }
            Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
                Self::stmt_reassigns_name(body, target)
            }
            Stmt::Function { body, .. } => Self::stmt_reassigns_name(body, target),
            Stmt::Impl { methods, .. } => methods.iter().any(|method| Self::stmt_reassigns_name(method, target)),
            Stmt::Let { .. }
            | Stmt::Define { .. }
            | Stmt::Expr(_)
            | Stmt::Return { .. }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Import { .. }
            | Stmt::Struct { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Trait { .. }
            | Stmt::Empty => false,
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
                    !loop_names.contains(name) && self.lookup(name).is_some() && !Self::stmt_mutates_name(body, name)
                });
            if safe_literal || safe_named {
                if !out.iter().any(|existing| existing == expr) {
                    out.push(expr.clone());
                }
                return;
            }
        } else if self.call_expr_safe_to_loop_hoist(expr, loop_names, body) {
            if !out.iter().any(|existing| existing == expr) {
                out.push(expr.clone());
            }
            return;
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
                // The range start register becomes the mutable loop index in ForRangeLoop,
                // so it cannot be reused as a stable invariant literal/register.
                let _ = start;
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

    pub(super) fn collect_loop_invariant_literal_lets(&self, body: &Stmt) -> Vec<(String, Expr)> {
        let mut lets = Vec::new();
        self.collect_loop_invariant_literal_lets_from_stmt(body, body, &mut lets);
        lets
    }

    fn collect_loop_invariant_literal_lets_from_stmt(&self, stmt: &Stmt, body: &Stmt, out: &mut Vec<(String, Expr)>) {
        match stmt {
            Stmt::Block { statements } => {
                for stmt in statements {
                    self.collect_loop_invariant_literal_lets_from_stmt(stmt, body, out);
                }
            }
            Stmt::Let {
                pattern: crate::expr::Pattern::Variable(name),
                value,
                is_const: false,
                ..
            } if Self::expr_is_immutable_literal(value) && !Self::stmt_reassigns_name(body, name) => {
                if !out.iter().any(|(existing, _)| existing == name) {
                    out.push((name.clone(), value.as_ref().clone()));
                }
            }
            Stmt::If {
                then_stmt, else_stmt, ..
            }
            | Stmt::IfLet {
                then_stmt, else_stmt, ..
            } => {
                self.collect_loop_invariant_literal_lets_from_stmt(then_stmt, body, out);
                if let Some(else_stmt) = else_stmt {
                    self.collect_loop_invariant_literal_lets_from_stmt(else_stmt, body, out);
                }
            }
            Stmt::While { body: nested, .. } | Stmt::WhileLet { body: nested, .. } | Stmt::For { body: nested, .. } => {
                self.collect_loop_invariant_literal_lets_from_stmt(nested, body, out);
            }
            Stmt::Function { .. } | Stmt::Impl { .. } => {}
            Stmt::Let { .. }
            | Stmt::Assign { .. }
            | Stmt::CompoundAssign { .. }
            | Stmt::Define { .. }
            | Stmt::Expr(_)
            | Stmt::Return { .. }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Import { .. }
            | Stmt::Struct { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Trait { .. }
            | Stmt::Empty => {}
        }
    }
}
