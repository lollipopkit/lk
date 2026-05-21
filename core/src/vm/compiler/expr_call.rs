use super::FunctionBuilder;
use crate::{expr::Expr, op::BinOp, val::Val, vm::Op};

pub(super) fn same_string_literal(lhs: &Expr, rhs: &Expr) -> bool {
    matches!((lhs, rhs), (Expr::Val(lhs), Expr::Val(rhs)) if lhs.as_str().is_some() && lhs.as_str() == rhs.as_str())
}

pub(super) fn split_join_same_separator_receiver<'a>(obj_expr: &'a Expr, args: &[Box<Expr>]) -> Option<&'a Expr> {
    if args.len() != 1 {
        return None;
    }
    let Expr::CallExpr(split_callee, split_args) = obj_expr else {
        return None;
    };
    if split_args.len() != 1 || !same_string_literal(split_args[0].as_ref(), args[0].as_ref()) {
        return None;
    }
    let Expr::Access(split_receiver, split_field) = split_callee.as_ref() else {
        return None;
    };
    let Expr::Val(split_method) = split_field.as_ref() else {
        return None;
    };
    (split_method.as_str() == Some("split")).then_some(split_receiver.as_ref())
}

impl FunctionBuilder {
    pub(crate) fn compile_method_call(&mut self, obj_expr: &Expr, field_expr: &Expr, args: &[Box<Expr>]) -> u16 {
        if args.len() == 1
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("join")
            && let Some(split_receiver) = split_join_same_separator_receiver(obj_expr, args)
        {
            return self.expr(split_receiver);
        }
        if args.len() == 3
            && self.unshadowed_module(obj_expr, "list")
            && self.literal_method_name(field_expr) == Some("set")
            && let Some(list_reg) = self.known_list_expr(args[0].as_ref())
            && let Some(dst) = self.emit_list_set_i(list_reg, &args[1], &args[2])
        {
            return dst;
        }
        if args.len() == 2
            && self.unshadowed_module(obj_expr, "list")
            && self.literal_method_name(field_expr) == Some("get")
            && let Some(list_reg) = self.known_list_expr(args[0].as_ref())
        {
            return self.emit_list_get_access(list_reg, &args[1]);
        }
        if args.len() == 2
            && self.unshadowed_module(obj_expr, "map")
            && self.literal_method_name(field_expr) == Some("get")
        {
            let map_reg = self.expr_or_lookup(args[0].as_ref());
            return self.emit_map_access(map_reg, &args[1]);
        }
        if args.len() == 2
            && self.unshadowed_module(obj_expr, "map")
            && self.literal_method_name(field_expr) == Some("has")
            && let Some(map_reg) = self.known_map_expr(args[0].as_ref())
        {
            return self.emit_map_has(map_reg, &args[1]);
        }
        if args.len() == 3
            && self.unshadowed_module(obj_expr, "map")
            && self.literal_method_name(field_expr) == Some("set")
            && let Expr::Var(map_name) = args[0].as_ref()
        {
            if let Some(map_reg) = self.lookup(map_name) {
                self.emit_map_set(map_reg, &args[1], &args[2]);
                return map_reg;
            }
        }
        if args.is_empty()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("len")
        {
            if let Some(len) = self.constant_len_expr(obj_expr) {
                return self.emit_const_int(len);
            }
            if let Expr::CallExpr(join_callee, join_args) = obj_expr
                && let Expr::Access(join_receiver, join_field) = join_callee.as_ref()
                && let Expr::Val(join_method) = join_field.as_ref()
                && join_method.as_str() == Some("join")
                && let Some(split_receiver) = split_join_same_separator_receiver(join_receiver, join_args)
            {
                if let Some(len) = self.constant_len_expr(split_receiver) {
                    return self.emit_const_int(len);
                }
                let obj_reg = self.expr(split_receiver);
                let out = self.alloc();
                if matches!(split_receiver, Expr::Val(value) if value.as_str().is_some()) {
                    self.emit(Op::StrLen { dst: out, src: obj_reg });
                } else {
                    self.emit(Op::Len { dst: out, src: obj_reg });
                }
                return out;
            }
            let obj_reg = self.expr(obj_expr);
            let out = self.alloc();
            self.emit_len_for_value(out, obj_reg, obj_expr);
            return out;
        }
        if args.len() == 1
            && self.literal_method_name(field_expr) == Some("push")
            && let Some(list_reg) = self.known_list_expr(obj_expr)
        {
            let val_reg = if let Expr::Var(arg_name) = args[0].as_ref() {
                self.lookup(arg_name).unwrap_or_else(|| self.expr(&args[0]))
            } else {
                self.expr(&args[0])
            };
            self.emit(Op::ListPush {
                list: list_reg,
                val: val_reg,
            });
            return list_reg;
        }
        if args.len() == 2
            && self.literal_method_name(field_expr) == Some("set")
            && let Some(list_reg) = self.known_list_expr(obj_expr)
            && let Some(out) = self.emit_list_set_i(list_reg, &args[0], &args[1])
        {
            return out;
        }
        if args.len() == 1
            && self.literal_method_name(field_expr) == Some("get")
            && let Some(list_reg) = self.known_list_expr(obj_expr)
        {
            return self.emit_list_get_access(list_reg, &args[0]);
        }
        if args.len() == 2
            && self.literal_method_name(field_expr) == Some("set")
            && let Some(map_reg) = self.known_map_expr(obj_expr)
        {
            self.emit_map_set(map_reg, &args[0], &args[1]);
            return map_reg;
        }
        if args.len() == 1
            && self.literal_method_name(field_expr) == Some("get")
            && let Some(map_reg) = self.known_map_expr(obj_expr)
        {
            return self.emit_map_access(map_reg, &args[0]);
        }
        if args.len() == 1
            && self.literal_method_name(field_expr) == Some("has")
            && let Some(map_reg) = self.known_map_expr(obj_expr)
        {
            return self.emit_map_has(map_reg, &args[0]);
        }
        if args.len() == 1
            && let Expr::Var(obj_name) = obj_expr
            && obj_name == "math"
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("floor")
        {
            if let Expr::Bin(lhs, BinOp::Div, rhs) = args[0].as_ref()
                && self.expr_numeric_fact(lhs.as_ref())
                && let Expr::Val(Val::Int(imm)) = rhs.as_ref()
                && *imm != 0
                && let Ok(imm) = i16::try_from(*imm)
            {
                let src = self.expr(lhs);
                let dst = self.alloc();
                self.emit(Op::FloorDivImm { dst, src, imm });
                return dst;
            }
            let src_reg = self.expr(&args[0]);
            let dst = self.alloc();
            self.emit(Op::Floor { dst, src: src_reg });
            return dst;
        }
        if args.len() == 1
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("starts_with")
            && let Expr::Val(arg_val) = args[0].as_ref()
            && arg_val.as_str().is_some()
        {
            let obj_reg = self.expr(obj_expr);
            let kidx = self.k(arg_val.clone());
            let dst = self.alloc();
            self.emit(Op::StartsWithK(dst, obj_reg, kidx));
            return dst;
        }
        if args.len() == 1
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("contains")
            && let Expr::Val(arg_val) = args[0].as_ref()
            && arg_val.as_str().is_some()
        {
            let obj_reg = self.expr(obj_expr);
            let kidx = self.k(arg_val.clone());
            let dst = self.alloc();
            self.emit(Op::ContainsK(dst, obj_reg, kidx));
            return dst;
        }
        if args.is_empty()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str().is_some()
            && let Expr::Var(receiver_name) = obj_expr
            && self.lookup(receiver_name).is_none()
        {
            let dst = self.alloc();
            let receiver = self.k(crate::val::Val::from_str(receiver_name.as_str()));
            let method = self.k(method_val.clone());
            self.emit(Op::CallGlobalMethod0 { dst, receiver, method });
            return dst;
        }
        if args.is_empty()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str().is_some()
        {
            let receiver = self.expr(obj_expr);
            let method = self.k(method_val.clone());
            self.emit(Op::CallMethod0 {
                dst: receiver,
                receiver,
                method,
            });
            return receiver;
        }

        let known_builtin = self.const_env.get("__lk_call_method").cloned();
        let builtin_reg = self.emit_known_or_global_callable("__lk_call_method", known_builtin.as_ref());

        let base = self.alloc();
        let method_slot = self.alloc();
        let arg_list = self.alloc();
        debug_assert_eq!(method_slot, base + 1);
        debug_assert_eq!(arg_list, base + 2);
        self.emit_expr_into(base, obj_expr);
        self.emit_expr_into(method_slot, field_expr);
        self.emit_list_from_exprs_into(arg_list, args);

        self.emit_positional_call(builtin_reg, base, 3, 1, known_builtin.as_ref());
        base
    }

    pub(crate) fn compile_method_call_named(
        &mut self,
        obj_expr: &Expr,
        field_expr: &Expr,
        pos_args: &[Box<Expr>],
        named_args: &[(String, Box<Expr>)],
    ) -> u16 {
        let known_builtin = self.const_env.get("__lk_call_method_named").cloned();
        let builtin_reg = self.emit_known_or_global_callable("__lk_call_method_named", known_builtin.as_ref());

        let base = self.alloc();
        let method_slot = self.alloc();
        let pos_list = self.alloc();
        let named_slot = self.alloc();
        debug_assert_eq!(method_slot, base + 1);
        debug_assert_eq!(pos_list, base + 2);
        debug_assert_eq!(named_slot, base + 3);
        self.emit_expr_into(base, obj_expr);
        self.emit_expr_into(method_slot, field_expr);
        self.emit_list_from_exprs_into(pos_list, pos_args);
        self.emit_map_from_named_args_into(named_slot, named_args);

        self.emit_positional_call(builtin_reg, base, 4, 1, known_builtin.as_ref());
        base
    }
}
