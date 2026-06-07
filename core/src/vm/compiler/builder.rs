use anyhow::{Result, anyhow, bail};

use crate::vm::analysis::{
    PerfCompareTestBranchFact, PerfContainerFact, PerfControlFlowFacts, PerfFusedBoolBranchFact, PerfLocalCopyFact,
    PerfRegisterCopyFact, PerfRegisterFact, PerfValueFact, PerfValueKind, PerformanceFacts,
};

use super::{Compiler, ConstHeapValue, Function, Instr, Opcode, support::*};

impl Compiler {
    #[inline]
    pub(super) fn alloc_reg(&mut self) -> u16 {
        let reg = self.next_reg;
        self.next_reg = self.next_reg.checked_add(1).expect("Compiler register overflow");
        if self.next_reg > self.peak_reg {
            self.peak_reg = self.next_reg;
        }
        reg
    }

    pub(super) fn alloc_regs(&mut self, count: usize) -> Result<u16> {
        let count = u16::try_from(count).map_err(|_| anyhow!("Compiler register block too large: {count}"))?;
        let base = self.next_reg;
        self.next_reg = self
            .next_reg
            .checked_add(count)
            .ok_or_else(|| anyhow!("Compiler register overflow"))?;
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
            .max(self.loop_cached_literal_register_floor())
    }

