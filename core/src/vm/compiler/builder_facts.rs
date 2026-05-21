use crate::{
    val::{Type, Val},
    vm::Op,
};

use super::FunctionBuilder;

impl FunctionBuilder {
    pub(crate) fn mark_direct_call_return_type(&mut self, name: &str, dst: u16) {
        let Some(Some(ty)) = self.inferred_function_return_types.get(name).cloned() else {
            return;
        };
        self.apply_type_fact(dst, &ty);
    }

    pub(crate) fn apply_type_fact(&mut self, dst: u16, ty: &Type) {
        match ty {
            Type::Int => {
                self.int_regs.insert(dst);
                self.float_regs.remove(&dst);
                self.string_regs.remove(&dst);
                self.clear_container_facts(dst);
            }
            Type::Float => {
                self.int_regs.remove(&dst);
                self.float_regs.insert(dst);
                self.string_regs.remove(&dst);
                self.clear_container_facts(dst);
            }
            Type::String => {
                self.int_regs.remove(&dst);
                self.float_regs.remove(&dst);
                self.string_regs.insert(dst);
                self.clear_container_facts(dst);
            }
            Type::List(value_ty) => {
                self.int_regs.remove(&dst);
                self.float_regs.remove(&dst);
                self.string_regs.remove(&dst);
                self.list_locals.insert(dst);
                self.record_list_value_type(dst, Self::normalized_value_fact(value_ty));
                self.list_lengths.remove(&dst);
                self.map_locals.remove(&dst);
                self.map_value_types.remove(&dst);
                self.map_value_adoptable.remove(&dst);
            }
            Type::Map(_, value_ty) => {
                self.int_regs.remove(&dst);
                self.float_regs.remove(&dst);
                self.string_regs.remove(&dst);
                self.list_locals.remove(&dst);
                self.list_value_types.remove(&dst);
                self.list_lengths.remove(&dst);
                self.list_value_adoptable.remove(&dst);
                self.map_locals.insert(dst);
                self.record_map_value_type(dst, Self::normalized_value_fact(value_ty));
            }
            _ => {}
        }
    }

    pub(crate) fn expr_type_hint(&self, expr: &crate::expr::Expr) -> Option<&Type> {
        let key = expr as *const crate::expr::Expr as usize;
        self.expr_type_hints.as_ref().and_then(|map| map.get(&key))
    }

