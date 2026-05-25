use anyhow::{Result, anyhow, bail};

use crate::vm::analysis::{
    PerfContainerFact, PerfControlFlowFacts, PerfFusedBoolBranchFact, PerfLocalCopyFact, PerfRegisterCopyFact,
    PerfRegisterFact, PerfValueFact, PerfValueKind,
};

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
        self.emit_move_with_policy(dst, src, context, false)
    }

    pub(super) fn emit_move_with_policy(&mut self, dst: u16, src: u16, context: &str, move_source: bool) -> Result<()> {
        if dst == src {
            return Ok(());
        }
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::Move,
            checked_u8(&format!("{context} dst"), dst)?,
            checked_u8(&format!("{context} src"), src)?,
            0,
        ));
        self.function
            .performance
            .set_register_copy_fact(pc, PerfRegisterCopyFact { move_source });
        if self.function.performance.is_local_slot(dst) {
            self.function
                .performance
                .set_local_copy_fact(pc, PerfLocalCopyFact { move_source });
        }
        self.function.performance.copy_register_fact(dst, src);
        Ok(())
    }

    pub(super) fn insert_local(&mut self, name: impl Into<String>, reg: u16) -> Option<u16> {
        self.function.performance.mark_local_slot(reg);
        self.locals.insert(name.into(), reg)
    }

    pub(super) fn mark_last_dead_write(&mut self) {
        if let Some(pc) = self.function.code.len().checked_sub(1) {
            self.function.performance.set_dead_write_fact(pc);
        }
    }

    pub(super) fn is_current_local_slot(&self, reg: u16) -> bool {
        self.locals.values().any(|slot| *slot == reg)
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

    pub(super) fn set_register_kind(&mut self, reg: u16, kind: PerfValueKind) {
        self.function.performance.set_register_kind(reg, kind);
    }

    pub(super) fn set_register_list_fact(&mut self, reg: u16, fact: PerfContainerFact) {
        let register = PerfRegisterFact {
            value: PerfValueFact {
                kind: PerfValueKind::List,
                ..PerfValueFact::default()
            },
            list: Some(fact),
            ..PerfRegisterFact::default()
        };
        self.function.performance.set_register_fact(reg, register);
    }

    pub(super) fn set_register_map_fact(&mut self, reg: u16, fact: PerfContainerFact) {
        let register = PerfRegisterFact {
            value: PerfValueFact {
                kind: PerfValueKind::Map,
                ..PerfValueFact::default()
            },
            map: Some(fact),
            ..PerfRegisterFact::default()
        };
        self.function.performance.set_register_fact(reg, register);
    }

    pub(super) fn finish(&mut self) -> Result<Function32> {
        self.function.register_count = self.peak_reg;
        let control_flow = build_control_flow_facts(&self.function.code)?;
        self.function.performance.set_control_flow_facts(control_flow);
        Ok(std::mem::take(&mut self.function))
    }
}

fn build_control_flow_facts(code: &[Instr32]) -> Result<PerfControlFlowFacts> {
    if code.is_empty() {
        return Ok(PerfControlFlowFacts::default());
    }

    let mut branch_targets = vec![false; code.len()];
    let mut block_starts = vec![false; code.len()];
    let mut fused_bool_branches = vec![None; code.len()];
    block_starts[0] = true;

    for (pc, instr) in code.iter().copied().enumerate() {
        if let Some(fact) = fused_bool_branch_fact(code, pc, instr) {
            fused_bool_branches[pc] = Some(fact);
        }
        match instr.opcode() {
            Opcode32::Test => {
                mark_target(pc + 1, code.len(), &mut branch_targets, &mut block_starts)?;
                mark_relative_target(
                    pc,
                    instr.c() as i8 as i32,
                    code.len(),
                    &mut branch_targets,
                    &mut block_starts,
                )?;
            }
            Opcode32::Jmp => {
                mark_relative_target(pc, instr.sj_arg(), code.len(), &mut branch_targets, &mut block_starts)?;
                mark_block_start(pc + 1, code.len(), &mut block_starts);
            }
            Opcode32::TryBegin => {
                mark_relative_target(
                    pc,
                    instr.sbx() as i32,
                    code.len(),
                    &mut branch_targets,
                    &mut block_starts,
                )?;
                mark_block_start(pc + 1, code.len(), &mut block_starts);
            }
            Opcode32::Return | Opcode32::Raise => {
                mark_block_start(pc + 1, code.len(), &mut block_starts);
            }
            _ => {}
        }
    }

    let mut block_ids = vec![0; code.len()];
    let mut current_block = 0_u32;
    for pc in 0..code.len() {
        if block_starts[pc] && pc != 0 {
            current_block = current_block
                .checked_add(1)
                .ok_or_else(|| anyhow!("Compiler32 control-flow block id overflow"))?;
        }
        block_ids[pc] = current_block;
    }

    Ok(PerfControlFlowFacts {
        block_ids,
        branch_targets,
        fused_bool_branches,
    })
}

fn fused_bool_branch_fact(code: &[Instr32], pc: usize, instr: Instr32) -> Option<PerfFusedBoolBranchFact> {
    if !opcode_writes_bool_result(instr.opcode()) {
        return None;
    }
    let test = code.get(pc + 1).copied()?;
    if test.opcode() != Opcode32::Test || test.a() != instr.a() || test.c() != 1 {
        return None;
    }
    let jmp = code.get(pc + 2).copied()?;
    if jmp.opcode() != Opcode32::Jmp {
        return None;
    }
    Some(PerfFusedBoolBranchFact {
        result_reg: instr.a(),
        jump_when: test.b() != 0,
        jump_offset: jmp.sj_arg(),
    })
}

fn opcode_writes_bool_result(opcode: Opcode32) -> bool {
    matches!(
        opcode,
        Opcode32::Not
            | Opcode32::IsNil
            | Opcode32::IsList
            | Opcode32::IsMap
            | Opcode32::CmpInt
            | Opcode32::CmpNeInt
            | Opcode32::CmpLtInt
            | Opcode32::CmpLeInt
            | Opcode32::CmpGtInt
            | Opcode32::CmpGeInt
            | Opcode32::Contains
    )
}

fn mark_relative_target(
    pc: usize,
    offset: i32,
    len: usize,
    branch_targets: &mut [bool],
    block_starts: &mut [bool],
) -> Result<()> {
    let target = pc as i64 + 1 + offset as i64;
    if target < 0 || target > len as i64 {
        bail!("Compiler32 branch target {target} out of bounds at pc {pc}");
    }
    mark_target(target as usize, len, branch_targets, block_starts)
}

fn mark_target(target: usize, len: usize, branch_targets: &mut [bool], block_starts: &mut [bool]) -> Result<()> {
    if target > len {
        bail!("Compiler32 branch target {target} out of bounds");
    }
    if target < len {
        branch_targets[target] = true;
        block_starts[target] = true;
    }
    Ok(())
}

fn mark_block_start(pc: usize, len: usize, block_starts: &mut [bool]) {
    if pc < len {
        block_starts[pc] = true;
    }
}
