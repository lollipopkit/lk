use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::{FunctionNamedParamType, Type, Val},
    vm::Op,
};

use super::FunctionBuilder;
use std::collections::HashSet;

fn detect_mutating_receiver(expr: &Expr) -> Option<&str> {
    if let Expr::CallExpr(callee, _args) = expr {
        if let Expr::Access(obj, field) = callee.as_ref() {
            if let Expr::Var(var_name) = obj.as_ref()
                && let Expr::Val(Val::Str(method)) = field.as_ref()
                && method.as_ref() == "push"
            {
                return Some(var_name);
            }
        }
    }
    None
}

impl FunctionBuilder {
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
        match self.try_eval_const_expr(expr)? {
            Val::Int(i) => Some(i),
            _ => None,
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

    pub fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Block { statements } => {
                self.with_const_scope(|builder| {
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
                    self.push_var_scope();
                    self.declare_for_pattern(pattern);

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
                    } else if let ForPattern::Variable(name) = pattern {
                        if let Some(idx) = self.lookup(name) {
                            self.emit(Op::StoreLocal(idx, r_idx));
                        }
                    }

                    self.with_const_scope(|builder| builder.stmt(body));

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
                    self.declare_for_pattern(pattern);
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
                    } else if let ForPattern::Variable(name) = pattern {
                        if let Some(idx) = self.lookup(name) {
                            self.emit(Op::StoreLocal(idx, r_item));
                        }
                    }

                    self.with_const_scope(|builder| builder.stmt(body));

                    let cont_target = self.code.len();
                    let pending_continues = std::mem::take(&mut self.continue_locations);
                    let k1 = self.k(Val::Int(1));
                    let r_one = self.alloc();
                    self.emit(Op::LoadK(r_one, k1));
                    self.emit(Op::Add(r_i, r_i, r_one));
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
                let const_value = self.try_eval_const_expr(value);
                if let Pattern::Variable(name) = pattern {
                    if !*is_const {
                        if let Some(v) = const_value.as_ref() {
                            self.const_env.define(name.clone(), v.clone());
                        }
                        let rv = if let Some(v) = const_value.clone() {
                            let dst = self.alloc();
                            let k = self.k(v);
                            self.emit(Op::LoadK(dst, k));
                            dst
                        } else {
                            self.expr(value)
                        };
                        let idx = self.get_or_define(name);
                        self.store_named(name, idx, rv);
                        let kname = self.k(Val::Str(name.clone().into()));
                        self.emit(Op::DefineGlobal(kname, idx));
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
                    let kname = self.k(Val::Str(name.clone().into()));
                    self.emit(Op::DefineGlobal(kname, idx));
                }
            }
            Stmt::Assign { name, value, .. } => {
                let is_const_target = self.const_names.contains(name);
                if !is_const_target {
                    if let Some(val) = self.try_eval_const_expr(value) {
                        let _ = self.const_env.assign(name, val.clone());
                        if self.const_bindings.contains_key(name) {
                            self.const_bindings.insert(name.clone(), val);
                        }
                    }
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
                if let Some(var_name) = detect_mutating_receiver(e) {
                    if let Some(idx) = self.lookup(var_name) {
                        self.emit(Op::StoreLocal(idx, reg));
                    }
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
                if let Some(pos) = break_jump {
                    if let Op::Jmp(ref mut ofs) = self.code[pos] {
                        *ofs = (end as isize - pos as isize) as i16;
                    }
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
                if let Some(pos) = nil_break {
                    if let Op::JmpIfNil(_, ref mut ofs) = self.code[pos] {
                        *ofs = (end as isize - pos as isize) as i16;
                    }
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
                let builtin_idx = self.k(Val::Str("__lkr_register_trait".into()));
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
                let builtin_idx = self.k(Val::Str("__lkr_register_trait_impl".into()));
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
