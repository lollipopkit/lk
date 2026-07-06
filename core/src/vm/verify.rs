//! Load-time bytecode verifier.
//!
//! `.lkm` artifacts are untrusted external input. `Instr::try_from_raw` already
//! rejects invalid opcode bits, but the executor's hot paths use release-mode
//! unchecked register indexing (`stack_index_unchecked`) and unchecked relative
//! jumps (`relative_pc_unchecked`), so a corrupted artifact could silently read
//! or write across frames or jump out of a function without any error. This
//! module proves those invariants once at load time so the unchecked paths are
//! sound by construction for verified modules:
//!
//! - every register operand is `< register_count`
//! - every register window (calls, container builds, returns, `ConcatN`) fits
//! - every jump/branch target lands in `0..=code.len()`
//! - every const-pool / function / native / global / capture index is in bounds
//! - deserialized `PerformanceFacts` (also untrusted) only name in-bounds
//!   registers, targets, and const indices, and opcodes with no fact-less
//!   fallback (`ForLoopI`, `GetIndexStrI`, `SetIndexStrI`) have their fact
//!
//! The compiler must produce verifiable output: `Compiler::compile_module`
//! re-runs this verifier under `debug_assertions`, so the whole test suite
//! guards against both compiler regressions and verifier false rejections.

use anyhow::{Result, bail};

use super::{Function, Module, Opcode};

pub fn verify_module(module: &Module) -> Result<()> {
    if module.functions.get(module.entry as usize).is_none() {
        bail!(
            "bytecode verifier: entry {} out of bounds for {} functions",
            module.entry,
            module.functions.len()
        );
    }
    for (index, function) in module.functions.iter().enumerate() {
        verify_function(function, index, module)?;
    }
    Ok(())
}

struct FunctionVerifier<'a> {
    function: &'a Function,
    function_index: usize,
    module: &'a Module,
    code_len: usize,
}

fn verify_function(function: &Function, function_index: usize, module: &Module) -> Result<()> {
    let verifier = FunctionVerifier {
        function,
        function_index,
        module,
        code_len: function.code.len(),
    };
    verifier.verify_instructions()?;
    verifier.verify_facts()
}

