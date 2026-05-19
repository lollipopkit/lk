use super::FunctionBuilder;
use crate::{
    expr::{Expr, SelectCase, SelectPattern},
    val::Val,
    vm::Op,
};

impl FunctionBuilder {
    pub(crate) fn compile_select_expr(&mut self, cases: &[SelectCase], default_case: Option<&Expr>) -> u16 {
        let n = cases.len() as u16;
        if n == 0 {
            if let Some(def) = default_case {
                return self.expr(def);
            }
            let dst = self.alloc();
            let k = self.k(Val::Nil);
            self.emit(Op::LoadK(dst, k));
            return dst;
        }

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

        let base_ch = self.n_regs;
        for case in cases {
            let r = self.alloc();
            let channel = match &case.pattern {
                SelectPattern::Recv { channel, .. } | SelectPattern::Send { channel, .. } => channel,
            };
            let rv = self.expr(channel);
            if rv != r {
                self.emit(Op::Move(r, rv));
            }
        }
        let r_chans = self.alloc();
        self.emit(Op::BuildList {
            dst: r_chans,
            base: base_ch,
            len: n,
        });

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

        let r_has_def = self.alloc();
        let k_hd = self.k(Val::Bool(default_case.is_some()));
        self.emit(Op::LoadK(r_has_def, k_hd));

        let known_builtin = self.const_env.get("select$block").cloned();
        let r_f = self.emit_known_or_global_callable("select$block", known_builtin.as_ref());
        let base = self.n_regs;
        let _ = self.alloc();
        let _ = self.alloc();
        let _ = self.alloc();
        let _ = self.alloc();
        let _ = self.alloc();
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
        self.emit_positional_call(r_f, base, 5, 1, known_builtin.as_ref());

        let out = self.alloc();
        let k_nil = self.k(Val::Nil);
        self.emit(Op::LoadK(out, k_nil));

        let r_is_def = self.alloc();
        let k0 = self.k(Val::Int(0));
        self.emit(Op::IndexK(r_is_def, base, k0));
        let j_non_default = self.code.len();
        self.emit(Op::JmpFalse(r_is_def, 0));
        if let Some(def) = default_case {
            let rdef = self.expr(def);
            if rdef != out {
                self.emit(Op::Move(out, rdef));
            }
        }
        let j_end_default = self.code.len();
        self.emit(Op::Jmp(0));

        let non_default_label = self.code.len();
        if let Op::JmpFalse(_, ref mut ofs) = self.code[j_non_default] {
            *ofs = (non_default_label as isize - j_non_default as isize) as i16;
        }

        let r_idx = self.alloc();
        let k1 = self.k(Val::Int(1));
        self.emit(Op::IndexK(r_idx, base, k1));
        let r_payload = self.alloc();
        let k2 = self.k(Val::Int(2));
        self.emit(Op::IndexK(r_payload, base, k2));

        let mut end_jumps = Vec::new();
        for (i, case) in cases.iter().enumerate() {
            let r_ki = self.alloc();
            let k_i = self.k(Val::Int(i as i64));
            self.emit(Op::LoadK(r_ki, k_i));
            let r_cmp = self.alloc();
            self.emit(Op::CmpEq(r_cmp, r_idx, r_ki));
            let jf = self.code.len();
            self.emit(Op::JmpFalse(r_cmp, 0));
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
}
