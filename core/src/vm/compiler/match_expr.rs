#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
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

        // Each arm is its own path, so `emitted_return` is saved and restored
        // around every body — the same discipline `lower_if` uses for its two
        // branches. Reading the flag *between* arms instead made a `return` in
        // the first arm skip the lowering of every later arm's body: `match n
        // { 0 => { return 7; } _ => { return 9; } }` compiled to a test with
        // nothing behind it, so `n == 1` fell out of the match, past the end
        // of a function declared `-> Int`, and answered nil.
        let watermark = self.next_reg;
        let mut end_jumps = Vec::new();
        let mut every_arm_returns = true;
        let mut some_arm_matches_everything = false;
        for arm in arms {
            // An arm that matches every value is entered unconditionally: the
            // test would always pass, and the edge where it fails is a path
            // that does not exist. Both backends read that phantom edge —
            // native lowering saw a function whose every arm returns still
            // able to fall off its end, and rejected it.
            let (test_pc, previous) = match self.bind_catch_all(&arm.pattern, value) {
                Some(previous) => (None, previous),
                None => {
                    let (condition, previous) = self.lower_pattern_match(&arm.pattern, value)?;
                    (Some(self.emit_test_placeholder(condition)?), previous)
                }
            };

            self.emitted_return = false;
            self.lower_expr_to_register(dst, &arm.body, "match result")?;
            let arm_returns = self.emitted_return;
            if !arm_returns {
                end_jumps.push(self.emit_jmp_placeholder());
            }
            every_arm_returns &= arm_returns;
            some_arm_matches_everything |= test_pc.is_none();

            self.restore_pattern_bindings(previous);
            self.next_reg = watermark; // recycle the arm's bindings and temporaries
            if let Some(test_pc) = test_pc {
                let next_arm = self.function.code.len();
                self.patch_test_false_jump(test_pc, next_arm)?;
            }
        }

        // No arm matched: the match answers nil. That path is what makes the
        // checker type a match without a catch-all `T?` rather than `T`.
        let falls_through = !some_arm_matches_everything;
        if falls_through {
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
        self.emitted_return = every_arm_returns && !falls_through;
        Ok(dst)
    }
}
