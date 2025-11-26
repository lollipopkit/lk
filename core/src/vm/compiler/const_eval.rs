use crate::{
    expr::{Expr, TemplateStringPart},
    val::{ClosureValue, Val},
    vm::CaptureSpec,
};

use super::FunctionBuilder;
use super::free_vars::FreeVarCollector;

impl FunctionBuilder {
    pub(super) fn push_const_scope(&mut self) {
        self.const_scope_stack.push(Vec::new());
        self.const_env.push_scope();
    }

    pub(super) fn pop_const_scope(&mut self) {
        if self.const_scope_stack.len() <= 1 {
            return;
        }
        if let Some(names) = self.const_scope_stack.pop() {
            for name in names {
                self.const_bindings.remove(&name);
            }
        }
        self.const_env.pop_scope();
    }

    pub(super) fn bind_const(&mut self, name: String, value: Val) {
        if let Some(scope) = self.const_scope_stack.last_mut() {
            scope.push(name.clone());
        }
        self.const_env.define_const(name.clone(), value.clone());
        self.const_bindings.insert(name, value);
    }

    pub(super) fn lookup_const(&self, name: &str) -> Option<&Val> {
        self.const_bindings.get(name)
    }

    pub(super) fn with_const_scope<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.push_const_scope();
        let result = f(self);
        self.pop_const_scope();
        result
    }

    pub(super) fn try_eval_const_expr(&mut self, expr: &Expr) -> Option<Val> {
        match expr {
            Expr::Call(name, _) => {
                if !self.call_safe_to_fold(name) {
                    return None;
                }
            }
            Expr::CallExpr(_, _) | Expr::CallNamed(_, _, _) | Expr::Closure { .. } => return None,
            _ => {}
        }
        if let Some(substituted) = self.substitute_const_expr(expr) {
            let folded = substituted.fold_constants();
            if let Expr::Val(v) = folded {
                return Some(v);
            }
        }
        if self.expr_uses_only_const_bindings(expr) {
            return expr.eval_with_ctx(&mut self.const_env.clone()).ok();
        }
        None
    }

    fn call_safe_to_fold(&self, func_name: &str) -> bool {
        if let Some(value) = self.const_env.get(func_name) {
            if let Val::Closure(closure_arc) = value {
                let closure = closure_arc.as_ref();
                if !closure
                    .capture_specs
                    .iter()
                    .all(|spec| matches!(spec, CaptureSpec::Const { .. }))
                {
                    return false;
                }
                return !self.closure_has_free_vars(closure);
            }
        }
        false
    }

    fn closure_has_free_vars(&self, closure: &ClosureValue) -> bool {
        let mut collector = FreeVarCollector::new();
        for param in closure.params.iter() {
            collector.declare(param.clone());
        }
        for named in closure.named_params.iter() {
            collector.declare(named.name.clone());
        }
        collector.visit_stmt(&closure.body);
        !collector.into_sorted_vec().is_empty()
    }

    fn expr_uses_only_const_bindings(&self, expr: &Expr) -> bool {
        use Expr::*;
        match expr {
            Val(_) => true,
            Var(name) => self.const_bindings.contains_key(name),
            Paren(inner) | Unary(_, inner) => self.expr_uses_only_const_bindings(inner),
            Bin(l, _, r) | And(l, r) | Or(l, r) | NullishCoalescing(l, r) | OptionalAccess(l, r) | Access(l, r) => {
                self.expr_uses_only_const_bindings(l) && self.expr_uses_only_const_bindings(r)
            }
            Conditional(c, t, e) => {
                self.expr_uses_only_const_bindings(c)
                    && self.expr_uses_only_const_bindings(t)
                    && self.expr_uses_only_const_bindings(e)
            }
            List(items) => items.iter().all(|item| self.expr_uses_only_const_bindings(item)),
            Map(pairs) => pairs
                .iter()
                .all(|(k, v)| self.expr_uses_only_const_bindings(k) && self.expr_uses_only_const_bindings(v)),
            Range { start, end, step, .. } => {
                start
                    .as_deref()
                    .map(|e| self.expr_uses_only_const_bindings(e))
                    .unwrap_or(true)
                    && end
                        .as_deref()
                        .map(|e| self.expr_uses_only_const_bindings(e))
                        .unwrap_or(true)
                    && step
                        .as_deref()
                        .map(|e| self.expr_uses_only_const_bindings(e))
                        .unwrap_or(true)
            }
            Select { cases, default_case } => {
                let cases_ok = cases.iter().all(|case| {
                    let pattern_ok = match &case.pattern {
                        crate::expr::SelectPattern::Recv { channel, .. } => self.expr_uses_only_const_bindings(channel),
                        crate::expr::SelectPattern::Send { channel, value } => {
                            self.expr_uses_only_const_bindings(channel) && self.expr_uses_only_const_bindings(value)
                        }
                    };
                    let guard_ok = case
                        .guard
                        .as_deref()
                        .map(|g| self.expr_uses_only_const_bindings(g))
                        .unwrap_or(true);
                    pattern_ok && guard_ok && self.expr_uses_only_const_bindings(&case.body)
                });
                cases_ok
                    && default_case
                        .as_deref()
                        .map(|e| self.expr_uses_only_const_bindings(e))
                        .unwrap_or(true)
            }
            TemplateString(parts) => parts.iter().all(|part| match part {
                TemplateStringPart::Literal(_) => true,
                TemplateStringPart::Expr(inner) => self.expr_uses_only_const_bindings(inner),
            }),
            Closure { .. } => false,
            Match { value, arms } => {
                self.expr_uses_only_const_bindings(value)
                    && arms.iter().all(|arm| self.expr_uses_only_const_bindings(&arm.body))
            }
            StructLiteral { fields, .. } => fields.iter().all(|(_, expr)| self.expr_uses_only_const_bindings(expr)),
            Call(_, args) => args.iter().all(|arg| self.expr_uses_only_const_bindings(arg)),
            CallExpr(callee, args) => {
                self.expr_uses_only_const_bindings(callee)
                    && args.iter().all(|arg| self.expr_uses_only_const_bindings(arg))
            }
            CallNamed(callee, pos_args, named_args) => {
                self.expr_uses_only_const_bindings(callee)
                    && pos_args.iter().all(|arg| self.expr_uses_only_const_bindings(arg))
                    && named_args
                        .iter()
                        .all(|(_, expr)| self.expr_uses_only_const_bindings(expr))
            }
        }
    }

    pub(super) fn substitute_const_expr(&self, expr: &Expr) -> Option<Expr> {
        use Expr::*;
        match expr {
            Val(v) => Some(Val(v.clone())),
            Var(name) => self.lookup_const(name).map(|v| Val(v.clone())),
            Paren(inner) => self.substitute_const_expr(inner).map(|e| Paren(Box::new(e))),
            Unary(op, inner) => {
                let inner_expr = self.substitute_const_expr(inner)?;
                Some(Unary(op.clone(), Box::new(inner_expr)))
            }
            Bin(l, op, r) => {
                let left = self.substitute_const_expr(l)?;
                let right = self.substitute_const_expr(r)?;
                Some(Bin(Box::new(left), op.clone(), Box::new(right)))
            }
            And(l, r) => {
                let left = self.substitute_const_expr(l)?;
                let right = self.substitute_const_expr(r)?;
                Some(And(Box::new(left), Box::new(right)))
            }
            Or(l, r) => {
                let left = self.substitute_const_expr(l)?;
                let right = self.substitute_const_expr(r)?;
                Some(Or(Box::new(left), Box::new(right)))
            }
            NullishCoalescing(l, r) => {
                let left = self.substitute_const_expr(l)?;
                let right = self.substitute_const_expr(r)?;
                Some(NullishCoalescing(Box::new(left), Box::new(right)))
            }
            Conditional(c, t, e) => {
                let cond = self.substitute_const_expr(c)?;
                let then_branch = self.substitute_const_expr(t)?;
                let else_branch = self.substitute_const_expr(e)?;
                Some(Conditional(
                    Box::new(cond),
                    Box::new(then_branch),
                    Box::new(else_branch),
                ))
            }
            Access(base, field) => {
                let base_expr = self.substitute_const_expr(base)?;
                let field_expr = self.substitute_const_expr(field)?;
                Some(Access(Box::new(base_expr), Box::new(field_expr)))
            }
            OptionalAccess(base, field) => {
                let base_expr = self.substitute_const_expr(base)?;
                let field_expr = self.substitute_const_expr(field)?;
                Some(OptionalAccess(Box::new(base_expr), Box::new(field_expr)))
            }
            List(items) => {
                let mut new_items = Vec::with_capacity(items.len());
                for item in items {
                    new_items.push(Box::new(self.substitute_const_expr(item)?));
                }
                Some(List(new_items))
            }
            Map(pairs) => {
                let mut new_pairs = Vec::with_capacity(pairs.len());
                for (k, v) in pairs {
                    let key = Box::new(self.substitute_const_expr(k)?);
                    let value = Box::new(self.substitute_const_expr(v)?);
                    new_pairs.push((key, value));
                }
                Some(Map(new_pairs))
            }
            TemplateString(parts) => {
                let mut new_parts = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        TemplateStringPart::Literal(s) => {
                            new_parts.push(TemplateStringPart::Literal(s.clone()));
                        }
                        TemplateStringPart::Expr(inner) => {
                            let substituted = self.substitute_const_expr(inner)?;
                            new_parts.push(TemplateStringPart::Expr(Box::new(substituted)));
                        }
                    }
                }
                Some(TemplateString(new_parts))
            }
            // Variants not supported for compile-time evaluation
            _ => None,
        }
    }
}