impl FunctionVerifier<'_> {
    fn fail(&self, pc: usize, message: impl core::fmt::Display) -> anyhow::Error {
        anyhow::anyhow!("bytecode verifier: fn {} pc {pc}: {message}", self.function_index)
    }

    fn check_reg(&self, pc: usize, role: &str, reg: u8) -> Result<()> {
        if u16::from(reg) >= self.function.register_count {
            return Err(self.fail(
                pc,
                format_args!(
                    "{role} register {reg} out of bounds (register_count {})",
                    self.function.register_count
                ),
            ));
        }
        Ok(())
    }

    /// Checks that the register window `base..base + span` fits the frame.
    /// An empty window still requires `base` itself to be addressable so the
    /// executor's base arithmetic cannot leave the frame.
    fn check_window(&self, pc: usize, role: &str, base: usize, span: usize) -> Result<()> {
        let regs = self.function.register_count as usize;
        // Operand register arithmetic in the executor is 8-bit (`u8`
        // `wrapping_add`), so a window reaching past register 255 would wrap
        // to the bottom of the frame even when a (malicious) `register_count`
        // claims room for it — cap the window to the encodable space too.
        const REGISTER_SPACE: usize = u8::MAX as usize + 1;
        if base >= regs || base + span > regs || base + span > REGISTER_SPACE {
            return Err(self.fail(
                pc,
                format_args!(
                    "{role} register window {base}..{} out of bounds (register_count {regs})",
                    base + span
                ),
            ));
        }
        Ok(())
    }

    /// Branch semantics are `pc + 1 + offset`; landing exactly on `code.len()`
    /// is the executor's legal "fell off the end" return, so targets are
    /// accepted in `0..=code.len()`.
    fn check_target(&self, pc: usize, role: &str, base_pc: usize, offset: i64) -> Result<()> {
        let target = base_pc as i64 + 1 + offset;
        if target < 0 || target > self.code_len as i64 {
            return Err(self.fail(
                pc,
                format_args!("{role} jump target {target} out of bounds (code len {})", self.code_len),
            ));
        }
        Ok(())
    }

    fn check_absolute_target(&self, pc: usize, role: &str, target: usize) -> Result<()> {
        if target > self.code_len {
            return Err(self.fail(
                pc,
                format_args!("{role} target pc {target} out of bounds (code len {})", self.code_len),
            ));
        }
        Ok(())
    }

    fn check_const(&self, pc: usize, role: &str, index: usize, pool_len: usize) -> Result<()> {
        if index >= pool_len {
            return Err(self.fail(
                pc,
                format_args!("{role} const index {index} out of bounds (pool len {pool_len})"),
            ));
        }
        Ok(())
    }

    /// Compare-test opcodes either follow a compiler control-flow fact or fall
    /// back to decoding `code[pc + 1]` as a `Jmp`. Without either, the executor
    /// would misinterpret an arbitrary instruction word as a jump offset.
    fn check_compare_test_shape(&self, pc: usize) -> Result<()> {
        if self.function.performance.compare_test_branch(pc).is_some() {
            return Ok(());
        }
        match self.function.code.get(pc + 1) {
            Some(next) if next.opcode() == Opcode::Jmp => Ok(()),
            Some(next) => Err(self.fail(
                pc,
                format_args!(
                    "compare-test without branch fact must be followed by Jmp, found {:?}",
                    next.opcode()
                ),
            )),
            None => Err(self.fail(pc, "compare-test without branch fact at end of function")),
        }
    }

    fn verify_instructions(&self) -> Result<()> {
        let consts = &self.function.consts;
        for (pc, instr) in self.function.code.iter().enumerate() {
            let opcode = instr.opcode();
            match opcode {
                Opcode::Nop | Opcode::Return0 | Opcode::TryEnd | Opcode::Wide => {}
                Opcode::Move | Opcode::LoadCellVal | Opcode::StoreCellVal => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                }
                Opcode::Move2 => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                    self.check_reg(pc, "c", instr.c())?;
                }
                Opcode::Return => {
                    self.check_window(pc, "return", instr.a() as usize, instr.b() as usize)?;
                }
                Opcode::Return1 => {
                    self.check_reg(pc, "a", instr.a())?;
                }
                Opcode::LoadNil | Opcode::LoadBool => {
                    self.check_reg(pc, "a", instr.a())?;
                }
                Opcode::LoadInt => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_const(pc, "LoadInt", instr.bx() as usize, consts.ints.len())?;
                }
                Opcode::LoadFloat => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_const(pc, "LoadFloat", instr.bx() as usize, consts.floats.len())?;
                }
                Opcode::LoadString => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_const(pc, "LoadString", instr.bx() as usize, consts.strings.len())?;
                }
                Opcode::LoadHeapConst => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_const(pc, "LoadHeapConst", instr.bx() as usize, consts.heap_values.len())?;
                }
                // Plain three-register ops: A dst/target, B and C sources.
                Opcode::AddInt
                | Opcode::SubInt
                | Opcode::MulInt
                | Opcode::DivInt
                | Opcode::ModInt
                | Opcode::AddMulInt
                | Opcode::Add2Int
                | Opcode::AddListInt
                | Opcode::SubListInt
                | Opcode::MinInt
                | Opcode::MaxInt
                | Opcode::MidInt
                | Opcode::AddFloat
                | Opcode::SubFloat
                | Opcode::MulFloat
                | Opcode::DivFloat
                | Opcode::ModFloat
                | Opcode::CmpInt
                | Opcode::CmpNeInt
                | Opcode::CmpLtInt
                | Opcode::CmpLeInt
                | Opcode::CmpGtInt
                | Opcode::CmpGeInt
                | Opcode::GetIndex
                | Opcode::SetIndex
                | Opcode::GetList
                | Opcode::Contains
                | Opcode::SliceFrom
                | Opcode::MapRest
                | Opcode::ConcatString
                | Opcode::StringSplit
                | Opcode::ListJoin => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                    self.check_reg(pc, "c", instr.c())?;
                }
                Opcode::AddIntI | Opcode::MulIntI => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                }
                Opcode::ModIntI => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                    if instr.sc() == 0 {
                        return Err(self.fail(pc, "ModIntI divisor immediate is zero"));
                    }
                }
                Opcode::GetIndexStrI | Opcode::SetIndexStrI => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                    self.check_reg(pc, "c", instr.c())?;
                    // No fact-less fallback: the executor bails at runtime, and
                    // the prefix const feeds map-key construction.
                    let Some(key_fact) = self.function.performance.known_key(pc).and_then(|fact| fact.string_int)
                    else {
                        return Err(self.fail(pc, format_args!("{opcode:?} requires a string-int key fact")));
                    };
                    self.check_const(
                        pc,
                        "string-int key prefix",
                        key_fact.prefix_key as usize,
                        consts.strings.len(),
                    )?;
                }
                Opcode::GetFieldK | Opcode::SetFieldK => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                    self.check_const(pc, "field key", instr.c() as usize, consts.strings.len())?;
                }
                Opcode::ListPush => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                }
                Opcode::Len
                | Opcode::ToIter
                | Opcode::ToString
                | Opcode::Not
                | Opcode::IsNil
                | Opcode::IsList
                | Opcode::IsMap => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                }
                Opcode::ConcatN => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_window(pc, "ConcatN", instr.b() as usize, instr.c() as usize)?;
                }
                // Compare-tests: A (and B for register forms) are sources; the
                // branch shape is validated separately.
                Opcode::TestEqInt
                | Opcode::TestNeInt
                | Opcode::TestLtInt
                | Opcode::TestLeInt
                | Opcode::TestGtInt
                | Opcode::TestGeInt
                | Opcode::TestEqIntI2 => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_reg(pc, "b", instr.b())?;
                    self.check_compare_test_shape(pc)?;
                }
                Opcode::TestEqIntI
                | Opcode::TestNeIntI
                | Opcode::TestLtIntI
                | Opcode::TestLeIntI
                | Opcode::TestGtIntI
                | Opcode::TestGeIntI => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_compare_test_shape(pc)?;
                }
                Opcode::Test => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_target(pc, "Test", pc, i64::from(instr.sc()))?;
                }
                Opcode::Jmp => {
                    self.check_target(pc, "Jmp", pc, i64::from(instr.sj_arg()))?;
                }
                Opcode::BrFalse
                | Opcode::BrTrue
                | Opcode::BrNil
                | Opcode::BrNotNil
                | Opcode::BrEqZeroInt
                | Opcode::BrNeZeroInt => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_target(pc, "branch", pc, i64::from(instr.sbx()))?;
                }
                Opcode::BrEqIntI4 | Opcode::BrNeIntI4 => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_target(pc, "branch", pc, i64::from(instr.branch_i4_offset()))?;
                }
                Opcode::BrModEqZeroIntI4 | Opcode::BrModNeZeroIntI4 => {
                    self.check_reg(pc, "a", instr.a())?;
                    if instr.branch_i4_immediate() == 0 {
                        return Err(self.fail(pc, format_args!("{opcode:?} divisor immediate is zero")));
                    }
                    self.check_target(pc, "branch", pc, i64::from(instr.branch_i4_offset()))?;
                }
                Opcode::ForLoopI => {
                    self.check_reg(pc, "index", instr.a())?;
                    self.check_reg(pc, "end", instr.b())?;
                    self.check_reg(pc, "step", instr.c())?;
                    let Some(fact) = self.function.performance.for_loop(pc) else {
                        return Err(self.fail(pc, "ForLoopI requires a for-loop fact"));
                    };
                    self.check_target(pc, "ForLoopI", pc, i64::from(fact.jump_offset))?;
                }
                Opcode::Call => {
                    // A = call window base (callee slot), args at A+1..A+1+C.
                    self.check_window(pc, "call", instr.a() as usize, 1 + instr.c() as usize)?;
                }
                Opcode::CallMethodK => {
                    // A = window base (receiver slot, args at A+1..A+1+C), B =
                    // method-name string constant index.
                    self.check_window(pc, "method call", instr.a() as usize, 1 + instr.c() as usize)?;
                    let name = instr.b() as usize;
                    if name >= self.function.consts.strings.len() {
                        return Err(self.fail(
                            pc,
                            format_args!(
                                "CallMethodK method-name const {name} out of bounds ({} strings)",
                                self.function.consts.strings.len()
                            ),
                        ));
                    }
                }
                Opcode::CallDirect => {
                    self.check_window(pc, "call", instr.a() as usize, 1 + instr.c() as usize)?;
                    let callee = instr.b() as usize;
                    if callee >= self.module.functions.len() {
                        return Err(self.fail(
                            pc,
                            format_args!(
                                "CallDirect function index {callee} out of bounds ({} functions)",
                                self.module.functions.len()
                            ),
                        ));
                    }
                }
                Opcode::CallNamed => {
                    let payload = instr.bx();
                    let positional = (payload & 0x7f) as usize;
                    let named = (payload >> 7) as usize;
                    self.check_window(pc, "named call", instr.a() as usize, 1 + positional + named * 2)?;
                }
                Opcode::LoadFunction => {
                    self.check_reg(pc, "a", instr.a())?;
                    let index = instr.bx() as usize;
                    if index >= self.module.functions.len() {
                        return Err(self.fail(
                            pc,
                            format_args!(
                                "LoadFunction index {index} out of bounds ({} functions)",
                                self.module.functions.len()
                            ),
                        ));
                    }
                }
                Opcode::LoadNative => {
                    self.check_reg(pc, "a", instr.a())?;
                    let index = instr.bx() as usize;
                    if index >= self.module.natives.len() {
                        return Err(self.fail(
                            pc,
                            format_args!(
                                "LoadNative index {index} out of bounds ({} natives)",
                                self.module.natives.len()
                            ),
                        ));
                    }
                }
                Opcode::MakeClosure => {
                    self.check_reg(pc, "a", instr.a())?;
                    let callee_index = instr.b() as usize;
                    let Some(callee) = self.module.functions.get(callee_index) else {
                        return Err(self.fail(
                            pc,
                            format_args!(
                                "MakeClosure function index {callee_index} out of bounds ({} functions)",
                                self.module.functions.len()
                            ),
                        ));
                    };
                    self.check_window(
                        pc,
                        "closure captures",
                        instr.c() as usize,
                        callee.capture_count as usize,
                    )?;
                }
                Opcode::LoadCapture => {
                    self.check_reg(pc, "a", instr.a())?;
                    let index = instr.bx();
                    if index >= self.function.capture_count {
                        return Err(self.fail(
                            pc,
                            format_args!(
                                "LoadCapture index {index} out of bounds (capture_count {})",
                                self.function.capture_count
                            ),
                        ));
                    }
                }
                Opcode::GetGlobal | Opcode::SetGlobal => {
                    self.check_reg(pc, "a", instr.a())?;
                    let slot = instr.bx() as usize;
                    if slot >= self.module.globals.len() {
                        return Err(self.fail(
                            pc,
                            format_args!(
                                "{opcode:?} slot {slot} out of bounds ({} globals)",
                                self.module.globals.len()
                            ),
                        ));
                    }
                }
                Opcode::NewList => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_window(pc, "NewList", instr.b() as usize, instr.c() as usize)?;
                }
                Opcode::NewMap => {
                    self.check_reg(pc, "a", instr.a())?;
                    self.check_window(pc, "NewMap", instr.b() as usize, instr.c() as usize * 2)?;
                }
                Opcode::NewRange => {
                    self.check_reg(pc, "a", instr.a())?;
                    // start / end / step occupy three consecutive registers.
                    self.check_window(pc, "NewRange", instr.b() as usize, 3)?;
                }
                Opcode::NewObject => {
                    self.check_reg(pc, "a", instr.a())?;
                    // B = type-name register, followed by C key/value pairs.
                    self.check_window(pc, "NewObject", instr.b() as usize, 1 + instr.c() as usize * 2)?;
                }
                Opcode::Raise => {
                    self.check_const(pc, "Raise", instr.bx() as usize, consts.strings.len())?;
                }
                Opcode::TryBegin => {
                    self.check_reg(pc, "catch", instr.a())?;
                    self.check_target(pc, "TryBegin", pc, i64::from(instr.sbx()))?;
                }
            }
        }
        Ok(())
    }

    /// Deserialized facts are untrusted input. Facts that name registers, jump
    /// targets, or const indices must be in bounds; stray facts beyond the code
    /// or attached to the wrong opcode are rejected.
    fn verify_facts(&self) -> Result<()> {
        let facts = &self.function.performance;
        let regs = self.function.register_count;

        for (pc, fact) in facts.for_loops.iter().enumerate() {
            let Some(fact) = fact else { continue };
            if pc >= self.code_len || self.function.code[pc].opcode() != Opcode::ForLoopI {
                return Err(self.fail(pc, "for-loop fact not attached to a ForLoopI instruction"));
            }
            self.check_target(pc, "for-loop fact", pc, i64::from(fact.jump_offset))?;
        }

        for (pc, fact) in facts.control_flow.compare_test_branches.iter().enumerate() {
            let Some(fact) = fact else { continue };
            if pc >= self.code_len || !self.function.code[pc].opcode().is_compare_test() {
                return Err(self.fail(pc, "compare-test branch fact not attached to a compare-test"));
            }
            self.check_absolute_target(pc, "compare-test fact", fact.target_pc)?;
        }

        for (pc, fact) in facts.control_flow.fused_bool_branches.iter().enumerate() {
            let Some(fact) = fact else { continue };
            if pc >= self.code_len {
                return Err(self.fail(pc, "fused bool branch fact beyond end of code"));
            }
            if u16::from(fact.result_reg) >= regs {
                return Err(self.fail(
                    pc,
                    format_args!("fused bool branch result register {} out of bounds", fact.result_reg),
                ));
            }
            let Some(jump_base) = pc.checked_add(fact.jump_base_pc_delta) else {
                return Err(self.fail(pc, "fused bool branch jump base overflows"));
            };
            self.check_absolute_target(pc, "fused bool branch jump base", jump_base)?;
            self.check_target(pc, "fused bool branch", jump_base, i64::from(fact.jump_offset))?;
            let Some(fallthrough) = pc.checked_add(fact.fallthrough_pc_delta) else {
                return Err(self.fail(pc, "fused bool branch fallthrough overflows"));
            };
            self.check_absolute_target(pc, "fused bool branch fallthrough", fallthrough)?;
        }

        for (pc, fact) in facts.call_sites.iter().enumerate() {
            let Some(fact) = fact else { continue };
            if pc >= self.code_len {
                return Err(self.fail(pc, "call-site fact beyond end of code"));
            }
            let span = 1 + fact.positional_count as usize + fact.named_count as usize * 2;
            self.check_window(pc, "call-site fact", fact.call_base as usize, span)?;
            // The executor lets an in-range fact *override* the instruction's
            // own operands (`call_fact_from_static_cache_or_instr`), so a
            // tampered fact must also agree with the instruction it annotates.
            let instr = self.function.code[pc];
            match instr.opcode() {
                Opcode::Call | Opcode::CallDirect => {
                    if fact.call_base != u16::from(instr.a())
                        || fact.positional_count != u16::from(instr.c())
                        || fact.named_count != 0
                    {
                        return Err(self.fail(pc, "call-site fact disagrees with the call instruction"));
                    }
                }
                Opcode::CallNamed => {
                    let payload = instr.bx();
                    if fact.call_base != u16::from(instr.a())
                        || fact.positional_count != (payload & 0x7f)
                        || fact.named_count != (payload >> 7)
                    {
                        return Err(self.fail(pc, "call-site fact disagrees with the named-call instruction"));
                    }
                }
                other => {
                    return Err(self.fail(
                        pc,
                        format_args!("call-site fact attached to non-call instruction {other:?}"),
                    ));
                }
            }
        }

        for (pc, fact) in facts.global_ops.iter().enumerate() {
            let Some(fact) = fact else { continue };
            if pc >= self.code_len {
                return Err(self.fail(pc, "global fact beyond end of code"));
            }
            if fact.slot as usize >= self.module.globals.len() {
                return Err(self.fail(
                    pc,
                    format_args!(
                        "global fact slot {} out of bounds ({} globals)",
                        fact.slot,
                        self.module.globals.len()
                    ),
                ));
            }
            // Same override rule as call facts: the slot must match the
            // instruction's own operand.
            let instr = self.function.code[pc];
            match instr.opcode() {
                Opcode::GetGlobal | Opcode::SetGlobal => {
                    if fact.slot != instr.bx() {
                        return Err(self.fail(pc, "global fact disagrees with the instruction's slot"));
                    }
                }
                other => {
                    return Err(self.fail(
                        pc,
                        format_args!("global fact attached to non-global instruction {other:?}"),
                    ));
                }
            }
        }

        for (pc, fact) in facts.key_ops.iter().enumerate() {
            let Some(fact) = fact else { continue };
            if pc >= self.code_len {
                return Err(self.fail(pc, "key fact beyond end of code"));
            }
            if let Some(const_key) = fact.const_key {
                self.check_const(pc, "key fact", const_key as usize, self.function.consts.strings.len())?;
            }
            if let Some(string_int) = fact.string_int {
                self.check_const(
                    pc,
                    "string-int key fact prefix",
                    string_int.prefix_key as usize,
                    self.function.consts.strings.len(),
                )?;
                if string_int.suffix_reg >= regs {
                    return Err(self.fail(
                        pc,
                        format_args!(
                            "string-int key fact suffix register {} out of bounds",
                            string_int.suffix_reg
                        ),
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ConstPool, Function, Instr, Module, Opcode};
    use super::*;
    use crate::vm::analysis::{PerfForLoopFact, PerformanceFacts};

    fn module_with_code(register_count: u16, code: Vec<Instr>) -> Module {
        Module::single(Function {
            code,
            register_count,
            ..Function::default()
        })
    }

    #[test]
    fn accepts_simple_valid_function() {
        let mut consts = ConstPool::default();
        let index = consts.push_int(7).expect("const");
        let module = Module::single(Function {
            consts,
            code: vec![
                Instr::abx(Opcode::LoadInt, 0, index),
                Instr::abc(Opcode::Move, 1, 0, 0),
                Instr::abc(Opcode::Return1, 1, 0, 0),
            ],
            register_count: 2,
            ..Function::default()
        });

        verify_module(&module).expect("valid module verifies");
    }

    #[test]
    fn rejects_out_of_bounds_register() {
        let module = module_with_code(
            2,
            vec![
                Instr::abc(Opcode::Move, 200, 0, 0),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
        );

        let err = verify_module(&module).expect_err("register 200 with 2 registers must be rejected");
        assert!(err.to_string().contains("register 200 out of bounds"), "{err}");
    }

    #[test]
    fn rejects_out_of_bounds_jump_target() {
        let module = module_with_code(
            1,
            vec![Instr::sj(Opcode::Jmp, 100), Instr::abc(Opcode::Return0, 0, 0, 0)],
        );

        let err = verify_module(&module).expect_err("jump target beyond code must be rejected");
        assert!(err.to_string().contains("jump target"), "{err}");
    }

    #[test]
    fn rejects_backward_jump_before_start() {
        let module = module_with_code(
            1,
            vec![Instr::sj(Opcode::Jmp, -5), Instr::abc(Opcode::Return0, 0, 0, 0)],
        );

        let err = verify_module(&module).expect_err("jump before function start must be rejected");
        assert!(err.to_string().contains("jump target"), "{err}");
    }

    #[test]
    fn rejects_out_of_bounds_const_index() {
        let module = module_with_code(
            1,
            vec![Instr::abx(Opcode::LoadInt, 0, 3), Instr::abc(Opcode::Return0, 0, 0, 0)],
        );

        let err = verify_module(&module).expect_err("const index without pool entry must be rejected");
        assert!(err.to_string().contains("const index 3 out of bounds"), "{err}");
    }

    #[test]
    fn rejects_for_loop_without_fact() {
        let module = module_with_code(
            3,
            vec![
                Instr::abc(Opcode::ForLoopI, 0, 1, 2),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
        );

        let err = verify_module(&module).expect_err("ForLoopI without fact must be rejected");
        assert!(err.to_string().contains("requires a for-loop fact"), "{err}");
    }

    #[test]
    fn accepts_for_loop_with_valid_fact() {
        let mut performance = PerformanceFacts::default();
        performance.for_loops = vec![Some(PerfForLoopFact {
            jump_offset: -1,
            inclusive: false,
            positive_step: true,
        })];
        let module = Module::single(Function {
            code: vec![
                Instr::abc(Opcode::ForLoopI, 0, 1, 2),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
            performance,
            register_count: 3,
            ..Function::default()
        });

        verify_module(&module).expect("ForLoopI with fact verifies");
    }

    #[test]
    fn rejects_for_loop_fact_with_out_of_bounds_target() {
        let mut performance = PerformanceFacts::default();
        performance.for_loops = vec![Some(PerfForLoopFact {
            jump_offset: 100,
            inclusive: false,
            positive_step: true,
        })];
        let module = Module::single(Function {
            code: vec![
                Instr::abc(Opcode::ForLoopI, 0, 1, 2),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
            performance,
            register_count: 3,
            ..Function::default()
        });

        let err = verify_module(&module).expect_err("for-loop fact target beyond code must be rejected");
        assert!(err.to_string().contains("jump target"), "{err}");
    }

    #[test]
    fn rejects_compare_test_without_fact_or_jmp() {
        let module = module_with_code(
            2,
            vec![
                Instr::abc(Opcode::TestLtInt, 0, 1, 1),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
        );

        let err = verify_module(&module).expect_err("compare-test followed by non-Jmp must be rejected");
        assert!(err.to_string().contains("must be followed by Jmp"), "{err}");
    }

    #[test]
    fn accepts_compare_test_followed_by_jmp() {
        let module = module_with_code(
            2,
            vec![
                Instr::abc(Opcode::TestLtInt, 0, 1, 1),
                Instr::sj(Opcode::Jmp, 0),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
        );

        verify_module(&module).expect("compare-test with following Jmp verifies");
    }

    #[test]
    fn rejects_call_window_exceeding_frame() {
        let module = module_with_code(
            4,
            vec![Instr::abc(Opcode::Call, 2, 0, 5), Instr::abc(Opcode::Return0, 0, 0, 0)],
        );

        let err = verify_module(&module).expect_err("call args beyond frame must be rejected");
        assert!(err.to_string().contains("register window"), "{err}");
    }

    #[test]
    fn rejects_call_direct_with_bad_function_index() {
        let module = module_with_code(
            4,
            vec![
                Instr::abc(Opcode::CallDirect, 0, 9, 1),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
        );

        let err = verify_module(&module).expect_err("CallDirect to missing function must be rejected");
        assert!(err.to_string().contains("function index 9 out of bounds"), "{err}");
    }

    #[test]
    fn rejects_global_slot_out_of_bounds() {
        let module = module_with_code(
            1,
            vec![
                Instr::abx(Opcode::GetGlobal, 0, 4),
                Instr::abc(Opcode::Return0, 0, 0, 0),
            ],
        );

        let err = verify_module(&module).expect_err("global slot without entry must be rejected");
        assert!(err.to_string().contains("slot 4 out of bounds"), "{err}");
    }
}