    pub(crate) fn update_int_reg_facts(&mut self, op: &Op) {
        self.invalidate_unknown_call_side_effect_facts(op);
        match *op {
            Op::LoadK(dst, kidx) => match self.consts.get(kidx as usize) {
                Some(Val::Int(_)) => {
                    self.int_regs.insert(dst);
                    self.float_regs.remove(&dst);
                    self.string_regs.remove(&dst);
                    self.clear_container_facts(dst);
                }
                Some(Val::Float(_)) => {
                    self.int_regs.remove(&dst);
                    self.float_regs.insert(dst);
                    self.string_regs.remove(&dst);
                    self.clear_container_facts(dst);
                }
                Some(value) if value.as_str().is_some() => {
                    self.int_regs.remove(&dst);
                    self.float_regs.remove(&dst);
                    self.string_regs.insert(dst);
                    self.clear_container_facts(dst);
                }
                Some(Val::List(list)) => {
                    let len = list.len();
                    let value_fact = (!list.is_empty()).then(|| Self::homogeneous_list_value_fact(list.iter()));
                    self.int_regs.remove(&dst);
                    self.float_regs.remove(&dst);
                    self.string_regs.remove(&dst);
                    self.list_locals.insert(dst);
                    self.record_list_length(dst, len);
                    if let Some(value_fact) = value_fact {
                        self.record_list_value_type(dst, value_fact);
                    } else {
                        self.record_empty_list_value_type(dst);
                    }
                    self.clear_map_facts(dst);
                }
                Some(Val::Map(map)) => {
                    self.int_regs.remove(&dst);
                    self.float_regs.remove(&dst);
                    self.string_regs.remove(&dst);
                    self.list_locals.remove(&dst);
                    self.list_value_types.remove(&dst);
                    self.list_lengths.remove(&dst);
                    self.list_value_adoptable.remove(&dst);
                    self.map_locals.insert(dst);
                    if map.is_empty() {
                        self.record_empty_map_value_type(dst);
                    } else {
                        self.record_map_value_type(dst, Self::homogeneous_map_value_fact(map.values()));
                    }
                }
                _ => {
                    self.int_regs.remove(&dst);
                    self.float_regs.remove(&dst);
                    self.string_regs.remove(&dst);
                    self.clear_container_facts(dst);
                }
            },
            Op::Move(dst, src) | Op::StoreLocal(dst, src) | Op::LoadLocal(dst, src) => {
                if self.int_regs.contains(&src) {
                    self.int_regs.insert(dst);
                } else {
                    self.int_regs.remove(&dst);
                }
                if self.float_regs.contains(&src) {
                    self.float_regs.insert(dst);
                } else {
                    self.float_regs.remove(&dst);
                }
                if self.string_regs.contains(&src) {
                    self.string_regs.insert(dst);
                } else {
                    self.string_regs.remove(&dst);
                }
                if self.list_locals.contains(&src) {
                    self.list_locals.insert(dst);
                } else {
                    self.list_locals.remove(&dst);
                }
                self.list_value_types.remove(&dst);
                if let Some(len) = self.list_lengths.get(&src).copied() {
                    self.list_lengths.insert(dst, len);
                } else {
                    self.list_lengths.remove(&dst);
                }
                self.list_value_adoptable.remove(&dst);
                if self.map_locals.contains(&src) {
                    self.map_locals.insert(dst);
                } else {
                    self.map_locals.remove(&dst);
                }
                self.map_value_types.remove(&dst);
                self.map_value_adoptable.remove(&dst);
            }
            Op::AddInt(dst, _, _)
            | Op::SubInt(dst, _, _)
            | Op::MulInt(dst, _, _)
            | Op::ModInt(dst, _, _)
            | Op::AddIntImm(dst, _, _)
            | Op::AddIntImmJmp { r: dst, .. }
            | Op::AddRangeCountImm { target: dst, .. }
            | Op::Len { dst, .. }
            | Op::ListLen { dst, .. }
            | Op::MapLen { dst, .. }
            | Op::StrLen { dst, .. }
            | Op::Floor { dst, .. }
            | Op::FloorDivImm { dst, .. } => {
                self.int_regs.insert(dst);
                self.float_regs.remove(&dst);
                self.string_regs.remove(&dst);
                self.list_locals.remove(&dst);
                self.list_value_types.remove(&dst);
                self.list_value_adoptable.remove(&dst);
                self.map_locals.remove(&dst);
                self.map_value_types.remove(&dst);
                self.map_value_adoptable.remove(&dst);
            }
            Op::ForRangePrep { step, .. } => {
                self.int_regs.insert(step);
                self.float_regs.remove(&step);
                self.string_regs.remove(&step);
            }
            Op::ForRangeLoop {
                idx, write_idx: true, ..
            }
            | Op::RangeLoopI {
                idx, write_idx: true, ..
            } => {
                self.int_regs.insert(idx);
                self.float_regs.remove(&idx);
                self.string_regs.remove(&idx);
            }
            Op::AddFloat(dst, _, _)
            | Op::SubFloat(dst, _, _)
            | Op::MulFloat(dst, _, _)
            | Op::DivFloat(dst, _, _)
            | Op::ModFloat(dst, _, _) => {
                self.int_regs.remove(&dst);
                self.float_regs.insert(dst);
                self.string_regs.remove(&dst);
                self.clear_container_facts(dst);
            }
            Op::ToStr(dst, _) | Op::StrConcatKnownCap(dst, _, _) | Op::StrConcatToStr(dst, _, _) => {
                self.int_regs.remove(&dst);
                self.float_regs.remove(&dst);
                self.string_regs.insert(dst);
                self.clear_container_facts(dst);
            }
            Op::Add(dst, _, _)
            | Op::Sub(dst, _, _)
            | Op::Mul(dst, _, _)
            | Op::Div(dst, _, _)
            | Op::Mod(dst, _, _)
            | Op::LoadGlobal(dst, _)
            | Op::LoadCapture { dst, .. }
            | Op::Access(dst, _, _)
            | Op::AccessK(dst, _, _)
            | Op::IndexK(dst, _, _)
            | Op::ListIndex(dst, _, _)
            | Op::ListIndexI(dst, _, _)
            | Op::ListSetI { dst, .. }
            | Op::StrIndex(dst, _, _)
            | Op::StrIndexI(dst, _, _)
            | Op::ContainsK(dst, _, _)
            | Op::MapHas(dst, _, _)
            | Op::MapGetInterned(dst, _, _)
            | Op::MapGetDynamic(dst, _, _)
            | Op::MapHasK(dst, _, _)
            | Op::MakeClosure { dst, .. }
            | Op::Call { base: dst, retc: 1, .. }
            | Op::CallExact { base: dst, retc: 1, .. }
            | Op::CallClosureExact { base: dst, retc: 1, .. }
            | Op::CallNativeFast { base: dst, retc: 1, .. }
            | Op::CallMethod0 { dst, .. }
            | Op::CallNamed {
                base_pos: dst, retc: 1, ..
            }
            | Op::CallNamedFallback {
                base_pos: dst, retc: 1, ..
            }
            | Op::ToBool(dst, _)
            | Op::Not(dst, _)
            | Op::CmpEq(dst, _, _)
            | Op::CmpNe(dst, _, _)
            | Op::CmpLt(dst, _, _)
            | Op::CmpLe(dst, _, _)
            | Op::CmpGt(dst, _, _)
            | Op::CmpGe(dst, _, _)
            | Op::CmpI { dst, .. }
            | Op::CmpEqImm(dst, _, _)
            | Op::CmpNeImm(dst, _, _)
            | Op::CmpLtImm(dst, _, _)
            | Op::CmpLeImm(dst, _, _)
            | Op::CmpGtImm(dst, _, _)
            | Op::CmpGeImm(dst, _, _)
            | Op::In(dst, _, _)
            | Op::NullishPick { dst, .. }
            | Op::JmpFalseSet { dst, .. }
            | Op::JmpTrueSet { dst, .. }
            | Op::ToIter { dst, .. }
            | Op::ListSlice { dst, .. }
            | Op::PatternMatch { dst, .. } => {
                self.int_regs.remove(&dst);
                self.float_regs.remove(&dst);
                self.string_regs.remove(&dst);
                self.list_locals.remove(&dst);
                self.list_value_types.remove(&dst);
                self.list_value_adoptable.remove(&dst);
                self.map_locals.remove(&dst);
                self.map_value_types.remove(&dst);
                self.map_value_adoptable.remove(&dst);
            }
            Op::CallGlobalMethod0 { dst, receiver, method } => {
                if !self.apply_global_method_return_fact(dst, receiver, method) {
                    self.int_regs.remove(&dst);
                    self.float_regs.remove(&dst);
                    self.string_regs.remove(&dst);
                    self.list_locals.remove(&dst);
                    self.list_value_types.remove(&dst);
                    self.list_lengths.remove(&dst);
                    self.list_value_adoptable.remove(&dst);
                    self.map_locals.remove(&dst);
                    self.map_value_types.remove(&dst);
                    self.map_value_adoptable.remove(&dst);
                }
            }
            Op::BuildList { dst, .. } => {
                self.int_regs.remove(&dst);
                self.float_regs.remove(&dst);
                self.string_regs.remove(&dst);
                self.list_locals.insert(dst);
                self.list_value_types.remove(&dst);
                self.list_lengths.remove(&dst);
                self.list_value_adoptable.remove(&dst);
                self.map_locals.remove(&dst);
                self.map_value_types.remove(&dst);
                self.map_value_adoptable.remove(&dst);
            }
            Op::BuildMap { dst, .. } => {
                self.int_regs.remove(&dst);
                self.float_regs.remove(&dst);
                self.string_regs.remove(&dst);
                self.list_locals.remove(&dst);
                self.list_value_types.remove(&dst);
                self.list_lengths.remove(&dst);
                self.list_value_adoptable.remove(&dst);
                self.map_locals.insert(dst);
                self.map_value_types.remove(&dst);
                self.map_value_adoptable.remove(&dst);
            }
            Op::ListFoldAdd { acc, .. } | Op::MapValuesFoldAdd { acc, .. } => {
                self.int_regs.remove(&acc);
                self.float_regs.remove(&acc);
                self.string_regs.remove(&acc);
                self.list_value_types.remove(&acc);
                self.list_lengths.remove(&acc);
                self.list_value_adoptable.remove(&acc);
                self.map_value_types.remove(&acc);
                self.map_value_adoptable.remove(&acc);
            }
            Op::ListPush { list, val } | Op::ListPushMove { list, val } => {
                self.update_list_value_type_after_write(list, val);
                if let Some(len) = self.list_lengths.get_mut(&list) {
                    *len = len.saturating_add(1);
                }
                self.list_value_types.remove(&val);
                self.list_lengths.remove(&val);
                self.list_value_adoptable.remove(&val);
            }
            Op::MapSetInterned(map, _, val) | Op::MapSetInternedMove(map, _, val) => {
                self.update_map_value_type_after_write(map, val);
                self.map_value_types.remove(&val);
                self.map_value_adoptable.remove(&val);
            }
            Op::MapSet { map, key, val } => {
                self.update_map_value_type_after_write(map, val);
                self.map_value_types.remove(&key);
                self.map_value_types.remove(&val);
                self.map_value_adoptable.remove(&key);
                self.map_value_adoptable.remove(&val);
            }
            Op::MapSetMove { map, key, val } => {
                self.update_map_value_type_after_write(map, val);
                self.int_regs.remove(&key);
                self.int_regs.remove(&val);
                self.float_regs.remove(&key);
                self.float_regs.remove(&val);
                self.string_regs.remove(&key);
                self.string_regs.remove(&val);
                self.list_locals.remove(&key);
                self.list_locals.remove(&val);
                self.list_value_types.remove(&key);
                self.list_value_types.remove(&val);
                self.list_lengths.remove(&key);
                self.list_lengths.remove(&val);
                self.list_value_adoptable.remove(&key);
                self.list_value_adoptable.remove(&val);
                self.map_locals.remove(&key);
                self.map_locals.remove(&val);
                self.map_value_types.remove(&key);
                self.map_value_types.remove(&val);
                self.map_value_adoptable.remove(&key);
                self.map_value_adoptable.remove(&val);
            }
            Op::Call { base, retc, .. }
            | Op::CallExact { base, retc, .. }
            | Op::CallClosureExact { base, retc, .. }
            | Op::CallNativeFast { base, retc, .. }
                if retc > 1 =>
            {
                for reg in base..base.saturating_add(retc as u16) {
                    self.int_regs.remove(&reg);
                    self.float_regs.remove(&reg);
                    self.string_regs.remove(&reg);
                    self.list_value_types.remove(&reg);
                    self.list_lengths.remove(&reg);
                    self.list_value_adoptable.remove(&reg);
                    self.map_value_types.remove(&reg);
                    self.map_value_adoptable.remove(&reg);
                }
            }
            Op::CallNamed { base_pos, retc, .. } if retc > 1 => {
                for reg in base_pos..base_pos.saturating_add(retc as u16) {
                    self.int_regs.remove(&reg);
                    self.float_regs.remove(&reg);
                    self.string_regs.remove(&reg);
                    self.list_value_types.remove(&reg);
                    self.list_lengths.remove(&reg);
                    self.list_value_adoptable.remove(&reg);
                    self.map_value_types.remove(&reg);
                    self.map_value_adoptable.remove(&reg);
                }
            }
            Op::CallNamedFallback { base_pos, retc, .. } if retc > 1 => {
                for reg in base_pos..base_pos.saturating_add(retc as u16) {
                    self.int_regs.remove(&reg);
                    self.float_regs.remove(&reg);
                    self.string_regs.remove(&reg);
                    self.list_value_types.remove(&reg);
                    self.list_lengths.remove(&reg);
                    self.list_value_adoptable.remove(&reg);
                    self.map_value_types.remove(&reg);
                    self.map_value_adoptable.remove(&reg);
                }
            }
            _ => {}
        }
    }

