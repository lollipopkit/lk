use core::ops::Range;

use anyhow::{Result, bail};

use crate::{
    val::{RuntimeVal, TypedList},
    vm::CallWindow,
};

use crate::vm::analysis::record_register_write_known_enabled;

use super::{Executor, ReturnValues};

impl Executor {
    #[inline(always)]
    pub(super) fn read_unchecked(&self, register: u8) -> &RuntimeVal {
        let index = self.stack_index_unchecked(register);
        &self.state.stack[index]
    }

    #[inline]
    pub(super) fn read(&self, register: u8) -> Result<&RuntimeVal> {
        let index = self.stack_index(register)?;
        Ok(&self.state.stack[index])
    }

    #[inline(always)]
    pub(super) fn write_unchecked(&mut self, register: u8, value: RuntimeVal) {
        let index = self.stack_index_unchecked(register);
        self.state.stack[index] = value;
    }

    #[inline]
    pub(super) fn write(&mut self, register: u8, value: RuntimeVal) -> Result<()> {
        let index = self.stack_index(register)?;
        self.write_stack_index(index, value);
        Ok(())
    }

    #[inline]
    pub(super) fn write_stack_index(&mut self, index: usize, value: RuntimeVal) {
        self.state.stack[index] = value;
        if self.collect_metrics {
            record_register_write_known_enabled();
        }
    }

    #[inline]
    pub(super) fn take(&mut self, register: u8) -> Result<RuntimeVal> {
        let index = self.stack_index(register)?;
        Ok(core::mem::take(&mut self.state.stack[index]))
    }

    /// Unchecked version of `stack_index` — elides bounds check in release builds.
    /// In debug builds the assertion still fires.
    #[inline(always)]
    pub(super) fn stack_index_unchecked(&self, register: u8) -> usize {
        debug_assert!(
            (register as u16) < self.register_count,
            "register {} out of bounds",
            register
        );
        self.frame_base + register as usize
    }

    #[inline]
    pub(super) fn stack_index(&self, register: u8) -> Result<usize> {
        if register as u16 >= self.register_count {
            bail!("register {} out of bounds", register);
        }
        Ok(self.frame_base + register as usize)
    }

    #[inline(always)]
    pub(super) fn stack_abc_unchecked(&self, instr: crate::vm::Instr) -> (usize, usize, usize) {
        let a = instr.a();
        let b = instr.b();
        let c = instr.c();
        let base = self.frame_base;
        (base + a as usize, base + b as usize, base + c as usize)
    }

    #[inline]
    pub(super) fn stack_abc_indices(&self, instr: crate::vm::Instr) -> Result<(usize, usize, usize)> {
        let a = instr.a();
        let b = instr.b();
        let c = instr.c();
        let max = a.max(b).max(c);
        if max as u16 >= self.register_count {
            bail!("register {} out of bounds", max);
        }
        let base = self.frame_base;
        Ok((base + a as usize, base + b as usize, base + c as usize))
    }

    #[inline]
    pub(super) fn stack_bc_indices(&self, lhs: u8, rhs: u8) -> Result<(usize, usize)> {
        let max = lhs.max(rhs);
        if max as u16 >= self.register_count {
            bail!("register {} out of bounds", max);
        }
        let base = self.frame_base;
        Ok((base + lhs as usize, base + rhs as usize))
    }

    pub(super) fn reset_entry_frame(&mut self, register_count: u16) {
        self.frame_base = 0;
        self.register_count = register_count;
        self.pc = 0;
        self.state.stack_top = register_count as usize;
        if self.state.stack.len() < self.state.stack_top {
            self.state.stack.resize(self.state.stack_top, RuntimeVal::Nil);
        }
        self.state.stack[..self.state.stack_top].fill(RuntimeVal::Nil);
    }

    pub(super) fn call_args_stack_range(&self, window: CallWindow) -> Result<Range<usize>> {
        let start = window.arg_base().as_usize();
        let count = window.arg_count as usize;
        if start + count > self.register_count as usize {
            bail!("call args range {}..{} out of bounds", start, start + count);
        }
        let range_start = self.frame_base + start;
        Ok(range_start..range_start + count)
    }

    pub(super) fn read_register_list(&self, base: u8, count: u8) -> Result<TypedList> {
        let range = self.register_range(base, count, "register range")?;
        Ok(TypedList::from_runtime_values(
            &self.state.stack[range],
            &self.state.heap,
        ))
    }

    pub(super) fn take_register_list(&mut self, base: u8, count: u8) -> Result<TypedList> {
        let range = self.register_range(base, count, "register range")?;
        // Building and clearing are separate steps: the narrowing rule lives
        // on `TypedList`, and emptying the registers afterwards is this
        // caller's business, not the list constructor's.
        let list = TypedList::from_runtime_values(&self.state.stack[range.clone()], &self.state.heap);
        self.state.stack[range].fill(RuntimeVal::Nil);
        Ok(list)
    }

    pub(super) fn take_return_values(&mut self, base: u8, count: u8) -> Result<ReturnValues> {
        let range = self.register_range(base, count, "return range")?;
        Ok(ReturnValues::take_from_slots(&mut self.state.stack[range]))
    }

    fn register_range(&self, base: u8, count: u8, label: &str) -> Result<Range<usize>> {
        let base = base as usize;
        let count = count as usize;
        if base + count > self.register_count as usize {
            bail!("{label} {}..{} out of bounds", base, base + count);
        }
        let range_start = self.frame_base + base;
        Ok(range_start..range_start + count)
    }

    pub(super) fn write_returns(
        &mut self,
        window: CallWindow,
        values: impl IntoIterator<Item = RuntimeVal>,
    ) -> Result<()> {
        let start = window.ret_base().as_usize();
        let count = window.ret_count as usize;
        if start + count > self.register_count as usize {
            bail!("return range {}..{} out of bounds", start, start + count);
        }
        let range_start = self.frame_base + start;
        let range_end = range_start + count;
        self.state.stack[range_start..range_end].fill(RuntimeVal::Nil);
        for (slot, value) in self.state.stack[range_start..range_end].iter_mut().zip(values) {
            *slot = value;
        }
        Ok(())
    }

    pub(super) fn clear_call_window_temps(&mut self, window: CallWindow, named_count: u16) -> Result<()> {
        let start = window.arg_base().as_usize();
        let count = window.arg_count as usize + named_count as usize * 2;
        if start + count > self.register_count as usize {
            bail!("call temp range {}..{} out of bounds", start, start + count);
        }
        let range_start = self.frame_base + start;
        let range_end = range_start + count;
        self.state.stack[range_start..range_end].fill(RuntimeVal::Nil);
        Ok(())
    }
}
