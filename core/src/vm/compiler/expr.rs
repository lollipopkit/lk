//! Expression compilation — translates AST expressions to bytecode.
//!
//! Handles all expression forms: literals, variables, binary/unary ops,
//! function calls, method calls, list/map literals, closures, and control
//! flow expressions (if-else, match).
//!
//! ## Type-Specialized Code Generation
//!
//! The compiler emits type-specialized opcodes when it can statically
//! determine operand types:
//! - `ArithFlavor::Int` → `AddInt`, `SubInt`, `MulInt` (no float coercion)
//! - `ArithFlavor::Float` → `AddFloat`, `SubFloat`, `DivFloat`
//! - `ArithFlavor::Any` → generic `Add`, `Sub`, `Mul` (runtime dispatch)
//!
//! ## Constant Folding
//!
//! `try_fold_bin` and `try_fold_unary` perform compile-time evaluation of
//! constant expressions, emitting `LoadK` for pre-computed results instead
//! of runtime operations.

use std::{collections::HashSet, sync::Arc};

use super::ArithFlavor;
use crate::{
    expr::{Expr, TemplateStringPart},
    op::{BinOp, UnaryOp},
    stmt::Stmt,
    val::Val,
    vm::{
        ClosureProto, IntCmpKind, Op, bytecode::rk_is_const, capture_names_from_specs, closure_code_cell,
        closure_empty_captures, closure_empty_closure_cell, closure_empty_env, closure_empty_upvalues,
    },
};

use super::FunctionBuilder;
use super::driver::Compiler;

impl FunctionBuilder {
    pub(crate) fn compile_template_string_len(&mut self, parts: &[TemplateStringPart]) -> u16 {
        let literal_len = parts
            .iter()
            .filter_map(|part| match part {
                TemplateStringPart::Literal(text) => Some(text.len() as i64),
                TemplateStringPart::Expr(_) => None,
            })
            .sum::<i64>();
        let out = self.alloc();
        let k = self.k(Val::Int(literal_len));
        self.emit(Op::LoadK(out, k));
        for part in parts {
            let TemplateStringPart::Expr(expr) = part else {
                continue;
            };
            let value = self.expr(expr);
            let text = self.alloc();
            self.emit(Op::ToStr(text, value));
            let len = self.alloc();
            self.emit(Op::StrLen { dst: len, src: text });
            self.emit(Op::AddInt(out, out, len));
        }
        out
    }

    fn lookup_loop_invariant_expr(&self, expr: &Expr) -> Option<u16> {
        self.loop_invariant_expr_regs
            .iter()
            .rev()
            .find_map(|(candidate, reg)| (candidate == expr).then_some(*reg))
    }

    fn collect_inline_straight_line_body(stmt: &Stmt) -> Option<(Vec<&Stmt>, &Expr)> {
        match stmt {
            Stmt::Return { value: Some(value) } | Stmt::Expr(value) => Some((Vec::new(), value.as_ref())),
            Stmt::Block { statements } if !statements.is_empty() && statements.len() <= 8 => {
                let (last, prefix) = statements.split_last()?;
                let returned = match last.as_ref() {
                    Stmt::Return { value: Some(value) } | Stmt::Expr(value) => value.as_ref(),
                    _ => return None,
                };
                let mut inline_prefix = Vec::with_capacity(prefix.len());
                for stmt in prefix {
                    let Stmt::Let {
                        pattern: crate::expr::Pattern::Variable(_),
                        value,
                        ..
                    } = stmt.as_ref()
                    else {
                        return None;
                    };
                    if Self::expr_contains_call(value) {
                        return None;
                    }
                    inline_prefix.push(stmt.as_ref());
                }
                Some((inline_prefix, returned))
            }
            _ => None,
        }
    }

    fn inline_expr_uses_only(expr: &Expr, allowed: &HashSet<String>) -> bool {
        expr.requested_ctx().iter().all(|name| allowed.contains(name))
    }

