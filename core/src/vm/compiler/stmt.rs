//! Statement compilation — translates AST statements to bytecode.
//!
//! Handles all statement forms: variable bindings (let/const), assignments,
//! loops (while, for, for-in), conditionals (if, if-let), returns, and
//! compound assignments. Also orchestrates trait/impl registration.
//!
//! ## Loop Lowering
//!
//! Simple `while (i < N) { ...; i = i + 1 }` loops are detected and lowered
//! to for-range via `try_lower_while_to_for_range`. This enables:
//!  1. BC32 packing (ForRange ops are BC32-encodable)
//!  2. Using `ForRangeState` (bare i64) instead of `Val::Int` comparison/increment
//!
//! For-range loops with constant bounds may be fully unrolled at compile time
//! via `try_precompute_range_loop`.
//!
//! ## Self-Assign Optimization
//!
//! `try_emit_simple_self_assign` detects `x = x + 1` / `x = x * y` and emits
//! in-place opcodes (`AddIntImm`, `AddInt`, `SubInt`, `MulInt`) that avoid
//! temporary register allocation and extra load/store cycles.

use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::{ClosureCapture, ClosureInit, ClosureValue, FunctionNamedParamType, Type, Val},
    vm::{CaptureSpec, Op},
};

use super::FunctionBuilder;
use std::{collections::HashSet, sync::Arc};

fn detect_mutating_receiver(expr: &Expr) -> Option<&str> {
    if let Expr::CallExpr(callee, _args) = expr
        && let Expr::Access(obj, field) = callee.as_ref()
        && let Expr::Var(var_name) = obj.as_ref()
        && let Expr::Val(Val::Str(method)) = field.as_ref()
        && method.as_ref() == "push"
    {
        return Some(var_name);
    }
    None
}

/// Strip the trailing increment statement from a while-loop body.
/// Used by the while→for-range lowering pass.
fn strip_trailing_increment(stmt: &Stmt, counter_name: &str) -> Stmt {
    match stmt {
        Stmt::Block { statements } => {
            if statements.len() <= 1 {
                Stmt::Block { statements: vec![] }
            } else {
                let mut trimmed = statements.clone();
                trimmed.pop();
                // Also strip trailing increment from the new last statement if it's a block
                if let Some(last) = trimmed.pop() {
                    trimmed.push(Box::new(strip_trailing_increment(&last, counter_name)));
                }
                Stmt::Block { statements: trimmed }
            }
        }
        Stmt::Assign { name, value, .. } if name == counter_name => Stmt::Block { statements: vec![] },
        Stmt::CompoundAssign { name, .. } if name == counter_name => Stmt::Block { statements: vec![] },
        other => other.clone(),
    }
}

