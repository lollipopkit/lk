use crate::{
    expr::Expr,
    op::BinOp,
    val::{Type, Val},
};

use super::FunctionBuilder;

impl FunctionBuilder {
    pub(crate) fn normalized_value_fact(ty: &Type) -> Option<Type> {
        match ty {
            Type::Int => Some(Type::Int),
            Type::Float => Some(Type::Float),
            Type::List(_) => Some(Type::List(Box::new(Type::Any))),
            Type::Map(_, _) => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
            Type::Optional(inner) => Self::normalized_value_fact(inner),
            _ => None,
        }
    }

    pub(crate) fn homogeneous_map_value_fact<'a>(values: impl IntoIterator<Item = &'a Val>) -> Option<Type> {
        Self::homogeneous_value_fact(values.into_iter().map(Self::val_value_fact))
    }

    pub(crate) fn homogeneous_list_value_fact<'a>(values: impl IntoIterator<Item = &'a Val>) -> Option<Type> {
        Self::homogeneous_value_fact(values.into_iter().map(Self::val_value_fact))
    }

    pub(crate) fn homogeneous_expr_value_fact<'a>(&self, values: impl IntoIterator<Item = &'a Expr>) -> Option<Type> {
        Self::homogeneous_value_fact(values.into_iter().map(|expr| self.expr_value_fact(expr)))
    }

    pub(crate) fn record_map_value_type(&mut self, map_reg: u16, value_fact: Option<Type>) {
        if let Some(value_fact) = value_fact {
            self.map_value_types.insert(map_reg, value_fact);
            self.map_value_adoptable.remove(&map_reg);
        } else {
            self.map_value_types.remove(&map_reg);
            self.map_value_adoptable.remove(&map_reg);
        }
    }

    pub(crate) fn record_empty_map_value_type(&mut self, map_reg: u16) {
        self.map_value_types.remove(&map_reg);
        self.map_value_adoptable.insert(map_reg);
    }

    pub(crate) fn invalidate_map_value_type(&mut self, map_reg: u16) {
        self.map_value_types.remove(&map_reg);
        self.map_value_adoptable.remove(&map_reg);
    }

    pub(crate) fn record_list_value_type(&mut self, list_reg: u16, value_fact: Option<Type>) {
        if let Some(value_fact) = value_fact {
            self.list_value_types.insert(list_reg, value_fact);
            self.list_value_adoptable.remove(&list_reg);
        } else {
            self.list_value_types.remove(&list_reg);
            self.list_value_adoptable.remove(&list_reg);
        }
    }

    pub(crate) fn record_empty_list_value_type(&mut self, list_reg: u16) {
        self.list_value_types.remove(&list_reg);
        self.list_value_adoptable.insert(list_reg);
    }

    pub(crate) fn invalidate_list_value_type(&mut self, list_reg: u16) {
        self.list_value_types.remove(&list_reg);
        self.list_value_adoptable.remove(&list_reg);
    }

    pub(crate) fn record_list_length(&mut self, list_reg: u16, len: usize) {
        self.list_lengths.insert(list_reg, len);
    }

    pub(crate) fn mark_map_lookup_result(&mut self, dst: u16, map_reg: u16) {
        let Some(value_fact) = self.map_value_types.get(&map_reg).cloned() else {
            return;
        };
        self.apply_type_fact(dst, &value_fact);
    }

    pub(crate) fn mark_list_lookup_result(&mut self, dst: u16, list_reg: u16) {
        let Some(value_fact) = self.list_value_types.get(&list_reg).cloned() else {
            return;
        };
        self.apply_type_fact(dst, &value_fact);
    }

    pub(crate) fn mark_list_lookup_result_if_in_bounds(&mut self, dst: u16, list_reg: u16, index: i64) {
        let Some(len) = self.list_lengths.get(&list_reg).copied() else {
            return;
        };
        let Some(index) = normalize_list_index(index, len) else {
            return;
        };
        if index < len {
            self.mark_list_lookup_result(dst, list_reg);
        }
    }

    pub(crate) fn update_list_value_type_after_write(&mut self, list_reg: u16, value_reg: u16) {
        self.update_container_value_type_after_write(ContainerFactKind::List, list_reg, value_reg);
    }

    pub(crate) fn update_map_value_type_after_write(&mut self, map_reg: u16, value_reg: u16) {
        self.update_container_value_type_after_write(ContainerFactKind::Map, map_reg, value_reg);
    }

    fn update_container_value_type_after_write(&mut self, kind: ContainerFactKind, container_reg: u16, value_reg: u16) {
        let Some(value_fact) = self.reg_value_fact(value_reg) else {
            match kind {
                ContainerFactKind::List => self.invalidate_list_value_type(container_reg),
                ContainerFactKind::Map => self.invalidate_map_value_type(container_reg),
            }
            return;
        };

        let (facts, adoptable) = match kind {
            ContainerFactKind::List => (&mut self.list_value_types, &mut self.list_value_adoptable),
            ContainerFactKind::Map => (&mut self.map_value_types, &mut self.map_value_adoptable),
        };

        if let Some(current) = facts.get(&container_reg) {
            if *current != value_fact {
                facts.remove(&container_reg);
                adoptable.remove(&container_reg);
            }
            return;
        }

        if adoptable.remove(&container_reg) {
            facts.insert(container_reg, value_fact);
        }
    }

    fn homogeneous_value_fact(facts: impl IntoIterator<Item = Option<Type>>) -> Option<Type> {
        let mut iter = facts.into_iter();
        let first = iter.next()??;
        if iter.all(|fact| fact.as_ref() == Some(&first)) {
            Some(first)
        } else {
            None
        }
    }

    fn val_value_fact(value: &Val) -> Option<Type> {
        match value {
            Val::Int(_) => Some(Type::Int),
            Val::Float(_) => Some(Type::Float),
            Val::List(_) => Some(Type::List(Box::new(Type::Any))),
            Val::Map(_) => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
            _ => None,
        }
    }

    fn reg_value_fact(&self, reg: u16) -> Option<Type> {
        if self.int_regs.contains(&reg) {
            return Some(Type::Int);
        }
        if self.float_regs.contains(&reg) {
            return Some(Type::Float);
        }
        if self.list_locals.contains(&reg) {
            return Some(Type::List(Box::new(Type::Any)));
        }
        if self.map_locals.contains(&reg) {
            return Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any)));
        }
        None
    }

    pub(crate) fn expr_value_fact(&self, expr: &Expr) -> Option<Type> {
        if let Some(ty) = self.expr_type_hint(expr).and_then(Self::normalized_value_fact) {
            return Some(ty);
        }
        match expr {
            Expr::Val(value) => Self::val_value_fact(value),
            Expr::Var(name) => {
                if let Some(reg) = self.lookup(name) {
                    if self.int_regs.contains(&reg) {
                        return Some(Type::Int);
                    }
                    if self.float_regs.contains(&reg) {
                        return Some(Type::Float);
                    }
                    if self.list_locals.contains(&reg) {
                        return Some(Type::List(Box::new(Type::Any)));
                    }
                    if self.map_locals.contains(&reg) {
                        return Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any)));
                    }
                }
                self.lookup_const(name).and_then(Self::val_value_fact)
            }
            Expr::Paren(inner) => self.expr_value_fact(inner),
            Expr::Bin(left, op, right) if !matches!(op, BinOp::Div) && op.is_arith() => {
                let left = self.expr_value_fact(left);
                let right = self.expr_value_fact(right);
                match (left, right) {
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
        let len = self.list_lengths.get(&reg).copied()?;
        let index = normalize_list_index(*index, len)?;
        if index >= len {
            return None;
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

#[derive(Clone, Copy)]
enum ContainerFactKind {
    List,
    Map,
}

fn normalize_list_index(index: i64, len: usize) -> Option<usize> {
    if index >= 0 {
        return usize::try_from(index).ok();
    }
    let len = i64::try_from(len).ok()?;
    usize::try_from(len.checked_add(index)?).ok()
}
