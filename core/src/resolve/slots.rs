//! Slot-based name resolution scaffold.
//!
//! This module provides a lightweight, feature-gated name-to-slot index
//! resolver that walks the Stmt/Expr AST and assigns per-function local
//! slot indices for variable bindings, approximating the runtime VmContext
//! behavior. It also records variable use sites with their resolved
//! VarSlot (depth, index) where possible. The output is an analysis-only
//! structure and does not mutate the AST.

use std::collections::HashMap;

use crate::{
    expr::{Expr, MatchArm, Pattern, SelectCase, SelectPattern, TemplateStringPart},
    stmt::{ForPattern, Program, Stmt},
    token::Span,
};

/// Resolved slot for a variable reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarSlot {
    /// Function frame depth: 0 = current function, 1 = captured from parent, ...
    pub depth: u16,
    /// Local index within the function frame (params and locals share a flat index space)
    pub index: u16,
}

/// A single declared binding within a function's frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    /// Variable name
    pub name: String,
    /// Assigned local slot index (unique in this function)
    pub index: u16,
    /// Whether this declaration is a parameter
    pub is_param: bool,
    /// Block nesting depth at declaration time (0 = function level)
    pub block_depth: u16,
    /// Optional source span of the declared identifier (if known)
    pub span: Option<Span>,
}

/// A variable use site resolved to a slot.
#[derive(Debug, Clone, PartialEq)]
pub struct VarUse {
    pub name: String,
    pub slot: VarSlot,
    /// Optional source span for this use-site (if known)
    pub span: Option<Span>,
}

/// Per-function slot layout and nested functions.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionLayout {
    /// Sequential declaration log (params first, then locals in allocation order)
    pub decls: Vec<Decl>,
    /// Total number of local slots allocated in this function (params + locals)
    pub total_locals: u16,
    /// All resolved variable use sites inside this function (excludes nested children)
    pub uses: Vec<VarUse>,
    /// Nested function/closure layouts
    pub children: Vec<FunctionLayout>,
}

/// Top-level resolution result for a program (treated as an implicit function frame).
#[derive(Debug, Clone, PartialEq)]
pub struct SlotResolution {
    pub root: FunctionLayout,
}

#[derive(Debug, Default)]
pub struct SlotResolver;

impl SlotResolver {
    pub fn new() -> Self {
        Self
    }

    /// Resolve slots for the entire program, returning an analysis-only layout tree.
    pub fn resolve_program_slots(&mut self, prog: &Program) -> SlotResolution {
        let mut core = ResolverCore::new();
        let root = core.resolve_program(prog);
        SlotResolution { root }
    }

    pub fn resolve_function_slots(
        &mut self,
        params: &[String],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
    ) -> FunctionLayout {
        let mut core = ResolverCore::new();
        core.resolve_function(params, named_params, body)
    }
}

// ---------------- Internal resolver implementation ----------------

#[derive(Debug)]
struct FnCtx {
    next_index: u16,
    /// Stack of block scopes for name -> local index mapping (for shadowing)
    scopes: Vec<HashMap<String, u16>>,
    /// Recorded declarations for this function
    decls: Vec<Decl>,
    /// Recorded var uses for this function
    uses: Vec<VarUse>,
    /// Nested children produced inside this function (Stmt::Function or closures)
    children: Vec<FunctionLayout>,
}

impl FnCtx {
    fn new() -> Self {
        Self {
            next_index: 0,
            scopes: vec![HashMap::new()],
            decls: Vec::new(),
            uses: Vec::new(),
            children: Vec::new(),
        }
    }

    fn block_depth(&self) -> u16 {
        // At least one scope exists
        (self.scopes.len() - 1) as u16
    }

    fn push_block(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_block(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Allocate a new local slot for name in the top-most block scope.
    fn define(&mut self, name: String, is_param: bool) -> u16 {
        let idx = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name.clone(), idx);
        }
        self.decls.push(Decl {
            name,
            index: idx,
            is_param,
            block_depth: self.block_depth(),
            span: None,
        });
        idx
    }

    /// Resolve a name in this function's block scopes, from innermost to outermost.
    fn resolve_local(&self, name: &str) -> Option<u16> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name).copied())
    }
}