    fn apply_global_method_return_fact(&mut self, dst: u16, receiver: u16, method: u16) -> bool {
        let Some(receiver) = self.consts.get(receiver as usize).and_then(Val::as_str) else {
            return false;
        };
        let Some(method) = self.consts.get(method as usize).and_then(Val::as_str) else {
            return false;
        };
        match (receiver, method) {
            ("os", "clock") => {
                self.apply_type_fact(dst, &Type::Float);
                true
            }
            ("os", "epoch" | "time") => {
                self.apply_type_fact(dst, &Type::Int);
                true
            }
            _ => false,
        }
    }

    fn clear_container_facts(&mut self, reg: u16) {
        self.list_locals.remove(&reg);
        self.list_value_types.remove(&reg);
        self.list_lengths.remove(&reg);
        self.list_value_adoptable.remove(&reg);
        self.clear_map_facts(reg);
    }

    fn clear_map_facts(&mut self, reg: u16) {
        self.map_locals.remove(&reg);
        self.map_value_types.remove(&reg);
        self.map_value_adoptable.remove(&reg);
    }

    fn invalidate_unknown_call_side_effect_facts(&mut self, op: &Op) {
        match *op {
            Op::Call { f, base, argc, .. }
            | Op::CallExact { f, base, argc, .. }
            | Op::CallClosureExact { f, base, argc, .. }
            | Op::CallNativeFast { f, base, argc, .. } => {
                self.clear_all_container_facts();
                self.clear_container_facts(f);
                for reg in base..base.saturating_add(argc as u16) {
                    self.clear_container_facts(reg);
                }
            }
            Op::CallNamed {
                f,
                base_pos,
                posc,
                base_named,
                namedc,
                ..
            }
            | Op::CallNamedFallback {
                f,
                base_pos,
                posc,
                base_named,
                namedc,
                ..
            } => {
                self.clear_all_container_facts();
                self.clear_container_facts(f);
                for reg in base_pos..base_pos.saturating_add(posc as u16) {
                    self.clear_container_facts(reg);
                }
                for reg in base_named..base_named.saturating_add((namedc as u16).saturating_mul(2)) {
                    self.clear_container_facts(reg);
                }
            }
            Op::CallMethod0 { receiver, .. } => {
                self.clear_all_container_facts();
                self.clear_container_facts(receiver);
            }
            _ => {}
        }
    }

    fn clear_all_container_facts(&mut self) {
        self.list_locals.clear();
        self.list_value_types.clear();
        self.list_lengths.clear();
        self.list_value_adoptable.clear();
        self.map_locals.clear();
        self.map_value_types.clear();
        self.map_value_adoptable.clear();
    }
}
