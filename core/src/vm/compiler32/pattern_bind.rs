use anyhow::{Result, bail};

use crate::{
    expr::{Expr, Pattern},
    val::Val,
};

use super::{Compiler32, Instr32, Opcode32, support::checked_u8};

impl Compiler32 {
    pub(super) fn lower_let(&mut self, pattern: &Pattern, value: &Expr) -> Result<()> {
        let value = self.lower_expr(value)?;
        self.bind_let_pattern(pattern, value)
    }

    fn bind_let_pattern(&mut self, pattern: &Pattern, value: u16) -> Result<()> {
        match pattern {
            Pattern::Variable(name) => {
                self.locals.insert(name.clone(), value);
                Ok(())
            }
            Pattern::Wildcard => Ok(()),
            Pattern::List { patterns, rest } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_let_sequence(patterns, value)?;
                if let Some(rest) = rest {
                    let start = self.lower_val(&Val::Int(patterns.len() as i64))?;
                    let slice = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::SliceFrom,
                        checked_u8("let rest slice", slice)?,
                        checked_u8("let rest value", value)?,
                        checked_u8("let rest start", start)?,
                    ));
                    self.locals.insert(rest.clone(), slice);
                }
                Ok(())
            }
            Pattern::Map { patterns, rest } => {
                let condition = self.lower_map_pattern_condition(value, patterns)?;
                self.emit_pattern_assert(condition)?;
                for (key, pattern) in patterns {
                    let key = self.lower_val(&Val::from_str(key))?;
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
                    self.locals.insert(rest.clone(), map);
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
            let key_reg = self.lower_val(&Val::from_str(key))?;
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
            let key = self.lower_val(&Val::Int(index))?;
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
