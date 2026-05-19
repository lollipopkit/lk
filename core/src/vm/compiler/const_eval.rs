use crate::{
    expr::{Expr, TemplateStringPart},
    stmt::Stmt,
    val::{ClosureValue, Val},
    vm::CaptureSpec,
};

use super::FunctionBuilder;
use super::free_vars::FreeVarCollector;
use std::collections::HashMap;

const CONST_CALL_FUEL: usize = 100_000;
const CONST_CALL_DEPTH: usize = 256;

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

    pub(super) fn bind_known_value(&mut self, name: String, value: Val) {
        self.const_env.define(name, value);
    }

    pub(super) fn forget_known_value(&mut self, name: &str) {
        self.const_bindings.remove(name);
        self.const_env.remove(name);
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
        if let Some(value) = self.try_eval_known_call(expr) {
            return Some(value);
        }
        match expr {
            Expr::Call(name, _) => {
                if !self.call_safe_to_fold(name) {
                    return None;
                }
            }
            Expr::CallExpr(callee, _) => {
                let Expr::Var(name) = callee.as_ref() else {
                    return None;
                };
                if !self.call_safe_to_fold(name) {
                    return None;
                }
            }
            Expr::CallNamed(_, _, _) | Expr::Closure { .. } => return None,
            _ => {}
        }
        if let Some(substituted) = self.substitute_const_expr(expr) {
            let folded = substituted.fold_constants();
            if let Expr::Val(v) = folded {
                return Some(v);
            }
        }
        if self.safe_call_uses_only_known_values(expr) {
            return expr.eval_with_ctx(&mut self.const_env.clone()).ok();
        }
        if self.expr_uses_only_const_bindings(expr) {
            return expr.eval_with_ctx(&mut self.const_env.clone()).ok();
        }
        None
    }

    pub(super) fn call_safe_to_fold(&self, func_name: &str) -> bool {
        if let Some(Val::Closure(closure_arc)) = self.const_env.get(func_name) {
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
        false
    }

    fn try_eval_known_call(&mut self, expr: &Expr) -> Option<Val> {
        let (name, args) = match expr {
            Expr::Call(name, args) => (name.as_str(), args.as_slice()),
            Expr::CallExpr(callee, args) => {
                let Expr::Var(name) = callee.as_ref() else {
                    return None;
                };
                (name.as_str(), args.as_slice())
            }
            _ => return None,
        };
        let Some(Val::Closure(closure)) = self.const_env.get(name).cloned() else {
            return None;
        };
        if !self.closure_safe_for_known_eval(name, closure.as_ref()) || closure.params.len() != args.len() {
            return None;
        }
        let mut fuel = CONST_CALL_FUEL;
        let mut memo = HashMap::new();
        let mut locals = HashMap::new();
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval_known_expr(arg, &mut locals, &mut fuel, CONST_CALL_DEPTH, &mut memo)?);
        }
        self.eval_known_closure(name, closure.as_ref(), &values, &mut fuel, CONST_CALL_DEPTH, &mut memo)
    }

    pub(super) fn closure_safe_for_known_eval(&self, name: &str, closure: &ClosureValue) -> bool {
        if !closure.named_params.is_empty()
            || !closure
                .capture_specs
                .iter()
                .all(|spec| matches!(spec, CaptureSpec::Const { .. }))
        {
            return false;
        }
        self.closure_free_vars(closure)
            .into_iter()
            .all(|free| free == name || matches!(self.const_env.get(&free), Some(Val::Closure(_))))
    }

    fn spend_const_fuel(fuel: &mut usize) -> Option<()> {
        *fuel = fuel.checked_sub(1)?;
        Some(())
    }

    fn eval_known_closure(
        &self,
        name: &str,
        closure: &ClosureValue,
        args: &[Val],
        fuel: &mut usize,
        depth: usize,
        memo: &mut HashMap<String, Val>,
    ) -> Option<Val> {
        Self::spend_const_fuel(fuel)?;
        let next_depth = depth.checked_sub(1)?;
        if args.len() != closure.params.len() {
            return None;
        }
        let memo_key = Self::known_call_memo_key(name, args);
        if let Some(key) = memo_key.as_ref()
            && let Some(value) = memo.get(key)
        {
            return Some(value.clone());
        }
        let mut locals = HashMap::with_capacity(closure.params.len());
        for (param, value) in closure.params.iter().zip(args.iter()) {
            locals.insert(param.clone(), value.clone());
        }
        let value = self.eval_known_stmt(closure.body.as_ref(), &mut locals, fuel, next_depth, memo)?;
        if let Some(key) = memo_key {
            memo.insert(key, value.clone());
        }
        Some(value)
    }

    fn known_call_memo_key(name: &str, args: &[Val]) -> Option<String> {
        let mut key = String::from(name);
        key.push('(');
        for (idx, arg) in args.iter().enumerate() {
            if idx > 0 {
                key.push(',');
            }
            match arg {
                Val::Nil => key.push_str("nil"),
                Val::Bool(value) => key.push_str(if *value { "true" } else { "false" }),
                Val::Int(value) => key.push_str(&format!("i:{value}")),
                Val::Float(value) => key.push_str(&format!("f:{:x}", value.to_bits())),
                Val::Str(value) => {
                    key.push_str("s:");
                    key.push_str(value);
                }
                _ => return None,
            }
        }
        key.push(')');
        Some(key)
    }

    fn eval_known_stmt(
        &self,
        stmt: &Stmt,
        locals: &mut HashMap<String, Val>,
        fuel: &mut usize,
        depth: usize,
        memo: &mut HashMap<String, Val>,
    ) -> Option<Val> {
        Self::spend_const_fuel(fuel)?;
        match stmt {
            Stmt::Return { value } => value
                .as_deref()
                .map(|expr| self.eval_known_expr(expr, locals, fuel, depth, memo))
                .unwrap_or(Some(Val::Nil)),
            Stmt::Expr(expr) => self.eval_known_expr(expr, locals, fuel, depth, memo),
            Stmt::Block { statements } => {
                for stmt in statements {
                    if let Some(value) = self.eval_known_stmt(stmt, locals, fuel, depth, memo) {
                        return Some(value);
                    }
                }
                None
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                let condition = self.eval_known_expr(condition, locals, fuel, depth, memo)?;
                if !matches!(condition, Val::Bool(false) | Val::Nil) {
                    self.eval_known_stmt(then_stmt, locals, fuel, depth, memo)
                } else {
                    else_stmt
                        .as_deref()
                        .and_then(|stmt| self.eval_known_stmt(stmt, locals, fuel, depth, memo))
                }
            }
            Stmt::Let {
                pattern: crate::expr::Pattern::Variable(name),
                value,
                ..
            } => {
                let value = self.eval_known_expr(value, locals, fuel, depth, memo)?;
                locals.insert(name.clone(), value);
                None
            }
            Stmt::Assign { name, value, .. } => {
                let value = self.eval_known_expr(value, locals, fuel, depth, memo)?;
                locals.insert(name.clone(), value);
                None
            }
            _ => None,
        }
    }

    fn eval_known_expr(
        &self,
        expr: &Expr,
        locals: &mut HashMap<String, Val>,
        fuel: &mut usize,
        depth: usize,
        memo: &mut HashMap<String, Val>,
    ) -> Option<Val> {
        Self::spend_const_fuel(fuel)?;
        match expr {
            Expr::Val(value) => Some(value.clone()),
            Expr::Var(name) => locals.get(name).cloned().or_else(|| self.const_env.get(name).cloned()),
            Expr::Paren(inner) => self.eval_known_expr(inner, locals, fuel, depth, memo),
            Expr::Unary(op, inner) => {
                let value = self.eval_known_expr(inner, locals, fuel, depth, memo)?;
                Expr::Unary(op.clone(), Box::new(Expr::Val(value)))
                    .eval_with_ctx(&mut self.const_env.clone())
                    .ok()
            }
            Expr::Bin(left, op, right) => {
                let left = self.eval_known_expr(left, locals, fuel, depth, memo)?;
                let right = self.eval_known_expr(right, locals, fuel, depth, memo)?;
                op.eval_vals(&left, &right).ok()
            }
            Expr::Call(name, args) => self.eval_known_named_call(name, args, locals, fuel, depth, memo),
            Expr::CallExpr(callee, args) => {
                let Expr::Var(name) = callee.as_ref() else {
                    return None;
                };
                self.eval_known_named_call(name, args, locals, fuel, depth, memo)
            }
            _ => None,
        }
    }

    fn eval_known_named_call(
        &self,
        name: &str,
        args: &[Box<Expr>],
        locals: &mut HashMap<String, Val>,
        fuel: &mut usize,
        depth: usize,
        memo: &mut HashMap<String, Val>,
    ) -> Option<Val> {
        let Some(Val::Closure(closure)) = self.const_env.get(name).cloned() else {
            return None;
        };
        if !self.closure_safe_for_known_eval(name, closure.as_ref()) || closure.params.len() != args.len() {
            return None;
        }
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval_known_expr(arg, locals, fuel, depth, memo)?);
        }
        self.eval_known_closure(name, closure.as_ref(), &values, fuel, depth, memo)
    }

    fn closure_free_vars(&self, closure: &ClosureValue) -> Vec<String> {
        let mut collector = FreeVarCollector::new();
        for param in closure.params.iter() {
            collector.declare(param.clone());
        }
        for named in closure.named_params.iter() {
            collector.declare(named.name.clone());
        }
        for spec in closure.capture_specs.iter() {
            if let CaptureSpec::Const { name, .. } = spec {
                collector.declare(name.clone());
            }
        }
        collector.visit_stmt(&closure.body);
        collector.into_sorted_vec()
    }

    fn closure_has_free_vars(&self, closure: &ClosureValue) -> bool {
        !self.closure_free_vars(closure).is_empty()
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
            Block(_) => false,
            Match { value, arms } => {
                self.expr_uses_only_const_bindings(value)
                    && arms.iter().all(|arm| self.expr_uses_only_const_bindings(&arm.body))
            }
            StructLiteral { fields, .. } => fields.iter().all(|(_, expr)| self.expr_uses_only_const_bindings(expr)),
            Call(_, args) => args.iter().all(|arg| self.expr_uses_only_const_bindings(arg)),
            CallExpr(callee, args) => match callee.as_ref() {
                Var(name) if self.call_safe_to_fold(name) => {
                    args.iter().all(|arg| self.expr_uses_only_const_bindings(arg))
                }
                _ => {
                    self.expr_uses_only_const_bindings(callee)
                        && args.iter().all(|arg| self.expr_uses_only_const_bindings(arg))
                }
            },
            CallNamed(callee, pos_args, named_args) => {
                self.expr_uses_only_const_bindings(callee)
                    && pos_args.iter().all(|arg| self.expr_uses_only_const_bindings(arg))
                    && named_args
                        .iter()
                        .all(|(_, expr)| self.expr_uses_only_const_bindings(expr))
            }
        }
    }

    fn safe_call_uses_only_known_values(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Call(name, args) => {
                self.call_safe_to_fold(name) && args.iter().all(|arg| self.expr_uses_only_known_values(arg))
            }
            Expr::CallExpr(callee, args) => match callee.as_ref() {
                Expr::Var(name) => {
                    self.call_safe_to_fold(name) && args.iter().all(|arg| self.expr_uses_only_known_values(arg))
                }
                _ => false,
            },
            _ => false,
        }
    }

    pub(super) fn expr_uses_only_known_values(&self, expr: &Expr) -> bool {
        use Expr::*;
        match expr {
            Val(_) => true,
            Var(name) => self.const_bindings.contains_key(name) || self.const_env.get(name).is_some(),
            Paren(inner) | Unary(_, inner) => self.expr_uses_only_known_values(inner),
            Bin(l, _, r) | And(l, r) | Or(l, r) | NullishCoalescing(l, r) | OptionalAccess(l, r) | Access(l, r) => {
                self.expr_uses_only_known_values(l) && self.expr_uses_only_known_values(r)
            }
            Conditional(c, t, e) => {
                self.expr_uses_only_known_values(c)
                    && self.expr_uses_only_known_values(t)
                    && self.expr_uses_only_known_values(e)
            }
            List(items) => items.iter().all(|item| self.expr_uses_only_known_values(item)),
            Map(pairs) => pairs
                .iter()
                .all(|(k, v)| self.expr_uses_only_known_values(k) && self.expr_uses_only_known_values(v)),
            Range { start, end, step, .. } => {
                start
                    .as_deref()
                    .map(|e| self.expr_uses_only_known_values(e))
                    .unwrap_or(true)
                    && end
                        .as_deref()
                        .map(|e| self.expr_uses_only_known_values(e))
                        .unwrap_or(true)
                    && step
                        .as_deref()
                        .map(|e| self.expr_uses_only_known_values(e))
                        .unwrap_or(true)
            }
            TemplateString(parts) => parts.iter().all(|part| match part {
                TemplateStringPart::Literal(_) => true,
                TemplateStringPart::Expr(inner) => self.expr_uses_only_known_values(inner),
            }),
            Call(name, args) => {
                self.call_safe_to_fold(name) && args.iter().all(|arg| self.expr_uses_only_known_values(arg))
            }
            CallExpr(callee, args) => match callee.as_ref() {
                Var(name) if self.call_safe_to_fold(name) => {
                    args.iter().all(|arg| self.expr_uses_only_known_values(arg))
                }
                _ => false,
            },
            Closure { .. } | Block(_) | CallNamed(_, _, _) | Select { .. } | Match { .. } | StructLiteral { .. } => {
                false
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
