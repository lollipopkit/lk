use std::collections::HashSet;

use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::Val,
    vm::Op,
};

use super::FunctionBuilder;

impl FunctionBuilder {
    pub(super) fn try_precompute_range_loop(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
        if !matches!(pattern, ForPattern::Ignore) {
            return false;
        }
        let Expr::Range {
            start,
            end,
            inclusive,
            step,
        } = iterable
        else {
            return false;
        };
        let Some(iter_count) =
            self.range_iteration_count(start.as_deref(), end.as_deref(), *inclusive, step.as_deref())
        else {
            return false;
        };
        let Stmt::Block { statements } = body else {
            return false;
        };
        let env_snapshot = self.const_env.clone();
        let bindings_snapshot = self.const_bindings.clone();
        let scope_snapshot = self.const_scope_stack.clone();
        let mut mutated_names = HashSet::new();
        for _ in 0..iter_count {
            if !self.eval_const_block(statements, &mut mutated_names) {
                self.const_env = env_snapshot;
                self.const_bindings = bindings_snapshot;
                self.const_scope_stack = scope_snapshot;
                return false;
            }
        }
        for name in mutated_names {
            if let Some(val) = self.const_env.get(&name).cloned() {
                if self.const_bindings.contains_key(&name) {
                    self.const_bindings.insert(name.clone(), val.clone());
                }
                if let Some(idx) = self.lookup(&name) {
                    let k = self.k(val);
                    self.emit(Op::LoadK(idx, k));
                    if self.should_export_global_write(&name) {
                        let kname = self.k(Val::from_str(name.as_str()));
                        self.emit(Op::DefineGlobal(kname, idx));
                    }
                }
            }
        }
        true
    }

    pub(super) fn try_elide_dead_range_loop(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
        if !matches!(pattern, ForPattern::Ignore) {
            return false;
        }
        let Expr::Range {
            start,
            end,
            inclusive,
            step,
        } = iterable
        else {
            return false;
        };
        if self
            .range_iteration_count(start.as_deref(), end.as_deref(), *inclusive, step.as_deref())
            .is_none()
        {
            return false;
        }
        let mut locals = HashSet::new();
        Self::stmt_is_local_only(body, &mut locals)
    }

