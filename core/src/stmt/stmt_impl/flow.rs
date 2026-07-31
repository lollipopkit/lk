//! "Can control reach the end of this body?"
//!
//! A function that declares `-> Int` and has a path with no `return` on it
//! answers `nil` for that path — a value of a type the declaration ruled out,
//! with no diagnostic anywhere:
//!
//! ```lk
//! fn g(c: Bool) -> Int { if c { return 1; } }
//! g(false) + 1              // Add expected numbers or strings, got Nil and Int
//! ```
//!
//! The runtime error names the operator, three call frames from the function
//! that promised an `Int`. This module is what lets the checker say it at the
//! declaration instead.
//!
//! **Only provable divergence counts.** The answer is used to *reject*, so a
//! construct this cannot analyse must answer `false` for "diverges" — the
//! program then needs an explicit `return`, which is a false alarm — or `true`
//! — and the hole stays. Between those, the second is the conservative choice
//! for a language with existing programs, so anything not listed here is
//! treated as "may fall through" only when that cannot produce a false alarm;
//! see the `Expr` arms, which are deliberately generous.

use crate::expr::{Expr, MatchArm, Pattern};
use crate::stmt::Stmt;

/// Does every path through `stmt` leave the function (via `return`, or a raise
/// that cannot be caught here)?
pub(crate) fn always_diverges(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::Attributed { item, .. } => always_diverges(item),
        Stmt::Block { statements, .. } => statements.iter().any(|stmt| always_diverges(stmt)),
        // Both arms, or nothing: an `if` with no `else` always has the path
        // where the condition was false.
        Stmt::If {
            then_stmt,
            else_stmt: Some(else_stmt),
            ..
        }
        | Stmt::IfLet {
            then_stmt,
            else_stmt: Some(else_stmt),
            ..
        } => always_diverges(then_stmt) && always_diverges(else_stmt),
        // `while true { … }` with no `break` never falls out of the loop. The
        // condition has already been constant-folded by the parser, so this is
        // the literal `true` a reader wrote.
        Stmt::While { condition, body } => is_true_literal(condition) && !contains_break(body),
        Stmt::Expr { value, .. } => expr_always_diverges(value),
        _ => false,
    }
}

fn is_true_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Literal(crate::val::LiteralVal::Bool(true)))
}

/// Does `expr` — as a *statement* — leave the function on every path?
///
/// Generous on purpose: an expression this does not recognise answers `false`,
/// which only ever means "the enclosing body needs an explicit `return`".
fn expr_always_diverges(expr: &Expr) -> bool {
    match expr {
        // `error(v)` / `panic(v)` raise, and a raise leaves the function unless
        // a `try` in *this* body catches it — which the `Try` arm below
        // accounts for.
        //
        // Both spellings: the parser produces `CallExpr(Var("error"), …)` for a
        // bare name, and `Call` for the desugarings that build one directly.
        // Matching only `Call` made `fn f() -> Int { error("x"); }` a false
        // alarm — which is how a new check earns its reputation.
        Expr::Call(name, _) => is_raising_builtin(name),
        Expr::CallExpr(callee, _) => matches!(callee.as_ref(), Expr::Var(name) if is_raising_builtin(name)),
        Expr::Paren(inner) | Expr::Unsafe(inner) => expr_always_diverges(inner),
        Expr::Block(statements) => statements.iter().any(|stmt| always_diverges(stmt)),
        Expr::Conditional(_, then_expr, else_expr) => {
            expr_always_diverges(then_expr) && expr_always_diverges(else_expr)
        }
        // Every arm diverges *and* the arms cover everything. Without a
        // catch-all the value may match none of them, and that path falls
        // through — `match`'s own exhaustiveness is checked elsewhere and does
        // not extend to "and therefore every path returned".
        Expr::Match { arms, .. } => arms.iter().any(|arm| is_catch_all(&arm.pattern)) && arms.iter().all(arm_diverges),
        // A `try` whose *handler* diverges: the body may or may not raise, so
        // the handler is the path that has to leave, and so does the body.
        Expr::Try { body, handler, .. } => {
            body.iter().any(|stmt| always_diverges(stmt)) && handler.iter().any(|stmt| always_diverges(stmt))
        }
        _ => false,
    }
}

fn is_raising_builtin(name: &str) -> bool {
    matches!(name, "error" | "panic")
}

fn arm_diverges(arm: &MatchArm) -> bool {
    expr_always_diverges(&arm.body)
}

fn is_catch_all(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Wildcard => true,
        // A bare binding matches anything; a guarded one does not.
        Pattern::Variable(_) => true,
        _ => false,
    }
}

/// A `break` that would leave *this* loop — nested loops swallow their own.
fn contains_break(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Break => true,
        Stmt::Attributed { item, .. } => contains_break(item),
        Stmt::Block { statements, .. } => statements.iter().any(|stmt| contains_break(stmt)),
        Stmt::If {
            then_stmt, else_stmt, ..
        }
        | Stmt::IfLet {
            then_stmt, else_stmt, ..
        } => contains_break(then_stmt) || else_stmt.as_deref().is_some_and(contains_break),
        // A `break` inside a nested loop belongs to that loop.
        Stmt::While { .. } | Stmt::WhileLet { .. } | Stmt::For { .. } => false,
        // An expression can hold a block (`match` arms, `if` values), and a
        // `break` in one of those does leave this loop.
        Stmt::Expr { value, .. } => expr_contains_break(value),
        _ => false,
    }
}

fn expr_contains_break(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) | Expr::Unsafe(inner) => expr_contains_break(inner),
        Expr::Block(statements) => statements.iter().any(|stmt| contains_break(stmt)),
        Expr::Conditional(_, then_expr, else_expr) => expr_contains_break(then_expr) || expr_contains_break(else_expr),
        Expr::Match { arms, .. } => arms.iter().any(|arm| expr_contains_break(&arm.body)),
        Expr::Try { body, handler, .. } => {
            body.iter().any(|stmt| contains_break(stmt)) || handler.iter().any(|stmt| contains_break(stmt))
        }
        _ => false,
    }
}