#[derive(Debug)]
struct ResolverCore {
    /// Function context stack; index 0 is root program frame
    fn_stack: Vec<FnCtx>,
}

impl ResolverCore {
    fn new() -> Self {
        Self { fn_stack: Vec::new() }
    }

    fn current_fn(&mut self) -> &mut FnCtx {
        self.fn_stack
            .last_mut()
            .expect("resolver bug: fn_stack should be non-empty")
    }

    fn with_new_function<F>(&mut self, f: F) -> FunctionLayout
    where
        F: FnOnce(&mut Self),
    {
        self.fn_stack.push(FnCtx::new());
        f(self);
        let ctx = self.fn_stack.pop().expect("fn_stack underflow");
        FunctionLayout {
            decls: ctx.decls,
            total_locals: ctx.next_index,
            uses: ctx.uses,
            children: ctx.children,
        }
    }

    fn resolve_program(&mut self, prog: &Program) -> FunctionLayout {
        // Treat the whole program as a function frame
        self.fn_stack.push(FnCtx::new());
        let mut children: Vec<FunctionLayout> = Vec::new();
        for stmt in &prog.statements {
            self.resolve_stmt(stmt, &mut children);
        }
        let ctx = self.fn_stack.pop().expect("fn_stack underflow (program)");
        FunctionLayout {
            decls: ctx.decls,
            total_locals: ctx.next_index,
            uses: ctx.uses,
            children: {
                // Merge closures/functions found into context + those passed-in (Stmt::Function).
                // Stmt::Function layouts were appended to `children` directly; closures were
                // collected in ctx.children. Combine them for the root.
                let mut all = ctx.children;
                all.extend(children);
                all
            },
        }
    }

    fn resolve_function(
        &mut self,
        params: &[String],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
    ) -> FunctionLayout {
        self.with_new_function(|this| {
            for p in params {
                this.current_fn().define(p.clone(), true);
            }
            for decl in named_params {
                this.current_fn().define(decl.name.clone(), true);
                if let Some(default) = &decl.default {
                    this.resolve_expr(default);
                }
            }
            let mut direct_children = Vec::new();
            this.resolve_stmt(body, &mut direct_children);
            this.current_fn().children.extend(direct_children);
        })
    }

