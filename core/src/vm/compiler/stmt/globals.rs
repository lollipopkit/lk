use crate::{
    expr::Expr,
    stmt::Stmt,
    val::Val,
    vm::{CaptureSpec, compiler::FunctionBuilder},
};

pub(super) fn detect_mutating_receiver(expr: &Expr) -> Option<&str> {
    if let Expr::CallExpr(callee, _args) = expr
        && let Expr::Access(obj, field) = callee.as_ref()
        && let Expr::Var(var_name) = obj.as_ref()
        && let Expr::Val(method_val) = field.as_ref()
        && method_val.as_str() == Some("push")
    {
        return Some(var_name);
    }
    None
}

impl FunctionBuilder {
    pub(crate) fn should_export_global_write(&self, name: &str) -> bool {
        self.export_toplevel_globals
            && self.global_defs.contains(name)
            && self.loop_depth == 0
            && self.var_scope_depth() == 0
    }

    pub(crate) fn flush_loop_global_writes(&mut self, body: &Stmt) {
        let names = self.loop_global_write_names(body);
        if self.loop_depth > 1 {
            self.pending_loop_global_writes.extend(names);
            return;
        }
        self.flush_global_write_names(names);
    }

    pub(crate) fn flush_pending_loop_global_writes(&mut self) {
        if self.pending_loop_global_writes.is_empty() {
            return;
        }
        let names = std::mem::take(&mut self.pending_loop_global_writes);
        self.flush_global_write_names(names);
    }

    fn loop_global_write_names(&self, body: &Stmt) -> std::collections::HashSet<String> {
        let mut assigned = std::collections::HashSet::new();
        Self::collect_stmt_assigned_names(body, &mut assigned);
        assigned
            .into_iter()
            .filter(|name| self.global_defs.contains(name))
            .collect()
    }

    fn flush_global_write_names(&mut self, names: std::collections::HashSet<String>) {
        let mut names: Vec<_> = names
            .into_iter()
            .chain(std::mem::take(&mut self.pending_loop_global_writes))
            .collect();
        names.sort();
        names.dedup();
        for name in names {
            if let Some(idx) = self.lookup(&name) {
                let kname = self.k(Val::from_str(name.as_str()));
                self.emit(crate::vm::Op::DefineGlobal(kname, idx));
            }
        }
    }

