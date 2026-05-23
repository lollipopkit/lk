use anyhow::{Result, bail};

use crate::{expr::Expr, operator::BinOp, val::LiteralVal};

use crate::vm::analysis::{PerfContainerMoveFact, PerfIndexTargetKind};

use super::{Compiler32, Instr32, Opcode32, checked_u8, facts::index_fact_from_target};

impl Compiler32 {
    pub(super) fn lower_assign(&mut self, name: &str, value: &Expr) -> Result<()> {
        if self.try_lower_rewritten_set_index_assign(name, value)? {
            return Ok(());
        }

        let src = self.lower_expr(value)?;
        if let Some(dst) = self.locals.get(name).copied() {
            if self.cell_locals.contains(name) {
                self.emit(Instr32::abc(
                    Opcode32::StoreCellVal,
                    checked_u8("assign cell", dst)?,
                    checked_u8("assign src", src)?,
                    0,
                ));
            } else {
                self.emit_move(dst, src, "assign local")?;
            }
        } else if let Some(capture) = self.capture_names.get(name).copied()
            && self.capture_cells.contains(name)
        {
            let cell = self.emit_load_capture(capture)?;
            self.emit(Instr32::abc(
                Opcode32::StoreCellVal,
                checked_u8("assign capture cell", cell)?,
                checked_u8("assign src", src)?,
                0,
            ));
        } else if let Some(slot) = self.global_names.get(name).copied() {
            self.emit_set_global(src, slot)?;
        } else {
            bail!("Compiler32 assignment to undefined local/global `{name}`");
        }
        Ok(())
    }

    pub(super) fn lower_compound_assign(&mut self, name: &str, op: &BinOp, value: &Expr) -> Result<()> {
        let rhs = self.lower_expr(value)?;
        if let Some(dst) = self.locals.get(name).copied() {
            let lhs = if self.cell_locals.contains(name) {
                self.emit_load_cell_value(dst)?
            } else {
                dst
            };
            let result = self.emit_bin_op_to_register(dst, op, lhs, rhs)?;
            if self.cell_locals.contains(name) {
                self.emit(Instr32::abc(
                    Opcode32::StoreCellVal,
                    checked_u8("compound assign cell", dst)?,
                    checked_u8("compound assign src", result)?,
                    0,
                ));
            } else {
                self.emit_move(dst, result, "compound assign local")?;
            }
        } else if let Some(capture) = self.capture_names.get(name).copied()
            && self.capture_cells.contains(name)
        {
            let cell = self.emit_load_capture(capture)?;
            let lhs = self.emit_load_cell_value(cell)?;
            let result = self.emit_bin_op_to_register(lhs, op, lhs, rhs)?;
            self.emit(Instr32::abc(
                Opcode32::StoreCellVal,
                checked_u8("compound assign capture cell", cell)?,
                checked_u8("compound assign capture src", result)?,
                0,
            ));
        } else if let Some(slot) = self.global_names.get(name).copied() {
            let lhs = self.emit_get_global(slot)?;
            let dst = self.alloc_reg();
            let result = self.emit_bin_op_to_register(dst, op, lhs, rhs)?;
            self.emit_set_global(result, slot)?;
        } else {
            bail!("Compiler32 compound assignment to undefined local/global `{name}`");
        }
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
        let target = self.lower_expr(target)?;
        let index_fact = index_fact_from_target(&self.function.performance, target)
            .filter(|fact| fact.target_kind != PerfIndexTargetKind::String);
        let move_key = set_index_key_move_preferred(key);
        let (key, key_fact) = self.lower_index_key(key)?;
        let value = self.lower_expr(value)?;
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::SetIndex,
            checked_u8("set index target", target)?,
            checked_u8("set index key", key)?,
            checked_u8("set index value", value)?,
        ));
        self.function.performance.set_container_move_fact(
            pc,
            PerfContainerMoveFact {
                move_key,
                move_value: true,
            },
        );
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        if let Some(fact) = key_fact {
            self.function.performance.set_key_fact(pc, fact);
        }
        Ok(())
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