    fn resolve_stmt(&mut self, stmt: &Stmt, children_out: &mut Vec<FunctionLayout>) {
        match stmt {
            Stmt::Import(_) => {
                // No local bindings
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                self.resolve_expr(condition);
                self.current_fn().push_block();
                self.resolve_stmt(then_stmt, children_out);
                self.current_fn().pop_block();
                if let Some(es) = else_stmt {
                    self.current_fn().push_block();
                    self.resolve_stmt(es, children_out);
                    self.current_fn().pop_block();
                }
            }
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => {
                self.resolve_expr(value);
                // then branch introduces pattern bindings
                self.current_fn().push_block();
                self.define_pattern(pattern, /*is_param=*/ false);
                self.resolve_stmt(then_stmt, children_out);
                self.current_fn().pop_block();
                if let Some(es) = else_stmt {
                    self.current_fn().push_block();
                    self.resolve_stmt(es, children_out);
                    self.current_fn().pop_block();
                }
            }
            Stmt::While { condition, body } => {
                self.resolve_expr(condition);
                self.current_fn().push_block();
                self.resolve_stmt(body, children_out);
                self.current_fn().pop_block();
            }
            Stmt::WhileLet { pattern, value, body } => {
                self.resolve_expr(value);
                self.current_fn().push_block();
                self.define_pattern(pattern, /*is_param=*/ false);
                self.resolve_stmt(body, children_out);
                self.current_fn().pop_block();
            }
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                self.resolve_expr(iterable);
                self.current_fn().push_block();
                self.define_for_pattern(pattern);
                self.resolve_stmt(body, children_out);
                self.current_fn().pop_block();
            }
            Stmt::Let { pattern, value, .. } => {
                self.resolve_expr(value);
                self.define_pattern(pattern, /*is_param=*/ false);
            }
            Stmt::Assign { value, .. } => {
                self.resolve_expr(value);
                // LHS is an existing name; usage may be recorded optionally
            }
            Stmt::CompoundAssign { value, .. } => {
                self.resolve_expr(value);
            }
            Stmt::Define { name, value } => {
                self.resolve_expr(value);
                let name = name.clone();
                self.current_fn().define(name, false);
            }
            Stmt::Break | Stmt::Continue => {}
            Stmt::Return { value } => {
                if let Some(v) = value {
                    self.resolve_expr(v);
                }
            }
            Stmt::Function { name, params, body, .. } => {
                // Define function name in current frame
                self.current_fn().define(name.clone(), false);

                // Build child function layout: params + body
                let child_layout = self.with_new_function(|this| {
                    // Define params as the first locals
                    for p in params {
                        this.current_fn().define(p.clone(), true);
                    }
                    // Body executes within its own block nesting
                    this.resolve_stmt(body, &mut Vec::new());
                });

                children_out.push(child_layout);
            }
            Stmt::Expr(expr) => {
                self.resolve_expr(expr);
            }
            Stmt::Struct { .. } => {
                // Type declaration, no expression resolution needed
            }
            Stmt::TypeAlias { .. } => {
                // Type alias declarations do not introduce runtime bindings
            }
            Stmt::Trait { .. } => {
                // Trait declaration contains no executable code
            }
            Stmt::Impl { methods, .. } => {
                // Methods are functions; register child layouts
                for m in methods {
                    if let Stmt::Function { name, params, body, .. } = m {
                        // Define method symbol in current frame (not strictly necessary for impl)
                        self.current_fn().define(name.clone(), false);
                        let child_layout = self.with_new_function(|this| {
                            for p in params {
                                this.current_fn().define(p.clone(), true);
                            }
                            this.resolve_stmt(body, &mut Vec::new());
                        });
                        children_out.push(child_layout);
                    }
                }
            }
            Stmt::Block { statements } => {
                self.current_fn().push_block();
                for s in statements {
                    self.resolve_stmt(s, children_out);
                }
                self.current_fn().pop_block();
            }
            Stmt::Empty => {}
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Val(_) => {}
            Expr::StructLiteral { fields, .. } => {
                for (_k, v) in fields {
                    self.resolve_expr(v);
                }
            }
            Expr::Var(name) => {
                if let Some(slot) = self.lookup_var(name) {
                    self.current_fn().uses.push(VarUse {
                        name: name.clone(),
                        slot,
                        span: None,
                    });
                }
            }
            Expr::Bin(l, _, r) => {
                self.resolve_expr(l);
                self.resolve_expr(r);
            }
            Expr::Unary(_, e) => self.resolve_expr(e),
            Expr::Conditional(c, t, e) => {
                self.resolve_expr(c);
                self.resolve_expr(t);
                self.resolve_expr(e);
            }
            Expr::And(l, r) | Expr::Or(l, r) | Expr::NullishCoalescing(l, r) => {
                self.resolve_expr(l);
                self.resolve_expr(r);
            }
            Expr::Access(l, r) | Expr::OptionalAccess(l, r) => {
                self.resolve_expr(l);
                self.resolve_expr(r);
            }
            Expr::Paren(e) => self.resolve_expr(e),
            Expr::List(items) => {
                for e in items {
                    self.resolve_expr(e);
                }
            }
            Expr::Map(pairs) => {
                for (k, v) in pairs {
                    self.resolve_expr(k);
                    self.resolve_expr(v);
                }
            }
            Expr::Call(name, args) => {
                // Function name call (rare path in current parser); treat name as a var use
                if let Some(slot) = self.lookup_var(name) {
                    self.current_fn().uses.push(VarUse {
                        name: name.clone(),
                        slot,
                        span: None,
                    });
                }
                for a in args {
                    self.resolve_expr(a);
                }
            }
            Expr::CallExpr(callee, args) => {
                self.resolve_expr(callee);
                for a in args {
                    self.resolve_expr(a);
                }
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                self.resolve_expr(callee);
                for a in pos_args {
                    self.resolve_expr(a);
                }
                for (_n, e) in named_args {
                    self.resolve_expr(e);
                }
            }
            Expr::Range { start, end, step, .. } => {
                if let Some(s) = start {
                    self.resolve_expr(s);
                }
                if let Some(e) = end {
                    self.resolve_expr(e);
                }
                if let Some(st) = step {
                    self.resolve_expr(st);
                }
            }
            Expr::Select { cases, default_case } => {
                for SelectCase { pattern, guard, body } in cases {
                    // Each case executes in its own block; optional binding
                    self.current_fn().push_block();
                    if let SelectPattern::Recv { binding, channel } = pattern {
                        self.resolve_expr(channel);
                        if let Some(b) = binding {
                            let _ = self.current_fn().define(b.clone(), false);
                        }
                    }
                    if let Some(g) = guard {
                        self.resolve_expr(g);
                    }
                    self.resolve_expr(body);
                    self.current_fn().pop_block();
                }
                if let Some(def) = default_case {
                    self.current_fn().push_block();
                    self.resolve_expr(def);
                    self.current_fn().pop_block();
                }
            }
            Expr::TemplateString(parts) => {
                for p in parts {
                    if let TemplateStringPart::Expr(e) = p {
                        self.resolve_expr(e);
                    }
                }
            }
            Expr::Closure { params, body } => {
                // Nested anonymous function
                let child = self.with_new_function(|this| {
                    for p in params {
                        this.current_fn().define(p.clone(), true);
                    }
                    this.resolve_stmt(&Stmt::Expr(body.clone()), &mut Vec::new());
                });
                // Attach as an anonymous child of the current function
                if let Some(parent_ctx) = self.fn_stack.last_mut() {
                    parent_ctx.children.push(child);
                }
            }
            Expr::Match { value, arms } => {
                self.resolve_expr(value);
                for MatchArm { pattern, body } in arms {
                    self.current_fn().push_block();
                    self.define_pattern(pattern, /*is_param=*/ false);
                    self.resolve_expr(body);
                    self.current_fn().pop_block();
                }
            }
        }
    }

    fn lookup_var(&self, name: &str) -> Option<VarSlot> {
        let mut depth: u16 = 0;
        for fn_ctx in self.fn_stack.iter().rev() {
            if let Some(idx) = fn_ctx.resolve_local(name) {
                return Some(VarSlot { depth, index: idx });
            }
            depth = depth.saturating_add(1);
        }
        None
    }

    fn define_pattern(&mut self, pat: &Pattern, is_param: bool) {
        match pat {
            Pattern::Literal(_) => {}
            Pattern::Variable(name) => {
                let name = name.clone();
                self.current_fn().define(name, is_param);
            }
            Pattern::Wildcard => {}
            Pattern::List { patterns, rest } => {
                for p in patterns {
                    self.define_pattern(p, is_param);
                }
                if let Some(r) = rest {
                    self.current_fn().define(r.clone(), is_param);
                }
            }
            Pattern::Map { patterns, rest } => {
                for (_k, p) in patterns {
                    self.define_pattern(p, is_param);
                }
                if let Some(r) = rest {
                    self.current_fn().define(r.clone(), is_param);
                }
            }
            Pattern::Or(alts) => {
                // Conservatively add all variables appearing in any alternative
                for p in alts {
                    self.define_pattern(p, is_param);
                }
            }
            Pattern::Guard { pattern, .. } => {
                self.define_pattern(pattern, is_param);
            }
            Pattern::Range { .. } => {}
        }
    }

    fn define_for_pattern(&mut self, pat: &ForPattern) {
        match pat {
            ForPattern::Variable(name) => {
                self.current_fn().define(name.clone(), false);
            }
            ForPattern::Ignore => {}
            ForPattern::Tuple(items) => {
                for p in items {
                    self.define_for_pattern(p);
                }
            }
            ForPattern::Array { patterns, rest } => {
                for p in patterns {
                    self.define_for_pattern(p);
                }
                if let Some(r) = rest {
                    self.current_fn().define(r.clone(), false);
                }
            }
            ForPattern::Object(entries) => {
                for (_k, v) in entries {
                    self.define_for_pattern(v);
                }
            }
        }
    }
}
