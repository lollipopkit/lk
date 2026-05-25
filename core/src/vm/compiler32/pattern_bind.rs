use anyhow::{Result, bail};

use crate::{
    expr::{Expr, Pattern},
    val::LiteralVal,
};

use super::{Compiler32, Instr32, Opcode32, support::checked_u8};

impl Compiler32 {
    pub(super) fn lower_let(&mut self, pattern: &Pattern, value: &Expr) -> Result<()> {
        if let Pattern::Variable(name) = pattern {
            let watermark = self.next_reg;
            let slot = if let Some(slot) = self.locals.get(name).copied() {
                slot
            } else {
                self.alloc_reg()
            };
            if !self.try_lower_expr_to_register(slot, value)? {
                let value = self.lower_expr(value)?;
                let move_source = !self.is_current_local_slot(value);
                self.emit_move_with_policy(slot, value, "let local", move_source)?;
            }
            if self.top_level
                && let Some(global_slot) = self.global_names.get(name).copied()
            {
                self.emit_set_global(slot, global_slot)?;
            }
            self.insert_local(name.clone(), slot);
            self.next_reg = self.live_register_floor().max(watermark).max(slot + 1);
            return Ok(());
        }
        if matches!(pattern, Pattern::Wildcard) {
            let watermark = self.next_reg;
            self.lower_expr(value)?;
            self.next_reg = watermark;
            return Ok(());
        }
        let value = self.lower_expr(value)?;
        self.bind_let_pattern(pattern, value)
    }

    fn bind_let_pattern(&mut self, pattern: &Pattern, value: u16) -> Result<()> {
        match pattern {
            Pattern::Variable(name) => {
                if self.top_level {
                    if let Some(slot) = self.global_names.get(name).copied() {
                        self.emit_set_global(value, slot)?;
                    }
                }
                self.insert_local(name.clone(), value);
                Ok(())
            }
            Pattern::Wildcard => Ok(()),
            Pattern::List { patterns, rest } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_let_sequence(patterns, value)?;
                if let Some(rest) = rest {
                    let start = self.lower_val(&LiteralVal::Int(patterns.len() as i64))?;
                    let slice = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::SliceFrom,
                        checked_u8("let rest slice", slice)?,
                        checked_u8("let rest value", value)?,
                        checked_u8("let rest start", start)?,
                    ));
                    self.insert_local(rest.clone(), slice);
                }
                Ok(())
            }
            Pattern::Map { patterns, rest } => {
                let condition = self.lower_map_pattern_condition(value, patterns)?;
                self.emit_pattern_assert(condition)?;
                for (key, pattern) in patterns {
                    let key = self.lower_val(&LiteralVal::from_str(key))?;
                    let field = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::GetIndex,
                        checked_u8("let map field", field)?,
                        checked_u8("let map value", value)?,
                        checked_u8("let map key", key)?,
                    ));
                    self.bind_let_pattern(pattern, field)?;
                }
                if let Some(rest) = rest {
                    let map = self.lower_map_rest(value, patterns)?;
                    self.insert_local(rest.clone(), map);
                }
                Ok(())
            }
            Pattern::Literal(_) | Pattern::Range { .. } | Pattern::Guard { .. } | Pattern::Or(_) => {
                let (condition, previous) = self.lower_pattern_match(pattern, value)?;
                if !previous.is_empty() {
                    self.restore_pattern_bindings(previous);
                    bail!("Compiler32 does not support binding variables inside refutable let pattern yet");
                }
                self.emit_pattern_assert(condition)
            }
        }
    }

    pub(super) fn lower_map_rest(&mut self, value: u16, patterns: &[(String, Pattern)]) -> Result<u16> {
        if patterns.len() > i8::MAX as usize {
            bail!("Compiler32 map rest has {} keys, max {}", patterns.len(), i8::MAX);
        }
        let base = self.alloc_regs(patterns.len() + 1)?;
        self.emit_move(base, value, "map rest source")?;
        for (offset, (key, _)) in patterns.iter().enumerate() {
            let key_reg = self.lower_val(&LiteralVal::from_str(key))?;
            self.emit_move(base + 1 + offset as u16, key_reg, "map rest key")?;
        }
        let dst = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::MapRest,
            checked_u8("map rest dst", dst)?,
            checked_u8("map rest base", base)?,
            checked_u8("map rest key count", patterns.len() as u16)?,
        ));
        Ok(dst)
    }

    fn bind_let_sequence(&mut self, patterns: &[Pattern], value: u16) -> Result<()> {
        for (index, pattern) in patterns.iter().enumerate() {
            let index = i64::try_from(index).map_err(|_| anyhow::anyhow!("Compiler32 let pattern index overflow"))?;
            let key = self.lower_val(&LiteralVal::Int(index))?;
            let field = self.alloc_reg();
            self.emit(Instr32::abc(
                Opcode32::GetIndex,
                checked_u8("let sequence field", field)?,
                checked_u8("let sequence value", value)?,
                checked_u8("let sequence index", key)?,
            ));
            self.bind_let_pattern(pattern, field)?;
        }
        Ok(())
    }
}
