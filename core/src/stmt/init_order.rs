//! Reading a top-level binding before its initializer has run.
//!
//! The top level executes in order, so this answers `nil`:
//!
//! ```lk
//! fn f() -> Int { return LATER; }
//! println(f());        // nil — `f` runs before line 3 does
//! const LATER = 7;
//! ```
//!
//! `typeof(f())` said `Nil` for a function declared `-> Int`, and whatever
//! touched the nil next reported *its* own complaint ("Add expected numbers,
//! got Nil and Int", "`len()` works on a String, List, …") — never the ordering.
//! Python raises `NameError` here and JavaScript raises out of the temporal dead
//! zone; answering nil is the worst of the three, and this language has a
//! checker to say so before anything runs.
//!
//! The direct case was already refused (`const B = A + 4;` above `const A = 1;`
//! — see `TypeChecker::pending_top_level`) and reading a later binding from
//! inside a function body is *ordinary*, because bodies run after the whole top
//! level. This is the third case: a top-level statement **calls** a function
//! that reaches one.
//!
//! # What it will not catch
//!
//! Deliberately one-sided — it reports only what it can prove, so every
//! approximation here loses cases rather than inventing them:
//!
//! - **Shadowing is subtracted wholesale.** A body's reads are every
//!   `Expr::Var` in it minus every name it binds *anywhere*, at any depth. A
//!   function with a local `LATER` therefore reports no read of the global one
//!   — including in the parts where the local is not in scope.
//! - **Indirect calls are invisible.** Only a syntactic `f(…)` naming a
//!   top-level `fn` joins the call graph; a function reached through a value
//!   does not.
//! - **Closure and nested-`fn` bodies do not count as executed** at the point
//!   they are written, only where they are called — which is the invisible case
//!   above.
//! - **A branch that never runs still counts.** `if never { return LATER; }`
//!   inside a called function is reported. Moving the binding up is always
//!   available and always correct, so a false report costs one line.
//!
//! Both walks match their AST enums **exhaustively**, with no catch-all arm: a
//! new `Stmt` or `Expr` variant breaks the build here rather than silently
//! falling out of the analysis.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use crate::compat::collections::{HashMap, HashSet};
use crate::expr::{Expr, Pattern, TemplateStringPart};
use crate::stmt::{ForPattern, Program, Stmt};

/// What a subtree reads, binds and calls.
#[derive(Debug, Default)]
struct Facts {
    /// Every `Expr::Var` name, whether it names a local, a global or a function.
    reads: HashSet<String>,
    /// Every name bound anywhere inside, at any depth (see the module doc:
    /// subtracted wholesale).
    binds: HashSet<String>,
    /// Every syntactic `name(…)` callee.
    calls: HashSet<String>,
    /// Every method name a call spells, in walk order.
    ///
    /// A `Vec`, not a set: the compiler seeds a function's constant pool with
    /// these before lowering the body, and the pool's order is part of the
    /// artifact. Walk order is deterministic; a hash set's is not.
    methods: Vec<String>,
}

impl Facts {
    /// The reads that survive this subtree's own bindings.
    fn free_reads(&self) -> HashSet<String> {
        self.reads.difference(&self.binds).cloned().collect()
    }
}

/// Whether the walk is inside something that runs *now*.
///
/// A `fn` declaration and a closure literal are values at the point they are
/// written; their bodies run where they are called, which the caller side of
/// this analysis is what covers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// Descend into everything, including declarations — used for a function
    /// body, where all of it runs when the function is called.
    Everything,
    /// Skip the bodies of `fn` declarations and closure literals.
    ExecutedNow,
}

