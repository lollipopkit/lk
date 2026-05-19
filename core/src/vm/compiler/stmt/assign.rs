use crate::{expr::Expr, val::Val, vm::Op};

use super::super::FunctionBuilder;

impl FunctionBuilder {
    pub(crate) fn stmt_assign(&mut self, name: &str, value: &Expr) {
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
                self.const_bindings.insert(name.to_string(), val.clone());
            }
        } else if !is_const_target {
            self.forget_known_value(name);
        }

        if let Some(idx) = self.lookup(name) {
            if !is_const_target {
                if let Some(val) = const_value {
                    let k = self.k(val);
                    self.emit(Op::LoadK(idx, k));
                    return;
                }
                if !FunctionBuilder::expr_contains_call(value) {
                    self.emit_expr_into(idx, value);
                    return;
                }
            }

            let rv = self.expr(value);
            self.store_named(name, idx, rv);
            return;
        }

        let msg = if is_const_target {
            format!("Cannot assign to const variable '{}'", name)
        } else {
            format!("Undefined variable: {}", name)
        };
        let msg_idx = self.k(Val::from_str(msg.as_str()));
        self.emit(Op::Raise { err_kidx: msg_idx });
    }
}
