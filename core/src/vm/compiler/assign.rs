use anyhow::{Result, bail};

use crate::{expr::Expr, operator::BinOp, val::LiteralVal};

use crate::vm::analysis::{PerfCellMoveFact, PerfContainerMoveFact, PerfIndexTargetKind, PerfValueKind};

use super::{
    Compiler, Instr, Opcode, checked_u8,
    facts::index_fact_from_target,
    get_field_key,
    support::int_immediate_operand,
    support::{NumericFlavor, numeric_flavor, simple_local_expr_name},
};

impl Compiler {
    pub(super) fn lower_assign(&mut self, name: &str, value: &Expr) -> Result<()> {
        if self.try_lower_rewritten_set_index_assign(name, value)? {
            self.clear_const_map_local(name);
            return Ok(());
        }

        if let Some(dst) = self.locals.get(name).copied() {
            if self.cell_locals.contains(name) {
                let src = self.lower_readonly_operand(value)?;
                let move_value = !self.is_current_local_slot(src);
                self.emit_store_cell_value_with_policy(dst, src, "assign cell", move_value)?;
            } else if self.try_rebind_simple_local_assign(name, value)? {
                return Ok(());
            } else {
                let (dst, rebind_dst) = self.local_write_slot(dst);
                if self.try_lower_expr_to_register(dst, value)? {
                    if rebind_dst {
                        self.insert_local(name.to_string(), dst);
                    }
                } else {
                    let src = self.lower_expr(value)?;
                    let move_source = !self.is_current_local_slot(src);
                    self.emit_move_with_policy(dst, src, "assign local", move_source)?;
                    if rebind_dst {
                        self.insert_local(name.to_string(), dst);
                    }
                }
            }
        } else if let Some(capture) = self.capture_names.get(name).copied()
            && self.capture_cells.contains(name)
        {
            let src = self.lower_readonly_operand(value)?;
            let move_value = !self.is_current_local_slot(src);
            let cell = self.emit_load_capture(capture)?;
            self.emit_store_cell_value_with_policy(cell, src, "assign capture cell", move_value)?;
        } else if let Some(slot) = self.global_names.get(name).copied() {
            let src = self.lower_readonly_operand(value)?;
            let move_source = !self.is_current_local_slot(src);
            self.emit_set_global_with_policy(src, slot, move_source)?;
        } else {
            bail!("Compiler assignment to undefined local/global `{name}`");
        }
        self.record_const_map_local_from_expr(name, value)?;
        Ok(())
    }

    pub(super) fn lower_compound_assign(&mut self, name: &str, op: &BinOp, value: &Expr) -> Result<()> {
        if self.try_lower_int_immediate_compound_assign(name, op, value)? {
            self.clear_const_map_local(name);
            return Ok(());
        }
        if self.try_lower_additive_compound_assign(name, op, value)? {
            self.clear_const_map_local(name);
            return Ok(());
        }

        let rhs = self.lower_readonly_operand(value)?;
        if let Some(dst) = self.locals.get(name).copied() {
            let lhs = if self.cell_locals.contains(name) {
                self.emit_load_cell_value(dst)?
            } else {
                dst
            };
            let (dst, rebind_dst) = if self.cell_locals.contains(name) {
                (dst, false)
            } else {
                self.local_write_slot(dst)
            };
            let result = self.emit_bin_op_to_register(dst, op, lhs, rhs)?;
            if self.cell_locals.contains(name) {
                self.emit_store_cell_value(dst, result, "compound assign cell")?;
            } else {
                if result != dst {
                    let move_source = !self.is_current_local_slot(result);
                    self.emit_move_with_policy(dst, result, "compound assign local", move_source)?;
                }
                if rebind_dst {
                    self.insert_local(name.to_string(), dst);
                }
            }
        } else if let Some(capture) = self.capture_names.get(name).copied()
            && self.capture_cells.contains(name)
        {
            let cell = self.emit_load_capture(capture)?;
            let lhs = self.emit_load_cell_value(cell)?;
            let result = self.emit_bin_op_to_register(lhs, op, lhs, rhs)?;
            self.emit_store_cell_value(cell, result, "compound assign capture cell")?;
        } else if let Some(slot) = self.global_names.get(name).copied() {
            let lhs = self.emit_get_global(slot)?;
            let dst = self.alloc_reg();
            let result = self.emit_bin_op_to_register(dst, op, lhs, rhs)?;
            self.emit_set_global_with_policy(result, slot, true)?;
        } else {
            bail!("Compiler compound assignment to undefined local/global `{name}`");
        }
        self.clear_const_map_local(name);
        Ok(())
    }

    fn try_lower_int_immediate_compound_assign(&mut self, name: &str, op: &BinOp, value: &Expr) -> Result<bool> {
        let Some(immediate) = int_immediate_operand(op, value) else {
            return Ok(false);
        };
        let Some(lhs) = self.locals.get(name).copied() else {
            return Ok(false);
        };
        if self.cell_locals.contains(name) || self.function.performance.value_kind(lhs) != PerfValueKind::Int {
            return Ok(false);
        }
        let (dst, rebind_dst) = self.local_write_slot(lhs);
        self.emit_int_immediate_to_register(dst, op, lhs, immediate)?;
        if rebind_dst {
            self.insert_local(name.to_string(), dst);
        }
        Ok(true)
    }