fn walk_stmt(stmt: &Stmt, depth: Depth, out: &mut Facts) {
    match stmt {
        Stmt::Attributed { attributes: _, item } => walk_stmt(item, depth, out),
        // An import binds names, and none of them is a top-level `let`.
        Stmt::Import(_) => {}
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            walk_expr(condition, depth, out);
            walk_stmt(then_stmt, depth, out);
            if let Some(else_stmt) = else_stmt {
                walk_stmt(else_stmt, depth, out);
            }
        }
        Stmt::IfLet {
            pattern,
            value,
            then_stmt,
            else_stmt,
        } => {
            collect_pattern(pattern, out);
            walk_expr(value, depth, out);
            walk_stmt(then_stmt, depth, out);
            if let Some(else_stmt) = else_stmt {
                walk_stmt(else_stmt, depth, out);
            }
        }
        Stmt::While { condition, body } => {
            walk_expr(condition, depth, out);
            walk_stmt(body, depth, out);
        }
        Stmt::WhileLet { pattern, value, body } => {
            collect_pattern(pattern, out);
            walk_expr(value, depth, out);
            walk_stmt(body, depth, out);
        }
        Stmt::For {
            pattern,
            iterable,
            body,
        } => {
            collect_for_pattern(pattern, out);
            walk_expr(iterable, depth, out);
            walk_stmt(body, depth, out);
        }
        Stmt::Let { pattern, value, .. } => {
            collect_pattern(pattern, out);
            walk_expr(value, depth, out);
        }
        // The target of an assignment is not a *read* of it, but it is also not
        // a binding: `x = 1` at the top level writes a binding declared
        // elsewhere. Neither set gets it.
        Stmt::Assign { name: _, value, .. } => walk_expr(value, depth, out),
        Stmt::CompoundAssign { name, value, .. } => {
            // `x += 1` reads `x` first.
            out.reads.insert(name.clone());
            walk_expr(value, depth, out);
        }
        Stmt::Define { name, value, .. } => {
            out.binds.insert(name.clone());
            walk_expr(value, depth, out);
        }
        Stmt::Defer { body, .. } => walk_stmt(body, depth, out),
        Stmt::Break | Stmt::Continue | Stmt::Empty => {}
        Stmt::Return { value } => {
            if let Some(value) = value {
                walk_expr(value, depth, out);
            }
        }
        // Declarations bind a *type* name, not a value binding, and hold no
        // expression that runs here.
        Stmt::Struct { .. } | Stmt::TypeAlias { .. } | Stmt::Trait { .. } => {}
        Stmt::Function {
            name,
            params,
            named_params,
            body,
            ..
        } => {
            out.binds.insert(name.clone());
            for param in params {
                out.binds.insert(param.clone());
            }
            for param in named_params {
                out.binds.insert(param.name.clone());
                if let Some(default) = &param.default {
                    walk_expr(default, depth, out);
                }
            }
            if depth == Depth::Everything {
                walk_stmt(body, depth, out);
            }
        }
        // An impl block's methods run when dispatched, like a `fn`.
        Stmt::Impl { methods, .. } => {
            if depth == Depth::Everything {
                for method in methods {
                    walk_stmt(method, depth, out);
                }
            }
        }
        Stmt::Expr { value, .. } => walk_expr(value, depth, out),
        Stmt::Block { statements } => {
            for stmt in statements {
                walk_stmt(stmt, depth, out);
            }
        }
    }
}

