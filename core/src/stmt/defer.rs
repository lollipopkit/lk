//! `defer` — a release that happens on every path, written once.
//!
//! ```lk
//! fn drive(card: Int) -> Bool {
//!     let pages = take_pages();
//!     defer give_pages_back(pages);
//!     if (!ready(card)) { return false; }   // released
//!     if (!answered(card)) { return false; } // released
//!     return true;                           // released
//! }
//! ```
//!
//! Why it exists here rather than being left to discipline: a kernel is where
//! the discipline fails, and it fails quietly. Writing this language's bare-metal
//! demonstration produced, in one sitting, an allocator that leaked the pages it
//! had already taken when a run turned out not to be contiguous, a task-stack
//! allocator with the same bug, and one function that had to be *split in two*
//! so that five pages could be released on the seven paths that gave up. None of
//! those was a wrong answer. Each was a machine that ran out of memory later, on
//! a path nothing exercised.
//!
//! # It is a shape, not a mechanism
//!
//! The rewrite happens once, on the AST, before the resolver and the type
//! checker and both compilers. Nothing downstream ever sees a `Stmt::Defer`.
//!
//! That is deliberate and it is the whole design. A release that must happen on
//! every path is a property of the *code*, and code is what a compiler already
//! has. Making it a runtime mechanism would mean a stack of pending actions, a
//! way to run them, and — the part that decides it — an implementation in each
//! backend, which is how the interpreter and the compiled build come to disagree
//! about a program. There is no version of that which is worth more than a
//! rewrite.
//!
//! # What it does not do
//!
//! **It does not run when a raise unwinds past it.** A `raise` leaves through
//! `longjmp` on the native path, which is not a `return` and is not visible to
//! this rewrite. A `defer` releasing something a handler then needs would be
//! worse than no `defer`, so this is said plainly rather than approximated.
//!
//! There *is* a third option, and it was **built and measured** (2026-07-30)
//! rather than argued about, so the question does not have to be re-opened from
//! scratch: wrap the body in a `try`, run the releases in the `catch`, and
//! re-raise. It stays a pure AST rewrite — `try`/`catch` and re-raising from a
//! handler both work in both backends — so it is not the runtime mechanism this
//! section argues against, and it does make `defer` run on the raise path.
//!
//! It was reverted, for a cost that only showed up once it existed. Wrapping a
//! whole function body means every register the body assigns becomes a try
//! region *output cell*, and register reuse makes that most of them. A cell
//! round-trip is defined for scalars and deliberately **not** for container
//! handles (`unbox_from_dyn` — reading one back as the wrong typed handle is a
//! wrong answer, not a rejection). So `examples/syntax/defer.lk` stopped
//! lowering natively the moment the wrap went in.
//!
//! That trade is the wrong way round: a documented semantic gap became a
//! *silent* three-times slowdown in exactly the code this feature exists for — a
//! kernel, which is compiled. The prerequisite is therefore not
//! `docs/aot/aot-gaps-and-lkrt.md` §17 (that one is done, and a `return` inside
//! a `try` body lowers now); it is a cell round-trip for container handles.
//!
//! That prerequisite is now met (2026-07-30): a boxed container round-trips by
//! pointer, and a *typed* one parks as a raw handle under its own tag, so every
//! container crosses a region.
//!
//! The wrap was then tried a second time, and reverted again — this time for the
//! *return* plumbing rather than the cells. A function whose returns are all
//! inside the wrapped body has no `Exit::Ret` of its own, so its return type
//! goes missing; carrying the parked type out to the caller fixes that shape and
//! breaks the mixed one (a real return *and* a parked one), and the fall-off
//! path then returns void against a typed signature. Each of those is
//! answerable; together they are a piece of work of their own, and a
//! half-finished version of it is how a function silently returns the wrong
//! thing. The measurements are in `docs/aot/aot-gaps-and-lkrt.md` §17.
//!
//! **It may only appear at the top level of a function body.** Not inside an
//! `if`, a loop, or a nested block. That is what makes the rewrite sound: at the
//! top level, textual order is execution order, so "every `defer` written above
//! this `return`" is exactly "every `defer` that has run". A `defer` inside a
//! branch would have to be tracked at run time, which is the mechanism this is
//! not.
//!
//! The restriction also matches what the feature is for. A resource taken in the
//! middle of a loop is released at the end of that iteration, which is an
//! ordinary pair of statements; what needs `defer` is the resource a whole
//! function holds.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::expr::{Expr, Pattern};
use crate::stmt::Stmt;