    fn try_inline_simple_known_call(&mut self, name: &str, args: &[Box<Expr>]) -> Option<u16> {
        let Some(Val::Closure(closure)) = self.const_env.get(name).cloned() else {
            return None;
        };
        if !closure.named_params.is_empty() || !closure.capture_specs.is_empty() || closure.params.len() != args.len() {
            return None;
        }
        let (prefix, returned) = Self::collect_inline_straight_line_body(closure.body.as_ref())?;
        if Self::expr_contains_call(returned) {
            return None;
        }
        let mut allowed_names = closure.params.iter().cloned().collect::<HashSet<_>>();
        for stmt in &prefix {
            let Stmt::Let {
                pattern: crate::expr::Pattern::Variable(local_name),
                value,
                ..
            } = stmt
            else {
                return None;
            };
            if !Self::inline_expr_uses_only(value, &allowed_names) {
                return None;
            }
            allowed_names.insert(local_name.clone());
        }
        if !Self::inline_expr_uses_only(returned, &allowed_names) {
            return None;
        }

        let mut arg_regs = Vec::with_capacity(args.len());
        for arg in args {
            arg_regs.push(self.expr(arg));
        }

        self.push_var_scope();
        for (param, reg) in closure.params.iter().zip(arg_regs) {
            self.define_var_as(param, reg);
        }
        for stmt in prefix {
            let Stmt::Let {
                pattern: crate::expr::Pattern::Variable(name),
                value,
                ..
            } = stmt
            else {
                unreachable!("inline prefix was validated before emission");
            };
            let reg = self.expr(value);
            self.define_var_as(name, reg);
        }
        let result = self.expr(returned);
        self.pop_var_scope();
        Some(result)
    }

    fn local_known_positional_callee(&self, name: &str, argc: usize) -> Option<Val> {
        let local = self.lookup(name)?;
        if !self.local_reg_last_written_by_closure(local) {
            return None;
        }
        let value = self.const_env.get(name)?.clone();
        match &value {
            Val::Closure(closure) if closure.named_params.is_empty() && closure.params.len() == argc => Some(value),
            _ => None,
        }
    }

    fn local_reg_last_written_by_closure(&self, reg: u16) -> bool {
        self.code
            .iter()
            .rev()
            .find(|op| super::peephole::op_writes_reg(op, reg))
            .is_some_and(|op| matches!(op, Op::MakeClosure { dst, .. } if *dst == reg))
    }

    fn try_fold_unary(&mut self, uop: &UnaryOp, inner: &Expr) -> Option<Val> {
        if let Expr::Val(v) = inner {
            match uop {
                UnaryOp::Not => {
                    if let Val::Bool(b) = v {
                        return Some(Val::Bool(!b));
                    }
                }
            }
        }
        None
    }

    fn try_fold_bin(&mut self, op: &BinOp, l: &Expr, r: &Expr) -> Option<Val> {
        match (l, r) {
            (Expr::Val(lv), Expr::Val(rv)) => {
                let res = if op.is_arith() {
                    op.eval_vals(lv, rv)
                } else if op.is_cmp() {
                    op.cmp(lv, rv).map(Val::Bool)
                } else {
                    return None;
                };
                res.ok()
            }
            _ => None,
        }
    }