    fn try_lower_additive_compound_assign(&mut self, name: &str, op: &BinOp, value: &Expr) -> Result<bool> {
        if !matches!(op, BinOp::Add) || self.cell_locals.contains(name) || expr_references_local_name(value, name) {
            return Ok(false);
        }
        let Some(lhs) = self.locals.get(name).copied() else {
            return self.try_lower_global_additive_compound_assign(name, value);
        };
        if self.function.performance.value_kind(lhs) != PerfValueKind::Int
            || !additive_expr_is_int_like(value, &self.locals, &self.function.performance)
        {
            return Ok(false);
        }

        let mut terms = Vec::new();
        collect_add_terms(value, &mut terms);
        if terms.len() < 2 {
            return Ok(false);
        }

        let (dst, rebind_dst) = self.local_write_slot(lhs);
        let mut acc = lhs;
        for term in terms {
            let term = if let Some(value) = self.cached_loop_int_expr_value(term) {
                if let Some(reg) = self.cached_loop_literal(&LiteralVal::Int(value)) {
                    reg
                } else {
                    self.lower_readonly_operand(term)?
                }
            } else {
                self.lower_readonly_operand(term)?
            };
            self.emit_bin_op_to_register_with_flavor(dst, &BinOp::Add, acc, term, NumericFlavor::Int)?;
            acc = dst;
        }
        if rebind_dst {
            self.insert_local(name.to_string(), dst);
        }
        Ok(true)
    }

    fn try_lower_global_additive_compound_assign(&mut self, name: &str, value: &Expr) -> Result<bool> {
        let Some(slot) = self.global_names.get(name).copied() else {
            return Ok(false);
        };
        if !additive_expr_is_int_like(value, &self.locals, &self.function.performance) {
            return Ok(false);
        }

        let mut terms = Vec::new();
        collect_add_terms(value, &mut terms);
        if terms.len() < 2 {
            return Ok(false);
        }

        let dst = self.emit_get_global(slot)?;
        let mut acc = dst;
        for term in terms {
            let term = if let Some(value) = self.cached_loop_int_expr_value(term) {
                if let Some(reg) = self.cached_loop_literal(&LiteralVal::Int(value)) {
                    reg
                } else {
                    self.lower_readonly_operand(term)?
                }
            } else {
                self.lower_readonly_operand(term)?
            };
            self.emit_bin_op_to_register_with_flavor(dst, &BinOp::Add, acc, term, NumericFlavor::Int)?;
            acc = dst;
        }
        self.emit_set_global_with_policy(dst, slot, true)?;
        Ok(true)
    }

    fn try_rebind_simple_local_assign(&mut self, name: &str, value: &Expr) -> Result<bool> {
        if self.local_rebind_suppression != 0 || !self.loops.is_empty() {
            return Ok(false);
        }
        let Some(src_name) = simple_local_expr_name(value) else {
            return Ok(false);
        };
        if src_name == name || self.cell_locals.contains(src_name) {
            return Ok(false);
        }
        let Some(src) = self.locals.get(src_name).copied() else {
            return Ok(false);
        };
        self.insert_local(name.to_string(), src);
        if let Some(map) = self.const_map_locals.get(src_name).cloned() {
            self.const_map_locals.insert(name.to_string(), map);
        } else {
            self.clear_const_map_local(name);
        }
        Ok(true)
    }

    pub(super) fn emit_store_cell_value(&mut self, cell: u16, src: u16, context: &str) -> Result<()> {
        self.emit_store_cell_value_with_policy(cell, src, context, true)
    }

    pub(super) fn emit_store_cell_value_with_policy(
        &mut self,
        cell: u16,
        src: u16,
        context: &str,
        move_value: bool,
    ) -> Result<()> {
        let pc = self.function.code.len();
        self.emit(Instr::abc(
            Opcode::StoreCellVal,
            checked_u8(&format!("{context} cell"), cell)?,
            checked_u8(&format!("{context} src"), src)?,
            0,
        ));
        self.function
            .performance
            .set_cell_move_fact(pc, PerfCellMoveFact { move_value });
        Ok(())
    }

    pub(super) fn try_lower_rewritten_set_index_expr(&mut self, expr: &Expr) -> Result<bool> {
        let Some((target, key, value)) = rewritten_map_set_call(expr) else {
            return Ok(false);
        };
        self.emit_set_index_expr(target, key, value)?;
        Ok(true)
    }

