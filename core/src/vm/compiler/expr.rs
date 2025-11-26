use super::builder::ArithFlavor;
use crate::{
    expr::{Expr, SelectPattern, TemplateStringPart},
    op::{BinOp, UnaryOp},
    stmt::Stmt,
    val::Val,
    vm::{
        ClosureProto, Op,
        bytecode::{rk_as_const, rk_index, rk_is_const, rk_make_const},
    },
};

use super::FunctionBuilder;
use super::driver::Compiler;

impl FunctionBuilder {
    fn emit_list_from_exprs(&mut self, items: &[Box<Expr>]) -> u16 {
        let dst = self.alloc();
        let count = items.len() as u16;
        let base = self.n_regs;
        for _ in 0..items.len() {
            let _ = self.alloc();
        }
        for (i, expr) in items.iter().enumerate() {
            let ri = self.expr(expr);
            let d = base + i as u16;
            if ri != d {
                self.emit(Op::Move(d, ri));
            }
        }
        self.emit(Op::BuildList { dst, base, len: count });
        dst
    }

    fn emit_map_from_named_args(&mut self, named_args: &[(String, Box<Expr>)]) -> u16 {
        let dst = self.alloc();
        let count = named_args.len() as u16;
        let base = self.n_regs;
        for _ in 0..(named_args.len() * 2) {
            let _ = self.alloc();
        }
        for (i, (name, expr)) in named_args.iter().enumerate() {
            let key_reg = base + (2 * i) as u16;
            let key_idx = self.k(Val::Str(name.clone().into()));
            self.emit(Op::LoadK(key_reg, key_idx));
            let val_reg = self.expr(expr);
            let dst_reg = key_reg + 1;
            if val_reg != dst_reg {
                self.emit(Op::Move(dst_reg, val_reg));
            }
        }
        self.emit(Op::BuildMap { dst, base, len: count });
        dst
    }

    fn compile_method_call(&mut self, obj_expr: &Expr, field_expr: &Expr, args: &[Box<Expr>]) -> u16 {
        let obj_reg = self.expr(obj_expr);
        let pos_list = self.emit_list_from_exprs(args);
        let method_reg = self.expr(field_expr);

        let builtin_reg = self.alloc();
        let builtin_idx = self.k(Val::Str("__lkr_call_method".into()));
        self.emit(Op::LoadGlobal(builtin_reg, builtin_idx));

        let base = self.alloc();
        let arg_obj = base;
        let arg_method = self.alloc();
        let arg_list = self.alloc();

        if obj_reg != arg_obj {
            self.emit(Op::Move(arg_obj, obj_reg));
        }
        if method_reg != arg_method {
            self.emit(Op::Move(arg_method, method_reg));
        }
        if pos_list != arg_list {
            self.emit(Op::Move(arg_list, pos_list));
        }

        self.emit(Op::Call {
            f: builtin_reg,
            base,
            argc: 3,
            retc: 1,
        });
        base
    }