    #[inline]
    pub(super) fn emit(&mut self, instr: Instr) {
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
        self.emit(Instr::abc(
            Opcode::Move,
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
        let name = name.into();
        self.single_char_string_locals.remove(&name);
        self.function.performance.mark_local_slot(reg);
        self.locals.insert(name, reg)
    }

    pub(super) fn local_slot_is_shared(&self, reg: u16) -> bool {
        let mut count = 0;
        for slot in self.locals.values().copied() {
            if slot == reg {
                count += 1;
                if count > 1 {
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn local_write_slot(&mut self, reg: u16) -> (u16, bool) {
        if !self.local_slot_is_shared(reg) && !self.is_loop_cached_literal_register(reg) {
            return (reg, false);
        }
        let new_reg = self.alloc_reg();
        self.function.performance.mark_local_slot(new_reg);
        (new_reg, true)
    }

    pub(super) fn mark_last_dead_write(&mut self) {
        if let Some(pc) = self.function.code.len().checked_sub(1) {
            self.function.performance.set_dead_write_fact(pc);
        }
    }

    pub(super) fn is_current_local_slot(&self, reg: u16) -> bool {
        self.locals.values().any(|slot| *slot == reg)
            || self.is_loop_cached_literal_register(reg)
            || self.single_char_string_locals.values().any(|slot| *slot == reg)
    }

    pub(super) fn emit_test_placeholder(&mut self, condition: u16) -> Result<usize> {
        let pc = self.function.code.len();
        self.emit(Instr::abc(Opcode::Test, checked_u8("test condition", condition)?, 1, 1));
        self.emit(Instr::sj(Opcode::Jmp, 0));
        Ok(pc)
    }

    pub(super) fn emit_jmp_placeholder(&mut self) -> usize {
        let pc = self.function.code.len();
        self.emit(Instr::sj(Opcode::Jmp, 0));
        pc
    }

    pub(super) fn emit_branch_placeholder(&mut self, opcode: Opcode, condition: u16) -> Result<usize> {
        let pc = self.function.code.len();
        self.emit(Instr::as_bx(opcode, checked_u8("branch condition", condition)?, 0));
        Ok(pc)
    }

    pub(super) fn emit_compare_test_placeholder(
        &mut self,
        opcode: Opcode,
        lhs: u16,
        rhs: u16,
        jump_when: bool,
    ) -> Result<usize> {
        let pc = self.function.code.len();
        self.emit(Instr::abc(
            opcode,
            checked_u8("compare lhs", lhs)?,
            checked_u8("compare rhs", rhs)?,
            u8::from(jump_when),
        ));
        self.emit(Instr::sj(Opcode::Jmp, 0));
        Ok(pc)
    }

    pub(super) fn emit_compare_test_immediate_placeholder(
        &mut self,
        opcode: Opcode,
        lhs: u16,
        rhs: i8,
        jump_when: bool,
    ) -> Result<usize> {
        let pc = self.function.code.len();
        self.emit(Instr::abc(
            opcode,
            checked_u8("compare lhs", lhs)?,
            u8::from(jump_when),
            rhs as u8,
        ));
        self.emit(Instr::sj(Opcode::Jmp, 0));
        Ok(pc)
    }

    pub(super) fn emit_raise(&mut self, message: &str) -> Result<()> {
        let const_index = self.push_string(message)?;
        self.emit(Instr::abx(Opcode::Raise, 0, const_index));
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
        let instr = *self
            .function
            .code
            .get(pc)
            .ok_or_else(|| anyhow!("Compiler test patch pc {pc} out of bounds"))?;
        if instr.opcode() != Opcode::Test {
            bail!("Compiler expected Test at patch pc {pc}");
        }
        let test_b: u8 = 1 - expected;
        self.function.code[pc] = Instr::abc(Opcode::Test, instr.a(), test_b, 1);
        self.patch_jmp(pc + 1, target)
    }

    pub(super) fn patch_jmp(&mut self, pc: usize, target: usize) -> Result<()> {
        let instr = *self
            .function
            .code
            .get(pc)
            .ok_or_else(|| anyhow!("Compiler jump patch pc {pc} out of bounds"))?;
        if instr.opcode() != Opcode::Jmp {
            bail!("Compiler expected Jmp at patch pc {pc}");
        }
        self.function.code[pc] = Instr::sj(Opcode::Jmp, jump_offset(pc, target)?);
        Ok(())
    }

    pub(super) fn patch_branch(&mut self, pc: usize, target: usize) -> Result<()> {
        let instr = *self
            .function
            .code
            .get(pc)
            .ok_or_else(|| anyhow!("Compiler branch patch pc {pc} out of bounds"))?;
        if !matches!(
            instr.opcode(),
            Opcode::BrFalse | Opcode::BrTrue | Opcode::BrNil | Opcode::BrNotNil
        ) {
            bail!("Compiler expected branch at patch pc {pc}");
        }
        self.function.code[pc] = Instr::as_bx(instr.opcode(), instr.a(), jump_offset(pc, target)? as i16);
        Ok(())
    }

    pub(super) fn patch_compare_test_jump(&mut self, pc: usize, target: usize) -> Result<()> {
        let instr = *self
            .function
            .code
            .get(pc)
            .ok_or_else(|| anyhow!("Compiler compare-test patch pc {pc} out of bounds"))?;
        if !instr.opcode().is_compare_test() {
            bail!("Compiler expected compare-test at patch pc {pc}");
        }
        self.patch_jmp(pc + 1, target)
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

    pub(super) fn push_heap_value(&mut self, value: ConstHeapValue) -> Result<u16> {
        self.function.consts.push_heap_value(value)
    }

    pub(super) fn emit_return(&mut self, base: u16) -> Result<()> {
        self.emit(Instr::abc(Opcode::Return1, checked_u8("return base", base)?, 0, 0));
        self.emitted_return = true;
        Ok(())
    }

    pub(super) fn emit_empty_return(&mut self) {
        self.emit(Instr::abc(Opcode::Return0, 0, 0, 0));
        self.emitted_return = true;
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

    pub(super) fn finish(&mut self) -> Result<Function> {
        self.function.register_count = self.peak_reg;
        let control_flow = build_control_flow_facts(&self.function.code, &self.function.performance)?;
        self.function.performance.set_control_flow_facts(control_flow);
        Ok(std::mem::take(&mut self.function))
    }
}

fn build_control_flow_facts(code: &[Instr], performance: &PerformanceFacts) -> Result<PerfControlFlowFacts> {
    if code.is_empty() {
        return Ok(PerfControlFlowFacts::default());
    }

    let mut branch_targets = vec![false; code.len()];
    let mut block_starts = vec![false; code.len()];
    let mut fused_bool_branches = vec![None; code.len()];
    let mut compare_test_branches = vec![None; code.len()];
    block_starts[0] = true;

    for (pc, instr) in code.iter().copied().enumerate() {
        if let Some(fact) = fused_bool_branch_fact(code, pc, instr) {
            fused_bool_branches[pc] = Some(fact);
        }
        match instr.opcode() {
            Opcode::Test => {
                mark_target(pc + 1, code.len(), &mut branch_targets, &mut block_starts)?;
                mark_relative_target(
                    pc,
                    instr.c() as i8 as i32,
                    code.len(),
                    &mut branch_targets,
                    &mut block_starts,
                )?;
            }
            Opcode::BrFalse | Opcode::BrTrue | Opcode::BrNil | Opcode::BrNotNil => {
                mark_relative_target(
                    pc,
                    instr.sbx() as i32,
                    code.len(),
                    &mut branch_targets,
                    &mut block_starts,
                )?;
                mark_block_start(pc + 1, code.len(), &mut block_starts);
            }
            opcode if opcode.is_compare_test() => {
                let jmp = code
                    .get(pc + 1)
                    .copied()
                    .ok_or_else(|| anyhow!("Compiler compare-test missing Jmp at pc {pc}"))?;
                if jmp.opcode() != Opcode::Jmp {
                    bail!("Compiler compare-test expected Jmp at pc {}", pc + 1);
                }
                let target_pc = relative_target(pc + 1, jmp.sj_arg(), code.len())?;
                compare_test_branches[pc] = Some(PerfCompareTestBranchFact { target_pc });
                mark_target(target_pc, code.len(), &mut branch_targets, &mut block_starts)?;
                mark_block_start(pc + 2, code.len(), &mut block_starts);
            }
            Opcode::Jmp => {
                mark_relative_target(pc, instr.sj_arg(), code.len(), &mut branch_targets, &mut block_starts)?;
                mark_block_start(pc + 1, code.len(), &mut block_starts);
            }
            Opcode::ForLoopI => {
                let fact = performance
                    .for_loop(pc)
                    .ok_or_else(|| anyhow!("Compiler ForLoopI missing performance fact at pc {pc}"))?;
                mark_relative_target(pc, fact.jump_offset, code.len(), &mut branch_targets, &mut block_starts)?;
                mark_block_start(pc + 1, code.len(), &mut block_starts);
            }
            Opcode::TryBegin => {
                mark_relative_target(
                    pc,
                    instr.sbx() as i32,
                    code.len(),
                    &mut branch_targets,
                    &mut block_starts,
                )?;
                mark_block_start(pc + 1, code.len(), &mut block_starts);
            }
            opcode if opcode.is_return() || opcode == Opcode::Raise => {
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
                .ok_or_else(|| anyhow!("Compiler control-flow block id overflow"))?;
        }
        block_ids[pc] = current_block;
    }

    Ok(PerfControlFlowFacts {
        block_ids,
        branch_targets,
        fused_bool_branches,
        compare_test_branches,
    })
}

fn fused_bool_branch_fact(code: &[Instr], pc: usize, instr: Instr) -> Option<PerfFusedBoolBranchFact> {
    if !opcode_writes_bool_result(instr.opcode()) {
        return None;
    }
    let branch = code.get(pc + 1).copied()?;
    if branch.a() != instr.a() {
        return None;
    }
    if branch.opcode() == Opcode::BrFalse || branch.opcode() == Opcode::BrTrue {
        return Some(PerfFusedBoolBranchFact {
            result_reg: instr.a(),
            jump_when: branch.opcode() == Opcode::BrTrue,
            jump_offset: branch.sbx() as i32,
            jump_base_pc_delta: 1,
            fallthrough_pc_delta: 2,
        });
    }
    if branch.opcode() != Opcode::Test || branch.c() != 1 {
        return None;
    }
    let jmp = code.get(pc + 2).copied()?;
    (jmp.opcode() == Opcode::Jmp).then_some(PerfFusedBoolBranchFact {
        result_reg: instr.a(),
        jump_when: branch.b() != 0,
        jump_offset: jmp.sj_arg(),
        jump_base_pc_delta: 2,
        fallthrough_pc_delta: 3,
    })
}

fn opcode_writes_bool_result(opcode: Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Not
            | Opcode::IsNil
            | Opcode::IsList
            | Opcode::IsMap
            | Opcode::CmpInt
            | Opcode::CmpNeInt
            | Opcode::CmpLtInt
            | Opcode::CmpLeInt
            | Opcode::CmpGtInt
            | Opcode::CmpGeInt
            | Opcode::Contains
    )
}

fn mark_relative_target(
    pc: usize,
    offset: i32,
    len: usize,
    branch_targets: &mut [bool],
    block_starts: &mut [bool],
) -> Result<()> {
    let target = relative_target(pc, offset, len)?;
    mark_target(target, len, branch_targets, block_starts)
}

fn relative_target(pc: usize, offset: i32, len: usize) -> Result<usize> {
    let target = pc as i64 + 1 + offset as i64;
    if target < 0 || target > len as i64 {
        bail!("Compiler branch target {target} out of bounds at pc {pc}");
    }
    Ok(target as usize)
}

fn mark_target(target: usize, len: usize, branch_targets: &mut [bool], block_starts: &mut [bool]) -> Result<()> {
    if target > len {
        bail!("Compiler branch target {target} out of bounds");
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
