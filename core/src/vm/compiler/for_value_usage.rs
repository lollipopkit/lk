#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::{Expr, Pattern, TemplateStringPart},
    stmt::Stmt,
};

pub(super) fn stmt_uses_for_binding_value(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Attributed { item, .. } => stmt_uses_for_binding_value(item, name),
        Stmt::Empty | Stmt::Break | Stmt::Continue | Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {
            false
        }
        Stmt::Expr(expr) | Stmt::Return { value: Some(expr) } => expr_uses_for_binding_value(expr, name),
        Stmt::Return { value: None } => false,
        Stmt::Let { value, .. } => expr_uses_for_binding_value(value, name),
        Stmt::Define { value, .. } => expr_uses_for_binding_value(value, name),
        Stmt::Assign { value, .. } | Stmt::CompoundAssign { value, .. } => expr_uses_for_binding_value(value, name),
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            expr_uses_for_binding_value(condition, name)
                || stmt_uses_for_binding_value(then_stmt, name)
                || else_stmt
                    .as_deref()
                    .is_some_and(|stmt| stmt_uses_for_binding_value(stmt, name))
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            expr_uses_for_binding_value(value, name)
                || stmt_uses_for_binding_value(then_stmt, name)
                || else_stmt
                    .as_deref()
                    .is_some_and(|stmt| stmt_uses_for_binding_value(stmt, name))
        }
        Stmt::While { condition, body } => {
            expr_uses_for_binding_value(condition, name) || stmt_uses_for_binding_value(body, name)
        }
        Stmt::WhileLet { value, body, .. } => {
            expr_uses_for_binding_value(value, name) || stmt_uses_for_binding_value(body, name)
        }
        Stmt::For { iterable, body, .. } => {
            expr_uses_for_binding_value(iterable, name) || stmt_uses_for_binding_value(body, name)
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                if stmt_uses_for_binding_value(stmt, name) {
                    return true;
                }
                if stmt_shadows_name(stmt, name) {
                    return false;
                }
            }
            false
        }
        Stmt::Function {
            params,
            named_params,
            body,
            ..
        } => {
            if params.iter().any(|param| param == name) || named_params.iter().any(|param| param.name == name) {
                return false;
            }
            stmt_uses_for_binding_value(body, name)
        }
        Stmt::Trait { .. } => false,
        Stmt::Impl { methods, .. } => methods.iter().any(|method| stmt_uses_for_binding_value(method, name)),
    }
}

fn expr_uses_for_binding_value(expr: &Expr, name: &str) -> bool {
    if is_single_char_len_expr(expr, name) {
        return false;
    }
    match expr {
        Expr::Paren(inner) | Expr::Unary(_, inner) | Expr::Yield(inner) => expr_uses_for_binding_value(inner, name),
        Expr::Var(value) => value == name,
        Expr::CallExpr(callee, args) if is_single_char_len_call(callee, args, name) => false,
        Expr::Bin(lhs, _, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs)
        | Expr::NullishCoalescing(lhs, rhs)
        | Expr::Access(lhs, rhs)
        | Expr::OptionalAccess(lhs, rhs) => {
            expr_uses_for_binding_value(lhs, name) || expr_uses_for_binding_value(rhs, name)
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            expr_uses_for_binding_value(condition, name)
                || expr_uses_for_binding_value(then_expr, name)
                || expr_uses_for_binding_value(else_expr, name)
        }
        Expr::Call(_, args) => args.iter().any(|arg| expr_uses_for_binding_value(arg, name)),
        Expr::CallExpr(callee, args) => {
            expr_uses_for_binding_value(callee, name) || args.iter().any(|arg| expr_uses_for_binding_value(arg, name))
        }
        Expr::CallNamed(callee, positional, named) => {
            expr_uses_for_binding_value(callee, name)
                || positional.iter().any(|arg| expr_uses_for_binding_value(arg, name))
                || named.iter().any(|(_, arg)| expr_uses_for_binding_value(arg, name))
        }
        Expr::List(values) => values.iter().any(|value| expr_uses_for_binding_value(value, name)),
        Expr::Map(entries) => entries
            .iter()
            .any(|(key, value)| expr_uses_for_binding_value(key, name) || expr_uses_for_binding_value(value, name)),
        Expr::StructLiteral { fields, .. } => fields.iter().any(|(_, value)| expr_uses_for_binding_value(value, name)),
        Expr::TemplateString(parts) => parts.iter().any(|part| match part {
            TemplateStringPart::Expr(expr) => expr_uses_for_binding_value(expr, name),
            TemplateStringPart::Literal(_) => false,
        }),
        Expr::Block(statements) => {
            for stmt in statements {
                if stmt_uses_for_binding_value(stmt, name) {
                    return true;
                }
                if stmt_shadows_name(stmt, name) {
                    return false;
                }
            }
            false
        }
        Expr::Range { start, end, step, .. } => [start, end, step]
            .into_iter()
            .flatten()
            .any(|expr| expr_uses_for_binding_value(expr, name)),
        Expr::Match { value, arms } => {
            expr_uses_for_binding_value(value, name)
                || arms.iter().any(|arm| expr_uses_for_binding_value(&arm.body, name))
        }
        Expr::Closure { params, body } => {
            if params.iter().any(|param| param == name) {
                return false;
            }
            expr_uses_for_binding_value(body, name)
        }
        Expr::Literal(_) => false,
    }
}

