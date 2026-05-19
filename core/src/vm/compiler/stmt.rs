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
    val::Val,
    vm::Op,
};

use super::FunctionBuilder;

mod assign;
mod globals;
/// Strip the trailing increment statement from a while-loop body.
/// Used by the while→for-range lowering pass.
mod loop_invariants;
mod loop_lowering;
mod optimizations;
mod ranges;
mod traits;

use globals::detect_mutating_receiver;

impl FunctionBuilder {
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
                        let err_idx = self.k(Val::from_str("Pattern does not match value"));
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
                    let flush_pos = self.code.len();
                    self.pop_var_scope();
                    self.flush_loop_global_writes(body);
                    if let Op::ForRangeLoop { ofs, .. } = &mut self.code[guard_pos] {
                        *ofs = (flush_pos as isize - guard_pos as isize) as i16;
                    }
                    let pending_breaks = std::mem::take(&mut self.break_locations);
                    for loc in pending_breaks {
                        if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                            *ofs = (flush_pos as isize - loc as isize) as i16;
                        }
                    }
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
                        let err_idx = self.k(Val::from_str("Pattern does not match value"));
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
                    let flush_pos = self.code.len();
                    self.pop_var_scope();
                    self.flush_loop_global_writes(body);
                    if let Op::JmpFalse(_, ref mut ofs) = self.code[jf_pos] {
                        *ofs = (flush_pos as isize - jf_pos as isize) as i16;
                    }
                    for loc in pending_continues {
                        if let Some(Op::Continue(ofs)) = self.code.get_mut(loc) {
                            *ofs = (cont_target as isize - loc as isize) as i16;
                        }
                    }
                    let pending_breaks = std::mem::take(&mut self.break_locations);
                    for loc in pending_breaks {
                        if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                            *ofs = (flush_pos as isize - loc as isize) as i16;
                        }
                    }

                    self.loop_depth = self.loop_depth.saturating_sub(1);
                    self.break_locations = saved_breaks;
                    self.continue_locations = saved_conts;
                }
            }
            Stmt::Define { name, value } => {
                self.global_defs.insert(name.clone());
                let idx = self.get_or_define(name);
                if !Self::expr_contains_call(value) {
                    self.emit_expr_into(idx, value);
                } else {
                    let rv = self.expr(value);
                    self.emit(Op::StoreLocal(idx, rv));
                }
                if self.export_toplevel_globals {
                    let kname = self.k(Val::from_str(name.as_str()));
                    self.emit(Op::DefineGlobal(kname, idx));
                }
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
                        // Bind directly to the expression result register. Call lowering
                        // reserves at least retc slots, so return slots remain stable for
                        // the rest of this frame and do not need a StoreLocal copy.
                        self.define_var_as(name, rv);
                        if self.export_toplevel_globals && self.loop_depth == 0 && self.var_scope_depth() == 0 {
                            self.global_defs.insert(name.clone());
                            let kname = self.k(Val::from_str(name.as_str()));
                            self.emit(Op::DefineGlobal(kname, rv));
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
                let err_idx = self.k(Val::from_str("Pattern does not match value"));
                self.emit(Op::PatternMatchOrFail {
                    src: rv,
                    plan: plan_idx,
                    err_kidx: err_idx,
                    is_const: *is_const,
                });

                if let Pattern::Variable(name) = pattern {
                    let idx = self.get_or_define(name);
                    if self.export_toplevel_globals && self.loop_depth == 0 && self.var_scope_depth() == 0 {
                        let kname = self.k(Val::from_str(name.as_str()));
                        self.emit(Op::DefineGlobal(kname, idx));
                    }
                }
            }
            Stmt::Assign { name, value, .. } => {
                self.stmt_assign(name, value);
            }
            Stmt::Expr(e) => {
                let globals_to_reload = self.called_closure_global_captures(e);
                let reg = self.expr(e);
                if let Some(var_name) = detect_mutating_receiver(e)
                    && let Some(idx) = self.lookup(var_name)
                    && idx != reg
                {
                    self.emit(Op::StoreLocal(idx, reg));
                }
                if !globals_to_reload.is_empty() {
                    let mut names: Vec<_> = globals_to_reload;
                    names.sort();
                    for name in names {
                        if let Some(idx) = self.lookup(&name) {
                            let kname = self.k(Val::from_str(name.as_str()));
                            self.emit(Op::LoadGlobal(idx, kname));
                        }
                    }
                }
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                let mut branch_assigned_names = std::collections::HashSet::new();
                Self::collect_stmt_assigned_names(then_stmt, &mut branch_assigned_names);
                if let Some(es) = else_stmt {
                    Self::collect_stmt_assigned_names(es, &mut branch_assigned_names);
                }

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
                for name in branch_assigned_names {
                    self.forget_known_value(&name);
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

                let flush_pos = self.code.len();
                self.flush_loop_global_writes(body);
                let current_breaks = std::mem::take(&mut self.break_locations);
                for loc in current_breaks {
                    if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                        *ofs = (flush_pos as isize - loc as isize) as i16;
                    }
                }

                self.loop_depth = self.loop_depth.saturating_sub(1);
                self.break_locations = saved_breaks;
                self.continue_locations = saved_conts;

                if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                    *ofs = (flush_pos as isize - jf as isize) as i16;
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
                param_types,
                body,
                named_params,
                ..
            } => {
                let idx = self.get_or_define(name);
                let effective_param_types = self.effective_function_param_types(name, params, param_types);
                self.emit_function_closure_into(
                    idx,
                    Some(name.as_str()),
                    params,
                    &effective_param_types,
                    named_params,
                    body.as_ref(),
                    true,
                );
                if self.export_toplevel_globals && self.loop_depth == 0 && self.var_scope_depth() == 0 {
                    let kname = self.k(Val::from_str(name.as_str()));
                    self.emit(Op::DefineGlobal(kname, idx));
                }
            }
            Stmt::Break => {
                if self.loop_depth == 0 {
                    let msg_idx = self.k(Val::from_str("break statement outside of loop"));
                    self.emit(Op::Raise { err_kidx: msg_idx });
                } else {
                    self.break_locations.push(self.code.len());
                    self.emit(Op::Break(0));
                }
            }
            Stmt::Continue => {
                if self.loop_depth == 0 {
                    let msg_idx = self.k(Val::from_str("continue statement outside of loop"));
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

                let flush_pos = self.code.len();
                self.flush_loop_global_writes(body);
                if let Some(pos) = break_jump
                    && let Op::Jmp(ref mut ofs) = self.code[pos]
                {
                    *ofs = (flush_pos as isize - pos as isize) as i16;
                }

                let current_breaks = std::mem::take(&mut self.break_locations);
                for loc in current_breaks {
                    if let Some(Op::Break(ofs)) = self.code.get_mut(loc) {
                        *ofs = (flush_pos as isize - loc as isize) as i16;
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
                    *ofs = (flush_pos as isize - pos as isize) as i16;
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
                        if self.should_export_global_write(name) {
                            let kname = self.k(Val::from_str(name.as_str()));
                            self.emit(Op::DefineGlobal(kname, idx));
                        }
                        return;
                    }
                    if matches!(op, BinOp::Sub)
                        && let Some(imm) = self.try_small_int_const(value)
                        && let Some(neg) = imm.checked_neg()
                        && (-128..=127).contains(&neg)
                    {
                        self.emit(Op::AddIntImm(idx, idx, neg));
                        if self.should_export_global_write(name) {
                            let kname = self.k(Val::from_str(name.as_str()));
                            self.emit(Op::DefineGlobal(kname, idx));
                        }
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
                        if self.should_export_global_write(name) {
                            let kname = self.k(Val::from_str(name.as_str()));
                            self.emit(Op::DefineGlobal(kname, idx));
                        }
                        return;
                    }

                    let r_current = if Self::expr_contains_call(value) && !self.expr_calls_preserve_binding(value, name)
                    {
                        let r = self.alloc();
                        self.emit(Op::LoadLocal(r, idx));
                        r
                    } else {
                        idx
                    };
                    let r_value = self.expr(value);
                    let int_operands = self.int_regs.contains(&r_current) && self.int_regs.contains(&r_value);
                    match op {
                        BinOp::Add if int_operands => self.emit(Op::AddInt(idx, r_current, r_value)),
                        BinOp::Add => self.emit(Op::Add(idx, r_current, r_value)),
                        BinOp::Sub if int_operands => self.emit(Op::SubInt(idx, r_current, r_value)),
                        BinOp::Sub => self.emit(Op::Sub(idx, r_current, r_value)),
                        BinOp::Mul if int_operands => self.emit(Op::MulInt(idx, r_current, r_value)),
                        BinOp::Mul => self.emit(Op::Mul(idx, r_current, r_value)),
                        BinOp::Div => self.emit(Op::Div(idx, r_current, r_value)),
                        BinOp::Mod if int_operands => self.emit(Op::ModInt(idx, r_current, r_value)),
                        BinOp::Mod => self.emit(Op::Mod(idx, r_current, r_value)),
                        _ => {
                            return;
                        }
                    }
                    if self.should_export_global_write(name) {
                        let kname = self.k(Val::from_str(name.as_str()));
                        self.emit(Op::DefineGlobal(kname, idx));
                    }
                }
                if self.capture_indices.contains_key(name) {
                    let current = self.expr(&Expr::Var(name.clone()));
                    let r_value = self.expr(value);
                    let out = self.alloc();
                    match op {
                        BinOp::Add => self.emit(Op::Add(out, current, r_value)),
                        BinOp::Sub => self.emit(Op::Sub(out, current, r_value)),
                        BinOp::Mul => self.emit(Op::Mul(out, current, r_value)),
                        BinOp::Div => self.emit(Op::Div(out, current, r_value)),
                        BinOp::Mod => self.emit(Op::Mod(out, current, r_value)),
                        _ => return,
                    }
                    let kname = self.k(Val::from_str(name.as_str()));
                    self.emit(Op::DefineGlobal(kname, out));
                }
            }
            Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {}
            Stmt::Trait { name, methods } => self.compile_trait_registration(name, methods),
            Stmt::Impl {
                trait_name,
                target_type,
                methods,
            } => self.compile_trait_impl_registration(trait_name, target_type, methods),
            Stmt::Empty => {}
        }
    }
}
