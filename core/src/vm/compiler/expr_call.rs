use super::FunctionBuilder;
use crate::{expr::Expr, vm::Op};

impl FunctionBuilder {
    pub(crate) fn compile_method_call(&mut self, obj_expr: &Expr, field_expr: &Expr, args: &[Box<Expr>]) -> u16 {
        if args.len() == 1
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("join")
            && let Expr::CallExpr(split_callee, split_args) = obj_expr
            && split_args.len() == 1
            && split_args[0].as_ref() == args[0].as_ref()
            && let Expr::Access(split_receiver, split_field) = split_callee.as_ref()
            && let Expr::Val(split_method) = split_field.as_ref()
            && split_method.as_str() == Some("split")
        {
            return self.expr(split_receiver);
        }
        if args.len() == 3
            && let Expr::Var(module_name) = obj_expr
            && module_name == "list"
            && self.lookup(module_name).is_none()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("set")
            && let Expr::Var(list_name) = args[0].as_ref()
            && let Some(list_reg) = self.lookup(list_name)
            && self.list_locals.contains(&list_reg)
            && let Some(dst) = self.emit_list_set_i(list_reg, &args[1], &args[2])
        {
            return dst;
        }
        if args.len() == 2
            && let Expr::Var(module_name) = obj_expr
            && module_name == "map"
            && self.lookup(module_name).is_none()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("get")
        {
            let map_reg = if let Expr::Var(map_name) = args[0].as_ref() {
                self.lookup(map_name).unwrap_or_else(|| self.expr(&args[0]))
            } else {
                self.expr(&args[0])
            };
            return self.emit_map_access(map_reg, &args[1]);
        }
        if args.len() == 2
            && let Expr::Var(module_name) = obj_expr
            && module_name == "map"
            && self.lookup(module_name).is_none()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("has")
            && let Expr::Var(map_name) = args[0].as_ref()
            && let Some(map_reg) = self.lookup(map_name)
            && self.map_locals.contains(&map_reg)
        {
            return self.emit_map_has(map_reg, &args[1]);
        }
        if args.len() == 3
            && let Expr::Var(module_name) = obj_expr
            && module_name == "map"
            && self.lookup(module_name).is_none()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("set")
            && let Expr::Var(map_name) = args[0].as_ref()
        {
            if let Some(map_reg) = self.lookup(map_name)
                && self.map_locals.contains(&map_reg)
            {
                self.emit_map_set(map_reg, &args[1], &args[2]);
                return map_reg;
            }
            if let Some(map_reg) = self.lookup(map_name) {
                self.emit_map_set(map_reg, &args[1], &args[2]);
                return map_reg;
            }
        }
        if args.is_empty()
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("len")
        {
            let obj_reg = self.expr(obj_expr);
            let out = self.alloc();
            if self.list_locals.contains(&obj_reg) {
                self.emit(Op::ListLen { dst: out, src: obj_reg });
            } else if self.map_locals.contains(&obj_reg) {
                self.emit(Op::MapLen { dst: out, src: obj_reg });
            } else if matches!(obj_expr, Expr::Val(value) if value.as_str().is_some()) {
                self.emit(Op::StrLen { dst: out, src: obj_reg });
            } else {
                self.emit(Op::Len { dst: out, src: obj_reg });
            }
            return out;
        }
        if args.len() == 1
            && let Expr::Var(var_name) = obj_expr
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("push")
            && let Some(list_reg) = self.lookup(var_name)
            && self.list_locals.contains(&list_reg)
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
            && let Expr::Var(var_name) = obj_expr
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("set")
            && let Some(list_reg) = self.lookup(var_name)
            && self.list_locals.contains(&list_reg)
            && let Some(out) = self.emit_list_set_i(list_reg, &args[0], &args[1])
        {
            return out;
        }
        if args.len() == 2
            && let Expr::Var(var_name) = obj_expr
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("set")
            && let Some(map_reg) = self.lookup(var_name)
            && self.map_locals.contains(&map_reg)
        {
            self.emit_map_set(map_reg, &args[0], &args[1]);
            return map_reg;
        }
        if args.len() == 1
            && let Expr::Var(var_name) = obj_expr
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("get")
            && let Some(map_reg) = self.lookup(var_name)
            && self.map_locals.contains(&map_reg)
        {
            return self.emit_map_access(map_reg, &args[0]);
        }
        if args.len() == 1
            && let Expr::Var(var_name) = obj_expr
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("has")
            && let Some(map_reg) = self.lookup(var_name)
            && self.map_locals.contains(&map_reg)
        {
            return self.emit_map_has(map_reg, &args[0]);
        }
        if args.len() == 1
            && let Expr::Var(obj_name) = obj_expr
            && obj_name == "math"
            && let Expr::Val(method_val) = field_expr
            && method_val.as_str() == Some("floor")
        {
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