fn is_single_char_len_expr(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Paren(inner) => is_single_char_len_expr(inner, name),
        Expr::CallExpr(callee, args) => is_single_char_len_call(callee, args, name),
        _ => expr.to_string() == format!("{name}.len()"),
    }
}

fn is_single_char_len_call(callee: &Expr, args: &[Box<Expr>], name: &str) -> bool {
    if !args.is_empty() {
        return false;
    }
    let Expr::Access(target, method) = callee else {
        return false;
    };
    matches!(target.as_ref(), Expr::Var(value) if value == name)
        && (matches!(
            method.as_ref(),
            Expr::Var(value) if value == "len"
        ) || matches!(
            method.as_ref(),
            Expr::Literal(value) if value.as_str() == Some("len")
        ))
}

pub(super) fn stmt_shadows_name_deep(stmt: &Stmt, name: &str) -> bool {
    if stmt_shadows_name(stmt, name) {
        return true;
    }
    match stmt {
        Stmt::Attributed { item, .. } => stmt_shadows_name_deep(item, name),
        Stmt::If {
            then_stmt, else_stmt, ..
        } => {
            stmt_shadows_name_deep(then_stmt, name)
                || else_stmt
                    .as_deref()
                    .is_some_and(|stmt| stmt_shadows_name_deep(stmt, name))
        }
        Stmt::IfLet {
            then_stmt, else_stmt, ..
        } => {
            stmt_shadows_name_deep(then_stmt, name)
                || else_stmt
                    .as_deref()
                    .is_some_and(|stmt| stmt_shadows_name_deep(stmt, name))
        }
        Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
            stmt_shadows_name_deep(body, name)
        }
        Stmt::Block { statements } => statements.iter().any(|stmt| stmt_shadows_name_deep(stmt, name)),
        Stmt::Impl { methods, .. } => methods.iter().any(|method| stmt_shadows_name_deep(method, name)),
        Stmt::Function { .. } => false,
        Stmt::Empty
        | Stmt::Expr(_)
        | Stmt::Return { .. }
        | Stmt::Let { .. }
        | Stmt::Define { .. }
        | Stmt::Assign { .. }
        | Stmt::CompoundAssign { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Import(_)
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. } => false,
    }
}

fn stmt_shadows_name(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Attributed { item, .. } => stmt_shadows_name(item, name),
        Stmt::Let { pattern, .. } => pattern_shadows_name(pattern, name),
        Stmt::Function {
            name: function_name, ..
        } => function_name == name,
        _ => false,
    }
}

fn pattern_shadows_name(pattern: &Pattern, name: &str) -> bool {
    match pattern {
        Pattern::Variable(value) => value == name,
        Pattern::List { patterns, .. } | Pattern::Or(patterns) => {
            patterns.iter().any(|pattern| pattern_shadows_name(pattern, name))
        }
        Pattern::Map { patterns, .. } => patterns.iter().any(|(_, pattern)| pattern_shadows_name(pattern, name)),
        Pattern::Guard { pattern, .. } => pattern_shadows_name(pattern, name),
        Pattern::Literal(_) | Pattern::Range { .. } | Pattern::Wildcard => false,
    }
}
