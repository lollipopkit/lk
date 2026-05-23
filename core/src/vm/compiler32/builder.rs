use anyhow::{Result, anyhow, bail};

use super::{Compiler32, ConstHeapValue32, Function32, Instr32, Opcode32, support::*};

impl Compiler32 {
    #[inline]
    pub(super) fn alloc_reg(&mut self) -> u16 {
        let reg = self.next_reg;
        self.next_reg = self.next_reg.checked_add(1).expect("Compiler32 register overflow");
        if self.next_reg > self.peak_reg {
            self.peak_reg = self.next_reg;
        }
        reg
    }

    pub(super) fn alloc_regs(&mut self, count: usize) -> Result<u16> {
        let count = u16::try_from(count).map_err(|_| anyhow!("Compiler32 register block too large: {count}"))?;
        let base = self.next_reg;
        self.next_reg = self
            .next_reg
            .checked_add(count)
            .ok_or_else(|| anyhow!("Compiler32 register overflow"))?;
        if self.next_reg > self.peak_reg {
            self.peak_reg = self.next_reg;
        }
        Ok(base)
    }

    pub(super) fn live_register_floor(&self) -> u16 {
        self.locals
            .values()
            .copied()
            .max()
            .map_or(self.function.param_count, |reg| reg + 1)
            .max(self.function.param_count)
    }

    #[inline]
    pub(super) fn emit(&mut self, instr: Instr32) {
        self.function.code.push(instr);
    }

    pub(super) fn emit_move(&mut self, dst: u16, src: u16, context: &str) -> Result<()> {
        self.emit(Instr32::abc(
            Opcode32::Move,
            checked_u8(&format!("{context} dst"), dst)?,
            checked_u8(&format!("{context} src"), src)?,
            0,
        ));
        Ok(())
    }

    pub(super) fn emit_test_placeholder(&mut self, condition: u16) -> Result<usize> {
        let pc = self.function.code.len();
        // Trampoline pair: Test (c=1 skips the Jmp when condition matches) + Jmp to the real target.
        // patch_test_false/true_jump fills in the Jmp offset; the Test field B is set to the
        // inverted sense so that a matching condition falls through to the Jmp.
        self.emit(Instr32::abc(
            Opcode32::Test,
            checked_u8("test condition", condition)?,
            1, // placeholder; overwritten by patch_test_jump
            1, // C=1: jump to pc+2 (body) when condition does NOT match the Jmp path
        ));
        // Always emit a Jmp placeholder immediately after; patch_test_jump will set its offset.
        self.emit(Instr32::sj(Opcode32::Jmp, 0));
        Ok(pc)
    }

    pub(super) fn emit_jmp_placeholder(&mut self) -> usize {
        let pc = self.function.code.len();
        self.emit(Instr32::sj(Opcode32::Jmp, 0));
        pc
    }

    pub(super) fn emit_raise(&mut self, message: &str) -> Result<()> {
        let const_index = self.push_string(message)?;
        self.emit(Instr32::abx(Opcode32::Raise, 0, const_index));
        Ok(())
    }

    pub(super) fn emit_pattern_assert(&mut self, condition: u16) -> Result<()> {
        let skip_raise = self.emit_test_placeholder(condition)?;
        self.emit_raise("Pattern does not match value")?;
        let end = self.function.code.len();
        self.patch_test_true_jump(skip_raise, end)
    }

    pub(super) fn patch_test_false_jump(&mut self, pc: usize, target: usize) -> Result<()> {
        self.patch_test_jump(pc, target, 1)
    }

    pub(super) fn patch_test_true_jump(&mut self, pc: usize, target: usize) -> Result<()> {
        self.patch_test_jump(pc, target, 0)
    }

    fn patch_test_jump(&mut self, pc: usize, target: usize, expected: u8) -> Result<()> {
        // Trampoline scheme: the Test instruction (at `pc`) skips 1 ahead (c=1) when the
        // condition does NOT satisfy the exit path, landing at pc+2 (body/continuation).
        // When the condition DOES satisfy the exit path, it falls through to pc+1 (the Jmp)
        // which carries the potentially-large offset to `target`.
        //
        // expected=1 (patch_test_false_jump): we want to jump to `target` when FALSY.
        //   Test B=0, c=1: TRUTHY → jump to pc+2 (body); FALSY → fallthrough to Jmp[target].
        // expected=0 (patch_test_true_jump): we want to jump to `target` when TRUTHY.
        //   Test B=1, c=1: FALSY → jump to pc+2 (continuation); TRUTHY → fallthrough to Jmp[target].
        let instr = *self
            .function
            .code
            .get(pc)
            .ok_or_else(|| anyhow!("Compiler32 test patch pc {pc} out of bounds"))?;
        if instr.opcode() != Opcode32::Test {
            bail!("Compiler32 expected Test at patch pc {pc}");
        }
        // Inverted B: when expected=1, we set B=0 (jump to pc+2 on truthy, fallthrough on falsy).
        let test_b: u8 = 1 - expected;
        self.function.code[pc] = Instr32::abc(Opcode32::Test, instr.a(), test_b, 1);
        // Patch the Jmp placeholder at pc+1 to jump to `target`.
        self.patch_jmp(pc + 1, target)
    }

    pub(super) fn patch_jmp(&mut self, pc: usize, target: usize) -> Result<()> {
        let instr = *self
            .function
            .code
            .get(pc)
            .ok_or_else(|| anyhow!("Compiler32 jump patch pc {pc} out of bounds"))?;
        if instr.opcode() != Opcode32::Jmp {
            bail!("Compiler32 expected Jmp at patch pc {pc}");
        }
        self.function.code[pc] = Instr32::sj(Opcode32::Jmp, jump_offset(pc, target)?);
        Ok(())
    }

    pub(super) fn push_int(&mut self, value: i64) -> Result<u16> {
        self.function.consts.push_int(value)
    }

    pub(super) fn push_float(&mut self, value: f64) -> Result<u16> {
        self.function.consts.push_float(value)
    }

    pub(super) fn push_string(&mut self, value: &str) -> Result<u16> {
        self.function.consts.push_string(value)
    }

    pub(super) fn push_heap_value(&mut self, value: ConstHeapValue32) -> Result<u16> {
        self.function.consts.push_heap_value(value)
    }

    pub(super) fn emit_return(&mut self, base: u16) -> Result<()> {
        self.emit(Instr32::abc(Opcode32::Return, checked_u8("return base", base)?, 1, 0));
        self.emitted_return = true;
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<Function32> {
        self.function.register_count = self.peak_reg;
        Ok(std::mem::take(&mut self.function))
    }
}
