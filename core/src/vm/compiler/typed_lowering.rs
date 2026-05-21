use crate::{
    expr::Expr,
    op::BinOp,
    typ::{NumericClass, NumericHierarchy},
    val::{Type, Val},
    vm::Op,
};

use super::{ArithFlavor, FunctionBuilder, map_facts::normalize_list_index};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContainerKind {
    List,
    Map,
    Str,
}

impl FunctionBuilder {
    fn numeric_class_hint(&self, expr: &Expr) -> Option<NumericClass> {
        self.expr_type_hint(expr).and_then(NumericHierarchy::classify)
    }

    pub(crate) fn select_arith_flavor(&self, op: &BinOp, left: &Expr, right: &Expr, whole: &Expr) -> ArithFlavor {
        if matches!(op, BinOp::Div) {
            return ArithFlavor::Float;
        }
        if self.expr_known_int(left) && self.expr_known_int(right) {
            return ArithFlavor::Int;
        }
        if self.expr_known_float(left) || self.expr_known_float(right) {
            return ArithFlavor::Float;
        }

        let result_class = self.numeric_class_hint(whole);
        let left_class = self.numeric_class_hint(left);
        let right_class = self.numeric_class_hint(right);
        match result_class {
            Some(NumericClass::Float) => ArithFlavor::Float,
            Some(NumericClass::Int) => {
                if left_class == Some(NumericClass::Int) && right_class == Some(NumericClass::Int) {
                    ArithFlavor::Int
                } else {
                    ArithFlavor::Any
                }
            }
            Some(NumericClass::Boxed) | None => {
                if left_class == Some(NumericClass::Float) || right_class == Some(NumericClass::Float) {
                    ArithFlavor::Float
                } else if left_class == Some(NumericClass::Int) && right_class == Some(NumericClass::Int) {
                    ArithFlavor::Int
                } else {
                    ArithFlavor::Any
                }
            }
        }
    }

    pub(crate) fn expr_known_int(&self, expr: &Expr) -> bool {
        self.expr_value_fact(expr) == Some(Type::Int)
    }

    pub(crate) fn expr_known_float(&self, expr: &Expr) -> bool {
        self.expr_value_fact(expr) == Some(Type::Float)
    }

    fn performance_reg_value_fact(&self, reg: u16) -> Option<Type> {
        self.analysis
            .as_ref()
            .and_then(|analysis| analysis.perf.value_type(reg))
    }

