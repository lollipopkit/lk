use anyhow::Result;

use crate::expr::{Expr, MatchArm};

use super::{Compiler, Instr, Opcode, support::checked_u8};

impl Compiler {
    pub(super) fn lower_match_expr(&mut self, value: &Expr, arms: &[MatchArm]) -> Result<u16> {
        let value = self.lower_readonly_operand(value)?;
        let dst = self.alloc_reg();
        if arms.is_empty() {
            self.emit(Instr::abc(Opcode::LoadNil, checked_u8("match dst", dst)?, 0, 0));
            return Ok(dst);
        }

        let mut end_jumps = Vec::new();
        for arm in arms {
            let (condition, previous) = self.lower_pattern_match(&arm.pattern, value)?;
            let test_pc = self.emit_test_placeholder(condition)?;
            if !self.emitted_return {
                self.lower_expr_to_register(dst, &arm.body, "match result")?;
                end_jumps.push(self.emit_jmp_placeholder());
            }
            self.restore_pattern_bindings(previous);
            let next_arm = self.function.code.len();
            self.patch_test_false_jump(test_pc, next_arm)?;
        }

        if !self.emitted_return {
            self.emit(Instr::abc(
                Opcode::LoadNil,
                checked_u8("match fallback dst", dst)?,
                0,
                0,
            ));
        }
        let end = self.function.code.len();
        for pc in end_jumps {
            self.patch_jmp(pc, end)?;
        }
        self.emitted_return = false;
        Ok(dst)
    }
}