    fn compile_method_call_named(
        &mut self,
        obj_expr: &Expr,
        field_expr: &Expr,
        pos_args: &[Box<Expr>],
        named_args: &[(String, Box<Expr>)],
    ) -> u16 {
        let obj_reg = self.expr(obj_expr);
        let pos_list = self.emit_list_from_exprs(pos_args);
        let named_map = self.emit_map_from_named_args(named_args);
        let method_reg = self.expr(field_expr);

        let builtin_reg = self.alloc();
        let builtin_idx = self.k(Val::Str("__lkr_call_method_named".into()));
        self.emit(Op::LoadGlobal(builtin_reg, builtin_idx));

        let base = self.alloc();
        let arg_obj = base;
        let arg_method = self.alloc();
        let arg_pos = self.alloc();
        let arg_named = self.alloc();

        if obj_reg != arg_obj {
            self.emit(Op::Move(arg_obj, obj_reg));
        }
        if method_reg != arg_method {
            self.emit(Op::Move(arg_method, method_reg));
        }
        if pos_list != arg_pos {
            self.emit(Op::Move(arg_pos, pos_list));
        }
        if named_map != arg_named {
            self.emit(Op::Move(arg_named, named_map));
        }

        self.emit(Op::Call {
            f: builtin_reg,
            base,
            argc: 4,
            retc: 1,
        });
        base
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

    fn try_const_operand(&mut self, expr: &Expr) -> Option<u16> {
        match expr {
            Expr::Val(v) => Some(self.k(v.clone())),
            Expr::Var(name) => {
                if let Some(val) = self.lookup_const(name) {
                    Some(self.k(val.clone()))
                } else {
                    None
                }
            }
            Expr::Paren(inner) => self.try_const_operand(inner),
            _ => None,
        }
    }

    fn try_small_int_const(&self, expr: &Expr) -> Option<i16> {
        fn fits_i8(value: i64) -> Option<i16> {
            if (-128..=127).contains(&value) {
                Some(value as i16)
            } else {
                None
            }
        }

        match expr {
            Expr::Val(Val::Int(v)) => fits_i8(*v),
            Expr::Var(name) => self.lookup_const(name).and_then(|val| match val {
                Val::Int(v) => fits_i8(*v),
                _ => None,
            }),
            Expr::Paren(inner) => self.try_small_int_const(inner),
            _ => None,
        }
    }

    fn expr_operand(&mut self, expr: &Expr) -> u16 {
        if let Some(kidx) = self.try_const_operand(expr) {
            rk_make_const(kidx)
        } else {
            self.expr(expr)
        }
    }

    fn operand_to_reg(&mut self, operand: u16) -> u16 {
        if rk_is_const(operand) {
            let dst = self.alloc();
            let kidx = rk_as_const(operand);
            self.emit(Op::LoadK(dst, kidx));
            dst
        } else {
            operand
        }
    }

    fn try_emit_binop_imm(&mut self, dst: u16, left_operand: u16, imm: i16, op: &BinOp, flavor: ArithFlavor) -> bool {
        if rk_is_const(left_operand) {
            return false;
        }
        let left_reg = rk_index(left_operand);

        let mut emit_add_int_imm = |value: i16| {
            self.emit(Op::AddIntImm(dst, left_reg, value));
            true
        };

        match op {
            BinOp::Add => {
                if flavor != ArithFlavor::Float {
                    return emit_add_int_imm(imm);
                }
            }
            BinOp::Sub => {
                if flavor != ArithFlavor::Float {
                    if let Some(neg) = imm.checked_neg() {
                        if (-128..=127).contains(&neg) {
                            return emit_add_int_imm(neg);
                        }
                    }
                }
            }
            BinOp::Eq => {
                self.emit(Op::CmpEqImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Ne => {
                self.emit(Op::CmpNeImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Lt => {
                self.emit(Op::CmpLtImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Le => {
                self.emit(Op::CmpLeImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Gt => {
                self.emit(Op::CmpGtImm(dst, left_reg, imm));
                return true;
            }
            BinOp::Ge => {
                self.emit(Op::CmpGeImm(dst, left_reg, imm));
                return true;
            }
            _ => {}
        }

        false
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

            if use_rk {
                if let Some(imm) = self.try_small_int_const(right_expr) {
                    if self.try_emit_binop_imm(dst, left_operand, imm, op, flavor) {
                        acc = dst;
                        continue;
                    }
                }
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
        match e {
            // Concurrency: select { case pat <= recv(ch) => expr; case _ <= send(ch, v) => expr; default => expr }
            // Blocking semantics via stdlib builtin `select$block` and runtime SelectOperation.
            Expr::Select { cases, default_case } => {
                let n = cases.len() as u16;
                // Handle degenerate case: no arms
                if n == 0 {
                    if let Some(def) = default_case {
                        return self.expr(def);
                    } else {
                        let dst = self.alloc();
                        let k = self.k(Val::Nil);
                        self.emit(Op::LoadK(dst, k));
                        return dst;
                    }
                }

                // Build arrays: types[0=recv,1=send], channels, values (Nil for recv), guards (Bool)
                // types
                let base_types = self.n_regs;
                for case in cases {
                    let r = self.alloc();
                    let t = match &case.pattern {
                        SelectPattern::Recv { .. } => 0i64,
                        SelectPattern::Send { .. } => 1i64,
                    };
                    let k = self.k(Val::Int(t));
                    self.emit(Op::LoadK(r, k));
                }
                let r_types = self.alloc();
                self.emit(Op::BuildList {
                    dst: r_types,
                    base: base_types,
                    len: n,
                });

                // channels
                let base_ch = self.n_regs;
                for case in cases {
                    let r = self.alloc();
                    match &case.pattern {
                        SelectPattern::Recv { channel, .. } => {
                            let rv = self.expr(channel);
                            if rv != r {
                                self.emit(Op::Move(r, rv));
                            }
                        }
                        SelectPattern::Send { channel, .. } => {
                            let rv = self.expr(channel);
                            if rv != r {
                                self.emit(Op::Move(r, rv));
                            }
                        }
                    }
                }
                let r_chans = self.alloc();
                self.emit(Op::BuildList {
                    dst: r_chans,
                    base: base_ch,
                    len: n,
                });

                // values
                let base_vals = self.n_regs;
                for case in cases {
                    let r = self.alloc();
                    match &case.pattern {
                        SelectPattern::Recv { .. } => {
                            let k = self.k(Val::Nil);
                            self.emit(Op::LoadK(r, k));
                        }
                        SelectPattern::Send { value, .. } => {
                            let rv = self.expr(value);
                            if rv != r {
                                self.emit(Op::Move(r, rv));
                            }
                        }
                    }
                }
                let r_vals = self.alloc();
                self.emit(Op::BuildList {
                    dst: r_vals,
                    base: base_vals,
                    len: n,
                });

                // guards
                let base_guards = self.n_regs;
                for case in cases {
                    let r = self.alloc();
                    if let Some(g) = &case.guard {
                        let gv = self.expr(g);
                        self.emit(Op::ToBool(r, gv));
                    } else {
                        let k = self.k(Val::Bool(true));
                        self.emit(Op::LoadK(r, k));
                    }
                }
                let r_guards = self.alloc();
                self.emit(Op::BuildList {
                    dst: r_guards,
                    base: base_guards,
                    len: n,
                });

                // has_default flag
                let r_has_def = self.alloc();
                let k_hd = self.k(Val::Bool(default_case.is_some()));
                self.emit(Op::LoadK(r_has_def, k_hd));

                // Call builtin select$block(types, channels, values, guards, has_default)
                let r_f = self.alloc();
                let kf = self.k(Val::Str("select$block".into()));
                self.emit(Op::LoadGlobal(r_f, kf));
                let base = self.n_regs;
                let _ = self.alloc(); // arg0
                let _ = self.alloc(); // arg1
                let _ = self.alloc(); // arg2
                let _ = self.alloc(); // arg3
                let _ = self.alloc(); // arg4
                if r_types != base {
                    self.emit(Op::Move(base, r_types));
                }
                if r_chans != base + 1 {
                    self.emit(Op::Move(base + 1, r_chans));
                }
                if r_vals != base + 2 {
                    self.emit(Op::Move(base + 2, r_vals));
                }
                if r_guards != base + 3 {
                    self.emit(Op::Move(base + 3, r_guards));
                }
                if r_has_def != base + 4 {
                    self.emit(Op::Move(base + 4, r_has_def));
                }
                self.emit(Op::Call {
                    f: r_f,
                    base,
                    argc: 5,
                    retc: 1,
                });

                let out = self.alloc();
                let k_nil = self.k(Val::Nil);
                self.emit(Op::LoadK(out, k_nil));

                // Decode result: [is_default, case_index, payload]
                let r_is_def = self.alloc();
                let k0 = self.k(Val::Int(0));
                self.emit(Op::IndexK(r_is_def, base, k0));
                let j_non_default = self.code.len();
                self.emit(Op::JmpFalse(r_is_def, 0));
                // Default path
                if let Some(def) = default_case {
                    let rdef = self.expr(def);
                    if rdef != out {
                        self.emit(Op::Move(out, rdef));
                    }
                } else {
                    // out already nil
                }
                let j_end_default = self.code.len();
                self.emit(Op::Jmp(0));
                // Non-default path
                let non_default_label = self.code.len();
                if let Op::JmpFalse(_, ref mut ofs) = self.code[j_non_default] {
                    *ofs = (non_default_label as isize - j_non_default as isize) as i16;
                }
                // Extract case_index and payload
                let r_idx = self.alloc();
                let k1 = self.k(Val::Int(1));
                self.emit(Op::IndexK(r_idx, base, k1));
                let r_payload = self.alloc();
                let k2 = self.k(Val::Int(2));
                self.emit(Op::IndexK(r_payload, base, k2));

                let mut end_jumps: Vec<usize> = Vec::new();
                // Dispatch by original case index
                for (i, case) in cases.iter().enumerate() {
                    let r_ki = self.alloc();
                    let k_i = self.k(Val::Int(i as i64));
                    self.emit(Op::LoadK(r_ki, k_i));
                    let r_cmp = self.alloc();
                    self.emit(Op::CmpEq(r_cmp, r_idx, r_ki));
                    let jf = self.code.len();
                    self.emit(Op::JmpFalse(r_cmp, 0));
                    // Matched arm: optional binding for recv, then evaluate body
                    if let SelectPattern::Recv {
                        binding: Some(name), ..
                    } = &case.pattern
                    {
                        let idx = self.get_or_define(name);
                        self.emit(Op::StoreLocal(idx, r_payload));
                    }
                    let r_body = self.expr(&case.body);
                    if r_body != out {
                        self.emit(Op::Move(out, r_body));
                    }
                    let jend = self.code.len();
                    self.emit(Op::Jmp(0));
                    end_jumps.push(jend);
                    let after = self.code.len();
                    if let Op::JmpFalse(_, ref mut ofs) = self.code[jf] {
                        *ofs = (after as isize - jf as isize) as i16;
                    }
                }
                let end = self.code.len();
                if let Op::Jmp(ref mut ofs) = self.code[j_end_default] {
                    *ofs = (end as isize - j_end_default as isize) as i16;
                }
                for j in end_jumps {
                    if let Op::Jmp(ref mut ofs) = self.code[j] {
                        *ofs = (end as isize - j as isize) as i16;
                    }
                }
                out
            }
            // Template string lowering: accumulate into a string using ToStr + Add
            Expr::TemplateString(parts) => {
                // out = ""
                let out = self.alloc();
                let k_empty = self.k(Val::Str("".into()));
                self.emit(Op::LoadK(out, k_empty));
                for part in parts {
                    match part {
                        TemplateStringPart::Literal(s) => {
                            let r = self.alloc();
                            let k = self.k(Val::Str(s.as_str().into()));
                            self.emit(Op::LoadK(r, k));
                            self.emit(Op::Add(out, out, r));
                        }
                        TemplateStringPart::Expr(expr) => {
                            // Compile inner expr, convert to string, then append
                            let rv = self.expr(expr);
                            let rs = self.alloc();
                            self.emit(Op::ToStr(rs, rv));
                            self.emit(Op::Add(out, out, rs));
                        }
                    }
                }
                out
            }
            Expr::Closure { params, body } => {
                let body_stmt = Stmt::Expr(Box::new((**body).clone()));
                let captures = self.collect_captures(None, params, &[], &body_stmt);
                let compiled = Compiler::new().compile_function_with_captures(params, &[], &body_stmt, &captures);
                let proto_idx = self.protos.len() as u16;
                self.protos.push(ClosureProto {
                    self_name: None,
                    params: params.clone(),
                    named_params: Vec::new(),
                    default_funcs: Vec::new(),
                    func: Some(Box::new(compiled)),
                    body: body_stmt.clone(),
                    captures,
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
                if let Some(val) = self.lookup_const(name) {
                    let value = val.clone();
                    let dst = self.alloc();
                    let k = self.k(value);
                    self.emit(Op::LoadK(dst, k));
                    return dst;
                }
                let dst = self.alloc();
                if let Some(idx) = self.lookup(name) {
                    self.emit(Op::LoadLocal(dst, idx));
                } else if let Some(cidx) = self.capture_indices.get(name) {
                    self.emit(Op::LoadCapture { dst, idx: *cidx });
                } else {
                    // Try global lookup at runtime
                    let kname = self.k(Val::Str(name.clone().into()));
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
                let b = self.expr(base);
                let out = self.alloc();
                if let Expr::Val(Val::Str(s)) = field.as_ref() {
                    let k = self.k(Val::Str(s.clone()));
                    self.emit(Op::AccessK(out, b, k));
                } else if let Expr::Val(Val::Int(i)) = field.as_ref() {
                    let k = self.k(Val::Int(*i));
                    self.emit(Op::IndexK(out, b, k));
                } else {
                    let f = self.expr(field);
                    self.emit(Op::Access(out, b, f));
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
                } else if let Expr::Val(Val::Str(s)) = field.as_ref() {
                    let k = self.k(Val::Str(s.clone()));
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
                return self.compile_bin_expr(e);
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
                dst
            }
            Expr::Map(pairs) => {
                let dst = self.alloc();
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
                dst
            }
            Expr::StructLiteral { name, fields } => {
                let fields_map = self.emit_map_from_named_args(fields);
                let type_reg = self.alloc();
                let type_idx = self.k(Val::Str(name.clone().into()));
                self.emit(Op::LoadK(type_reg, type_idx));

                let builtin_reg = self.alloc();
                let builtin_idx = self.k(Val::Str("__lkr_make_struct".into()));
                self.emit(Op::LoadGlobal(builtin_reg, builtin_idx));

                let base = self.alloc();
                let arg_type = base;
                let arg_fields = self.alloc();

                if type_reg != arg_type {
                    self.emit(Op::Move(arg_type, type_reg));
                }
                if fields_map != arg_fields {
                    self.emit(Op::Move(arg_fields, fields_map));
                }

                self.emit(Op::Call {
                    f: builtin_reg,
                    base,
                    argc: 2,
                    retc: 1,
                });
                base
            }
            Expr::Call(name, args) => {
                let f = self.alloc();
                let kname = self.k(Val::Str(name.clone().into()));
                self.emit(Op::LoadGlobal(f, kname));
                let argc = args.len() as u8;
                let base = self.n_regs;
                for _ in 0..args.len() {
                    let _ = self.alloc();
                }
                for (i, arg) in args.iter().enumerate() {
                    let ri = self.expr(arg);
                    let dst = base + i as u16;
                    if ri != dst {
                        self.emit(Op::Move(dst, ri));
                    }
                }
                self.emit(Op::Call { f, base, argc, retc: 1 });
                base
            }
            Expr::CallExpr(callee, args) => {
                if let Expr::Access(obj_expr, field_expr) = callee.as_ref() {
                    return self.compile_method_call(obj_expr, field_expr, args.as_slice());
                }
                let f = self.expr(callee);
                let argc = args.len() as u8;
                let base = self.n_regs;
                for _ in 0..args.len() {
                    let _ = self.alloc();
                }
                for (i, arg) in args.iter().enumerate() {
                    let ri = self.expr(arg);
                    let dst = base + i as u16;
                    if ri != dst {
                        self.emit(Op::Move(dst, ri));
                    }
                }
                self.emit(Op::Call { f, base, argc, retc: 1 });
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
                let base_pos = self.n_regs;
                for _ in 0..pos_args.len() {
                    let _ = self.alloc();
                }
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
                    let kname = self.k(Val::Str(name.clone().into()));
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