    pub(crate) fn reg_value_fact(&self, reg: u16) -> Option<Type> {
        if let Some(fact) = self.performance_reg_value_fact(reg) {
            return Some(fact);
        }
        if self.int_regs.contains(&reg) {
            return Some(Type::Int);
        }
        if self.float_regs.contains(&reg) {
            return Some(Type::Float);
        }
        if self.string_regs.contains(&reg) {
            return Some(Type::String);
        }
        if self.list_locals.contains(&reg) {
            return Some(Type::List(Box::new(Type::Any)));
        }
        if self.map_locals.contains(&reg) {
            return Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any)));
        }
        None
    }

    pub(crate) fn reg_known_int(&self, reg: u16) -> bool {
        self.reg_value_fact(reg) == Some(Type::Int)
    }

    pub(crate) fn reg_container_kind(&self, reg: u16) -> Option<ContainerKind> {
        match self.reg_value_fact(reg) {
            Some(Type::List(_)) => Some(ContainerKind::List),
            Some(Type::Map(_, _)) => Some(ContainerKind::Map),
            Some(Type::String) => Some(ContainerKind::Str),
            _ => None,
        }
    }

    pub(crate) fn expr_numeric_fact(&self, expr: &Expr) -> bool {
        self.expr_known_int(expr) || self.expr_known_float(expr)
    }

    pub(crate) fn literal_method_name<'a>(&self, field: &'a Expr) -> Option<&'a str> {
        let Expr::Val(value) = field else {
            return None;
        };
        value.as_str()
    }

    pub(crate) fn unshadowed_module(&self, expr: &Expr, module: &str) -> bool {
        matches!(expr, Expr::Var(name) if name == module && self.lookup(name).is_none())
    }

    pub(crate) fn known_list_expr(&self, expr: &Expr) -> Option<u16> {
        let Expr::Var(name) = expr else {
            return None;
        };
        let reg = self.lookup(name)?;
        (self.reg_container_kind(reg) == Some(ContainerKind::List)).then_some(reg)
    }

    pub(crate) fn known_map_expr(&self, expr: &Expr) -> Option<u16> {
        let Expr::Var(name) = expr else {
            return None;
        };
        let reg = self.lookup(name)?;
        (self.reg_container_kind(reg) == Some(ContainerKind::Map)).then_some(reg)
    }

    pub(crate) fn list_known_len_for_reg(&self, reg: u16) -> Option<usize> {
        self.analysis
            .as_ref()
            .and_then(|analysis| analysis.perf.list_known_len(reg))
            .or_else(|| self.list_lengths.get(&reg).copied())
    }

    pub(crate) fn expr_or_lookup(&mut self, expr: &Expr) -> u16 {
        if let Expr::Var(name) = expr {
            self.lookup(name).unwrap_or_else(|| self.expr(expr))
        } else {
            self.expr(expr)
        }
    }

    pub(crate) fn emit_in_place_numeric_op(&mut self, dst: u16, left: u16, right: u16, op: &BinOp) -> bool {
        let int_operands = self.reg_known_int(left) && self.reg_known_int(right);
        match op {
            BinOp::Add if int_operands => self.emit(crate::vm::Op::AddInt(dst, left, right)),
            BinOp::Add => self.emit(crate::vm::Op::Add(dst, left, right)),
            BinOp::Sub if int_operands => self.emit(crate::vm::Op::SubInt(dst, left, right)),
            BinOp::Sub => self.emit(crate::vm::Op::Sub(dst, left, right)),
            BinOp::Mul if int_operands => self.emit(crate::vm::Op::MulInt(dst, left, right)),
            BinOp::Mul => self.emit(crate::vm::Op::Mul(dst, left, right)),
            BinOp::Mod if int_operands => self.emit(crate::vm::Op::ModInt(dst, left, right)),
            BinOp::Mod => self.emit(crate::vm::Op::Mod(dst, left, right)),
            BinOp::Div => self.emit(crate::vm::Op::Div(dst, left, right)),
            _ => return false,
        }
        true
    }

    pub(crate) fn constant_len_expr(&self, expr: &Expr) -> Option<i64> {
        match expr {
            Expr::Paren(inner) => self.constant_len_expr(inner),
            Expr::Val(value) => match value {
                Val::ShortStr(value) => Some(value.as_str().len() as i64),
                Val::Str(value) => Some(value.len() as i64),
                Val::List(value) => Some(value.len() as i64),
                Val::Map(value) => Some(value.len() as i64),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn emit_const_int(&mut self, value: i64) -> u16 {
        let dst = self.alloc();
        let kidx = self.k(Val::Int(value));
        self.emit(Op::LoadK(dst, kidx));
        dst
    }

    pub(crate) fn emit_len_for_value(&mut self, dst: u16, src: u16, source_expr: &Expr) {
        match self.reg_container_kind(src) {
            Some(ContainerKind::List) => self.emit(Op::ListLen { dst, src }),
            Some(ContainerKind::Map) => self.emit(Op::MapLen { dst, src }),
            Some(ContainerKind::Str) => self.emit(Op::StrLen { dst, src }),
            None if matches!(source_expr, Expr::Val(value) if value.as_str().is_some()) => {
                self.emit(Op::StrLen { dst, src });
            }
            None => self.emit(Op::Len { dst, src }),
        }
    }

    pub(crate) fn emit_field_access_for_reg(&mut self, base_reg: u16, field: &Expr) -> u16 {
        let dst = self.alloc();
        match field {
            Expr::Val(field_val) if field_val.as_str().is_some() => {
                let key = self.k(field_val.clone());
                if self.reg_container_kind(base_reg) == Some(ContainerKind::Map) {
                    self.emit(Op::MapGetInterned(dst, base_reg, key));
                    self.mark_map_lookup_result(dst, base_reg);
                } else {
                    self.emit(Op::AccessK(dst, base_reg, key));
                }
            }
            Expr::Val(Val::Int(index)) => {
                if self.reg_container_kind(base_reg) == Some(ContainerKind::List)
                    && let Ok(index_i16) = i16::try_from(*index)
                {
                    self.emit(Op::ListIndexI(dst, base_reg, index_i16));
                    self.mark_list_lookup_result_if_in_bounds(dst, base_reg, *index);
                } else {
                    let key = self.k(Val::Int(*index));
                    self.emit(Op::IndexK(dst, base_reg, key));
                    if self.reg_container_kind(base_reg) == Some(ContainerKind::List) {
                        self.mark_list_lookup_result_if_in_bounds(dst, base_reg, *index);
                    }
                }
            }
            _ => {
                let field_reg = self.expr(field);
                if self.reg_container_kind(base_reg) == Some(ContainerKind::Map) {
                    self.emit(Op::MapGetDynamic(dst, base_reg, field_reg));
                    self.mark_map_lookup_result(dst, base_reg);
                } else if self.reg_container_kind(base_reg) == Some(ContainerKind::List)
                    && self.reg_known_int(field_reg)
                {
                    self.emit(Op::ListIndex(dst, base_reg, field_reg));
                    self.mark_list_lookup_result(dst, base_reg);
                } else if self.reg_container_kind(base_reg) == Some(ContainerKind::Str) && self.reg_known_int(field_reg)
                {
                    self.emit(Op::StrIndex(dst, base_reg, field_reg));
                } else {
                    self.emit(Op::Access(dst, base_reg, field_reg));
                }
            }
        }
        dst
    }

    pub(crate) fn emit_typed_list_access(&mut self, list_reg: u16, index_expr: &Expr) -> u16 {
        let dst = self.alloc();
        if let Expr::Val(Val::Int(index)) = index_expr {
            if *index < 0 {
                let nil = self.k(Val::Nil);
                self.emit(Op::LoadK(dst, nil));
                return dst;
            }
            if self
                .list_known_len_for_reg(list_reg)
                .is_some_and(|len| usize::try_from(*index).ok().is_none_or(|index| index >= len))
            {
                let nil = self.k(Val::Nil);
                self.emit(Op::LoadK(dst, nil));
                return dst;
            }
            if let Ok(index_i16) = i16::try_from(*index) {
                self.emit(Op::ListIndexI(dst, list_reg, index_i16));
                self.mark_list_lookup_result_if_in_bounds(dst, list_reg, *index);
                return dst;
            }
        }

        let index_reg = if let Expr::Var(arg_name) = index_expr {
            self.lookup(arg_name).unwrap_or_else(|| self.expr(index_expr))
        } else {
            self.expr(index_expr)
        };
        if self.reg_known_int(index_reg) {
            self.emit(Op::ListIndex(dst, list_reg, index_reg));
            self.mark_list_lookup_result(dst, list_reg);
        } else {
            self.emit(Op::Access(dst, list_reg, index_reg));
        }
        dst
    }

    pub(crate) fn emit_typed_map_access(&mut self, map_reg: u16, key_expr: &Expr) -> u16 {
        let dst = self.alloc();
        if let Some(key_idx) = self.map_literal_key_const(key_expr) {
            self.emit(Op::MapGetInterned(dst, map_reg, key_idx));
        } else {
            let key_reg = self.expr_or_lookup(key_expr);
            if self.reg_container_kind(map_reg) == Some(ContainerKind::Map) {
                self.emit(Op::MapGetDynamic(dst, map_reg, key_reg));
            } else {
                self.emit(Op::Access(dst, map_reg, key_reg));
            }
        }
        self.mark_map_lookup_result(dst, map_reg);
        dst
    }

    pub(crate) fn expr_value_fact(&self, expr: &Expr) -> Option<Type> {
        if let Some(ty) = self.expr_type_hint(expr).and_then(Self::normalized_value_fact) {
            return Some(ty);
        }
        match expr {
            Expr::Val(value) => Self::val_value_fact(value),
            Expr::Var(name) => {
                if let Some(reg) = self.lookup(name)
                    && let Some(fact) = self.reg_value_fact(reg)
                {
                    return Some(fact);
                }
                self.lookup_const(name).and_then(Self::val_value_fact)
            }
            Expr::Paren(inner) => self.expr_value_fact(inner),
            Expr::Bin(left, op, right) if !matches!(op, BinOp::Div) && op.is_arith() => {
                match (self.expr_value_fact(left), self.expr_value_fact(right)) {
                    (Some(Type::Int), Some(Type::Int)) => Some(Type::Int),
                    (Some(Type::Int | Type::Float), Some(Type::Int | Type::Float)) => Some(Type::Float),
                    _ => None,
                }
            }
            Expr::Access(base, field) => self
                .list_value_fact_for_base(base.as_ref(), field.as_ref())
                .or_else(|| self.map_value_fact_for_base(base.as_ref())),
            Expr::Call(name, _) => self.direct_call_return_value_fact(name),
            Expr::CallExpr(callee, args) => self.call_expr_value_fact(callee.as_ref(), args.as_slice()),
            _ => None,
        }
    }

    fn call_expr_value_fact(&self, callee: &Expr, args: &[Box<Expr>]) -> Option<Type> {
        if let Expr::Var(name) = callee {
            return self.direct_call_return_value_fact(name);
        }
        let Expr::Access(_, method) = callee else {
            return None;
        };
        let Expr::Val(method_value) = method.as_ref() else {
            return None;
        };
        if args.is_empty() && method_value.as_str() == Some("len") {
            return Some(Type::Int);
        }
        self.list_get_value_fact(callee, args)
            .or_else(|| self.map_get_value_fact(callee, args))
    }

    fn list_get_value_fact(&self, callee: &Expr, args: &[Box<Expr>]) -> Option<Type> {
        let Expr::Access(receiver, method) = callee else {
            return None;
        };
        let Expr::Val(method_value) = method.as_ref() else {
            return None;
        };
        if method_value.as_str() != Some("get") {
            return None;
        }
        if args.len() == 1 {
            return self.list_get_value_fact_for_base(receiver.as_ref(), args[0].as_ref());
        }
        if args.len() == 2
            && matches!(receiver.as_ref(), Expr::Var(name) if name == "list" && self.lookup(name).is_none())
        {
            return self.list_get_value_fact_for_base(args[0].as_ref(), args[1].as_ref());
        }
        None
    }

    fn map_get_value_fact(&self, callee: &Expr, args: &[Box<Expr>]) -> Option<Type> {
        let Expr::Access(receiver, method) = callee else {
            return None;
        };
        let Expr::Val(method_value) = method.as_ref() else {
            return None;
        };
        if method_value.as_str() != Some("get") {
            return None;
        }
        if args.len() == 1 {
            return self.map_value_fact_for_base(receiver.as_ref());
        }
        if args.len() == 2
            && matches!(receiver.as_ref(), Expr::Var(name) if name == "map" && self.lookup(name).is_none())
        {
            return self.map_value_fact_for_base(args[0].as_ref());
        }
        None
    }

    fn map_value_fact_for_base(&self, base: &Expr) -> Option<Type> {
        let Expr::Var(name) = base else {
            return None;
        };
        let reg = self.lookup(name)?;
        if let Some(value_fact) = self
            .analysis
            .as_ref()
            .and_then(|analysis| analysis.perf.map_value_type(reg))
        {
            return Some(value_fact);
        }
        self.map_value_types.get(&reg).cloned()
    }

    fn list_value_fact_for_base(&self, base: &Expr, index: &Expr) -> Option<Type> {
        let Expr::Var(name) = base else {
            return None;
        };
        let reg = self.lookup(name)?;
        let Expr::Val(Val::Int(index)) = index else {
            return None;
        };
        let perf = self.analysis.as_ref().map(|analysis| &analysis.perf);
        let len = self.list_known_len_for_reg(reg)?;
        let index = normalize_list_index(*index, len)?;
        if index >= len {
            return None;
        }
        if let Some(value_fact) = perf.and_then(|perf| perf.list_value_type(reg)) {
            return Some(value_fact);
        }
        self.list_value_types.get(&reg).cloned()
    }

    fn list_get_value_fact_for_base(&self, base: &Expr, index: &Expr) -> Option<Type> {
        let Expr::Val(Val::Int(value)) = index else {
            return None;
        };
        if *value < 0 {
            return None;
        }
        self.list_value_fact_for_base(base, index)
    }

    fn direct_call_return_value_fact(&self, name: &str) -> Option<Type> {
        self.inferred_function_return_types
            .get(name)
            .and_then(|ty| ty.as_ref())
            .and_then(Self::normalized_value_fact)
    }
}
