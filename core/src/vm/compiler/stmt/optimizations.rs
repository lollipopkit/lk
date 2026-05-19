use std::{collections::HashSet, sync::Arc};

use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::{ClosureCapture, ClosureInit, ClosureValue, Val},
    vm::{CaptureSpec, Op},
};

use super::FunctionBuilder;

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
    fn call_expr_name(expr: &Expr) -> Option<(&str, &[Box<Expr>])> {
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

    pub(super) fn try_specialize_const_closure_factory(&mut self, value: &Expr) -> Option<Val> {
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

    pub(super) fn simple_return_expr(stmt: &Stmt) -> Option<&Expr> {
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

    pub(super) fn collect_loop_invariant_exprs_from_expr(
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

    pub(super) fn collect_loop_invariant_exprs_from_stmt(
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

    pub(super) fn collect_loop_invariant_exprs(&self, pattern: &ForPattern, body: &Stmt) -> Vec<Expr> {
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

    pub(super) fn try_emit_immediate_closure_factory_call_pair(&mut self, first: &Stmt, second: &Stmt) -> bool {
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

    pub(super) fn cached_loop_call_assignment<'a>(&self, body: &'a Stmt) -> Option<(&'a str, &'a Expr)> {
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

    pub(super) fn emit_cached_loop_call_assignment(
        &mut self,
        target: &str,
        value: &Expr,
        flag_reg: u16,
        cache_reg: u16,
    ) -> bool {
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

    pub(super) fn cached_loop_delta<'a>(&self, body: &'a Stmt) -> Option<(&'a str, &'a Expr, Vec<Box<Stmt>>)> {
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

    pub(super) fn try_emit_range_count_accumulator(
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
            && let Expr::Val(method_val) = field.as_ref()
            && matches!(method_val.as_str(), Some("push") | Some("set"))
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
                    && let Expr::Val(method_val3) = field.as_ref()
                    && matches!(method_val3.as_str(), Some("values") | Some("keys") | Some("len"))
                {
                    return args.is_empty();
                }
                false
            }
            _ => false,
        }
    }

    pub(super) fn emit_cached_loop_delta(
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

    pub(super) fn try_emit_simple_self_assign(&mut self, name: &str, value: &Expr) -> bool {
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

    pub(super) fn declare_for_pattern(&mut self, pattern: &ForPattern) {
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

    pub(super) fn pattern_requires_check(pattern: &ForPattern) -> bool {
        !matches!(pattern, ForPattern::Variable(_) | ForPattern::Ignore)
    }

    pub(super) fn pattern_from_for(pattern: &ForPattern) -> Pattern {
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
    pub(super) fn try_lower_while_to_for_range(&mut self, condition: &Expr, body: &Stmt) -> bool {
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
}