/// The name a deferred `return`'s value is parked under.
///
/// Written so it cannot collide with anything a person types: `return f(page)`
/// with `defer free(page)` above it has to evaluate `f(page)` *before* the
/// release, or the value being returned is computed out of memory that has just
/// been given back. So the value is bound first, the releases run, and the
/// binding is what is returned.
const RETURN_SLOT: &str = "__lk_defer_return";

/// Rewrites a program so every `defer` runs on the way out of its function.
pub fn desugar_defers(statements: &mut Vec<Box<Stmt>>) -> Result<(), String> {
    rewrite_sequence(statements)?;
    for stmt in statements.iter_mut() {
        descend(stmt)?;
    }
    Ok(())
}

/// Walks into every nested function and rewrites its body too.
///
/// The walk is separate from the rewrite because a `defer` belongs to the
/// function it is written in: a function nested inside one that defers something
/// has its own way out, and running the outer function's releases when the inner
/// one returns would release things the outer one is still using.
fn descend(stmt: &mut Stmt) -> Result<(), String> {
    match stmt {
        Stmt::Attributed { item, .. } | Stmt::Defer { body: item, .. } => descend(item),
        Stmt::Function { body, .. } => {
            if let Stmt::Block { statements } = body.as_mut() {
                rewrite_sequence(statements)?;
            }
            descend(body)
        }
        Stmt::Block { statements } => {
            for inner in statements.iter_mut() {
                reject_stray(inner)?;
                descend(inner)?;
            }
            Ok(())
        }
        Stmt::If {
            then_stmt, else_stmt, ..
        } => {
            descend(then_stmt)?;
            match else_stmt {
                Some(other) => descend(other),
                None => Ok(()),
            }
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } => descend(body),
        Stmt::Impl { methods, .. } => {
            for method in methods.iter_mut() {
                descend(method)?;
            }
            Ok(())
        }
        // `try { … } catch e { … }` — an expression now, so it arrives wrapped.
        Stmt::Expr(expr) => match expr.as_mut() {
            Expr::Try { body, handler, .. } => {
                for inner in body.iter_mut().chain(handler.iter_mut()) {
                    reject_stray(inner)?;
                    descend(inner)?;
                }
                Ok(())
            }
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

/// A `defer` that survived the rewrite is one the rewrite could not reason
/// about, which is one in a branch, a loop or a nested block.
fn reject_stray(stmt: &Stmt) -> Result<(), String> {
    match stmt {
        Stmt::Defer { span, .. } => Err(defer_placement_error(span.as_ref())),
        _ => Ok(()),
    }
}

fn defer_placement_error(span: Option<&crate::token::Span>) -> String {
    let where_at = match span {
        Some(span) => format!(" at line {}", span.start.line),
        None => String::new(),
    };
    format!(
        "`defer`{where_at} must be at the top level of a function body — not inside an `if`, a \
         loop, or a nested block. It is a rewrite of the code's shape rather than a runtime \
         mechanism, and what makes that sound is that at the top level textual order is execution \
         order: every `defer` written above a `return` is exactly every `defer` that has run. A \
         resource taken inside a loop is released at the end of that iteration, which is an \
         ordinary pair of statements."
    )
}

/// The rewrite, on one function body.
///
/// Each `defer` is removed and its statement remembered. Every `return` gets the
/// statements written above it, in reverse; so does the end of the body. Reverse
/// because releases nest: the second thing taken is the first thing given back,
/// and a lock released before the thing it protects is a window.
#[allow(clippy::vec_box, reason = "the AST stores a block's statements as `Vec<Box<Stmt>>`")]
fn rewrite_sequence(body: &mut Vec<Box<Stmt>>) -> Result<(), String> {
    if !body.iter().any(|stmt| matches!(stmt.as_ref(), Stmt::Defer { .. })) {
        return Ok(());
    }

    let mut out: Vec<Box<Stmt>> = Vec::with_capacity(body.len());
    let mut pending: Vec<Box<Stmt>> = Vec::new();
    for stmt in body.drain(..) {
        match *stmt {
            Stmt::Defer { body, .. } => pending.push(body),
            other => out.push(Box::new(with_releases(other, &pending))),
        }
    }
    // The fall-off. A body whose last statement is already a `return` gets these
    // too — unreachable, harmless, and cheaper than proving it is.
    for release in pending.iter().rev() {
        out.push(release.clone());
    }
    *body = out;
    Ok(())
}

/// Puts `pending`'s releases in front of every `return` inside `stmt`.
///
/// Inside, not before: a `return` nested in an `if` or a loop is still a way out
/// of the function, and it is the one a leak hides behind.
fn with_releases(stmt: Stmt, pending: &[Box<Stmt>]) -> Stmt {
    if pending.is_empty() {
        return stmt;
    }
    match stmt {
        Stmt::Return { value } => {
            let mut block: Vec<Box<Stmt>> = Vec::with_capacity(pending.len() + 2);
            let returned = match value {
                // The value first, under a name, because it may read the very
                // thing about to be released.
                Some(expr) => {
                    block.push(Box::new(Stmt::Let {
                        pattern: Pattern::Variable(RETURN_SLOT.to_string()),
                        type_annotation: None,
                        value: expr,
                        span: None,
                        is_const: false,
                    }));
                    Some(Box::new(Expr::Var(RETURN_SLOT.to_string())))
                }
                None => None,
            };
            for release in pending.iter().rev() {
                block.push(release.clone());
            }
            block.push(Box::new(Stmt::Return { value: returned }));
            Stmt::Block { statements: block }
        }
        Stmt::Attributed { attributes, item } => Stmt::Attributed {
            attributes,
            item: Box::new(with_releases(*item, pending)),
        },
        Stmt::Block { statements } => Stmt::Block {
            statements: map_releases(statements, pending),
        },
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => Stmt::If {
            condition,
            then_stmt: Box::new(with_releases(*then_stmt, pending)),
            else_stmt: else_stmt.map(|other| Box::new(with_releases(*other, pending))),
        },
        Stmt::While { condition, body } => Stmt::While {
            condition,
            body: Box::new(with_releases(*body, pending)),
        },
        Stmt::For {
            pattern,
            iterable,
            body,
        } => Stmt::For {
            pattern,
            iterable,
            body: Box::new(with_releases(*body, pending)),
        },
        Stmt::Expr(expr) => match *expr {
            Expr::Try {
                body,
                catch_var,
                handler,
            } => Stmt::Expr(Box::new(Expr::Try {
                body: map_releases(body, pending),
                catch_var,
                handler: map_releases(handler, pending),
            })),
            other => Stmt::Expr(Box::new(other)),
        },
        // A nested function's `return` leaves *it*, not the enclosing function.
        other => other,
    }
}

#[allow(clippy::vec_box, reason = "the AST stores a block's statements as `Vec<Box<Stmt>>`")]
fn map_releases(stmts: Vec<Box<Stmt>>, pending: &[Box<Stmt>]) -> Vec<Box<Stmt>> {
    stmts
        .into_iter()
        .map(|stmt| Box::new(with_releases(*stmt, pending)))
        .collect()
}