    pub(crate) fn called_closure_global_captures(&self, expr: &Expr) -> Vec<String> {
        let name = match expr {
            Expr::Call(name, _) => name.as_str(),
            Expr::CallExpr(callee, _) => {
                let Expr::Var(name) = callee.as_ref() else {
                    return Vec::new();
                };
                name.as_str()
            }
            _ => return Vec::new(),
        };
        let Some(Val::Closure(closure)) = self.const_env.get(name) else {
            return if self.lookup(name).is_some() {
                self.global_defs.iter().cloned().collect()
            } else {
                Vec::new()
            };
        };
        let mut names: Vec<_> = closure
            .capture_specs
            .iter()
            .filter_map(|spec| match spec {
                CaptureSpec::Global { name } | CaptureSpec::Register { name, .. }
                    if self.global_defs.contains(name) =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect();
        collect_global_writes(&closure.body, &self.global_defs, &mut names);
        names.sort();
        names.dedup();
        names
    }

    pub(crate) fn expr_calls_preserve_binding(&self, expr: &Expr, target: &str) -> bool {
        self.expr_calls_preserve_binding_inner(expr, target, &mut Vec::new())
    }

    fn expr_calls_preserve_binding_inner(&self, expr: &Expr, target: &str, call_stack: &mut Vec<String>) -> bool {
        match expr {
            Expr::Call(name, args) => {
                self.known_call_preserves_binding(name, target, call_stack)
                    && args
                        .iter()
                        .all(|arg| self.expr_calls_preserve_binding_inner(arg, target, call_stack))
            }
            Expr::CallExpr(callee, args) => {
                let callee_preserves = match callee.as_ref() {
                    Expr::Var(name) => self.known_call_preserves_binding(name, target, call_stack),
                    _ => self.expr_calls_preserve_binding_inner(callee, target, call_stack),
                };
                callee_preserves
                    && args
                        .iter()
                        .all(|arg| self.expr_calls_preserve_binding_inner(arg, target, call_stack))
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                self.expr_calls_preserve_binding_inner(callee, target, call_stack)
                    && pos_args
                        .iter()
                        .all(|arg| self.expr_calls_preserve_binding_inner(arg, target, call_stack))
                    && named_args
                        .iter()
                        .all(|(_, arg)| self.expr_calls_preserve_binding_inner(arg, target, call_stack))
            }
            Expr::Paren(inner) | Expr::Unary(_, inner) => {
                self.expr_calls_preserve_binding_inner(inner, target, call_stack)
            }
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right) => {
                self.expr_calls_preserve_binding_inner(left, target, call_stack)
                    && self.expr_calls_preserve_binding_inner(right, target, call_stack)
            }
            Expr::Access(obj, field) | Expr::OptionalAccess(obj, field) => {
                self.expr_calls_preserve_binding_inner(obj, target, call_stack)
                    && self.expr_calls_preserve_binding_inner(field, target, call_stack)
            }
            Expr::List(items) => items
                .iter()
                .all(|item| self.expr_calls_preserve_binding_inner(item, target, call_stack)),
            Expr::Map(items) => items.iter().all(|(key, value)| {
                self.expr_calls_preserve_binding_inner(key, target, call_stack)
                    && self.expr_calls_preserve_binding_inner(value, target, call_stack)
            }),
            Expr::Conditional(condition, then_expr, else_expr) => {
                self.expr_calls_preserve_binding_inner(condition, target, call_stack)
                    && self.expr_calls_preserve_binding_inner(then_expr, target, call_stack)
                    && self.expr_calls_preserve_binding_inner(else_expr, target, call_stack)
            }
            Expr::Range { start, end, step, .. } => {
                start
                    .as_ref()
                    .is_none_or(|expr| self.expr_calls_preserve_binding_inner(expr, target, call_stack))
                    && end
                        .as_ref()
                        .is_none_or(|expr| self.expr_calls_preserve_binding_inner(expr, target, call_stack))
                    && step
                        .as_ref()
                        .is_none_or(|expr| self.expr_calls_preserve_binding_inner(expr, target, call_stack))
            }
            Expr::Val(_) | Expr::Var(_) => true,
            Expr::Closure { .. }
            | Expr::Block(_)
            | Expr::Select { .. }
            | Expr::StructLiteral { .. }
            | Expr::Match { .. }
            | Expr::TemplateString(_) => false,
        }
    }

    fn known_call_preserves_binding(&self, name: &str, target: &str, call_stack: &mut Vec<String>) -> bool {
        let Some(callee) = self.const_env.get(name) else {
            return false;
        };
        let Val::Closure(closure) = callee else {
            return true;
        };
        if call_stack.iter().any(|active| active == name) {
            return false;
        }
        call_stack.push(name.to_string());
        let preserves = !Self::stmt_assigns_name(&closure.body, target)
            && self.stmt_calls_preserve_binding(&closure.body, target, call_stack);
        call_stack.pop();
        preserves
    }

    fn stmt_calls_preserve_binding(&self, stmt: &Stmt, target: &str, call_stack: &mut Vec<String>) -> bool {
        match stmt {
            Stmt::Block { statements } => statements
                .iter()
                .all(|stmt| self.stmt_calls_preserve_binding(stmt, target, call_stack)),
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::CompoundAssign { value, .. }
            | Stmt::Define { value, .. }
            | Stmt::Expr(value)
            | Stmt::Return { value: Some(value) } => self.expr_calls_preserve_binding_inner(value, target, call_stack),
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            }
            | Stmt::IfLet {
                value: condition,
                then_stmt,
                else_stmt,
                ..
            } => {
                self.expr_calls_preserve_binding_inner(condition, target, call_stack)
                    && self.stmt_calls_preserve_binding(then_stmt, target, call_stack)
                    && else_stmt
                        .as_ref()
                        .is_none_or(|stmt| self.stmt_calls_preserve_binding(stmt, target, call_stack))
            }
            Stmt::While { condition, body } => {
                self.expr_calls_preserve_binding_inner(condition, target, call_stack)
                    && self.stmt_calls_preserve_binding(body, target, call_stack)
            }
            Stmt::WhileLet { value, body, .. }
            | Stmt::For {
                iterable: value, body, ..
            } => {
                self.expr_calls_preserve_binding_inner(value, target, call_stack)
                    && self.stmt_calls_preserve_binding(body, target, call_stack)
            }
            Stmt::Function { .. }
            | Stmt::Impl { .. }
            | Stmt::Import(_)
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Return { value: None }
            | Stmt::Struct { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Trait { .. }
            | Stmt::Empty => true,
        }
    }
}

fn collect_global_writes(stmt: &Stmt, globals: &std::collections::HashSet<String>, out: &mut Vec<String>) {
    match stmt {
        Stmt::Assign { name, .. } | Stmt::CompoundAssign { name, .. } | Stmt::Define { name, .. }
            if globals.contains(name) =>
        {
            out.push(name.clone());
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_global_writes(stmt, globals, out);
            }
        }
        Stmt::If {
            then_stmt, else_stmt, ..
        }
        | Stmt::IfLet {
            then_stmt, else_stmt, ..
        } => {
            collect_global_writes(then_stmt, globals, out);
            if let Some(stmt) = else_stmt {
                collect_global_writes(stmt, globals, out);
            }
        }
        Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
            collect_global_writes(body, globals, out);
        }
        Stmt::Function { .. } | Stmt::Impl { .. } => {}
        Stmt::Import(_)
        | Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::CompoundAssign { .. }
        | Stmt::Define { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Return { .. }
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Expr(_)
        | Stmt::Empty => {}
    }
}