impl FunctionBuilder {
    fn call_expr_name<'a>(expr: &'a Expr) -> Option<(&'a str, &'a [Box<Expr>])> {
        match expr {
            Expr::Call(name, args) => Some((name.as_str(), args.as_slice())),
            Expr::CallExpr(callee, args) => {
                let Expr::Var(name) = callee.as_ref() else {
                    return None;
                };
                Some((name.as_str(), args.as_slice()))
            }
            _ => None,
        }
    }

    fn simple_call_operand<'a>(params: &'a [String], args: &'a [Box<Expr>], operand: &'a Expr) -> Option<&'a Expr> {
        match operand {
            Expr::Var(param_name) => {
                if let Some(param_idx) = params.iter().position(|param| param == param_name) {
                    args.get(param_idx).map(|arg| arg.as_ref())
                } else {
                    Some(operand)
                }
            }
            other => Some(other),
        }
    }

    fn small_int_from_capture(captures: &ClosureCapture, expr: &Expr) -> Option<i16> {
        let Expr::Var(name) = expr else {
            return None;
        };
        let (_, value) = captures
            .iter()
            .find(|(capture_name, _)| *capture_name == name.as_str())?;
        let Val::Int(value) = value else {
            return None;
        };
        (-128..=127).contains(value).then_some(*value as i16)
    }

    fn try_eval_arg_const(&mut self, expr: &Expr) -> Option<Val> {
        match expr {
            Expr::Val(value) => Some(value.clone()),
            Expr::Var(name) => self.lookup_const(name).cloned(),
            Expr::Paren(inner) => self.try_eval_arg_const(inner),
            _ => self.try_eval_const_expr(expr),
        }
    }

    fn try_specialize_const_closure_factory(&mut self, value: &Expr) -> Option<Val> {
        let (func_name, args) = Self::call_expr_name(value)?;
        if !self.call_safe_to_fold(func_name) {
            return None;
        }

        let Some(Val::Closure(factory)) = self.const_env.get(func_name).cloned() else {
            return None;
        };
        if !factory.named_params.is_empty() || factory.params.len() != args.len() {
            return None;
        }

        let Some(returned) = Self::simple_return_expr(factory.body.as_ref()) else {
            return None;
        };
        let Expr::Closure { params, body } = returned else {
            return None;
        };

        let mut captures = Vec::new();
        let mut capture_specs = Vec::new();
        for (idx, param) in factory.params.iter().enumerate() {
            if params.iter().any(|inner| inner == param) {
                continue;
            }
            let arg_value = self.try_eval_arg_const(args[idx].as_ref())?;
            let kidx = self.k(arg_value.clone());
            captures.push((param.clone(), arg_value));
            capture_specs.push(CaptureSpec::Const {
                name: param.clone(),
                kidx,
            });
        }

        let capture_names = captures.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>();
        let capture_values = captures.into_iter().map(|(_, value)| value).collect::<Vec<_>>();
        let body_stmt = Stmt::Expr(body.clone());
        Some(Val::Closure(Arc::new(ClosureValue::new(ClosureInit {
            params: Arc::new(params.clone()),
            named_params: Arc::new(Vec::new()),
            body: Arc::new(body_stmt),
            env: Arc::new(self.const_env.clone()),
            upvalues: Arc::new(Vec::new()),
            captures: ClosureCapture::from_pairs(capture_names, capture_values),
            capture_specs: Arc::new(capture_specs),
            default_funcs: Arc::new(Vec::new()),
            code: Arc::new(once_cell::sync::OnceCell::new()),
            debug_name: Some(func_name.to_string()),
            debug_location: None,
        }))))
    }

    fn simple_return_expr(stmt: &Stmt) -> Option<&Expr> {
        match stmt {
            Stmt::Return { value: Some(value) } => Some(value.as_ref()),
            Stmt::Expr(expr) => Some(expr.as_ref()),
            Stmt::Block { statements } if statements.len() == 1 => Self::simple_return_expr(statements[0].as_ref()),
            _ => None,
        }
    }

    fn expr_mentions_name(expr: &Expr, target: &str) -> bool {
        match expr {
            Expr::Var(name) => name == target,
            Expr::Paren(inner) | Expr::Unary(_, inner) => Self::expr_mentions_name(inner, target),
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right)
            | Expr::Access(left, right)
            | Expr::OptionalAccess(left, right) => {
                Self::expr_mentions_name(left, target) || Self::expr_mentions_name(right, target)
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                Self::expr_mentions_name(condition, target)
                    || Self::expr_mentions_name(then_expr, target)
                    || Self::expr_mentions_name(else_expr, target)
            }
            Expr::List(items) => items.iter().any(|item| Self::expr_mentions_name(item, target)),
            Expr::Map(pairs) => pairs
                .iter()
                .any(|(key, value)| Self::expr_mentions_name(key, target) || Self::expr_mentions_name(value, target)),
            Expr::Range { start, end, step, .. } => {
                start
                    .as_deref()
                    .is_some_and(|expr| Self::expr_mentions_name(expr, target))
                    || end
                        .as_deref()
                        .is_some_and(|expr| Self::expr_mentions_name(expr, target))
                    || step
                        .as_deref()
                        .is_some_and(|expr| Self::expr_mentions_name(expr, target))
            }
            Expr::TemplateString(parts) => parts.iter().any(|part| match part {
                crate::expr::TemplateStringPart::Literal(_) => false,
                crate::expr::TemplateStringPart::Expr(expr) => Self::expr_mentions_name(expr, target),
            }),
            Expr::Call(_, args) => args.iter().any(|arg| Self::expr_mentions_name(arg, target)),
            Expr::CallExpr(callee, args) => {
                Self::expr_mentions_name(callee, target) || args.iter().any(|arg| Self::expr_mentions_name(arg, target))
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                Self::expr_mentions_name(callee, target)
                    || pos_args.iter().any(|arg| Self::expr_mentions_name(arg, target))
                    || named_args
                        .iter()
                        .any(|(_, expr)| Self::expr_mentions_name(expr, target))
            }
            Expr::Match { value, arms } => {
                Self::expr_mentions_name(value, target)
                    || arms.iter().any(|arm| Self::expr_mentions_name(&arm.body, target))
            }
            Expr::StructLiteral { fields, .. } => fields.iter().any(|(_, expr)| Self::expr_mentions_name(expr, target)),
            Expr::Val(_) | Expr::Closure { .. } | Expr::Select { .. } => false,
        }
    }

    fn expr_is_loop_cacheable(expr: &Expr) -> bool {
        match expr {
            Expr::Val(_) | Expr::Var(_) => true,
            Expr::Paren(inner) | Expr::Unary(_, inner) => Self::expr_is_loop_cacheable(inner),
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right) => {
                Self::expr_is_loop_cacheable(left) && Self::expr_is_loop_cacheable(right)
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                Self::expr_is_loop_cacheable(condition)
                    && Self::expr_is_loop_cacheable(then_expr)
                    && Self::expr_is_loop_cacheable(else_expr)
            }
            _ => false,
        }
    }

    fn collect_for_pattern_names(pattern: &ForPattern, out: &mut HashSet<String>) {
        match pattern {
            ForPattern::Variable(name) => {
                out.insert(name.clone());
            }
            ForPattern::Ignore => {}
            ForPattern::Tuple(patterns) | ForPattern::Array { patterns, rest: None } => {
                for pattern in patterns {
                    Self::collect_for_pattern_names(pattern, out);
                }
            }
            ForPattern::Array {
                patterns,
                rest: Some(rest),
            } => {
                for pattern in patterns {
                    Self::collect_for_pattern_names(pattern, out);
                }
                out.insert(rest.clone());
            }
            ForPattern::Object(entries) => {
                for (_, pattern) in entries {
                    Self::collect_for_pattern_names(pattern, out);
                }
            }
        }
    }

    fn expr_worth_loop_hoisting(expr: &Expr) -> bool {
        match expr {
            Expr::Bin(_, _, _)
            | Expr::Unary(_, _)
            | Expr::Conditional(_, _, _)
            | Expr::And(_, _)
            | Expr::Or(_, _)
            | Expr::NullishCoalescing(_, _) => true,
            Expr::Paren(inner) => Self::expr_worth_loop_hoisting(inner),
            _ => false,
        }
    }

    fn collect_loop_invariant_exprs_from_expr(
        &self,
        expr: &Expr,
        loop_names: &HashSet<String>,
        body: &Stmt,
        out: &mut Vec<Expr>,
    ) {
        if Self::expr_worth_loop_hoisting(expr) && Self::expr_is_loop_cacheable(expr) {
            let names = expr.requested_ctx();
            let safe = !names.is_empty()
                && names.iter().all(|name| {
                    !loop_names.contains(name) && self.lookup(name).is_some() && !Self::stmt_assigns_name(body, name)
                });
            if safe {
                if !out.iter().any(|existing| existing == expr) {
                    out.push(expr.clone());
                }
                return;
            }
        }

        match expr {
            Expr::Paren(inner) | Expr::Unary(_, inner) => {
                self.collect_loop_invariant_exprs_from_expr(inner, loop_names, body, out);
            }
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right)
            | Expr::Access(left, right)
            | Expr::OptionalAccess(left, right) => {
                self.collect_loop_invariant_exprs_from_expr(left, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_expr(right, loop_names, body, out);
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                self.collect_loop_invariant_exprs_from_expr(condition, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_expr(then_expr, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_expr(else_expr, loop_names, body, out);
            }
            Expr::List(items) => {
                for item in items {
                    self.collect_loop_invariant_exprs_from_expr(item, loop_names, body, out);
                }
            }
            Expr::Map(pairs) => {
                for (key, value) in pairs {
                    self.collect_loop_invariant_exprs_from_expr(key, loop_names, body, out);
                    self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                }
            }
            Expr::Range { start, end, step, .. } => {
                if let Some(expr) = start {
                    self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                }
                if let Some(expr) = end {
                    self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                }
                if let Some(expr) = step {
                    self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                }
            }
            Expr::TemplateString(parts) => {
                for part in parts {
                    if let crate::expr::TemplateStringPart::Expr(expr) = part {
                        self.collect_loop_invariant_exprs_from_expr(expr, loop_names, body, out);
                    }
                }
            }
            Expr::Call(_, args) => {
                for arg in args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
            }
            Expr::CallExpr(callee, args) => {
                self.collect_loop_invariant_exprs_from_expr(callee, loop_names, body, out);
                for arg in args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                self.collect_loop_invariant_exprs_from_expr(callee, loop_names, body, out);
                for arg in pos_args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
                for (_, arg) in named_args {
                    self.collect_loop_invariant_exprs_from_expr(arg, loop_names, body, out);
                }
            }
            Expr::Match { value, arms } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                for arm in arms {
                    self.collect_loop_invariant_exprs_from_expr(&arm.body, loop_names, body, out);
                }
            }
            Expr::StructLiteral { fields, .. } => {
                for (_, value) in fields {
                    self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                }
            }
            Expr::Val(_) | Expr::Var(_) | Expr::Closure { .. } | Expr::Select { .. } => {}
        }
    }

    fn collect_loop_invariant_exprs_from_stmt(
        &self,
        stmt: &Stmt,
        loop_names: &HashSet<String>,
        body: &Stmt,
        out: &mut Vec<Expr>,
    ) {
        match stmt {
            Stmt::Block { statements } => {
                for statement in statements {
                    self.collect_loop_invariant_exprs_from_stmt(statement, loop_names, body, out);
                }
            }
            Stmt::Let { value, .. }
            | Stmt::Assign { value, .. }
            | Stmt::Define { value, .. }
            | Stmt::Expr(value)
            | Stmt::Return { value: Some(value) }
            | Stmt::CompoundAssign { value, .. } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                self.collect_loop_invariant_exprs_from_expr(condition, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(then_stmt, loop_names, body, out);
                if let Some(else_stmt) = else_stmt {
                    self.collect_loop_invariant_exprs_from_stmt(else_stmt, loop_names, body, out);
                }
            }
            Stmt::While {
                condition,
                body: nested_body,
            } => {
                self.collect_loop_invariant_exprs_from_expr(condition, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(nested_body, loop_names, body, out);
            }
            Stmt::For {
                iterable,
                body: nested_body,
                ..
            } => {
                self.collect_loop_invariant_exprs_from_expr(iterable, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(nested_body, loop_names, body, out);
            }
            Stmt::IfLet {
                value,
                then_stmt,
                else_stmt,
                ..
            } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(then_stmt, loop_names, body, out);
                if let Some(else_stmt) = else_stmt {
                    self.collect_loop_invariant_exprs_from_stmt(else_stmt, loop_names, body, out);
                }
            }
            Stmt::WhileLet {
                value, body: then_stmt, ..
            } => {
                self.collect_loop_invariant_exprs_from_expr(value, loop_names, body, out);
                self.collect_loop_invariant_exprs_from_stmt(then_stmt, loop_names, body, out);
            }
            Stmt::Function { .. } | Stmt::Impl { .. } => {}
            Stmt::Return { value: None }
            | Stmt::Break
            | Stmt::Continue
            | Stmt::Import { .. }
            | Stmt::Struct { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Trait { .. }
            | Stmt::Empty => {}
        }
    }

    fn collect_loop_invariant_exprs(&self, pattern: &ForPattern, body: &Stmt) -> Vec<Expr> {
        let mut loop_names = HashSet::new();
        Self::collect_for_pattern_names(pattern, &mut loop_names);
        let mut exprs = Vec::new();
        self.collect_loop_invariant_exprs_from_stmt(body, &loop_names, body, &mut exprs);
        exprs
    }

    fn try_emit_simple_self_call(&mut self, name: &str, value: &Expr) -> bool {
        let Some(dst) = self.lookup(name) else {
            return false;
        };
        let Some((func_name, args)) = Self::call_expr_name(value) else {
            return false;
        };
        if !self.call_safe_to_fold(func_name) {
            return false;
        }

        let Some(Val::Closure(closure)) = self.const_env.get(func_name) else {
            return false;
        };
        if !closure.named_params.is_empty() || closure.params.len() != args.len() {
            return false;
        }

        let params = closure.params.clone();
        let body = closure.body.clone();
        let captures = closure.captures.clone();
        let Some(ret) = Self::simple_return_expr(body.as_ref()) else {
            return false;
        };
        let Expr::Bin(left, op, right) = ret else {
            return false;
        };

        let Some(left_arg) = Self::simple_call_operand(params.as_ref(), args, left.as_ref()) else {
            return false;
        };
        let Expr::Var(left_name) = left_arg else {
            return false;
        };
        if left_name != name {
            return false;
        }

        let Some(right_arg) = Self::simple_call_operand(params.as_ref(), args, right.as_ref()) else {
            return false;
        };

        if matches!(op, BinOp::Add)
            && let Some(imm) = self
                .try_small_int_const(right_arg)
                .or_else(|| Self::small_int_from_capture(captures.as_ref(), right_arg))
        {
            self.emit(Op::AddIntImm(dst, dst, imm));
            return true;
        }
        if matches!(op, BinOp::Add)
            && let Expr::Var(rhs_name) = right_arg
            && let Some(rhs) = self.lookup(rhs_name)
        {
            if self.int_regs.contains(&dst) && self.int_regs.contains(&rhs) {
                self.emit(Op::AddInt(dst, dst, rhs));
            } else {
                self.emit(Op::Add(dst, dst, rhs));
            }
            return true;
        }
        if matches!(op, BinOp::Sub)
            && let Some(imm) = self
                .try_small_int_const(right_arg)
                .or_else(|| Self::small_int_from_capture(captures.as_ref(), right_arg))
            && let Some(neg) = imm.checked_neg()
            && (-128..=127).contains(&neg)
        {
            self.emit(Op::AddIntImm(dst, dst, neg));
            return true;
        }
        if matches!(op, BinOp::Sub)
            && let Expr::Var(rhs_name) = right_arg
            && let Some(rhs) = self.lookup(rhs_name)
        {
            if self.int_regs.contains(&dst) && self.int_regs.contains(&rhs) {
                self.emit(Op::SubInt(dst, dst, rhs));
            } else {
                self.emit(Op::Sub(dst, dst, rhs));
            }
            return true;
        }

        false
    }

    fn try_emit_immediate_closure_factory_call_pair(&mut self, first: &Stmt, second: &Stmt) -> bool {
        let Stmt::Let {
            pattern: Pattern::Variable(closure_name),
            value: factory_call,
            is_const: false,
            ..
        } = first
        else {
            return false;
        };
        let Stmt::Assign {
            name: dst_name,
            value: call_value,
            ..
        } = second
        else {
            return false;
        };
        let Some(dst) = self.lookup(dst_name) else {
            return false;
        };

        let Some((factory_name, factory_args)) = Self::call_expr_name(factory_call) else {
            return false;
        };
        if factory_args.len() != 1 || !self.call_safe_to_fold(factory_name) {
            return false;
        }

        let Some(Val::Closure(factory)) = self.const_env.get(factory_name).cloned() else {
            return false;
        };
        if !factory.named_params.is_empty() || factory.params.len() != 1 {
            return false;
        }

        let Some(returned) = Self::simple_return_expr(factory.body.as_ref()) else {
            return false;
        };
        let Expr::Closure { params, body } = returned else {
            return false;
        };
        if params.len() != 1 {
            return false;
        }

        let Some((callee_name, call_args)) = Self::call_expr_name(call_value) else {
            return false;
        };
        if callee_name != closure_name || call_args.len() != 1 {
            return false;
        }
        let Expr::Var(call_arg_name) = call_args[0].as_ref() else {
            return false;
        };
        if call_arg_name != dst_name {
            return false;
        }

        let Expr::Bin(left, op, right) = body.as_ref() else {
            return false;
        };
        let Expr::Var(left_name) = left.as_ref() else {
            return false;
        };
        if left_name != &params[0] {
            return false;
        }
        let Expr::Var(right_name) = right.as_ref() else {
            return false;
        };
        if right_name != &factory.params[0] {
            return false;
        }

        let capture_reg = match factory_args[0].as_ref() {
            Expr::Var(name) => self.lookup(name).unwrap_or_else(|| self.expr(factory_args[0].as_ref())),
            expr => self.expr(expr),
        };
        match op {
            BinOp::Add => self.emit(Op::AddInt(dst, dst, capture_reg)),
            BinOp::Sub => self.emit(Op::SubInt(dst, dst, capture_reg)),
            _ => return false,
        }
        true
    }

    fn cached_loop_call_assignment<'a>(&self, body: &'a Stmt) -> Option<(&'a str, &'a Expr)> {
        let stmt = match body {
            Stmt::Block { statements } if statements.len() == 1 => statements[0].as_ref(),
            other => other,
        };
        let Stmt::Assign { name, value, .. } = stmt else {
            return None;
        };
        let (func_name, args) = Self::call_expr_name(value)?;
        let Some(Val::Closure(closure)) = self.const_env.get(func_name) else {
            return None;
        };
        if !self.closure_safe_for_known_eval(func_name, closure.as_ref()) {
            return None;
        }
        if args
            .iter()
            .any(|arg| Self::expr_mentions_name(arg, name) || !Self::expr_is_loop_cacheable(arg))
        {
            return None;
        }
        Some((name.as_str(), value.as_ref()))
    }

    fn emit_cached_loop_call_assignment(&mut self, target: &str, value: &Expr, flag_reg: u16, cache_reg: u16) -> bool {
        let Some(target_reg) = self.lookup(target) else {
            return false;
        };
        self.forget_known_value(target);

        let j_compute = self.code.len();
        self.emit(Op::JmpFalse(flag_reg, 0));
        let j_assign = self.code.len();
        self.emit(Op::Jmp(0));

        let compute_pos = self.code.len();
        if let Op::JmpFalse(_, ref mut ofs) = self.code[j_compute] {
            *ofs = (compute_pos as isize - j_compute as isize) as i16;
        }

        let rv = self.expr(value);
        if rv != cache_reg {
            self.emit(Op::Move(cache_reg, rv));
        }
        let true_idx = self.k(Val::Bool(true));
        self.emit(Op::LoadK(flag_reg, true_idx));

        let assign_pos = self.code.len();
        if let Op::Jmp(ref mut ofs) = self.code[j_assign] {
            *ofs = (assign_pos as isize - j_assign as isize) as i16;
        }
        self.store_named(target, target_reg, cache_reg);
        true
    }

    fn cached_loop_delta<'a>(&self, body: &'a Stmt) -> Option<(&'a str, &'a Expr, Vec<Box<Stmt>>)> {
        let Stmt::Block { statements } = body else {
            return None;
        };
        let (last, prefix) = statements.split_last()?;
        let (name, value) = match last.as_ref() {
            Stmt::CompoundAssign {
                name,
                op: BinOp::Add,
                value,
                ..
            } => (name.as_str(), value.as_ref()),
            Stmt::Assign { name, value, .. } => {
                let Expr::Bin(left, BinOp::Add, right) = value.as_ref() else {
                    return None;
                };
                let Expr::Var(left_name) = left.as_ref() else {
                    return None;
                };
                if left_name != name {
                    return None;
                }
                (name.as_str(), right.as_ref())
            }
            _ => return None,
        };
        let mut locals = HashSet::new();
        for stmt in prefix {
            if Self::stmt_mentions_name(stmt, name) {
                return None;
            }
            if !Self::stmt_is_cache_prefix_only(stmt, &mut locals) {
                return None;
            }
        }
        if locals.contains(name)
            || !Self::expr_is_cache_prefix_pure(value, &locals)
            || Self::expr_mentions_name(value, name)
        {
            return None;
        }
        Some((name, value, prefix.to_vec()))
    }

    fn range_count_accumulator_delta(body: &Stmt) -> Option<(&str, i16)> {
        let stmt = match body {
            Stmt::Block { statements } if statements.len() == 1 => statements[0].as_ref(),
            other => other,
        };
        match stmt {
            Stmt::CompoundAssign {
                name,
                op: BinOp::Add,
                value,
                ..
            } => {
                let Expr::Val(Val::Int(delta)) = value.as_ref() else {
                    return None;
                };
                i16::try_from(*delta).ok().map(|delta| (name.as_str(), delta))
            }
            Stmt::CompoundAssign {
                name,
                op: BinOp::Sub,
                value,
                ..
            } => {
                let Expr::Val(Val::Int(delta)) = value.as_ref() else {
                    return None;
                };
                let delta = delta.checked_neg().and_then(|value| i16::try_from(value).ok())?;
                Some((name.as_str(), delta))
            }
            Stmt::Assign { name, value, .. } => {
                let Expr::Bin(left, op @ (BinOp::Add | BinOp::Sub), right) = value.as_ref() else {
                    return None;
                };
                let Expr::Var(left_name) = left.as_ref() else {
                    return None;
                };
                if left_name != name {
                    return None;
                }
                let Expr::Val(Val::Int(delta)) = right.as_ref() else {
                    return None;
                };
                let delta = match op {
                    BinOp::Add => i16::try_from(*delta).ok()?,
                    BinOp::Sub => delta.checked_neg().and_then(|value| i16::try_from(value).ok())?,
                    _ => unreachable!(),
                };
                Some((name.as_str(), delta))
            }
            _ => None,
        }
    }

    fn try_emit_range_count_accumulator(
        &mut self,
        pattern: &ForPattern,
        body: &Stmt,
        idx: u16,
        limit: u16,
        step: u16,
        inclusive: bool,
        explicit: bool,
    ) -> bool {
        if explicit || !matches!(pattern, ForPattern::Ignore) {
            return false;
        }
        let Some((target, imm)) = Self::range_count_accumulator_delta(body) else {
            return false;
        };
        let Some(target_reg) = self.lookup(target) else {
            return false;
        };
        self.forget_known_value(target);
        self.emit(Op::AddRangeCountImm {
            target: target_reg,
            idx,
            limit,
            step,
            inclusive,
            explicit,
            imm,
        });
        true
    }

    fn stmt_mentions_name(stmt: &Stmt, target: &str) -> bool {
        match stmt {
            Stmt::Block { statements } => statements.iter().any(|stmt| Self::stmt_mentions_name(stmt, target)),
            Stmt::Let { pattern, value, .. } => {
                matches!(pattern, Pattern::Variable(name) if name == target) || Self::expr_mentions_name(value, target)
            }
            Stmt::Assign { name, value, .. } | Stmt::CompoundAssign { name, value, .. } => {
                name == target || Self::expr_mentions_name(value, target)
            }
            Stmt::Expr(expr) | Stmt::Return { value: Some(expr) } => Self::expr_mentions_name(expr, target),
            Stmt::Return { value: None } | Stmt::Break | Stmt::Continue | Stmt::Empty => false,
            Stmt::Define { name, value } => name == target || Self::expr_mentions_name(value, target),
            Stmt::Function { name, body, .. } => name == target || Self::stmt_mentions_name(body, target),
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                Self::expr_mentions_name(condition, target)
                    || Self::stmt_mentions_name(then_stmt, target)
                    || else_stmt
                        .as_deref()
                        .is_some_and(|stmt| Self::stmt_mentions_name(stmt, target))
            }
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => {
                Self::pattern_mentions_name(pattern, target)
                    || Self::expr_mentions_name(value, target)
                    || Self::stmt_mentions_name(then_stmt, target)
                    || else_stmt
                        .as_deref()
                        .is_some_and(|stmt| Self::stmt_mentions_name(stmt, target))
            }
            Stmt::While { condition, body } => {
                Self::expr_mentions_name(condition, target) || Self::stmt_mentions_name(body, target)
            }
            Stmt::WhileLet { pattern, value, body } => {
                Self::pattern_mentions_name(pattern, target)
                    || Self::expr_mentions_name(value, target)
                    || Self::stmt_mentions_name(body, target)
            }
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                Self::for_pattern_mentions_name(pattern, target)
                    || Self::expr_mentions_name(iterable, target)
                    || Self::stmt_mentions_name(body, target)
            }
            Stmt::Struct { name, .. } | Stmt::TypeAlias { name, .. } | Stmt::Trait { name, .. } => name == target,
            Stmt::Impl {
                trait_name, methods, ..
            } => trait_name == target || methods.iter().any(|stmt| Self::stmt_mentions_name(stmt, target)),
            Stmt::Import(_) => false,
        }
    }

    fn pattern_mentions_name(pattern: &Pattern, target: &str) -> bool {
        match pattern {
            Pattern::Variable(name) => name == target,
            Pattern::Wildcard | Pattern::Literal(_) => false,
            Pattern::Range { start, end, .. } => {
                Self::expr_mentions_name(start, target) || Self::expr_mentions_name(end, target)
            }
            Pattern::List { patterns, rest } => {
                rest.as_deref() == Some(target)
                    || patterns
                        .iter()
                        .any(|pattern| Self::pattern_mentions_name(pattern, target))
            }
            Pattern::Or(patterns) => patterns
                .iter()
                .any(|pattern| Self::pattern_mentions_name(pattern, target)),
            Pattern::Map { patterns, rest } => {
                rest.as_deref() == Some(target)
                    || patterns
                        .iter()
                        .any(|(_, pattern)| Self::pattern_mentions_name(pattern, target))
            }
            Pattern::Guard { pattern, guard } => {
                Self::pattern_mentions_name(pattern, target) || Self::expr_mentions_name(guard, target)
            }
        }
    }

    fn for_pattern_mentions_name(pattern: &ForPattern, target: &str) -> bool {
        match pattern {
            ForPattern::Variable(name) => name == target,
            ForPattern::Ignore => false,
            ForPattern::Tuple(patterns) | ForPattern::Array { patterns, rest: None } => patterns
                .iter()
                .any(|pattern| Self::for_pattern_mentions_name(pattern, target)),
            ForPattern::Array {
                patterns,
                rest: Some(rest),
            } => {
                rest == target
                    || patterns
                        .iter()
                        .any(|pattern| Self::for_pattern_mentions_name(pattern, target))
            }
            ForPattern::Object(entries) => entries
                .iter()
                .any(|(_, pattern)| Self::for_pattern_mentions_name(pattern, target)),
        }
    }

    fn stmt_is_cache_prefix_only(stmt: &Stmt, locals: &mut HashSet<String>) -> bool {
        match stmt {
            Stmt::Block { statements } => {
                let mut scoped = locals.clone();
                statements
                    .iter()
                    .all(|stmt| Self::stmt_is_cache_prefix_only(stmt, &mut scoped))
            }
            Stmt::Let {
                pattern: Pattern::Variable(name),
                value,
                ..
            } => {
                if !Self::expr_is_cache_prefix_pure(value, locals) {
                    return false;
                }
                locals.insert(name.clone());
                true
            }
            Stmt::Assign { name, value, .. } | Stmt::CompoundAssign { name, value, .. } => {
                locals.contains(name) && Self::expr_is_cache_prefix_pure(value, locals)
            }
            Stmt::Expr(expr) => Self::expr_stmt_is_cache_prefix_only(expr, locals),
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                if !Self::expr_is_cache_prefix_pure(iterable, locals) {
                    return false;
                }
                let mut scoped = locals.clone();
                if !Self::bind_for_pattern_names(pattern, &mut scoped) {
                    return false;
                }
                Self::stmt_is_cache_prefix_only(body, &mut scoped)
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                Self::expr_is_cache_prefix_pure(condition, locals)
                    && Self::stmt_is_cache_prefix_only(then_stmt, &mut locals.clone())
                    && else_stmt
                        .as_deref()
                        .map(|stmt| Self::stmt_is_cache_prefix_only(stmt, &mut locals.clone()))
                        .unwrap_or(true)
            }
            Stmt::Empty => true,
            _ => false,
        }
    }

    fn expr_stmt_is_cache_prefix_only(expr: &Expr, locals: &HashSet<String>) -> bool {
        if let Expr::CallExpr(callee, args) = expr
            && let Expr::Access(obj, field) = callee.as_ref()
            && let Expr::Var(receiver) = obj.as_ref()
            && locals.contains(receiver)
            && let Expr::Val(Val::Str(method)) = field.as_ref()
            && matches!(method.as_ref(), "push" | "set")
        {
            return args.iter().all(|arg| Self::expr_is_cache_prefix_pure(arg, locals));
        }
        Self::expr_is_cache_prefix_pure(expr, locals)
    }

    fn expr_is_cache_prefix_pure(expr: &Expr, locals: &HashSet<String>) -> bool {
        match expr {
            Expr::Val(_) | Expr::Var(_) => true,
            Expr::Paren(inner) | Expr::Unary(_, inner) => Self::expr_is_cache_prefix_pure(inner, locals),
            Expr::Bin(left, _, right)
            | Expr::And(left, right)
            | Expr::Or(left, right)
            | Expr::NullishCoalescing(left, right)
            | Expr::Access(left, right)
            | Expr::OptionalAccess(left, right) => {
                Self::expr_is_cache_prefix_pure(left, locals) && Self::expr_is_cache_prefix_pure(right, locals)
            }
            Expr::Conditional(condition, then_expr, else_expr) => {
                Self::expr_is_cache_prefix_pure(condition, locals)
                    && Self::expr_is_cache_prefix_pure(then_expr, locals)
                    && Self::expr_is_cache_prefix_pure(else_expr, locals)
            }
            Expr::List(items) => items.iter().all(|item| Self::expr_is_cache_prefix_pure(item, locals)),
            Expr::Map(pairs) => pairs.iter().all(|(key, value)| {
                Self::expr_is_cache_prefix_pure(key, locals) && Self::expr_is_cache_prefix_pure(value, locals)
            }),
            Expr::Range { start, end, step, .. } => {
                start
                    .as_deref()
                    .map(|expr| Self::expr_is_cache_prefix_pure(expr, locals))
                    .unwrap_or(true)
                    && end
                        .as_deref()
                        .map(|expr| Self::expr_is_cache_prefix_pure(expr, locals))
                        .unwrap_or(true)
                    && step
                        .as_deref()
                        .map(|expr| Self::expr_is_cache_prefix_pure(expr, locals))
                        .unwrap_or(true)
            }
            Expr::TemplateString(parts) => parts.iter().all(|part| match part {
                crate::expr::TemplateStringPart::Literal(_) => true,
                crate::expr::TemplateStringPart::Expr(expr) => Self::expr_is_cache_prefix_pure(expr, locals),
            }),
            Expr::CallExpr(callee, args) => {
                if let Expr::Access(obj, field) = callee.as_ref()
                    && let Expr::Var(receiver) = obj.as_ref()
                    && locals.contains(receiver)
                    && let Expr::Val(Val::Str(method)) = field.as_ref()
                    && matches!(method.as_ref(), "values" | "keys" | "len")
                {
                    return args.is_empty();
                }
                false
            }
            _ => false,
        }
    }

    fn emit_cached_loop_delta(
        &mut self,
        target: &str,
        value: &Expr,
        prefix: &[Box<Stmt>],
        flag_reg: u16,
        cache_reg: u16,
    ) -> bool {
        let Some(target_reg) = self.lookup(target) else {
            return false;
        };
        self.forget_known_value(target);

        let j_compute = self.code.len();
        self.emit(Op::JmpFalse(flag_reg, 0));
        let j_add = self.code.len();
        self.emit(Op::Jmp(0));

        let compute_pos = self.code.len();
        if let Op::JmpFalse(_, ref mut ofs) = self.code[j_compute] {
            *ofs = (compute_pos as isize - j_compute as isize) as i16;
        }

        self.push_var_scope();
        self.with_const_scope(|builder| {
            for stmt in prefix {
                builder.stmt(stmt);
            }
        });
        let delta = self.expr(value);
        self.pop_var_scope();

        if delta != cache_reg {
            self.emit(Op::Move(cache_reg, delta));
        }
        let true_idx = self.k(Val::Bool(true));
        self.emit(Op::LoadK(flag_reg, true_idx));

        let add_pos = self.code.len();
        if let Op::Jmp(ref mut ofs) = self.code[j_add] {
            *ofs = (add_pos as isize - j_add as isize) as i16;
        }

        if self.int_regs.contains(&target_reg) && self.int_regs.contains(&cache_reg) {
            self.emit(Op::AddInt(target_reg, target_reg, cache_reg));
        } else {
            self.emit(Op::Add(target_reg, target_reg, cache_reg));
        }
        true
    }

    fn try_emit_simple_self_assign(&mut self, name: &str, value: &Expr) -> bool {
        if self.try_emit_simple_self_call(name, value) {
            return true;
        }

        let Some(dst) = self.lookup(name) else {
            return false;
        };
        let Expr::Bin(left, op, right) = value else {
            return false;
        };
        let Expr::Var(left_name) = left.as_ref() else {
            return false;
        };
        if left_name != name {
            return false;
        }

        if matches!(op, BinOp::Add)
            && let Some(imm) = self.try_small_int_const(right)
        {
            self.emit(Op::AddIntImm(dst, dst, imm));
            return true;
        }
        if matches!(op, BinOp::Sub)
            && let Some(imm) = self.try_small_int_const(right)
            && let Some(neg) = imm.checked_neg()
            && (-128..=127).contains(&neg)
        {
            self.emit(Op::AddIntImm(dst, dst, neg));
            return true;
        }

        match (op, right.as_ref()) {
            (BinOp::Add, Expr::Val(Val::Int(imm))) if (-128..=127).contains(imm) => {
                self.emit(Op::AddIntImm(dst, dst, *imm as i16));
                true
            }
            (BinOp::Sub, Expr::Val(Val::Int(imm)))
                if imm.checked_neg().is_some_and(|neg| (-128..=127).contains(&neg)) =>
            {
                self.emit(Op::AddIntImm(dst, dst, (-*imm) as i16));
                true
            }
            (BinOp::Add, Expr::Var(rhs_name)) => {
                if let Some(rhs) = self.lookup(rhs_name) {
                    self.emit(Op::AddInt(dst, dst, rhs));
                    true
                } else {
                    false
                }
            }
            (BinOp::Sub, Expr::Var(rhs_name)) => {
                if let Some(rhs) = self.lookup(rhs_name) {
                    self.emit(Op::SubInt(dst, dst, rhs));
                    true
                } else {
                    false
                }
            }
            // Compound self-assign: x = x OP <complex_expr>
            // Evaluate RHS into a temp, then emit in-place binary op.
            // Only add/sub/mul have dedicated Int opcodes; div/mod fall back.
            (BinOp::Add, _) => {
                let rhs_reg = self.expr(right);
                self.emit(Op::AddInt(dst, dst, rhs_reg));
                true
            }
            (BinOp::Sub, _) => {
                let rhs_reg = self.expr(right);
                self.emit(Op::SubInt(dst, dst, rhs_reg));
                true
            }
            (BinOp::Mul, _) => {
                let rhs_reg = self.expr(right);
                self.emit(Op::MulInt(dst, dst, rhs_reg));
                true
            }
            _ => false,
        }
    }

    fn declare_for_pattern(&mut self, pattern: &ForPattern) {
        match pattern {
            ForPattern::Variable(name) => {
                self.define_scoped_var(name);
            }
            ForPattern::Ignore => {}
            ForPattern::Tuple(patterns) => {
                for pat in patterns {
                    self.declare_for_pattern(pat);
                }
            }
            ForPattern::Array { patterns, rest } => {
                for pat in patterns {
                    self.declare_for_pattern(pat);
                }
                if let Some(name) = rest {
                    self.define_scoped_var(name);
                }
            }
            ForPattern::Object(entries) => {
                for (_, pat) in entries {
                    self.declare_for_pattern(pat);
                }
            }
        }
    }

    fn pattern_requires_check(pattern: &ForPattern) -> bool {
        !matches!(pattern, ForPattern::Variable(_) | ForPattern::Ignore)
    }

    fn pattern_from_for(pattern: &ForPattern) -> Pattern {
        match pattern {
            ForPattern::Variable(name) => Pattern::Variable(name.clone()),
            ForPattern::Ignore => Pattern::Wildcard,
            ForPattern::Tuple(patterns) => Pattern::List {
                patterns: patterns.iter().map(Self::pattern_from_for).collect(),
                rest: None,
            },
            ForPattern::Array { patterns, rest } => Pattern::List {
                patterns: patterns.iter().map(Self::pattern_from_for).collect(),
                rest: rest.clone(),
            },
            ForPattern::Object(entries) => Pattern::Map {
                patterns: entries
                    .iter()
                    .map(|(k, p)| (k.clone(), Self::pattern_from_for(p)))
                    .collect(),
                rest: None,
            },
        }
    }

    /// Try to lower a simple `while (i < N) { body; i = i + 1; }` loop into a for-range loop.
    /// This enables BC32 packing and uses the efficient ForRangeState instead of Val-based
    /// comparison/increment for each iteration. Only applied when the body is simple enough
    /// that for-range overhead (3 words/tags per iteration) beats peephole-fused while (1 dispatch).
    fn try_lower_while_to_for_range(&mut self, condition: &Expr, body: &Stmt) -> bool {
        // Match: condition is `counter < limit` where counter is a local var.
        let (counter_name, limit_val) = match condition {
            Expr::Bin(left, BinOp::Lt, right) => {
                let Expr::Var(name) = left.as_ref() else {
                    return false;
                };
                let Expr::Val(Val::Int(n)) = right.as_ref() else {
                    return false;
                };
                (name.as_str(), *n)
            }
            _ => return false,
        };

        // The counter must be a known local variable.
        let counter_reg = match self.lookup(counter_name) {
            Some(r) => r,
            None => return false,
        };

        // Check body ends with increment.
        fn body_ends_with_inc(s: &Stmt, counter_name: &str) -> bool {
            match s {
                Stmt::Block { statements } => statements.last().is_some_and(|s| body_ends_with_inc(s, counter_name)),
                Stmt::Assign { name, value, .. } => {
                    name == counter_name
                        && matches!(
                            value.as_ref(),
                            Expr::Bin(left, BinOp::Add, right)
                                if matches!(left.as_ref(), Expr::Var(n) if n == counter_name)
                                    && matches!(right.as_ref(), Expr::Val(Val::Int(1)))
                        )
                }
                Stmt::CompoundAssign { name, op, value, .. } => {
                    name == counter_name && matches!(op, BinOp::Add) && matches!(value.as_ref(), Expr::Val(Val::Int(1)))
                }
                _ => false,
            }
        }

        if !body_ends_with_inc(body, counter_name) {
            return false;
        }

        // Only apply for very simple bodies — for-range has 2-word BC32 encoding
        // overhead per iteration vs peephole-fused while's 1 dispatch.
        fn ops_in_body(s: &Stmt, counter_name: &str) -> usize {
            match s {
                Stmt::Block { statements } => statements.iter().map(|s| ops_in_body(s, counter_name)).sum(),
                Stmt::Assign { name, .. } | Stmt::CompoundAssign { name, .. } if name == counter_name => 0,
                Stmt::Expr(_) => 1,
                Stmt::Assign { .. } | Stmt::Let { .. } | Stmt::CompoundAssign { .. } => 1,
                _ => 8,
            }
        }
        if ops_in_body(body, counter_name) > 6 {
            return false;
        }

        // ── Lower to for-range ──
        let limit_reg = self.alloc();
        let limit_k = self.k(Val::Int(limit_val));
        self.emit(Op::LoadK(limit_reg, limit_k));

        let step_reg = self.alloc();

        self.emit(Op::ForRangePrep {
            idx: counter_reg,
            limit: limit_reg,
            step: step_reg,
            inclusive: false,
            explicit: false,
        });

        let guard_pos = self.code.len();
        self.emit(Op::ForRangeLoop {
            idx: counter_reg,
            limit: limit_reg,
            step: step_reg,
            inclusive: false,
            write_idx: true,
            ofs: 0,
        });

        // Emit body WITHOUT the trailing increment.
        let saved_breaks = std::mem::take(&mut self.break_locations);
        let saved_conts = std::mem::take(&mut self.continue_locations);
        self.loop_depth = self.loop_depth.saturating_add(1);

        let body_without_inc = strip_trailing_increment(body, counter_name);
        self.with_const_scope(|builder| builder.stmt(&body_without_inc));

        let pending_continues = std::mem::take(&mut self.continue_locations);
        let step_pos = self.code.len();
        self.emit(Op::ForRangeStep {
            idx: counter_reg,
            step: step_reg,
            back_ofs: 0,
        });

        let back = (guard_pos as isize - step_pos as isize) as i16;
        if let Op::ForRangeStep { back_ofs, .. } = &mut self.code[step_pos] {
            *back_ofs = back;
        }
        for loc in pending_continues {
            if let Some(Op::Continue(ofs)) = self.code.get_mut(loc) {
                *ofs = (step_pos as isize - loc as isize) as i16;
            }
        }

        let end_pos = self.code.len();
        if let Op::ForRangeLoop { ofs, .. } = &mut self.code[guard_pos] {
            *ofs = (end_pos as isize - guard_pos as isize) as i16;
        }
        let pending_breaks = std::mem::take(&mut self.break_locations);
        for loc in pending_breaks {
            if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                *ofs = (end_pos as isize - loc as isize) as i16;
            }
        }

        self.loop_depth = self.loop_depth.saturating_sub(1);
        self.break_locations = saved_breaks;
        self.continue_locations = saved_conts;

        true
    }

    fn try_precompute_range_loop(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
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
            if let Some(val) = self.const_env.get(&name) {
                self.const_bindings.insert(name, val.clone());
            }
        }
        true
    }

    fn try_elide_dead_range_loop(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
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

    fn bind_for_pattern_names(pattern: &ForPattern, locals: &mut HashSet<String>) -> bool {
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
            && let Expr::Val(Val::Str(method)) = field.as_ref()
            && method.as_ref() == "push"
        {
            return args.iter().all(|arg| Self::expr_is_local_pure(arg, locals));
        }
        if let Expr::CallExpr(callee, args) = expr
            && let Expr::Access(obj, field) = callee.as_ref()
            && let Expr::Var(receiver) = obj.as_ref()
            && locals.contains(receiver)
            && let Expr::Val(Val::Str(method)) = field.as_ref()
            && method.as_ref() == "set"
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
                    && let Expr::Val(Val::Str(method)) = field.as_ref()
                    && matches!(method.as_ref(), "values" | "keys" | "len")
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

    fn stmt_assigns_name(stmt: &Stmt, target: &str) -> bool {
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

    fn try_emit_list_fold_add(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
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

    fn try_emit_map_values_fold_add(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> bool {
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
        let (Expr::Var(map_name), Expr::Val(Val::Str(method))) = (obj_expr.as_ref(), field_expr.as_ref()) else {
            return false;
        };
        if method.as_ref() != "values" {
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

    pub fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Block { statements } => {
                self.with_const_scope(|builder| {
                    if statements.len() == 2
                        && builder.try_emit_immediate_closure_factory_call_pair(&statements[0], &statements[1])
                    {
                        return;
                    }
                    for st in statements {
                        builder.stmt(st);
                    }
                });
            }
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                if self.try_precompute_range_loop(pattern, iterable, body) {
                    return;
                }
                if self.try_elide_dead_range_loop(pattern, iterable, body) {
                    return;
                }
                if self.try_emit_list_fold_add(pattern, iterable, body) {
                    return;
                }
                if self.try_emit_map_values_fold_add(pattern, iterable, body) {
                    return;
                }
                if let Expr::Range {
                    start,
                    end,
                    inclusive,
                    step,
                } = iterable.as_ref()
                {
                    self.loop_depth = self.loop_depth.saturating_add(1);
                    let saved_breaks = std::mem::take(&mut self.break_locations);
                    let saved_conts = std::mem::take(&mut self.continue_locations);
                    let r_idx = match start {
                        Some(e) => self.expr(e),
                        None => {
                            let r = self.alloc();
                            let k0 = self.k(Val::Int(0));
                            self.emit(Op::LoadK(r, k0));
                            r
                        }
                    };
                    let r_lim = match end {
                        Some(e) => self.expr(e),
                        None => {
                            self.loop_depth = self.loop_depth.saturating_sub(1);
                            self.break_locations = saved_breaks;
                            self.continue_locations = saved_conts;
                            return;
                        }
                    };
                    let r_step = if let Some(st_expr) = step {
                        self.expr(st_expr)
                    } else {
                        self.alloc()
                    };
                    if self.try_emit_range_count_accumulator(
                        pattern,
                        body,
                        r_idx,
                        r_lim,
                        r_step,
                        *inclusive,
                        step.is_some(),
                    ) {
                        self.loop_depth = self.loop_depth.saturating_sub(1);
                        self.break_locations = saved_breaks;
                        self.continue_locations = saved_conts;
                        return;
                    }
                    let cached_loop_call = if matches!(pattern, ForPattern::Ignore) {
                        self.cached_loop_call_assignment(body)
                            .map(|(target, value)| (target.to_string(), value.clone()))
                    } else {
                        None
                    };
                    let cached_loop_delta = if matches!(pattern, ForPattern::Ignore) && cached_loop_call.is_none() {
                        self.cached_loop_delta(body)
                            .map(|(target, value, prefix)| (target.to_string(), value.clone(), prefix))
                    } else {
                        None
                    };
                    let cached_loop_regs = cached_loop_call.as_ref().map(|_| {
                        let flag = self.alloc();
                        let value = self.alloc();
                        let false_idx = self.k(Val::Bool(false));
                        self.emit(Op::LoadK(flag, false_idx));
                        (flag, value)
                    });
                    let cached_delta_regs = cached_loop_delta.as_ref().map(|_| {
                        let flag = self.alloc();
                        let value = self.alloc();
                        let false_idx = self.k(Val::Bool(false));
                        self.emit(Op::LoadK(flag, false_idx));
                        (flag, value)
                    });
                    let loop_invariant_exprs = if cached_loop_call.is_none() && cached_loop_delta.is_none() {
                        self.collect_loop_invariant_exprs(pattern, body)
                    } else {
                        Vec::new()
                    };
                    let loop_invariant_start = self.loop_invariant_expr_regs.len();
                    for expr in loop_invariant_exprs {
                        let reg = self.expr(&expr);
                        self.loop_invariant_expr_regs.push((expr, reg));
                    }
                    let direct_range_var = match pattern {
                        ForPattern::Variable(name) => !Self::stmt_assigns_name(body, name),
                        _ => false,
                    };
                    self.push_var_scope();
                    if let (true, ForPattern::Variable(name)) = (direct_range_var, pattern) {
                        self.define_var_as(name, r_idx);
                    } else {
                        self.declare_for_pattern(pattern);
                    }

                    self.emit(Op::ForRangePrep {
                        idx: r_idx,
                        limit: r_lim,
                        step: r_step,
                        inclusive: *inclusive,
                        explicit: step.is_some(),
                    });

                    let guard_pos = self.code.len();
                    self.emit(Op::ForRangeLoop {
                        idx: r_idx,
                        limit: r_lim,
                        step: r_step,
                        inclusive: *inclusive,
                        write_idx: !matches!(pattern, ForPattern::Ignore),
                        ofs: 0,
                    });

                    if Self::pattern_requires_check(pattern) {
                        let plan = Self::pattern_from_for(pattern);
                        let plan_idx = self.register_pattern_plan(&plan);
                        let match_reg = self.alloc();
                        self.emit(Op::PatternMatch {
                            dst: match_reg,
                            src: r_idx,
                            plan: plan_idx,
                        });
                        let jf = self.code.len();
                        self.emit(Op::JmpFalse(match_reg, 0));
                        let skip_pos = self.code.len();
                        self.emit(Op::Jmp(0));
                        let fail_pos = self.code.len();
                        let err_idx = self.k(Val::Str("Pattern does not match value".into()));
                        self.emit(Op::Raise { err_kidx: err_idx });
                        let after_fail = self.code.len();
                        if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                            *ofs = (fail_pos as isize - jf as isize) as i16;
                        }
                        if let Op::Jmp(ref mut ofs) = self.code[skip_pos] {
                            *ofs = (after_fail as isize - skip_pos as isize) as i16;
                        }
                    } else if !direct_range_var
                        && let ForPattern::Variable(name) = pattern
                        && let Some(idx) = self.lookup(name)
                    {
                        self.emit(Op::StoreLocal(idx, r_idx));
                    }

                    if let (Some((target, value)), Some((flag_reg, cache_reg))) =
                        (cached_loop_call.as_ref(), cached_loop_regs)
                    {
                        if !self.emit_cached_loop_call_assignment(target, value, flag_reg, cache_reg) {
                            self.with_const_scope(|builder| builder.stmt(body));
                        }
                    } else if let (Some((target, value, prefix)), Some((flag_reg, cache_reg))) =
                        (cached_loop_delta.as_ref(), cached_delta_regs)
                    {
                        if !self.emit_cached_loop_delta(target, value, prefix, flag_reg, cache_reg) {
                            self.with_const_scope(|builder| builder.stmt(body));
                        }
                    } else {
                        self.with_const_scope(|builder| builder.stmt(body));
                    }
                    self.loop_invariant_expr_regs.truncate(loop_invariant_start);

                    let pending_continues = std::mem::take(&mut self.continue_locations);
                    let step_pos = self.code.len();
                    self.emit(Op::ForRangeStep {
                        idx: r_idx,
                        step: r_step,
                        back_ofs: 0,
                    });

                    let back = (guard_pos as isize - step_pos as isize) as i16;
                    if let Op::ForRangeStep { back_ofs, .. } = &mut self.code[step_pos] {
                        *back_ofs = back;
                    }
                    for loc in pending_continues {
                        if let Some(Op::Continue(ofs)) = self.code.get_mut(loc) {
                            *ofs = (step_pos as isize - loc as isize) as i16;
                        }
                    }
                    let end_pos = self.code.len();
                    if let Op::ForRangeLoop { ofs, .. } = &mut self.code[guard_pos] {
                        *ofs = (end_pos as isize - guard_pos as isize) as i16;
                    }
                    let pending_breaks = std::mem::take(&mut self.break_locations);
                    for loc in pending_breaks {
                        if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                            *ofs = (end_pos as isize - loc as isize) as i16;
                        }
                    }
                    self.pop_var_scope();
                    self.loop_depth = self.loop_depth.saturating_sub(1);
                    self.break_locations = saved_breaks;
                    self.continue_locations = saved_conts;
                } else {
                    self.loop_depth = self.loop_depth.saturating_add(1);
                    let saved_breaks = std::mem::take(&mut self.break_locations);
                    let saved_conts = std::mem::take(&mut self.continue_locations);

                    let r_src = self.expr(iterable);
                    let r_it = self.alloc();
                    self.emit(Op::ToIter { dst: r_it, src: r_src });
                    let r_len = self.alloc();
                    self.emit(Op::Len { dst: r_len, src: r_it });
                    let r_i = self.alloc();
                    let k0 = self.k(Val::Int(0));
                    self.emit(Op::LoadK(r_i, k0));
                    let r_cmp = self.alloc();
                    let guard_pos = self.code.len();
                    self.emit(Op::CmpLt(r_cmp, r_i, r_len));
                    let jf_pos = self.code.len();
                    self.emit(Op::JmpFalse(r_cmp, 0));

                    let r_item = self.alloc();
                    self.emit(Op::Index {
                        dst: r_item,
                        base: r_it,
                        idx: r_i,
                    });

                    self.push_var_scope();
                    let simple_var_pattern = match pattern {
                        ForPattern::Variable(name) => {
                            self.define_var_as(name, r_item);
                            true
                        }
                        ForPattern::Ignore => true,
                        _ => false,
                    };
                    if !simple_var_pattern {
                        self.declare_for_pattern(pattern);
                    }
                    if Self::pattern_requires_check(pattern) {
                        let plan = Self::pattern_from_for(pattern);
                        let plan_idx = self.register_pattern_plan(&plan);
                        let match_reg = self.alloc();
                        self.emit(Op::PatternMatch {
                            dst: match_reg,
                            src: r_item,
                            plan: plan_idx,
                        });
                        let jf = self.code.len();
                        self.emit(Op::JmpFalse(match_reg, 0));
                        let skip_pos = self.code.len();
                        self.emit(Op::Jmp(0));
                        let fail_pos = self.code.len();
                        let err_idx = self.k(Val::Str("Pattern does not match value".into()));
                        self.emit(Op::Raise { err_kidx: err_idx });
                        let after_fail = self.code.len();
                        if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                            *ofs = (fail_pos as isize - jf as isize) as i16;
                        }
                        if let Op::Jmp(ref mut ofs) = self.code[skip_pos] {
                            *ofs = (after_fail as isize - skip_pos as isize) as i16;
                        }
                    }

                    self.with_const_scope(|builder| builder.stmt(body));

                    let cont_target = self.code.len();
                    let pending_continues = std::mem::take(&mut self.continue_locations);
                    self.emit(Op::AddIntImm(r_i, r_i, 1));
                    let back = (guard_pos as isize - self.code.len() as isize) as i16;
                    self.emit(Op::Jmp(back));
                    let end_pos = self.code.len();
                    if let Op::JmpFalse(_, ref mut ofs) = self.code[jf_pos] {
                        *ofs = (end_pos as isize - jf_pos as isize) as i16;
                    }
                    for loc in pending_continues {
                        if let Some(Op::Continue(ofs)) = self.code.get_mut(loc) {
                            *ofs = (cont_target as isize - loc as isize) as i16;
                        }
                    }
                    let pending_breaks = std::mem::take(&mut self.break_locations);
                    for loc in pending_breaks {
                        if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                            *ofs = (end_pos as isize - loc as isize) as i16;
                        }
                    }

                    self.loop_depth = self.loop_depth.saturating_sub(1);
                    self.pop_var_scope();
                    self.break_locations = saved_breaks;
                    self.continue_locations = saved_conts;
                }
            }
            Stmt::Define { name, value } => {
                self.global_defs.insert(name.clone());
                let idx = self.get_or_define(name);
                let rv = self.expr(value);
                self.emit(Op::StoreLocal(idx, rv));
                let kname = self.k(Val::Str(name.clone().into()));
                self.emit(Op::DefineGlobal(kname, idx));
            }
            Stmt::Let {
                pattern,
                value,
                is_const,
                ..
            } => {
                let mut const_value = self.try_eval_const_expr(value);
                if let Pattern::Variable(name) = pattern {
                    if !*is_const && let Some(specialized) = self.try_specialize_const_closure_factory(value) {
                        self.const_env.define(name.clone(), specialized.clone());
                        const_value = Some(specialized);
                    }
                    if let (false, Some(v)) = (*is_const, const_value.as_ref()) {
                        self.bind_known_value(name.clone(), v.clone());
                    }
                    if !*is_const
                        && const_value.is_none()
                        && let Expr::Closure { params, body } = value.as_ref()
                    {
                        self.register_closure_const_env(name, params, body);
                    }
                    if !*is_const {
                        let rv = if let Some(v) = const_value.clone() {
                            let dst = self.alloc();
                            if matches!(v, Val::Map(_)) {
                                self.map_locals.insert(dst);
                            }
                            let k = self.k(v);
                            self.emit(Op::LoadK(dst, k));
                            dst
                        } else {
                            self.expr(value)
                        };
                        // For simple (non-call) expressions, map the variable directly
                        // to the expression result register, eliminating the StoreLocal copy.
                        // This is unsafe for function calls because the result register
                        // may be clobbered by later call frames.
                        if !Self::expr_contains_call(value) {
                            self.define_var_as(name, rv);
                            if self.loop_depth == 0 && self.var_scope_depth() == 0 {
                                let kname = self.k(Val::Str(name.clone().into()));
                                self.emit(Op::DefineGlobal(kname, rv));
                            }
                        } else {
                            let idx = self.get_or_define(name);
                            self.store_named(name, idx, rv);
                            if self.loop_depth == 0 && self.var_scope_depth() == 0 {
                                let kname = self.k(Val::Str(name.clone().into()));
                                self.emit(Op::DefineGlobal(kname, idx));
                            }
                        }
                        return;
                    }
                }

                if *is_const {
                    self.record_const_pattern_names(pattern);
                }

                if let Pattern::Variable(name) = pattern {
                    if *is_const {
                        if let Some(v) = const_value.as_ref() {
                            let _ = self.get_or_define(name);
                            self.bind_const(name.clone(), v.clone());
                            return;
                        }
                    } else if let Some(v) = const_value.as_ref() {
                        self.const_env.define(name.clone(), v.clone());
                    }
                }

                let rv = if let Some(v) = const_value {
                    let dst = self.alloc();
                    let k = self.k(v);
                    self.emit(Op::LoadK(dst, k));
                    dst
                } else {
                    self.expr(value)
                };
                let plan_idx = self.register_pattern_plan(pattern);
                let err_idx = self.k(Val::Str("Pattern does not match value".into()));
                self.emit(Op::PatternMatchOrFail {
                    src: rv,
                    plan: plan_idx,
                    err_kidx: err_idx,
                    is_const: *is_const,
                });

                if let Pattern::Variable(name) = pattern {
                    let idx = self.get_or_define(name);
                    if self.loop_depth == 0 && self.var_scope_depth() == 0 {
                        let kname = self.k(Val::Str(name.clone().into()));
                        self.emit(Op::DefineGlobal(kname, idx));
                    }
                }
            }
            Stmt::Assign { name, value, .. } => {
                let is_const_target = self.const_names.contains(name);
                if !is_const_target && self.try_emit_simple_self_assign(name, value) {
                    return;
                }
                let const_value = if is_const_target {
                    None
                } else {
                    self.try_eval_const_expr(value)
                };
                if let Some(val) = const_value.as_ref() {
                    if self.const_env.assign(name, val.clone()).is_ok() {
                        self.const_bindings.insert(name.clone(), val.clone());
                    }
                } else if !is_const_target {
                    self.forget_known_value(name);
                }
                if let Some(val) = const_value.as_ref()
                    && let Some(idx) = self.lookup(name)
                {
                    let dst = self.alloc();
                    let k = self.k(val.clone());
                    self.emit(Op::LoadK(dst, k));
                    self.store_named(name, idx, dst);
                    return;
                }
                // Quick path: `a = b` where b is a simple local variable.
                // Emit a single StoreLocal(a_reg, b_reg) or Move(a_reg, b_reg)
                // instead of LoadLocal(tmp, b_reg) + StoreLocal(a_reg, tmp).
                if !is_const_target
                    && let Expr::Var(src_name) = value.as_ref()
                    && let Some(idx) = self.lookup(name)
                    && let Some(src_idx) = self.lookup(src_name)
                {
                    self.emit(Op::StoreLocal(idx, src_idx));
                    return;
                }
                let rv = self.expr(value);
                if let Some(idx) = self.lookup(name) {
                    self.store_named(name, idx, rv);
                } else {
                    let msg = if is_const_target {
                        format!("Cannot assign to const variable '{}'", name)
                    } else {
                        format!("Undefined variable: {}", name)
                    };
                    let msg_idx = self.k(Val::Str(msg.into()));
                    self.emit(Op::Raise { err_kidx: msg_idx });
                }
            }
            Stmt::Expr(e) => {
                let reg = self.expr(e);
                if let Some(var_name) = detect_mutating_receiver(e)
                    && let Some(idx) = self.lookup(var_name)
                    && idx != reg
                {
                    self.emit(Op::StoreLocal(idx, reg));
                }
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                let rc = self.expr(condition);
                let jf = self.code.len();
                self.emit(Op::JmpFalse(rc, 0));
                self.with_const_scope(|builder| builder.stmt(then_stmt));
                let jend_pos = self.code.len();
                let need_else = else_stmt.is_some();
                if need_else {
                    self.emit(Op::Jmp(0));
                }
                let else_label = self.code.len();
                if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                    *ofs = (else_label as isize - jf as isize) as i16;
                }
                if let Some(es) = else_stmt {
                    self.with_const_scope(|builder| builder.stmt(es));
                }
                if need_else {
                    let cur_len = self.code.len();
                    if let Op::Jmp(ref mut ofs) = self.code[jend_pos] {
                        *ofs = (cur_len as isize - jend_pos as isize) as i16;
                    }
                }
            }
            Stmt::While { condition, body } => {
                // Try to lower simple `while (i < N) { ...; i = i + 1 }` to for-range.
                // ForRange uses ForRangeState (bare integers, no Val boxing) instead of
                // Val-based comparison + increment per iteration, AND is BC32-encodable.
                if self.try_lower_while_to_for_range(condition, body.as_ref()) {
                    return;
                }

                let start = self.code.len();
                let rc = self.expr(condition);
                let jf = self.code.len();
                self.emit(Op::JmpFalse(rc, 0));

                let saved_breaks = std::mem::take(&mut self.break_locations);
                let saved_conts = std::mem::take(&mut self.continue_locations);

                self.loop_depth = self.loop_depth.saturating_add(1);
                self.with_const_scope(|builder| builder.stmt(body));

                let current_conts = std::mem::take(&mut self.continue_locations);
                for loc in current_conts {
                    if let Some(Op::Continue(ofs)) = self.code.get_mut(loc) {
                        *ofs = (start as isize - loc as isize) as i16;
                    }
                }

                let back = start as isize - self.code.len() as isize;
                self.emit(Op::Jmp(back as i16));

                let end = self.code.len();
                let current_breaks = std::mem::take(&mut self.break_locations);
                for loc in current_breaks {
                    if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                        *ofs = (end as isize - loc as isize) as i16;
                    }
                }

                self.loop_depth = self.loop_depth.saturating_sub(1);
                self.break_locations = saved_breaks;
                self.continue_locations = saved_conts;

                if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                    *ofs = (end as isize - jf as isize) as i16;
                }
            }
            Stmt::Return { value } => {
                let base = if let Some(v) = value {
                    self.expr(v)
                } else {
                    let k = self.k(Val::Nil);
                    let r = self.alloc();
                    self.emit(Op::LoadK(r, k));
                    r
                };
                self.emit(Op::Ret { base, retc: 1 });
            }
            Stmt::Function {
                name,
                params,
                body,
                named_params,
                ..
            } => {
                let dst = self.emit_function_closure(Some(name.as_str()), params, named_params, body.as_ref(), true);
                let idx = self.get_or_define(name);
                self.emit(Op::StoreLocal(idx, dst));
                let kname = self.k(Val::Str(name.clone().into()));
                self.emit(Op::DefineGlobal(kname, idx));
            }
            Stmt::Break => {
                if self.loop_depth == 0 {
                    let msg_idx = self.k(Val::Str("break statement outside of loop".into()));
                    self.emit(Op::Raise { err_kidx: msg_idx });
                } else {
                    self.break_locations.push(self.code.len());
                    self.emit(Op::Break(0));
                }
            }
            Stmt::Continue => {
                if self.loop_depth == 0 {
                    let msg_idx = self.k(Val::Str("continue statement outside of loop".into()));
                    self.emit(Op::Raise { err_kidx: msg_idx });
                } else {
                    self.continue_locations.push(self.code.len());
                    self.emit(Op::Continue(0));
                }
            }
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => {
                let rv = self.expr(value);
                let plan_idx = self.register_pattern_plan(pattern);
                let match_reg = self.alloc();
                self.emit(Op::PatternMatch {
                    dst: match_reg,
                    src: rv,
                    plan: plan_idx,
                });

                let jf = self.code.len();
                self.emit(Op::JmpFalse(match_reg, 0));

                self.with_const_scope(|builder| builder.stmt(then_stmt));

                let jend_pos = self.code.len();
                let need_else = else_stmt.is_some();
                if need_else {
                    self.emit(Op::Jmp(0));
                }

                let end = self.code.len();
                if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                    *ofs = (end as isize - jf as isize) as i16;
                }

                if need_else {
                    if let Some(es) = else_stmt {
                        self.with_const_scope(|builder| builder.stmt(es));
                    }
                    let cur_len = self.code.len();
                    if let Op::Jmp(ref mut ofs) = self.code[jend_pos] {
                        *ofs = (cur_len as isize - jend_pos as isize) as i16;
                    }
                }
            }
            Stmt::WhileLet { pattern, value, body } => {
                let start = self.code.len();
                let rv = self.expr(value);

                let scan_head_var = match value.as_ref() {
                    Expr::Access(obj, field)
                        if matches!(obj.as_ref(), Expr::Var(_))
                            && matches!(field.as_ref(), Expr::Val(Val::Int(i)) if *i == 0) =>
                    {
                        if let Expr::Var(name) = obj.as_ref() {
                            Some(name.as_str())
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                let (match_pattern, prefix_relaxed) = match pattern {
                    Pattern::List { patterns, rest } if rest.is_none() => (
                        Pattern::List {
                            patterns: patterns.clone(),
                            rest: Some("__whilelet_rest".to_string()),
                        },
                        true,
                    ),
                    other => (other.clone(), false),
                };

                let nil_break = if matches!(pattern, Pattern::Variable(_)) {
                    let pos = self.code.len();
                    self.emit(Op::JmpIfNil(rv, 0));
                    Some(pos)
                } else {
                    None
                };

                let plan_idx = self.register_pattern_plan(&match_pattern);
                let rest_reg = if prefix_relaxed {
                    let reg = self.lookup("__whilelet_rest");
                    self.vars.remove("__whilelet_rest");
                    reg
                } else {
                    None
                };
                let advance_target = if prefix_relaxed {
                    if let Expr::Var(name) = value.as_ref() {
                        self.lookup(name)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let match_reg = self.alloc();
                self.emit(Op::PatternMatch {
                    dst: match_reg,
                    src: rv,
                    plan: plan_idx,
                });

                let jf = self.code.len();
                self.emit(Op::JmpFalse(match_reg, 0));

                let saved_breaks = std::mem::take(&mut self.break_locations);
                let saved_conts = std::mem::take(&mut self.continue_locations);

                self.loop_depth = self.loop_depth.saturating_add(1);
                self.with_const_scope(|builder| builder.stmt(body));

                let advance_label = self.code.len();
                if let (Some(rest_reg), Some(target)) = (rest_reg, advance_target) {
                    self.emit(Op::StoreLocal(target, rest_reg));
                }

                let current_conts = std::mem::take(&mut self.continue_locations);
                for loc in current_conts {
                    if let Some(Op::Continue(ofs)) = self.code.get_mut(loc) {
                        *ofs = (advance_label as isize - loc as isize) as i16;
                    }
                }

                let back = start as isize - self.code.len() as isize;
                self.emit(Op::Jmp(back as i16));

                let fail_block_pos = self.code.len();

                let mut break_jump: Option<usize> = None;
                if let Some(var_name) = scan_head_var
                    && let Some(var_idx) = self.lookup(var_name)
                {
                    let len_reg = self.alloc();
                    self.emit(Op::Len {
                        dst: len_reg,
                        src: var_idx,
                    });
                    let zero_reg = self.alloc();
                    let k_zero = self.k(Val::Int(0));
                    self.emit(Op::LoadK(zero_reg, k_zero));
                    let cmp_reg = self.alloc();
                    self.emit(Op::CmpEq(cmp_reg, len_reg, zero_reg));
                    let advance_jump = self.code.len();
                    self.emit(Op::JmpFalse(cmp_reg, 0));
                    let break_pos = self.code.len();
                    self.emit(Op::Jmp(0));
                    let advance_pos = self.code.len();
                    let start_reg = self.alloc();
                    let k_one = self.k(Val::Int(1));
                    self.emit(Op::LoadK(start_reg, k_one));
                    let tail_reg = self.alloc();
                    self.emit(Op::ListSlice {
                        dst: tail_reg,
                        src: var_idx,
                        start: start_reg,
                    });
                    self.emit(Op::StoreLocal(var_idx, tail_reg));
                    let restart = (start as isize - self.code.len() as isize) as i16;
                    self.emit(Op::Jmp(restart));

                    if let Op::JmpFalse(_, ref mut ofs) = self.code[advance_jump] {
                        *ofs = (advance_pos as isize - advance_jump as isize) as i16;
                    }
                    break_jump = Some(break_pos);
                }

                let end = self.code.len();
                if let Some(pos) = break_jump
                    && let Op::Jmp(ref mut ofs) = self.code[pos]
                {
                    *ofs = (end as isize - pos as isize) as i16;
                }

                let current_breaks = std::mem::take(&mut self.break_locations);
                for loc in current_breaks {
                    if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                        *ofs = (end as isize - loc as isize) as i16;
                    }
                }

                self.loop_depth = self.loop_depth.saturating_sub(1);
                self.break_locations = saved_breaks;
                self.continue_locations = saved_conts;

                if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                    *ofs = (fail_block_pos as isize - jf as isize) as i16;
                }
                if let Some(pos) = nil_break
                    && let Op::JmpIfNil(_, ref mut ofs) = self.code[pos]
                {
                    *ofs = (end as isize - pos as isize) as i16;
                }
            }
            Stmt::CompoundAssign { name, op, value, .. } => {
                if let Some(val) = self.try_eval_const_expr(value) {
                    let _ = self.const_env.assign(name, val.clone());
                    if self.const_bindings.contains_key(name) {
                        self.const_bindings.insert(name.clone(), val);
                    }
                }
                if let Some(idx) = self.lookup(name) {
                    if matches!(op, BinOp::Add)
                        && let Some(imm) = self.try_small_int_const(value)
                    {
                        self.emit(Op::AddIntImm(idx, idx, imm));
                        return;
                    }
                    if matches!(op, BinOp::Sub)
                        && let Some(imm) = self.try_small_int_const(value)
                        && let Some(neg) = imm.checked_neg()
                        && (-128..=127).contains(&neg)
                    {
                        self.emit(Op::AddIntImm(idx, idx, neg));
                        return;
                    }

                    if !Self::expr_contains_call(value) {
                        let r_value = if let Expr::Var(rhs_name) = value.as_ref() {
                            self.lookup(rhs_name).unwrap_or_else(|| self.expr(value))
                        } else {
                            self.expr(value)
                        };
                        let int_operands = self.int_regs.contains(&idx) && self.int_regs.contains(&r_value);
                        match op {
                            BinOp::Add if int_operands => self.emit(Op::AddInt(idx, idx, r_value)),
                            BinOp::Add => self.emit(Op::Add(idx, idx, r_value)),
                            BinOp::Sub if int_operands => self.emit(Op::SubInt(idx, idx, r_value)),
                            BinOp::Sub => self.emit(Op::Sub(idx, idx, r_value)),
                            BinOp::Mul if int_operands => self.emit(Op::MulInt(idx, idx, r_value)),
                            BinOp::Mul => self.emit(Op::Mul(idx, idx, r_value)),
                            BinOp::Mod if int_operands => self.emit(Op::ModInt(idx, idx, r_value)),
                            BinOp::Mod => self.emit(Op::Mod(idx, idx, r_value)),
                            BinOp::Div => self.emit(Op::Div(idx, idx, r_value)),
                            _ => return,
                        }
                        return;
                    }

                    let r_current = self.alloc();
                    self.emit(Op::LoadLocal(r_current, idx));
                    let r_value = self.expr(value);
                    let r_result = self.alloc();
                    match op {
                        BinOp::Add => self.emit(Op::Add(r_result, r_current, r_value)),
                        BinOp::Sub => self.emit(Op::Sub(r_result, r_current, r_value)),
                        BinOp::Mul => self.emit(Op::Mul(r_result, r_current, r_value)),
                        BinOp::Div => self.emit(Op::Div(r_result, r_current, r_value)),
                        BinOp::Mod => self.emit(Op::Mod(r_result, r_current, r_value)),
                        _ => {
                            return;
                        }
                    }

                    self.emit(Op::StoreLocal(idx, r_result));
                }
            }
            Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {}
            Stmt::Trait { name, methods } => {
                let builtin_idx = self.k(Val::Str("__lk_register_trait".into()));
                let reg_fn = self.alloc();
                self.emit(Op::LoadGlobal(reg_fn, builtin_idx));

                let arg_base = self.alloc();
                let name_idx = self.k(Val::Str(name.clone().into()));
                self.emit(Op::LoadK(arg_base, name_idx));

                let method_entries: Vec<Val> = methods
                    .iter()
                    .map(|(method_name, ty)| {
                        let type_str = ty.display();
                        Val::List(vec![Val::Str(method_name.clone().into()), Val::Str(type_str.into())].into())
                    })
                    .collect();
                let methods_list = Val::List(method_entries.into());
                let methods_idx = self.k(methods_list);
                let arg_methods = self.alloc();
                self.emit(Op::LoadK(arg_methods, methods_idx));

                self.emit(Op::Call {
                    f: reg_fn,
                    base: arg_base,
                    argc: 2,
                    retc: 1,
                });
            }
            Stmt::Impl {
                trait_name,
                target_type,
                methods,
            } => {
                let builtin_idx = self.k(Val::Str("__lk_register_trait_impl".into()));
                let reg_fn = self.alloc();
                self.emit(Op::LoadGlobal(reg_fn, builtin_idx));

                let arg_base = self.alloc();
                let arg_target = self.alloc();
                let arg_methods = self.alloc();

                let trait_name_idx = self.k(Val::Str(trait_name.clone().into()));
                self.emit(Op::LoadK(arg_base, trait_name_idx));

                let target_type_str = target_type.display();
                let target_type_idx = self.k(Val::Str(target_type_str.into()));
                self.emit(Op::LoadK(arg_target, target_type_idx));

                let mut entry_regs: Vec<u16> = Vec::with_capacity(methods.len());
                for method in methods {
                    if let Stmt::Function {
                        name,
                        params,
                        param_types,
                        return_type,
                        body,
                        named_params,
                    } = method
                    {
                        let closure_reg =
                            self.emit_function_closure(Some(name.as_str()), params, named_params, body.as_ref(), false);

                        let entry_base = self.alloc();
                        let name_idx = self.k(Val::Str(name.clone().into()));
                        self.emit(Op::LoadK(entry_base, name_idx));

                        let closure_slot = self.alloc();
                        self.emit(Op::Move(closure_slot, closure_reg));

                        let positional_types: Vec<Type> = params
                            .iter()
                            .enumerate()
                            .map(|(i, _)| param_types.get(i).cloned().flatten().unwrap_or(Type::Any))
                            .collect();
                        let named_type_sigs: Vec<FunctionNamedParamType> = named_params
                            .iter()
                            .map(|np| FunctionNamedParamType {
                                name: np.name.clone(),
                                ty: np.type_annotation.clone().unwrap_or(Type::Any),
                                has_default: np.default.is_some(),
                            })
                            .collect();
                        let return_ty = return_type.clone().unwrap_or(Type::Any);
                        let signature = Type::Function {
                            params: positional_types,
                            named_params: named_type_sigs,
                            return_type: Box::new(return_ty),
                        };
                        let signature_str = signature.display();
                        let signature_idx = self.k(Val::Str(signature_str.into()));
                        let signature_slot = self.alloc();
                        self.emit(Op::LoadK(signature_slot, signature_idx));

                        let entry_list = self.alloc();
                        self.emit(Op::BuildList {
                            dst: entry_list,
                            base: entry_base,
                            len: 3,
                        });
                        entry_regs.push(entry_list);
                    }
                }

                if entry_regs.is_empty() {
                    let empty_list = Val::List(Vec::<Val>::new().into());
                    let empty_idx = self.k(empty_list);
                    self.emit(Op::LoadK(arg_methods, empty_idx));
                } else {
                    let first_slot = self.alloc();
                    self.emit(Op::Move(first_slot, entry_regs[0]));
                    for entry_reg in entry_regs.iter().skip(1) {
                        let slot = self.alloc();
                        self.emit(Op::Move(slot, *entry_reg));
                    }
                    self.emit(Op::BuildList {
                        dst: arg_methods,
                        base: first_slot,
                        len: entry_regs.len() as u16,
                    });
                }

                self.emit(Op::Call {
                    f: reg_fn,
                    base: arg_base,
                    argc: 3,
                    retc: 1,
                });
            }
            Stmt::Empty => {}
        }
    }
}
