use std::collections::HashSet;

use crate::{
    expr::{Expr, Pattern, SelectPattern, TemplateStringPart},
    stmt::Stmt,
};

pub(super) fn collect_expr_free_vars(expr: &Expr, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match expr {
        Expr::Var(name) => {
            if !bound.contains(name) {
                free.push(name.clone());
            }
        }
        Expr::Bin(lhs, _, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs)
        | Expr::NullishCoalescing(lhs, rhs)
        | Expr::Access(lhs, rhs)
        | Expr::OptionalAccess(lhs, rhs) => {
            collect_expr_free_vars(lhs, bound, free);
            collect_expr_free_vars(rhs, bound, free);
        }
        Expr::CallExpr(callee, args) => {
            collect_expr_free_vars(callee, bound, free);
            for arg in args {
                collect_expr_free_vars(arg, bound, free);
            }
        }
        Expr::Unary(_, inner) | Expr::Paren(inner) => collect_expr_free_vars(inner, bound, free),
        Expr::Conditional(condition, then_expr, else_expr) => {
            collect_expr_free_vars(condition, bound, free);
            collect_expr_free_vars(then_expr, bound, free);
            collect_expr_free_vars(else_expr, bound, free);
        }
        Expr::List(values) => {
            for value in values {
                collect_expr_free_vars(value, bound, free);
            }
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_expr_free_vars(key, bound, free);
                collect_expr_free_vars(value, bound, free);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr_free_vars(value, bound, free);
            }
        }
        Expr::Call(name, args) => {
            if !bound.contains(name) {
                free.push(name.clone());
            }
            for arg in args {
                collect_expr_free_vars(arg, bound, free);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            collect_expr_free_vars(callee, bound, free);
            for arg in positional {
                collect_expr_free_vars(arg, bound, free);
            }
            for (_, arg) in named {
                collect_expr_free_vars(arg, bound, free);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for value in [start, end, step].into_iter().flatten() {
                collect_expr_free_vars(value, bound, free);
            }
        }
        Expr::Select { cases, default_case } => {
            for case in cases {
                match &case.pattern {
                    SelectPattern::Recv { channel, .. } => collect_expr_free_vars(channel, bound, free),
                    SelectPattern::Send { channel, value } => {
                        collect_expr_free_vars(channel, bound, free);
                        collect_expr_free_vars(value, bound, free);
                    }
                }
                if let Some(guard) = &case.guard {
                    collect_expr_free_vars(guard, bound, free);
                }
                collect_expr_free_vars(&case.body, bound, free);
            }
            if let Some(default_case) = default_case {
                collect_expr_free_vars(default_case, bound, free);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                if let TemplateStringPart::Expr(expr) = part {
                    collect_expr_free_vars(expr, bound, free);
                }
            }
        }
        Expr::Closure { params, body } => {
            let mut nested_bound = bound.clone();
            nested_bound.extend(params.iter().cloned());
            collect_expr_free_vars(body, &mut nested_bound, free);
        }
        Expr::Block(statements) => collect_stmt_free_vars(statements, bound, free),
        Expr::Match { value, arms } => {
            collect_expr_free_vars(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                collect_pattern_bound_vars(&arm.pattern, &mut arm_bound);
                collect_expr_free_vars(&arm.body, &mut arm_bound, free);
            }
        }
        Expr::Val(_) => {}
    }
}

fn collect_stmt_free_vars(statements: &[Box<Stmt>], bound: &mut HashSet<String>, free: &mut Vec<String>) {
    for stmt in statements {
        match stmt.as_ref() {
            Stmt::Expr(expr) => collect_expr_free_vars(expr, bound, free),
            Stmt::Return { value: Some(value) } => collect_expr_free_vars(value, bound, free),
            Stmt::Return { value: None } | Stmt::Empty | Stmt::Break | Stmt::Continue => {}
            Stmt::Let { pattern, value, .. } => {
                collect_expr_free_vars(value, bound, free);
                collect_pattern_bound_vars(pattern, bound);
            }
            Stmt::Define { name, value } => {
                collect_expr_free_vars(value, bound, free);
                bound.insert(name.clone());
            }
            Stmt::Assign { name, value, .. } | Stmt::CompoundAssign { name, value, .. } => {
                if !bound.contains(name) {
                    free.push(name.clone());
                }
                collect_expr_free_vars(value, bound, free);
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                collect_expr_free_vars(condition, bound, free);
                collect_single_stmt_free_vars(then_stmt, &mut bound.clone(), free);
                if let Some(else_stmt) = else_stmt {
                    collect_single_stmt_free_vars(else_stmt, &mut bound.clone(), free);
                }
            }
            Stmt::While { condition, body } => {
                collect_expr_free_vars(condition, bound, free);
                collect_single_stmt_free_vars(body, &mut bound.clone(), free);
            }
            Stmt::Block { statements } => collect_stmt_free_vars(statements, &mut bound.clone(), free),
            Stmt::Function { name, .. } => {
                bound.insert(name.clone());
            }
            _ => {}
        }
    }
}

fn collect_single_stmt_free_vars(stmt: &Stmt, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    collect_stmt_free_vars(&[Box::new(stmt.clone())], bound, free);
}

fn collect_pattern_bound_vars(pattern: &Pattern, bound: &mut HashSet<String>) {
    match pattern {
        Pattern::Variable(name) => {
            bound.insert(name.clone());
        }
        Pattern::List { patterns, rest } => {
            for pattern in patterns {
                collect_pattern_bound_vars(pattern, bound);
            }
            if let Some(rest) = rest {
                bound.insert(rest.clone());
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, pattern) in patterns {
                collect_pattern_bound_vars(pattern, bound);
            }
            if let Some(rest) = rest {
                bound.insert(rest.clone());
            }
        }
        Pattern::Or(patterns) => {
            for pattern in patterns {
                collect_pattern_bound_vars(pattern, bound);
            }
        }
        Pattern::Guard { pattern, .. } => collect_pattern_bound_vars(pattern, bound),
        Pattern::Literal(_) | Pattern::Wildcard | Pattern::Range { .. } => {}
    }
}