    fn emit_bin_op(&mut self, dst: u16, left: u16, right: u16, op: &BinOp, flavor: ArithFlavor) {
        let int_cmp = match op {
            BinOp::Eq => Some(IntCmpKind::Eq),
            BinOp::Ne => Some(IntCmpKind::Ne),
            BinOp::Lt => Some(IntCmpKind::Lt),
            BinOp::Le => Some(IntCmpKind::Le),
            BinOp::Gt => Some(IntCmpKind::Gt),
            BinOp::Ge => Some(IntCmpKind::Ge),
            _ => None,
        };
        if flavor == ArithFlavor::Int
            && let Some(kind) = int_cmp
            && !rk_is_const(left)
            && !rk_is_const(right)
        {
            self.emit(Op::CmpI {
                dst,
                a: left,
                b: right,
                kind,
            });
            return;
        }

        match op {
            BinOp::Add => match flavor {
                ArithFlavor::Int => self.emit(Op::AddInt(dst, left, right)),
                ArithFlavor::Float => self.emit(Op::AddFloat(dst, left, right)),
                ArithFlavor::Any => self.emit(Op::Add(dst, left, right)),
            },
            BinOp::Sub => match flavor {
                ArithFlavor::Int => self.emit(Op::SubInt(dst, left, right)),
                ArithFlavor::Float => self.emit(Op::SubFloat(dst, left, right)),
                ArithFlavor::Any => self.emit(Op::Sub(dst, left, right)),
            },
            BinOp::Mul => match flavor {
                ArithFlavor::Int => self.emit(Op::MulInt(dst, left, right)),
                ArithFlavor::Float => self.emit(Op::MulFloat(dst, left, right)),
                ArithFlavor::Any => self.emit(Op::Mul(dst, left, right)),
            },
            BinOp::Div => match flavor {
                ArithFlavor::Float => self.emit(Op::DivFloat(dst, left, right)),
                _ => self.emit(Op::Div(dst, left, right)),
            },
            BinOp::Mod => match flavor {
                ArithFlavor::Int => self.emit(Op::ModInt(dst, left, right)),
                ArithFlavor::Float => self.emit(Op::ModFloat(dst, left, right)),
                ArithFlavor::Any => self.emit(Op::Mod(dst, left, right)),
            },
            BinOp::Eq => self.emit(Op::CmpEq(dst, left, right)),
            BinOp::Ne => self.emit(Op::CmpNe(dst, left, right)),
            BinOp::Lt => self.emit(Op::CmpLt(dst, left, right)),
            BinOp::Le => self.emit(Op::CmpLe(dst, left, right)),
            BinOp::Gt => self.emit(Op::CmpGt(dst, left, right)),
            BinOp::Ge => self.emit(Op::CmpGe(dst, left, right)),
            BinOp::In => self.emit(Op::In(dst, left, right)),
        }
    }

    pub(crate) fn emit_expr_into(&mut self, dst: u16, expr: &Expr) {
        match expr {
            Expr::Val(value) => {
                let kidx = self.k(value.clone());
                self.emit(Op::LoadK(dst, kidx));
            }
            Expr::Var(name) => {
                if let Some(src) = self.lookup(name) {
                    if src != dst {
                        self.emit(Op::Move(dst, src));
                    }
                } else if let Some(value) = self.lookup_const(name).cloned() {
                    let kidx = self.k(value);
                    self.emit(Op::LoadK(dst, kidx));
                } else {
                    let kname = self.k(Val::from_str(name.as_str()));
                    self.emit(Op::LoadGlobal(dst, kname));
                }
            }
            Expr::Paren(inner) => self.emit_expr_into(dst, inner),
            Expr::Bin(left, op, right) if Self::op_supports_rk(op) => {
                if let Some(value) = self.try_fold_bin(op, left, right) {
                    let kidx = self.k(value);
                    self.emit(Op::LoadK(dst, kidx));
                    return;
                }
                let left_operand = self.expr_operand(left);
                let flavor = self.select_arith_flavor(op, left, right, expr);
                if let Some(imm) = self.try_small_int_const(right)
                    && self.try_emit_binop_imm(dst, left_operand, imm, op, flavor)
                {
                    return;
                }
                let right_operand = self.expr_operand(right);
                self.emit_bin_op(dst, left_operand, right_operand, op, flavor);
            }
            _ => {
                let src = self.expr(expr);
                if src != dst {
                    self.emit(Op::Move(dst, src));
                }
            }
        }
    }

    fn op_supports_rk(op: &BinOp) -> bool {
        matches!(
            op,
            BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
        )
    }