    fn stmt_is_local_only(stmt: &Stmt, locals: &mut HashSet<String>) -> bool {
        match stmt {
            Stmt::Block { statements } => {
                let mut scoped = locals.clone();
                statements
                    .iter()
                    .all(|stmt| Self::stmt_is_local_only(stmt, &mut scoped))
            }
            Stmt::Let {
                pattern: Pattern::Variable(name),
                value,
                ..
            } => {
                if !Self::expr_is_local_pure(value, locals) {
                    return false;
                }
                locals.insert(name.clone());
                true
            }
            Stmt::Assign { name, value, .. } | Stmt::CompoundAssign { name, value, .. } => {
                locals.contains(name) && Self::expr_is_local_pure(value, locals)
            }
            Stmt::Expr(expr) => Self::expr_stmt_is_local_only(expr, locals),
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                if !Self::expr_is_local_pure(iterable, locals) {
                    return false;
                }
                let mut scoped = locals.clone();
                if !Self::bind_for_pattern_names(pattern, &mut scoped) {
                    return false;
                }
                Self::stmt_is_local_only(body, &mut scoped)
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                Self::expr_is_local_pure(condition, locals)
                    && Self::stmt_is_local_only(then_stmt, &mut locals.clone())
                    && else_stmt
                        .as_deref()
                        .map(|stmt| Self::stmt_is_local_only(stmt, &mut locals.clone()))
                        .unwrap_or(true)
            }
            Stmt::Empty => true,
            _ => false,
        }
    }

    pub(super) fn bind_for_pattern_names(pattern: &ForPattern, locals: &mut HashSet<String>) -> bool {
        match pattern {
            ForPattern::Variable(name) => {
                locals.insert(name.clone());
                true
            }
            ForPattern::Ignore => true,
            ForPattern::Tuple(patterns) | ForPattern::Array { patterns, rest: None } => patterns
                .iter()
                .all(|pattern| Self::bind_for_pattern_names(pattern, locals)),
            ForPattern::Array {
                patterns,
                rest: Some(rest),
            } => {
                for pattern in patterns {
                    if !Self::bind_for_pattern_names(pattern, locals) {
                        return false;
                    }
                }
                locals.insert(rest.clone());
                true
            }
            ForPattern::Object(entries) => entries
                .iter()
                .all(|(_, pattern)| Self::bind_for_pattern_names(pattern, locals)),
        }
    }

    fn expr_stmt_is_local_only(expr: &Expr, locals: &HashSet<String>) -> bool {
        if let Expr::CallExpr(callee, args) = expr
            && let Expr::Access(obj, field) = callee.as_ref()
            && let Expr::Var(receiver) = obj.as_ref()
            && locals.contains(receiver)
            && let Expr::Val(method_val) = field.as_ref()
            && method_val.as_str() == Some("push")
        {
            return args.iter().all(|arg| Self::expr_is_local_pure(arg, locals));
        }
        if let Expr::CallExpr(callee, args) = expr
            && let Expr::Access(obj, field) = callee.as_ref()
            && let Expr::Var(receiver) = obj.as_ref()
            && locals.contains(receiver)
            && let Expr::Val(method_val) = field.as_ref()
            && method_val.as_str() == Some("set")
            && args.iter().all(|arg| Self::expr_is_local_pure(arg, locals))
        {
            return args.iter().all(|arg| Self::expr_is_local_pure(arg, locals));
        }
        Self::expr_is_local_pure(expr, locals)
    }

    fn expr_is_local_pure(expr: &Expr, locals: &HashSet<String>) -> bool {
        match expr {
            Expr::Val(_) => true,
            Expr::Var(name) => locals.contains(name),
            Expr::Paren(inner) | Expr::Unary(_, inner) => Self::expr_is_local_pure(inner, locals),
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right)
            | Expr::Access(left, right)
            | Expr::OptionalAccess(left, right) => {
                Self::expr_is_local_pure(left, locals) && Self::expr_is_local_pure(right, locals)
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                Self::expr_is_local_pure(condition, locals)
                    && Self::expr_is_local_pure(then_expr, locals)
                    && Self::expr_is_local_pure(else_expr, locals)
            }
            Expr::List(items) => items.iter().all(|item| Self::expr_is_local_pure(item, locals)),
            Expr::Map(pairs) => pairs
                .iter()
                .all(|(key, value)| Self::expr_is_local_pure(key, locals) && Self::expr_is_local_pure(value, locals)),
            Expr::Range { start, end, step, .. } => {
                start
                    .as_deref()
                    .map(|expr| Self::expr_is_local_pure(expr, locals))
                    .unwrap_or(true)
                    && end
                        .as_deref()
                        .map(|expr| Self::expr_is_local_pure(expr, locals))
                        .unwrap_or(true)
                    && step
                        .as_deref()
                        .map(|expr| Self::expr_is_local_pure(expr, locals))
                        .unwrap_or(true)
            }
            Expr::TemplateString(parts) => parts.iter().all(|part| match part {
                crate::expr::TemplateStringPart::Literal(_) => true,
                crate::expr::TemplateStringPart::Expr(expr) => Self::expr_is_local_pure(expr, locals),
            }),
            Expr::CallExpr(callee, args) => {
                if let Expr::Access(obj, field) = callee.as_ref()
                    && let Expr::Var(receiver) = obj.as_ref()
                    && locals.contains(receiver)
                    && let Expr::Val(method_val5) = field.as_ref()
                    && matches!(method_val5.as_str(), Some("values") | Some("keys") | Some("len"))
                {
                    return args.is_empty();
                }
                false
            }
            _ => false,
        }
    }

    fn range_iteration_count(
        &mut self,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        step_expr: Option<&Expr>,
    ) -> Option<usize> {
        let start_val = match start {
            Some(expr) => self.eval_const_int(expr)?,
            None => 0,
        };
        let end_val = self.eval_const_int(end?)?;
        let step_val = match step_expr {
            Some(expr) => self.eval_const_int(expr)?,
            None => 1,
        };
        if step_expr.is_none() && start_val > end_val {
            return None;
        }
        if step_val == 0 {
            return None;
        }
        if step_val > 0 {
            if inclusive {
                if start_val > end_val {
                    return Some(0);
                }
                Some(((end_val - start_val) / step_val + 1) as usize)
            } else if start_val >= end_val {
                Some(0)
            } else {
                Some(((end_val - start_val - 1) / step_val + 1) as usize)
            }
        } else if inclusive {
            if start_val < end_val {
                Some(0)
            } else {
                Some(((start_val - end_val) / (-step_val) + 1) as usize)
            }
        } else if start_val <= end_val {
            Some(0)
        } else {
            Some(((start_val - end_val - 1) / (-step_val) + 1) as usize)
        }
    }

    fn eval_const_int(&mut self, expr: &Expr) -> Option<i64> {
        if self.expr_uses_only_known_values(expr)
            && let Ok(Val::Int(i)) = expr.eval_with_ctx(&mut self.const_env.clone())
        {
            return Some(i);
        }
        match self.try_eval_const_expr(expr)? {
            Val::Int(i) => Some(i),
            _ => None,
        }
    }

    pub(super) fn stmt_assigns_name(stmt: &Stmt, target: &str) -> bool {
        match stmt {
            Stmt::Block { statements } => statements.iter().any(|stmt| Self::stmt_assigns_name(stmt, target)),
            Stmt::Assign { name, .. } | Stmt::CompoundAssign { name, .. } => name == target,
            Stmt::Let {
                pattern: Pattern::Variable(name),
                ..
            } => name == target,
            Stmt::If {
                then_stmt, else_stmt, ..
            }
            | Stmt::IfLet {
                then_stmt, else_stmt, ..
            } => {
                Self::stmt_assigns_name(then_stmt, target)
                    || else_stmt
                        .as_deref()
                        .is_some_and(|branch| Self::stmt_assigns_name(branch, target))
            }
            Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
                Self::stmt_assigns_name(body, target)
            }
            Stmt::Function { body, .. } => Self::stmt_assigns_name(body, target),
            _ => false,
        }
    }

    pub(super) fn collect_stmt_assigned_names(stmt: &Stmt, out: &mut HashSet<String>) {
        match stmt {
            Stmt::Block { statements } => {
                for stmt in statements {
                    Self::collect_stmt_assigned_names(stmt, out);
                }
            }
            Stmt::Assign { name, .. } | Stmt::CompoundAssign { name, .. } | Stmt::Define { name, .. } => {
                out.insert(name.clone());
            }
            Stmt::Let { pattern, .. } => {
                Self::collect_pattern_assigned_names(pattern, out);
            }
            Stmt::If {
                then_stmt, else_stmt, ..
            }
            | Stmt::IfLet {
                then_stmt, else_stmt, ..
            } => {
                Self::collect_stmt_assigned_names(then_stmt, out);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_stmt_assigned_names(else_stmt, out);
                }
            }
            Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
                Self::collect_stmt_assigned_names(body, out);
            }
            Stmt::Function { name, .. } => {
                out.insert(name.clone());
            }
            Stmt::Impl { methods, .. } => {
                for method in methods {
                    Self::collect_stmt_assigned_names(method, out);
                }
            }
            Stmt::Import(_)
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

    fn collect_pattern_assigned_names(pattern: &Pattern, out: &mut HashSet<String>) {
        match pattern {
            Pattern::Variable(name) => {
                out.insert(name.clone());
            }
            Pattern::List { patterns, rest } => {
                for pattern in patterns {
                    Self::collect_pattern_assigned_names(pattern, out);
                }
                if let Some(rest) = rest {
                    out.insert(rest.clone());
                }
            }
            Pattern::Map { patterns, rest } => {
                for (_, pattern) in patterns {
                    Self::collect_pattern_assigned_names(pattern, out);
                }
                if let Some(rest) = rest {
                    out.insert(rest.clone());
                }
            }
            Pattern::Or(patterns) => {
                for pattern in patterns {
                    Self::collect_pattern_assigned_names(pattern, out);
                }
            }
            Pattern::Guard { pattern, .. } => Self::collect_pattern_assigned_names(pattern, out),
            Pattern::Literal(_) | Pattern::Wildcard | Pattern::Range { .. } => {}
        }
    }

    fn eval_const_block(&mut self, statements: &[Box<Stmt>], mutated: &mut HashSet<String>) -> bool {
        for stmt in statements {
            if !self.eval_const_stmt(stmt, mutated) {
                return false;
            }
        }
        true
    }

    fn eval_const_stmt(&mut self, stmt: &Stmt, mutated: &mut HashSet<String>) -> bool {
        match stmt {
            Stmt::Block { statements } => self.eval_const_block(statements, mutated),
            Stmt::CompoundAssign { name, op, value, .. } => {
                let Some(current) = self.const_env.get(name).cloned() else {
                    return false;
                };
                let Some(rhs) = self.try_eval_const_expr(value) else {
                    return false;
                };
                let Ok(val) = op.eval_vals(&current, &rhs) else {
                    return false;
                };
                if self.const_env.assign(name, val.clone()).is_err() {
                    return false;
                }
                if self.const_bindings.contains_key(name) {
                    self.const_bindings.insert(name.clone(), val);
                }
                mutated.insert(name.clone());
                true
            }
            Stmt::Assign { name, value, .. } => {
                let Some(val) = self.try_eval_const_expr(value) else {
                    return false;
                };
                if self.const_env.assign(name, val.clone()).is_err() {
                    return false;
                }
                if self.const_bindings.contains_key(name) {
                    self.const_bindings.insert(name.clone(), val);
                }
                mutated.insert(name.clone());
                true
            }
            Stmt::Expr(expr) => self.try_eval_const_expr(expr).is_some(),
            _ => false,
        }
    }

    pub(super) fn try_emit_list_fold_add(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
        let ForPattern::Variable(item_name) = pattern else {
            return false;
        };
        let Expr::Var(list_name) = iterable else {
            return false;
        };
        let Some(list_reg) = self.lookup(list_name) else {
            return false;
        };
        if !self.list_locals.contains(&list_reg) {
            return false;
        }

        let folded_body = match body {
            Stmt::CompoundAssign {
                name,
                op: BinOp::Add,
                value,
                ..
            } => Some((name, value.as_ref())),
            Stmt::Block { statements } if statements.len() == 1 => match statements[0].as_ref() {
                Stmt::CompoundAssign {
                    name,
                    op: BinOp::Add,
                    value,
                    ..
                } => Some((name, value.as_ref())),
                _ => None,
            },
            _ => None,
        };
        let Some((acc_name, Expr::Var(value_name))) = folded_body else {
            return false;
        };
        if value_name != item_name {
            return false;
        }
        let Some(acc_reg) = self.lookup(acc_name) else {
            return false;
        };

        self.emit(Op::ListFoldAdd {
            acc: acc_reg,
            list: list_reg,
        });
        true
    }

    pub(super) fn try_emit_map_values_fold_add(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
        let ForPattern::Variable(item_name) = pattern else {
            return false;
        };
        let Expr::CallExpr(callee, args) = iterable else {
            return false;
        };
        if !args.is_empty() {
            return false;
        }
        let Expr::Access(obj_expr, field_expr) = callee.as_ref() else {
            return false;
        };
        let (Expr::Var(map_name), Expr::Val(method_val)) = (obj_expr.as_ref(), field_expr.as_ref()) else {
            return false;
        };
        if method_val.as_str() != Some("values") {
            return false;
        }
        let Some(map_reg) = self.lookup(map_name) else {
            return false;
        };
        if !self.map_locals.contains(&map_reg) {
            return false;
        }

        let folded_body = match body {
            Stmt::CompoundAssign {
                name,
                op: BinOp::Add,
                value,
                ..
            } => Some((name, value.as_ref())),
            Stmt::Block { statements } if statements.len() == 1 => match statements[0].as_ref() {
                Stmt::CompoundAssign {
                    name,
                    op: BinOp::Add,
                    value,
                    ..
                } => Some((name, value.as_ref())),
                _ => None,
            },
            _ => None,
        };
        let Some((acc_name, Expr::Var(value_name))) = folded_body else {
            return false;
        };
        if value_name != item_name {
            return false;
        }
        let Some(acc_reg) = self.lookup(acc_name) else {
            return false;
        };

        self.emit(Op::MapValuesFoldAdd {
            acc: acc_reg,
            map: map_reg,
        });
        true
    }
}