    fn try_lower_rewritten_set_index_assign(&mut self, name: &str, value: &Expr) -> Result<bool> {
        if let Some((target, key, value)) = rewritten_list_set_assign(name, value) {
            self.emit_set_index_expr(target, key, value)?;
            return Ok(true);
        }
        if let Some((target, key, value)) = rewritten_object_set_assign(name, value) {
            self.emit_set_index_expr(target, key, value)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn emit_set_index_expr(&mut self, target: &Expr, key: &Expr, value: &Expr) -> Result<()> {
        self.clear_const_map_target(target);
        let target = self.lower_readonly_access_target(target)?;
        let index_fact = index_fact_from_target(&self.function.performance, target)
            .filter(|fact| fact.target_kind != PerfIndexTargetKind::String);
        let move_key = set_index_key_move_preferred(key);
        let (key, key_fact) = self.lower_index_key_for_target(target, index_fact, key)?;
        let move_key = move_key && !self.is_current_local_slot(key);
        let value = self.lower_readonly_operand(value)?;
        let move_value = !self.is_current_local_slot(value);
        let pc = self.function.code.len();
        if let Some(const_key) = get_field_key(index_fact, key_fact) {
            self.emit(Instr::abc(
                Opcode::SetFieldK,
                checked_u8("set field target", target)?,
                checked_u8("set field value", value)?,
                checked_u8("set field key", const_key)?,
            ));
        } else {
            self.emit(Instr::abc(
                Opcode::SetIndex,
                checked_u8("set index target", target)?,
                checked_u8("set index key", key)?,
                checked_u8("set index value", value)?,
            ));
            if let Some(fact) = key_fact {
                self.function.performance.set_key_fact(pc, fact);
            }
        }
        self.function
            .performance
            .set_container_move_fact(pc, PerfContainerMoveFact { move_key, move_value });
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        Ok(())
    }
}

fn collect_add_terms<'a>(expr: &'a Expr, terms: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Paren(inner) => collect_add_terms(inner, terms),
        Expr::Bin(lhs, BinOp::Add, rhs) => {
            collect_add_terms(lhs, terms);
            collect_add_terms(rhs, terms);
        }
        _ => terms.push(expr),
    }
}

fn additive_expr_is_int_like(
    expr: &Expr,
    locals: &std::collections::HashMap<String, u16>,
    facts: &crate::vm::analysis::PerformanceFacts,
) -> bool {
    match expr {
        Expr::Paren(inner) => additive_expr_is_int_like(inner, locals, facts),
        Expr::Literal(LiteralVal::Int(_)) => true,
        Expr::Var(name) => locals
            .get(name)
            .copied()
            .is_some_and(|reg| facts.value_kind(reg) == PerfValueKind::Int),
        Expr::Bin(lhs, op, rhs)
            if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod)
                && numeric_flavor(lhs, op, rhs) == NumericFlavor::Int =>
        {
            additive_expr_is_int_like(lhs, locals, facts) && additive_expr_is_int_like(rhs, locals, facts)
        }
        _ => false,
    }
}

fn expr_references_local_name(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Paren(inner) => expr_references_local_name(inner, name),
        Expr::Var(value) => value == name,
        Expr::Bin(lhs, _, rhs) => expr_references_local_name(lhs, name) || expr_references_local_name(rhs, name),
        _ => false,
    }
}

fn rewritten_list_set_assign<'a>(name: &str, expr: &'a Expr) -> Option<(&'a Expr, &'a Expr, &'a Expr)> {
    let Expr::Access(list_set, index) = expr else {
        return None;
    };
    if !matches!(index.as_ref(), Expr::Literal(LiteralVal::Int(0))) {
        return None;
    }
    let Expr::CallExpr(callee, args) = list_set.as_ref() else {
        return None;
    };
    if args.len() != 3 || !is_access_name(callee, "list", "set") || !is_var(&args[0], name) {
        return None;
    }
    Some((&args[0], &args[1], &args[2]))
}

fn rewritten_object_set_assign<'a>(name: &str, expr: &'a Expr) -> Option<(&'a Expr, &'a Expr, &'a Expr)> {
    let Expr::CallExpr(callee, args) = expr else {
        return None;
    };
    if args.len() != 3 || !is_var(callee, "__lk_set_field") || !is_var(&args[0], name) {
        return None;
    }
    Some((&args[0], &args[1], &args[2]))
}

fn rewritten_map_set_call(expr: &Expr) -> Option<(&Expr, &Expr, &Expr)> {
    let Expr::CallExpr(callee, args) = expr else {
        return None;
    };
    if args.len() != 3 || !is_var(callee, "__lk_set_index") {
        return None;
    }
    Some((&args[0], &args[1], &args[2]))
}

fn is_access_name(expr: &Expr, receiver: &str, method: &str) -> bool {
    let Expr::Access(target, field) = expr else {
        return false;
    };
    is_var(target, receiver) && is_string_literal(field, method)
}

fn is_var(expr: &Expr, expected: &str) -> bool {
    matches!(expr, Expr::Var(name) if name == expected)
}

fn set_index_key_move_preferred(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => set_index_key_move_preferred(inner),
        Expr::Var(_) => false,
        _ => true,
    }
}

fn is_string_literal(expr: &Expr, expected: &str) -> bool {
    match expr {
        Expr::Literal(value) => value.as_str() == Some(expected),
        _ => false,
    }
}