    fn compile_bin_expr(&mut self, root: &Expr) -> u16 {
        let mut chain: Vec<(&Expr, &Expr, &BinOp, &Expr)> = Vec::new();
        let mut current = root;
        while let Expr::Bin(left, op, right) = current {
            chain.push((current, left.as_ref(), op, right.as_ref()));
            current = left;
        }

        let mut acc = self.expr_operand(current);
        for (node_expr, left_expr, op, right_expr) in chain.into_iter().rev() {
            let use_rk = Self::op_supports_rk(op);
            let left_operand = if use_rk { acc } else { self.operand_to_reg(acc) };
            let flavor = self.select_arith_flavor(op, left_expr, right_expr, node_expr);
            let dst = self.alloc();

            if use_rk
                && let Some(imm) = self.try_small_int_const(right_expr)
                && self.try_emit_binop_imm(dst, left_operand, imm, op, flavor)
            {
                acc = dst;
                continue;
            }

            let right_operand = if use_rk {
                self.expr_operand(right_expr)
            } else {
                self.expr(right_expr)
            };
            self.emit_bin_op(dst, left_operand, right_operand, op, flavor);
            acc = dst;
        }
        acc
    }

    pub fn expr(&mut self, e: &Expr) -> u16 {
        if let Some(reg) = self.lookup_loop_invariant_expr(e) {
            return reg;
        }

        match e {
            Expr::Select { cases, default_case } => self.compile_select_expr(cases, default_case.as_deref()),
            // Template string lowering: accumulate into a string using ToStr + Add.
            // The VM fuses ToStr+Add when the lhs is already a string, avoiding
            // the temporary string for primitive interpolations on hot paths.
            Expr::TemplateString(parts) => {
                let out = self.alloc();
                let mut initialized = false;
                for part in parts {
                    match part {
                        TemplateStringPart::Literal(s) => {
                            if s.is_empty() {
                                continue;
                            }
                            if !initialized {
                                let k = self.k(Val::from_str(s.as_str()));
                                self.emit(Op::LoadK(out, k));
                                initialized = true;
                                continue;
                            }
                            let r = self.alloc();
                            let k = self.k(Val::from_str(s.as_str()));
                            self.emit(Op::LoadK(r, k));
                            self.emit(Op::StrConcatKnownCap(out, out, r));
                        }
                        TemplateStringPart::Expr(expr) => {
                            let rv = self.expr(expr);
                            if !initialized {
                                self.emit(Op::ToStr(out, rv));
                                initialized = true;
                                continue;
                            }
                            self.emit(Op::StrConcatToStr(out, out, rv));
                        }
                    }
                }
                if !initialized {
                    let k_empty = self.k(Val::from_str(""));
                    self.emit(Op::LoadK(out, k_empty));
                }
                out
            }
            Expr::Closure { params, body } => {
                let body_stmt = match body.as_ref() {
                    Expr::Block(statements) => Stmt::Block {
                        statements: statements.clone(),
                    },
                    _ => Stmt::Expr(Box::new((**body).clone())),
                };
                let captures = self.collect_captures(None, params, &[], &body_stmt);
                let compiled = Compiler::new().compile_function_with_captures(params, &[], &body_stmt, &captures);
                let proto_idx = self.protos.len() as u16;
                let func = Arc::new(compiled);
                self.protos.push(ClosureProto {
                    self_name: None,
                    params: Arc::new(params.clone()),
                    param_types: Arc::new(Vec::new()),
                    named_params: Arc::new(Vec::new()),
                    default_funcs: Arc::new(Vec::new()),
                    func: Some(Arc::clone(&func)),
                    body: Arc::new(body_stmt.clone()),
                    capture_names: capture_names_from_specs(&captures),
                    captures: Arc::new(captures),
                    code: closure_code_cell(Some(&func)),
                    empty_env: closure_empty_env(),
                    empty_upvalues: closure_empty_upvalues(),
                    empty_captures: closure_empty_captures(),
                    empty_closure: closure_empty_closure_cell(),
                });
                let dst = self.alloc();
                self.emit(Op::MakeClosure { dst, proto: proto_idx });
                dst
            }
            Expr::Val(v) => {
                let dst = self.alloc();
                let k = self.k(v.clone());
                self.emit(Op::LoadK(dst, k));
                dst
            }
            Expr::Var(name) => {
                let dst = self.alloc();
                if self.const_names.contains(name)
                    && let Some(val) = self.lookup_const(name)
                {
                    let value = val.clone();
                    let k = self.k(value);
                    self.emit(Op::LoadK(dst, k));
                } else if let Some(idx) = self.lookup(name) {
                    self.emit(Op::LoadLocal(dst, idx));
                } else if let Some(cidx) = self.capture_indices.get(name) {
                    self.emit(Op::LoadCapture { dst, idx: *cidx });
                } else if self.export_toplevel_globals && self.global_defs.contains(name) {
                    let kname = self.k(Val::from_str(name.as_str()));
                    self.emit(Op::LoadGlobal(dst, kname));
                } else {
                    // Try global lookup at runtime
                    let kname = self.k(Val::from_str(name.as_str()));
                    self.emit(Op::LoadGlobal(dst, kname));
                }
                dst
            }
            Expr::Paren(inner) => self.expr(inner),
            Expr::Unary(uop, inner) => {
                if let Some(v) = self.try_fold_unary(uop, inner) {
                    let dst = self.alloc();
                    let k = self.k(v);
                    self.emit(Op::LoadK(dst, k));
                    return dst;
                }
                let r = self.expr(inner);
                match uop {
                    UnaryOp::Not => {
                        let out = self.alloc();
                        self.emit(Op::Not(out, r));
                        out
                    }
                }
            }
            Expr::And(l, r) => {
                // Short-circuiting AND producing a boolean result:
                // rl = l; if !rl { out=false; jmp end } ; rr = r; out = bool(rr)
                let out = self.alloc();
                let rl = self.expr(l);
                let jpos = self.code.len();
                self.emit(Op::JmpFalseSet {
                    r: rl,
                    dst: out,
                    ofs: 0,
                });
                let rr = self.expr(r);
                self.emit(Op::ToBool(out, rr));
                let end = self.code.len();
                if let Op::JmpFalseSet { ofs, .. } = &mut self.code[jpos] {
                    *ofs = (end as isize - jpos as isize) as i16;
                }
                out
            }
            Expr::Or(l, r) => {
                // Short-circuiting OR producing a boolean result:
                // rl = l; if rl { out=true; jmp end } ; rr = r; out = bool(rr)
                let out = self.alloc();
                let rl = self.expr(l);
                let jpos = self.code.len();
                self.emit(Op::JmpTrueSet {
                    r: rl,
                    dst: out,
                    ofs: 0,
                });
                let rr = self.expr(r);
                self.emit(Op::ToBool(out, rr));
                let end = self.code.len();
                if let Op::JmpTrueSet { ofs, .. } = &mut self.code[jpos] {
                    *ofs = (end as isize - jpos as isize) as i16;
                }
                out
            }
            Expr::Access(base, field) => {
                if let (Expr::Val(vb), Expr::Val(vf)) = (base.as_ref(), field.as_ref()) {
                    let folded = vb.access(vf).unwrap_or(Val::Nil);
                    let dst = self.alloc();
                    let k = self.k(folded);
                    self.emit(Op::LoadK(dst, k));
                    return dst;
                }
                let b = if let Expr::Var(name) = base.as_ref() {
                    self.lookup(name).unwrap_or_else(|| self.expr(base))
                } else {
                    self.expr(base)
                };
                let out = self.alloc();
                if let Expr::Val(field_val) = field.as_ref()
                    && let Some(s) = field_val.as_str()
                {
                    let k = self.k(Val::from_str(s));
                    if self.map_locals.contains(&b) {
                        self.emit(Op::MapGetInterned(out, b, k));
                        self.mark_map_lookup_result(out, b);
                    } else {
                        self.emit(Op::AccessK(out, b, k));
                    }
                } else if let Expr::Val(Val::Int(i)) = field.as_ref() {
                    if self.list_locals.contains(&b)
                        && let Ok(index) = i16::try_from(*i)
                    {
                        self.emit(Op::ListIndexI(out, b, index));
                        self.mark_list_lookup_result(out, b);
                    } else {
                        let k = self.k(Val::Int(*i));
                        self.emit(Op::IndexK(out, b, k));
                        if self.list_locals.contains(&b) {
                            self.mark_list_lookup_result(out, b);
                        }
                    }
                } else {
                    let f = self.expr(field);
                    if self.map_locals.contains(&b) {
                        self.emit(Op::MapGetDynamic(out, b, f));
                        self.mark_map_lookup_result(out, b);
                    } else {
                        self.emit(Op::Access(out, b, f));
                        if self.list_locals.contains(&b) && self.int_regs.contains(&f) {
                            self.mark_list_lookup_result(out, b);
                        }
                    }
                }
                out
            }
            Expr::OptionalAccess(base, field) => {
                let b = self.expr(base);
                let out = self.alloc();
                let j_is_nil = self.code.len();
                self.emit(Op::JmpIfNil(b, 0));
                if let Expr::Val(Val::Int(i)) = field.as_ref() {
                    let k = self.k(Val::Int(*i));
                    self.emit(Op::IndexK(out, b, k));
                } else if let Expr::Val(field_val2) = field.as_ref()
                    && let Some(s) = field_val2.as_str()
                {
                    let k = self.k(Val::from_str(s));
                    self.emit(Op::AccessK(out, b, k));
                } else {
                    let f = self.expr(field);
                    self.emit(Op::Access(out, b, f));
                }
                let jend = self.code.len();
                self.emit(Op::Jmp(0));
                let nil_path = self.code.len();
                if let Op::JmpIfNil(_, ref mut ofs) = self.code[j_is_nil] {
                    *ofs = (nil_path as isize - j_is_nil as isize) as i16;
                }
                let k_nil = self.k(Val::Nil);
                self.emit(Op::LoadK(out, k_nil));
                let end = self.code.len();
                if let Op::Jmp(ref mut ofs) = self.code[jend] {
                    *ofs = (end as isize - jend as isize) as i16;
                }
                out
            }
            Expr::NullishCoalescing(l, r) => {
                if let Expr::Val(vl) = l.as_ref() {
                    if *vl == Val::Nil {
                        return self.expr(r);
                    } else {
                        let dst = self.alloc();
                        let k = self.k(vl.clone());
                        self.emit(Op::LoadK(dst, k));
                        return dst;
                    }
                }
                let out = self.alloc();
                let rl = self.expr(l);
                let pick_pos = self.code.len();
                self.emit(Op::NullishPick {
                    l: rl,
                    dst: out,
                    ofs: 0,
                });
                let rr = self.expr(r);
                self.emit(Op::Move(out, rr));
                let end = self.code.len();
                if let Op::NullishPick { ofs, .. } = &mut self.code[pick_pos] {
                    *ofs = (end as isize - pick_pos as isize) as i16;
                }
                out
            }
            Expr::Bin(l, op, r) => {
                if let Some(v) = self.try_fold_bin(op, l, r) {
                    let dst = self.alloc();
                    let k = self.k(v);
                    self.emit(Op::LoadK(dst, k));
                    return dst;
                }
                self.compile_bin_expr(e)
            }
            Expr::Match { value, arms } => {
                let src = self.expr(value);
                let out = self.alloc();
                let mut end_jumps = Vec::with_capacity(arms.len());

                for arm in arms {
                    self.push_var_scope();
                    let plan = self.register_scoped_pattern_plan(&arm.pattern);
                    let matched = self.alloc();
                    self.emit(Op::PatternMatch {
                        dst: matched,
                        src,
                        plan,
                    });
                    let fail_jump = self.code.len();
                    self.emit(Op::JmpFalse(matched, 0));

                    let body = self.expr(&arm.body);
                    if body != out {
                        self.emit(Op::Move(out, body));
                    }
                    let end_jump = self.code.len();
                    self.emit(Op::Jmp(0));
                    end_jumps.push(end_jump);

                    let next_arm = self.code.len();
                    if let Op::JmpFalse(_, ofs) = &mut self.code[fail_jump] {
                        *ofs = (next_arm as isize - fail_jump as isize) as i16;
                    }
                    self.pop_var_scope();
                }

                let err_kidx = self.k(Val::from_str("No pattern matched"));
                self.emit(Op::Raise { err_kidx });
                let end = self.code.len();
                for jump in end_jumps {
                    if let Op::Jmp(ofs) = &mut self.code[jump] {
                        *ofs = (end as isize - jump as isize) as i16;
                    }
                }
                out
            }
            Expr::Range { .. } => {
                let dst = self.alloc();
                let value = self.try_eval_const_expr(e).unwrap_or(Val::Nil);
                let k = self.k(value);
                self.emit(Op::LoadK(dst, k));
                dst
            }
            Expr::List(items) => {
                let dst = self.alloc();
                let count = items.len() as u16;
                let base = self.n_regs;
                for _ in 0..items.len() {
                    let _ = self.alloc();
                }
                for (i, it) in items.iter().enumerate() {
                    let ri = self.expr(it);
                    let d = base + i as u16;
                    if ri != d {
                        self.emit(Op::Move(d, ri));
                    }
                }
                self.emit(Op::BuildList { dst, base, len: count });
                if items.is_empty() {
                    self.record_empty_list_value_type(dst);
                } else {
                    let value_fact = self.homogeneous_expr_value_fact(items.iter().map(|expr| expr.as_ref()));
                    self.record_list_value_type(dst, value_fact);
                }
                dst
            }
            Expr::Map(pairs) => {
                let dst = self.alloc();
                self.map_locals.insert(dst);
                let n = pairs.len() as u16;
                let base = self.n_regs;
                for _ in 0..(pairs.len() * 2) {
                    let _ = self.alloc();
                }
                for (i, (k, v)) in pairs.iter().enumerate() {
                    let rk = self.expr(k);
                    let rv = self.expr(v);
                    let dk = base + (2 * i) as u16;
                    let dv = dk + 1;
                    if rk != dk {
                        self.emit(Op::Move(dk, rk));
                    }
                    if rv != dv {
                        self.emit(Op::Move(dv, rv));
                    }
                }
                self.emit(Op::BuildMap { dst, base, len: n });
                if pairs.is_empty() {
                    self.record_empty_map_value_type(dst);
                } else {
                    let value_fact = self.homogeneous_expr_value_fact(pairs.iter().map(|(_, value)| value.as_ref()));
                    self.record_map_value_type(dst, value_fact);
                }
                dst
            }
            Expr::StructLiteral { name, fields } => {
                let fields_map = self.emit_map_from_named_args(fields);
                let type_reg = self.alloc();
                let type_idx = self.k(Val::from_str(name.as_str()));
                self.emit(Op::LoadK(type_reg, type_idx));

                let known_builtin = self.const_env.get("__lk_make_struct").cloned();
                let builtin_reg = self.emit_known_or_global_callable("__lk_make_struct", known_builtin.as_ref());

                let base = self.alloc();
                let arg_type = base;
                let arg_fields = self.alloc();

                if type_reg != arg_type {
                    self.emit(Op::Move(arg_type, type_reg));
                }
                if fields_map != arg_fields {
                    self.emit(Op::Move(arg_fields, fields_map));
                }

                self.emit_positional_call(builtin_reg, base, 2, 1, known_builtin.as_ref());
                base
            }
            Expr::Call(name, args) => {
                if let Some(inlined) = self.try_inline_simple_known_call(name, args) {
                    return inlined;
                }
                // If the callee is a locally-defined function registered in const_env,
                // load it from the constant pool instead of via LoadGlobal (avoids hashtable lookup).
                let known_callee = self.const_env.get(name).cloned();
                let use_direct_call = known_callee.is_some() && self.call_safe_to_fold(name);
                let f = if use_direct_call {
                    let func_val = known_callee.as_ref().expect("known callee checked above");
                    let kidx = self.k(func_val.clone());
                    let f = self.alloc();
                    self.emit(Op::LoadK(f, kidx));
                    f
                } else if let Some(local) = self.lookup(name) {
                    local
                } else {
                    let kname = self.k(Val::from_str(name.as_str()));
                    let f = self.alloc();
                    self.emit(Op::LoadGlobal(f, kname));
                    f
                };
                let argc = args.len() as u8;
                let base = self.reserve_call_window(args.len(), 1);
                for (i, arg) in args.iter().enumerate() {
                    self.emit_expr_into(base + i as u16, arg);
                }
                self.emit_positional_call(f, base, argc, 1, known_callee.as_ref());
                self.mark_direct_call_return_type(name, base);
                base
            }
            Expr::CallExpr(callee, args) => {
                if let Expr::Access(obj_expr, field_expr) = callee.as_ref() {
                    return self.compile_method_call(obj_expr, field_expr, args.as_slice());
                }
                if let Expr::Var(name) = callee.as_ref()
                    && let Some(inlined) = self.try_inline_simple_known_call(name, args)
                {
                    return inlined;
                }
                let known_callee = if let Expr::Var(name) = callee.as_ref() {
                    self.local_known_positional_callee(name, args.len()).or_else(|| {
                        (self.lookup(name).is_none())
                            .then(|| self.lookup_const(name).cloned())
                            .flatten()
                    })
                } else {
                    None
                };
                let f = if let Expr::Var(name) = callee.as_ref() {
                    self.lookup(name).unwrap_or_else(|| self.expr(callee))
                } else {
                    self.expr(callee)
                };
                let argc = args.len() as u8;
                let base = self.reserve_call_window(args.len(), 1);
                for (i, arg) in args.iter().enumerate() {
                    self.emit_expr_into(base + i as u16, arg);
                }
                self.emit_positional_call(f, base, argc, 1, known_callee.as_ref());
                if let Expr::Var(name) = callee.as_ref() {
                    self.mark_direct_call_return_type(name, base);
                }
                base
            }
            Expr::CallNamed(callee, pos_args, named_args) => {
                if let Expr::Access(obj_expr, field_expr) = callee.as_ref() {
                    return self.compile_method_call_named(
                        obj_expr,
                        field_expr,
                        pos_args.as_slice(),
                        named_args.as_slice(),
                    );
                }
                let f = self.expr(callee);
                let base_pos = self.reserve_call_window(pos_args.len(), 1);
                for (i, arg) in pos_args.iter().enumerate() {
                    let ri = self.expr(arg);
                    let dst = base_pos + i as u16;
                    if ri != dst {
                        self.emit(Op::Move(dst, ri));
                    }
                }
                let base_named = self.n_regs;
                for _ in 0..(named_args.len() * 2) {
                    let _ = self.alloc();
                }
                for (i, (name, expr)) in named_args.iter().enumerate() {
                    let kname = self.k(Val::from_str(name.as_str()));
                    let name_reg = base_named + (2 * i) as u16;
                    self.emit(Op::LoadK(name_reg, kname));
                    let vreg = self.expr(expr);
                    let dstv = name_reg + 1;
                    if vreg != dstv {
                        self.emit(Op::Move(dstv, vreg));
                    }
                }
                self.emit(Op::CallNamed {
                    f,
                    base_pos,
                    posc: pos_args.len() as u8,
                    base_named,
                    namedc: named_args.len() as u8,
                    retc: 1,
                });
                base_pos
            }
            _ => {
                let dst = self.alloc();
                let k = self.k(Val::Nil);
                self.emit(Op::LoadK(dst, k));
                dst
            }
        }
    }
}
