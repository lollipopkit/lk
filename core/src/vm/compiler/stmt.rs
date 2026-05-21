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
    expr::{Expr, Pattern, TemplateStringPart},
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::Val,
    vm::{IntCmpKind, Op},
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

struct MapAccessExpr<'a> {
    map_name: &'a str,
    key: &'a Expr,
    module_style: bool,
}

struct MapSetExpr<'a> {
    map_name: &'a str,
    key: &'a Expr,
    value: &'a Expr,
    module_style: bool,
}

impl FunctionBuilder {
    fn const_value_is_mutable_container(value: &Val) -> bool {
        matches!(value, Val::List(_) | Val::Map(_))
    }

    fn template_split_join_len_pair<'a>(
        first: &'a Stmt,
        second: &'a Stmt,
        rest: &[Box<Stmt>],
    ) -> Option<(&'a str, &'a [TemplateStringPart], &'a str)> {
        let Stmt::Let {
            pattern: Pattern::Variable(line_name),
            value: line_value,
            is_const: false,
            ..
        } = first
        else {
            return None;
        };
        let Expr::TemplateString(parts) = line_value.as_ref() else {
            return None;
        };
        let Stmt::Let {
            pattern: Pattern::Variable(len_name),
            value,
            is_const: false,
            ..
        } = second
        else {
            return None;
        };
        let Expr::CallExpr(len_callee, len_args) = value.as_ref() else {
            return None;
        };
        if !len_args.is_empty() {
            return None;
        }
        let Expr::Access(join_call, len_field) = len_callee.as_ref() else {
            return None;
        };
        let Expr::Val(len_method) = len_field.as_ref() else {
            return None;
        };
        if len_method.as_str() != Some("len") {
            return None;
        }
        let Expr::CallExpr(join_callee, join_args) = join_call.as_ref() else {
            return None;
        };
        let Expr::Access(join_receiver, join_field) = join_callee.as_ref() else {
            return None;
        };
        let Expr::Val(join_method) = join_field.as_ref() else {
            return None;
        };
        if join_method.as_str() != Some("join") {
            return None;
        }
        let split_receiver = super::expr_call::split_join_same_separator_receiver(join_receiver, join_args)?;
        if !matches!(split_receiver, Expr::Var(name) if name == line_name) {
            return None;
        }
        if rest.iter().any(|stmt| Self::stmt_mentions_name(stmt, line_name)) {
            return None;
        }
        Some((line_name.as_str(), parts.as_slice(), len_name.as_str()))
    }

    fn map_get_expr<'a>(expr: &'a Expr) -> Option<MapAccessExpr<'a>> {
        let Expr::CallExpr(callee, args) = expr else {
            return None;
        };
        let Expr::Access(receiver, method) = callee.as_ref() else {
            return None;
        };
        let Expr::Val(method) = method.as_ref() else {
            return None;
        };
        if method.as_str() != Some("get") {
            return None;
        }
        match receiver.as_ref() {
            Expr::Var(module_name) if module_name == "map" && args.len() == 2 => {
                let Expr::Var(map_name) = args[0].as_ref() else {
                    return None;
                };
                Some(MapAccessExpr {
                    map_name: map_name.as_str(),
                    key: args[1].as_ref(),
                    module_style: true,
                })
            }
            Expr::Var(map_name) if args.len() == 1 => Some(MapAccessExpr {
                map_name: map_name.as_str(),
                key: args[0].as_ref(),
                module_style: false,
            }),
            _ => None,
        }
    }

    fn map_set_expr<'a>(stmt: &'a Stmt) -> Option<MapSetExpr<'a>> {
        let Stmt::Expr(expr) = stmt else {
            return None;
        };
        let Expr::CallExpr(callee, args) = expr.as_ref() else {
            return None;
        };
        let Expr::Access(receiver, method) = callee.as_ref() else {
            return None;
        };
        let Expr::Val(method) = method.as_ref() else {
            return None;
        };
        if method.as_str() != Some("set") {
            return None;
        }
        match receiver.as_ref() {
            Expr::Var(module_name) if module_name == "map" && args.len() == 3 => {
                let Expr::Var(map_name) = args[0].as_ref() else {
                    return None;
                };
                Some(MapSetExpr {
                    map_name: map_name.as_str(),
                    key: args[1].as_ref(),
                    value: args[2].as_ref(),
                    module_style: true,
                })
            }
            Expr::Var(map_name) if args.len() == 2 => Some(MapSetExpr {
                map_name: map_name.as_str(),
                key: args[0].as_ref(),
                value: args[1].as_ref(),
                module_style: false,
            }),
            _ => None,
        }
    }

    fn single_expr_stmt(stmt: &Stmt) -> Option<&Stmt> {
        match stmt {
            Stmt::Block { statements } if statements.len() == 1 => Some(statements[0].as_ref()),
            other => Some(other),
        }
    }

    fn int_cmp_kind_for_op(op: &BinOp) -> Option<IntCmpKind> {
        match op {
            BinOp::Eq => Some(IntCmpKind::Eq),
            BinOp::Ne => Some(IntCmpKind::Ne),
            BinOp::Lt => Some(IntCmpKind::Lt),
            BinOp::Le => Some(IntCmpKind::Le),
            BinOp::Gt => Some(IntCmpKind::Gt),
            BinOp::Ge => Some(IntCmpKind::Ge),
            _ => None,
        }
    }

    fn try_emit_int_conditional_move(&mut self, condition: &Expr, then_stmt: &Stmt) -> bool {
        let Some(then_stmt) = Self::single_expr_stmt(then_stmt) else {
            return false;
        };
        let Stmt::Assign {
            name: dst_name, value, ..
        } = then_stmt
        else {
            return false;
        };
        if self.const_names.contains(dst_name) {
            return false;
        }
        let Expr::Var(src_name) = value.as_ref() else {
            return false;
        };
        let Expr::Bin(left, op, right) = condition else {
            return false;
        };
        let Some(kind) = Self::int_cmp_kind_for_op(op) else {
            return false;
        };
        let (Expr::Var(left_name), Expr::Var(right_name)) = (left.as_ref(), right.as_ref()) else {
            return false;
        };
        let (Some(dst), Some(src), Some(a), Some(b)) = (
            self.lookup(dst_name),
            self.lookup(src_name),
            self.lookup(left_name),
            self.lookup(right_name),
        ) else {
            return false;
        };
        if !(self.reg_known_int(dst) && self.reg_known_int(src) && self.reg_known_int(a) && self.reg_known_int(b)) {
            return false;
        }
        self.emit(Op::CMoveInt { dst, src, a, b, kind });
        if self.should_export_global_write(dst_name) {
            let kname = self.k(Val::from_str(dst_name.as_str()));
            self.emit(Op::DefineGlobal(kname, dst));
        }
        self.forget_known_value(dst_name);
        true
    }

    fn condition_is_var_eq_nil(expr: &Expr, name: &str) -> bool {
        let Expr::Bin(left, BinOp::Eq, right) = expr else {
            return false;
        };
        (matches!(left.as_ref(), Expr::Var(var) if var == name) && matches!(right.as_ref(), Expr::Val(Val::Nil)))
            || (matches!(right.as_ref(), Expr::Var(var) if var == name) && matches!(left.as_ref(), Expr::Val(Val::Nil)))
    }

    fn expr_is_pure_arith(expr: &Expr) -> bool {
        match expr {
            Expr::Val(Val::Int(_) | Val::Float(_)) | Expr::Var(_) => true,
            Expr::Paren(inner) | Expr::Unary(_, inner) => Self::expr_is_pure_arith(inner),
            Expr::Bin(left, op, right) if op.is_arith() => {
                Self::expr_is_pure_arith(left) && Self::expr_is_pure_arith(right)
            }
            _ => false,
        }
    }

    fn add_expr_uses_current_and_delta(expr: &Expr, current_name: &str, delta_name: &str) -> bool {
        let Expr::Bin(left, BinOp::Add, right) = expr else {
            return false;
        };
        (matches!(left.as_ref(), Expr::Var(name) if name == current_name)
            && matches!(right.as_ref(), Expr::Var(name) if name == delta_name))
            || (matches!(right.as_ref(), Expr::Var(name) if name == current_name)
                && matches!(left.as_ref(), Expr::Var(name) if name == delta_name))
    }

    fn add_expr_uses_current_and_value(expr: &Expr, current_name: &str, value: &Expr) -> bool {
        let Expr::Bin(left, BinOp::Add, right) = expr else {
            return false;
        };
        (matches!(left.as_ref(), Expr::Var(name) if name == current_name) && right.as_ref() == value)
            || (matches!(right.as_ref(), Expr::Var(name) if name == current_name) && left.as_ref() == value)
    }

    fn try_emit_map_upsert_default_add(&mut self, first: &Stmt, second: &Stmt, rest: &[Box<Stmt>]) -> bool {
        let Stmt::Let {
            pattern: Pattern::Variable(current_name),
            value: current_value,
            is_const: false,
            ..
        } = first
        else {
            return false;
        };
        let Some(get_expr) = Self::map_get_expr(current_value) else {
            return false;
        };
        if get_expr.module_style && self.lookup("map").is_some() {
            return false;
        }
        let Stmt::If {
            condition,
            then_stmt,
            else_stmt: Some(else_stmt),
        } = second
        else {
            return false;
        };
        if !Self::condition_is_var_eq_nil(condition, current_name)
            || rest.iter().any(|stmt| Self::stmt_mentions_name(stmt, current_name))
        {
            return false;
        }
        let Some(then_set) = Self::single_expr_stmt(then_stmt).and_then(Self::map_set_expr) else {
            return false;
        };
        let Some(else_set) = Self::single_expr_stmt(else_stmt).and_then(Self::map_set_expr) else {
            return false;
        };
        let default_expr = then_set.value;
        if ((then_set.module_style || else_set.module_style) && self.lookup("map").is_some())
            || then_set.map_name != get_expr.map_name
            || else_set.map_name != get_expr.map_name
            || then_set.key != get_expr.key
            || else_set.key != get_expr.key
            || !Self::expr_is_pure_arith(default_expr)
            || Self::expr_mentions_name(default_expr, current_name)
            || !Self::add_expr_uses_current_and_value(else_set.value, current_name, default_expr)
        {
            return false;
        }

        let Some(map_reg) = self.lookup(get_expr.map_name) else {
            return false;
        };
        let current_reg = self.emit_map_access(map_reg, get_expr.key);
        self.define_var_as(current_name, current_reg);

        let rc = self.expr(condition);
        let jf = self.code.len();
        self.emit(Op::JmpFalse(rc, 0));

        self.with_const_scope(|builder| {
            builder.emit_map_set(map_reg, get_expr.key, default_expr);
        });

        let jend_pos = self.code.len();
        self.emit(Op::Jmp(0));
        let else_label = self.code.len();
        if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
            *ofs = (else_label as isize - jf as isize) as i16;
        }
        self.with_const_scope(|builder| {
            builder.emit_map_set(map_reg, get_expr.key, else_set.value);
        });
        let cur_len = self.code.len();
        if let Op::Jmp(ref mut ofs) = self.code[jend_pos] {
            *ofs = (cur_len as isize - jend_pos as isize) as i16;
        }
        self.forget_known_value(current_name);
        true
    }

    fn try_emit_delayed_map_upsert_delta(
        &mut self,
        first: &Stmt,
        second: &Stmt,
        third: &Stmt,
        rest: &[Box<Stmt>],
    ) -> bool {
        let Stmt::Let {
            pattern: Pattern::Variable(current_name),
            value: current_value,
            is_const: false,
            ..
        } = first
        else {
            return false;
        };
        let Some(get_expr) = Self::map_get_expr(current_value) else {
            return false;
        };
        if get_expr.module_style && self.lookup("map").is_some() {
            return false;
        }
        let Stmt::Let {
            pattern: Pattern::Variable(delta_name),
            value: delta_expr,
            is_const: false,
            ..
        } = second
        else {
            return false;
        };
        if !Self::expr_is_pure_arith(delta_expr)
            || Self::expr_mentions_name(delta_expr, current_name)
            || rest.iter().any(|stmt| Self::stmt_mentions_name(stmt, current_name))
            || rest.iter().any(|stmt| Self::stmt_mentions_name(stmt, delta_name))
        {
            return false;
        }
        let Stmt::If {
            condition,
            then_stmt,
            else_stmt: Some(else_stmt),
        } = third
        else {
            return false;
        };
        if !Self::condition_is_var_eq_nil(condition, current_name) {
            return false;
        }
        let Some(then_set) = Self::single_expr_stmt(then_stmt).and_then(Self::map_set_expr) else {
            return false;
        };
        let Some(else_set) = Self::single_expr_stmt(else_stmt).and_then(Self::map_set_expr) else {
            return false;
        };
        if ((then_set.module_style || else_set.module_style) && self.lookup("map").is_some())
            || then_set.map_name != get_expr.map_name
            || else_set.map_name != get_expr.map_name
            || then_set.key != get_expr.key
            || else_set.key != get_expr.key
            || !matches!(then_set.value, Expr::Var(name) if name == delta_name)
            || !Self::add_expr_uses_current_and_delta(else_set.value, current_name, delta_name)
        {
            return false;
        }

        let Some(map_reg) = self.lookup(get_expr.map_name) else {
            return false;
        };
        let current_reg = self.emit_map_access(map_reg, get_expr.key);
        self.define_var_as(current_name, current_reg);

        let rc = self.expr(condition);
        let jf = self.code.len();
        self.emit(Op::JmpFalse(rc, 0));

        self.with_const_scope(|builder| {
            let delta_reg = builder.expr(delta_expr);
            builder.define_var_as(delta_name, delta_reg);
            builder.emit_map_set(map_reg, get_expr.key, &Expr::Var(delta_name.to_string()));
        });

        let jend_pos = self.code.len();
        self.emit(Op::Jmp(0));
        let else_label = self.code.len();
        if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
            *ofs = (else_label as isize - jf as isize) as i16;
        }
        self.with_const_scope(|builder| {
            let delta_reg = builder.expr(delta_expr);
            builder.define_var_as(delta_name, delta_reg);
            builder.emit_map_set(map_reg, get_expr.key, else_set.value);
        });
        let cur_len = self.code.len();
        if let Op::Jmp(ref mut ofs) = self.code[jend_pos] {
            *ofs = (cur_len as isize - jend_pos as isize) as i16;
        }
        self.forget_known_value(current_name);
        self.forget_known_value(delta_name);
        true
    }

    pub fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Block { statements } => {
                self.with_const_scope(|builder| {
                    let function_names = Self::direct_function_names_in_block(statements);
                    builder.push_function_name_scope(&function_names);
                    if statements.len() == 2
                        && builder.try_emit_immediate_closure_factory_call_pair(&statements[0], &statements[1])
                    {
                        builder.pop_function_name_scope(&function_names);
                        return;
                    }
                    let mut idx = 0;
                    while idx < statements.len() {
                        if let Some((_line_name, parts, len_name)) = statements.get(idx + 1).and_then(|next| {
                            Self::template_split_join_len_pair(
                                statements[idx].as_ref(),
                                next.as_ref(),
                                &statements[idx + 2..],
                            )
                        }) {
                            let len_reg = builder.compile_template_string_len(parts);
                            builder.define_var_as(len_name, len_reg);
                            idx += 2;
                            continue;
                        }
                        if idx + 2 < statements.len()
                            && builder.try_emit_delayed_map_upsert_delta(
                                statements[idx].as_ref(),
                                statements[idx + 1].as_ref(),
                                statements[idx + 2].as_ref(),
                                &statements[idx + 3..],
                            )
                        {
                            idx += 3;
                            continue;
                        }
                        if idx + 1 < statements.len()
                            && builder.try_emit_map_upsert_default_add(
                                statements[idx].as_ref(),
                                statements[idx + 1].as_ref(),
                                &statements[idx + 2..],
                            )
                        {
                            idx += 2;
                            continue;
                        }
                        builder.stmt(&statements[idx]);
                        idx += 1;
                    }
                    builder.pop_function_name_scope(&function_names);
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
                        Some(e) => {
                            let r = self.alloc();
                            self.emit_expr_into(r, e);
                            r
                        }
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
                    let loop_invariant_lets = if cached_loop_call.is_none() && cached_loop_delta.is_none() {
                        self.collect_loop_invariant_literal_lets(body)
                    } else {
                        Vec::new()
                    };
                    let loop_invariant_start = self.loop_invariant_expr_regs.len();
                    let loop_invariant_let_start = self.loop_invariant_let_regs.len();
                    for expr in loop_invariant_exprs {
                        let reg = self.expr(&expr);
                        self.loop_invariant_expr_regs.push((expr, reg));
                    }
                    for (name, expr) in loop_invariant_lets {
                        if let Some(reg) = self.lookup_loop_invariant_expr(&expr) {
                            self.loop_invariant_let_regs.push((name, reg));
                        }
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
                    self.loop_invariant_let_regs.truncate(loop_invariant_let_start);

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
                    if self.try_emit_typed_for_iter_loop(pattern, iterable, body) {
                        return;
                    }
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
                    if let (false, Some(v)) = (*is_const, const_value.as_ref())
                        && !Self::const_value_is_mutable_container(v)
                    {
                        self.bind_known_value(name.clone(), v.clone());
                    }
                    if !*is_const
                        && const_value.is_none()
                        && let Expr::Closure { params, body } = value.as_ref()
                    {
                        self.register_closure_const_env(name, params, body);
                    }
                    if !*is_const {
                        let rv = if let Some(v) = const_value.clone()
                            && !Self::const_value_is_mutable_container(&v)
                        {
                            if let Some(reg) = self.lookup_loop_invariant_let(name) {
                                reg
                            } else {
                                let dst = self.alloc();
                                if matches!(v, Val::Map(_)) {
                                    self.map_locals.insert(dst);
                                }
                                let k = self.k(v);
                                self.emit(Op::LoadK(dst, k));
                                dst
                            }
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
                    } else if let Some(v) = const_value.as_ref()
                        && !Self::const_value_is_mutable_container(v)
                    {
                        self.const_env.define(name.clone(), v.clone());
                    }
                }

                let rv = if let Some(v) = const_value
                    && (*is_const || !Self::const_value_is_mutable_container(&v))
                {
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
                if else_stmt.is_none() && self.try_emit_int_conditional_move(condition, then_stmt) {
                    return;
                }
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
                        if !self.emit_in_place_numeric_op(idx, idx, r_value, op) {
                            return;
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
                    if !self.emit_in_place_numeric_op(idx, r_current, r_value, op) {
                        return;
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