fn walk_expr(expr: &Expr, depth: Depth, out: &mut Facts) {
    match expr {
        Expr::Var(name) => {
            out.reads.insert(name.clone());
        }
        Expr::Literal(_) => {}
        Expr::Call(name, args) => {
            out.calls.insert(name.clone());
            for arg in args {
                walk_expr(arg, depth, out);
            }
        }
        Expr::CallExpr(callee, args) => {
            walk_callee(callee, depth, out);
            for arg in args {
                walk_expr(arg, depth, out);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            walk_callee(callee, depth, out);
            for arg in positional {
                walk_expr(arg, depth, out);
            }
            for (_, arg) in named {
                walk_expr(arg, depth, out);
            }
        }
        Expr::Bin(left, _, right)
        | Expr::And(left, right)
        | Expr::Or(left, right)
        | Expr::NullishCoalescing(left, right) => {
            walk_expr(left, depth, out);
            walk_expr(right, depth, out);
        }
        // A field name is an expression node, and in `a.b` the `b` is a `Var`
        // that names nothing — walking it would report a read of a top-level
        // binding that happens to share the field's name.
        Expr::Access(target, field) | Expr::OptionalAccess(target, field) => {
            walk_expr(target, depth, out);
            if !matches!(**field, Expr::Var(_)) {
                walk_expr(field, depth, out);
            }
        }
        Expr::Unary(_, inner) | Expr::Paren(inner) | Expr::Unsafe(inner) | Expr::Cast(inner, _) => {
            walk_expr(inner, depth, out)
        }
        Expr::Conditional(cond, then_expr, else_expr) => {
            walk_expr(cond, depth, out);
            walk_expr(then_expr, depth, out);
            walk_expr(else_expr, depth, out);
        }
        Expr::List(items) => {
            for item in items {
                walk_expr(item, depth, out);
            }
        }
        Expr::Map(pairs) => {
            for (key, value) in pairs {
                walk_expr(key, depth, out);
                walk_expr(value, depth, out);
            }
        }
        Expr::StructLiteral { name: _, fields } => {
            for (_, value) in fields {
                walk_expr(value, depth, out);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for part in [start, end, step].iter().copied().flatten() {
                walk_expr(part, depth, out);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                match part {
                    TemplateStringPart::Literal(_) => {}
                    TemplateStringPart::Expr(expr) => walk_expr(expr, depth, out),
                }
            }
        }
        Expr::Closure { params, body, .. } => {
            for param in params {
                out.binds.insert(param.clone());
            }
            if depth == Depth::Everything {
                walk_expr(body, depth, out);
            }
        }
        Expr::Match { value, arms } => {
            walk_expr(value, depth, out);
            for arm in arms {
                collect_pattern(&arm.pattern, out);
                walk_expr(&arm.body, depth, out);
            }
        }
        Expr::Block(statements) => {
            for stmt in statements {
                walk_stmt(stmt, depth, out);
            }
        }
        Expr::Try {
            body,
            catch_var,
            handler,
        } => {
            out.binds.insert(catch_var.clone());
            for stmt in body {
                walk_stmt(stmt, depth, out);
            }
            for stmt in handler {
                walk_stmt(stmt, depth, out);
            }
        }
    }
}

/// The callee position of a call.
///
/// `f(x)` reaches the parser as `CallExpr(Var("f"), …)`, not as `Call("f", …)`
/// — that spelling is for a call the parser could resolve to a name directly.
/// Walking the callee as an ordinary expression made every call a *read* of its
/// own name and recorded no call at all, which is why the whole analysis
/// answered nothing on `println(f())`.
fn walk_callee(callee: &Expr, depth: Depth, out: &mut Facts) {
    if let Expr::Var(name) = callee {
        out.calls.insert(name.clone());
        return;
    }
    // `a.m(…)` is `CallExpr(Access(a, m), …)`, and `m` is a string literal —
    // that is what a member is. A bare `Var` is a bracket index (`a[m](…)`),
    // which calls whatever the element holds and names no method.
    if let Expr::Access(target, field) | Expr::OptionalAccess(target, field) = callee {
        let name = match &**field {
            Expr::Literal(value) => value.as_str().map(alloc::string::ToString::to_string),
            _ => None,
        };
        if let Some(name) = name {
            if !out.methods.contains(&name) {
                out.methods.push(name);
            }
            walk_expr(target, depth, out);
            return;
        }
    }
    walk_expr(callee, depth, out);
}

/// Every method name a body's calls spell, in source order and deduplicated.
///
/// The compiler seeds a function's constant pool with these before lowering it.
/// `CallMethodK` carries the name's constant index in **8 bits** (the `abc`
/// form is full: 7 opcode + 8 A + 1 K + 8 B + 8 C), so a name landing past 255
/// falls back to a `__lk_call_method` helper call — which the native backend
/// cannot lower, taking the whole program with it.
///
/// Measured before the seeding: 130 structs each with one method, called once
/// each from `main`, stopped lowering at the 129th — the struct names and field
/// names of the literals share the same per-function pool and pushed the method
/// names past the byte. Seeding first makes the bound what it reads like: 256
/// distinct method names called from one function.
pub(crate) fn method_names_called(body: &Stmt) -> Vec<String> {
    let mut facts = Facts::default();
    walk_stmt(body, Depth::Everything, &mut facts);
    facts.methods
}

/// The same, for the top level — whose statements are the entry function's
/// body and are not wrapped in a `Stmt`.
///
/// `Depth::ExecutedNow` so a `fn`'s own body is left to its own seeding: those
/// names belong in *that* function's pool, and crowding the entry's pool with
/// them is what this whole seeding is avoiding.
pub(crate) fn method_names_called_at_top_level(program: &Program) -> Vec<String> {
    let mut facts = Facts::default();
    for stmt in &program.statements {
        walk_stmt(stmt, Depth::ExecutedNow, &mut facts);
    }
    facts.methods
}

fn collect_for_pattern(pattern: &ForPattern, out: &mut Facts) {
    match pattern {
        ForPattern::Variable(name) => {
            out.binds.insert(name.clone());
        }
        ForPattern::Ignore => {}
        ForPattern::Tuple(patterns) => {
            for pattern in patterns {
                collect_for_pattern(pattern, out);
            }
        }
        ForPattern::Array { patterns, rest } => {
            for pattern in patterns {
                collect_for_pattern(pattern, out);
            }
            if let Some(rest) = rest {
                out.binds.insert(rest.clone());
            }
        }
        ForPattern::Object(entries) => {
            for (_, pattern) in entries {
                collect_for_pattern(pattern, out);
            }
        }
    }
}

fn collect_pattern(pattern: &Pattern, out: &mut Facts) {
    match pattern {
        Pattern::Variable(name) => {
            out.binds.insert(name.clone());
        }
        Pattern::Wildcard | Pattern::Literal(_) => {}
        Pattern::List { patterns, rest } => {
            for pattern in patterns {
                collect_pattern(pattern, out);
            }
            if let Some(rest) = rest {
                out.binds.insert(rest.clone());
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, pattern) in patterns {
                collect_pattern(pattern, out);
            }
            if let Some(rest) = rest {
                out.binds.insert(rest.clone());
            }
        }
        Pattern::Or(patterns) => {
            for pattern in patterns {
                collect_pattern(pattern, out);
            }
        }
        Pattern::Guard { pattern, guard } => {
            collect_pattern(pattern, out);
            walk_expr(guard, Depth::ExecutedNow, out);
        }
        Pattern::Range { start, end, .. } => {
            walk_expr(start, Depth::ExecutedNow, out);
            walk_expr(end, Depth::ExecutedNow, out);
        }
    }
}

/// Per top-level `fn` name: the names its body reads, closed over the functions
/// it calls.
pub(crate) struct InitOrder {
    reads_of: HashMap<String, HashSet<String>>,
}

fn unwrap_attributes(stmt: &Stmt) -> &Stmt {
    match stmt {
        Stmt::Attributed { item, .. } => unwrap_attributes(item),
        other => other,
    }
}

impl InitOrder {
    pub(crate) fn of(program: &Program) -> Self {
        let mut direct: HashMap<String, (HashSet<String>, HashSet<String>)> = HashMap::new();
        for stmt in &program.statements {
            let Stmt::Function { name, body, .. } = unwrap_attributes(stmt) else {
                continue;
            };
            let mut facts = Facts::default();
            walk_stmt(body, Depth::Everything, &mut facts);
            // The function's own parameters are bound by the declaration, not
            // by the body, so they are collected here rather than by the walk.
            if let Stmt::Function {
                params, named_params, ..
            } = unwrap_attributes(stmt)
            {
                for param in params {
                    facts.binds.insert(param.clone());
                }
                for param in named_params {
                    facts.binds.insert(param.name.clone());
                }
            }
            direct.insert(name.clone(), (facts.free_reads(), facts.calls));
        }

        // Transitive closure over the call graph: an explicit work stack, not
        // recursion. O(V + E) amortized — the fixpoint loop this replaced was
        // O(rounds × functions), where "rounds" is the depth of the call chain,
        // so a 4000-`fn` program with a deep chain ran it 4000 times.
        //
        // Iterative for the reason `HeapStore::collect` is: the depth here is
        // the program's call depth, which a generated file can make as large as
        // it likes, and putting it on the Rust stack turns that into a process
        // abort with no line to blame.
        //
        // A cycle contributes nothing on its back edge (`in_progress` below),
        // so mutually recursive functions can lose a read the other one has.
        // That is the same one-sided trade as the rest of this module — see the
        // header — and it is the only place the answer depends on where the
        // walk entered.
        let mut reads_of: HashMap<String, HashSet<String>> = HashMap::new();
        let mut roots: Vec<&String> = direct.keys().collect();
        roots.sort();
        let mut in_progress: HashSet<String> = HashSet::new();
        for root in roots {
            let mut work: Vec<(String, bool)> = alloc::vec![(root.clone(), false)];
            while let Some((name, expanded)) = work.pop() {
                if reads_of.contains_key(&name) {
                    continue;
                }
                let Some((own_reads, calls)) = direct.get(&name) else {
                    continue;
                };
                if expanded {
                    let mut reads = own_reads.clone();
                    for callee in calls {
                        if let Some(callee_reads) = reads_of.get(callee) {
                            reads.extend(callee_reads.iter().cloned());
                        }
                    }
                    in_progress.remove(&name);
                    reads_of.insert(name, reads);
                    continue;
                }
                if !in_progress.insert(name.clone()) {
                    continue;
                }
                work.push((name, true));
                for callee in calls {
                    if !reads_of.contains_key(callee) && !in_progress.contains(callee) {
                        work.push((callee.clone(), false));
                    }
                }
            }
            in_progress.clear();
        }

        Self { reads_of }
    }

    /// The binding a top-level statement would read before it is initialized,
    /// and the function that reaches it — or `None` when nothing does.
    ///
    /// `pending` is the set of top-level bindings whose `let`/`const` has not
    /// been reached yet.
    pub(crate) fn premature_read(&self, stmt: &Stmt, pending: &HashSet<String>) -> Option<(String, String)> {
        if pending.is_empty() {
            return None;
        }
        let mut facts = Facts::default();
        walk_stmt(stmt, Depth::ExecutedNow, &mut facts);
        // Sorted so the message does not depend on hash order.
        let mut callees: Vec<&String> = facts.calls.iter().collect();
        callees.sort();
        for callee in callees {
            let Some(reads) = self.reads_of.get(callee) else {
                continue;
            };
            let mut hits: Vec<&String> = reads.intersection(pending).collect();
            hits.sort();
            if let Some(name) = hits.first() {
                return Some(((*name).clone(), callee.clone()));
            }
        }
        None
    }
}
