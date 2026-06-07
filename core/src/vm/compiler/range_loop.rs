use anyhow::Result;

use crate::{
    operator::BinOp,
    stmt::Stmt,
    val::LiteralVal,
    vm::{Instr, Opcode, analysis::PerfForLoopFact},
};

use super::{Compiler, LoopPatch, checked_u8, support::jump_offset};

impl Compiler {
    pub(super) fn lower_for_range_static_loop(
        &mut self,
        index: u16,
        end: u16,
        step: u16,
        inclusive: bool,
        positive_step: bool,
        body: &Stmt,
    ) -> Result<()> {
        let condition = self.alloc_reg();
        self.emit(Instr::abc(
            match (positive_step, inclusive) {
                (true, true) => Opcode::CmpLeInt,
                (true, false) => Opcode::CmpLtInt,
                (false, true) => Opcode::CmpGeInt,
                (false, false) => Opcode::CmpGtInt,
            },
            checked_u8("for range static condition dst", condition)?,
            checked_u8("for range static index", index)?,
            checked_u8("for range static end", end)?,
        ));

        let exit_test = self.emit_test_placeholder(condition)?;
        let body_start = self.function.code.len();
        self.loops.push(LoopPatch::default());
        self.emitted_return = false;
        self.lower_stmt(body)?;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");

        let step_start = self.function.code.len();
        if !self.emitted_return {
            let offset = jump_offset(step_start, body_start)?;
            self.emit(Instr::abc(
                Opcode::ForLoopI,
                checked_u8("for loop index", index)?,
                checked_u8("for loop end", end)?,
                checked_u8("for loop step", step)?,
            ));
            self.function.performance.set_for_loop_fact(
                step_start,
                PerfForLoopFact {
                    jump_offset: offset,
                    inclusive,
                    positive_step,
                },
            );
        }

        let loop_end = self.function.code.len();
        self.patch_test_false_jump(exit_test, loop_end)?;
        self.patch_loop_jumps(loop_patch, loop_end, step_start)?;
        self.emitted_return = false;
        Ok(())
    }

    pub(super) fn lower_for_range_dynamic_loop(
        &mut self,
        index: u16,
        end: u16,
        step: u16,
        inclusive: bool,
        body: &Stmt,
    ) -> Result<()> {
        let zero = self.lower_val(&LiteralVal::Int(0))?;
        let loop_start = self.function.code.len();
        let is_positive = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::CmpGtInt,
            checked_u8("for step sign dst", is_positive)?,
            checked_u8("for step", step)?,
            checked_u8("for zero", zero)?,
        ));

        let negative_branch = self.emit_test_placeholder(is_positive)?;
        let positive_cond = self.alloc_reg();
        self.emit(Instr::abc(
            if inclusive { Opcode::CmpLeInt } else { Opcode::CmpLtInt },
            checked_u8("for positive cond dst", positive_cond)?,
            checked_u8("for index", index)?,
            checked_u8("for end", end)?,
        ));
        let positive_exit_test = self.emit_test_placeholder(positive_cond)?;
        let positive_body_jump = self.emit_jmp_placeholder();

        let negative_start = self.function.code.len();
        self.patch_test_false_jump(negative_branch, negative_start)?;
        let negative_cond = self.alloc_reg();
        self.emit(Instr::abc(
            if inclusive { Opcode::CmpGeInt } else { Opcode::CmpGtInt },
            checked_u8("for negative cond dst", negative_cond)?,
            checked_u8("for index", index)?,
            checked_u8("for end", end)?,
        ));
        let negative_exit_test = self.emit_test_placeholder(negative_cond)?;

        let body_start = self.function.code.len();
        self.patch_jmp(positive_body_jump, body_start)?;
        self.loops.push(LoopPatch::default());
        self.emitted_return = false;
        self.lower_stmt(body)?;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");

        let step_start = self.function.code.len();
        if !self.emitted_return {
            self.emit_bin_op_to_register(index, &BinOp::Add, index, step)?;
            let jmp_back = self.emit_jmp_placeholder();
            self.patch_jmp(jmp_back, loop_start)?;
        }

        let loop_end = self.function.code.len();
        self.patch_test_false_jump(positive_exit_test, loop_end)?;
        self.patch_test_false_jump(negative_exit_test, loop_end)?;
        self.patch_loop_jumps(loop_patch, loop_end, step_start)?;
        self.emitted_return = false;
        Ok(())
    }

    fn patch_loop_jumps(&mut self, loop_patch: LoopPatch, loop_end: usize, step_start: usize) -> Result<()> {
        for pc in loop_patch.breaks {
            self.patch_jmp(pc, loop_end)?;
        }
        for pc in loop_patch.continues {
            self.patch_jmp(pc, step_start)?;
        }
        Ok(())
    }
}
