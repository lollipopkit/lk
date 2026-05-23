use std::ops::Range;

use anyhow::{Result, anyhow, bail};

use crate::{val::RuntimeVal, vm::CallWindow32};

use crate::vm::analysis::record_register_write;

use super::Executor32;

impl Executor32 {
    #[inline]
    pub(super) fn read(&self, register: u8) -> Result<&RuntimeVal> {
        let index = self.stack_index(register)?;
        self.state
            .stack
            .get(index)
            .ok_or_else(|| anyhow!("register {} out of bounds", register))
    }

    #[inline]
    pub(super) fn write(&mut self, register: u8, value: RuntimeVal) -> Result<()> {
        let index = self.stack_index(register)?;
        self.state.stack[index] = value;
        record_register_write();
        Ok(())
    }

    #[inline]
    fn stack_index(&self, register: u8) -> Result<usize> {
        if register as u16 >= self.register_count {
            bail!("register {} out of bounds", register);
        }
        Ok(self.frame_base + register as usize)
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

    pub(super) fn call_args_stack_range(&self, window: CallWindow32) -> Result<Range<usize>> {
        let start = window.arg_base().as_usize();
        let count = window.arg_count as usize;
        if start + count > self.register_count as usize {
            bail!("call args range {}..{} out of bounds", start, start + count);
        }
        let range_start = self.frame_base + start;
        Ok(range_start..range_start + count)
    }

    pub(super) fn read_register_slice(&self, base: u8, count: u8) -> Result<&[RuntimeVal]> {
        let range = self.register_range(base, count, "register range")?;
        Ok(&self.state.stack[range])
    }

    pub(super) fn read_register_range_owned(&self, base: u8, count: u8) -> Result<Vec<RuntimeVal>> {
        Ok(self.read_register_slice(base, count)?.to_vec())
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
        window: CallWindow32,
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
}
