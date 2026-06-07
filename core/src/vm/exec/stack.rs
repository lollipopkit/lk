use std::ops::Range;

use anyhow::{Result, bail};

use std::sync::Arc;

use crate::{
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
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
        Ok(std::mem::take(&mut self.state.stack[index]))
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
        Ok(typed_list_from_runtime_slots(
            &self.state.stack[range],
            &self.state.heap,
        ))
    }

    pub(super) fn take_register_list(&mut self, base: u8, count: u8) -> Result<TypedList> {
        let range = self.register_range(base, count, "register range")?;
        Ok(take_typed_list_from_runtime_slots(
            &mut self.state.stack[range],
            &self.state.heap,
        ))
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

fn typed_list_from_runtime_slots(values: &[RuntimeVal], heap: &HeapStore) -> TypedList {
    match runtime_slot_list_shape(values, heap) {
        RuntimeSlotListShape::Mixed => {
            let mut out = Vec::with_capacity(values.len());
            out.extend_from_slice(values);
            TypedList::Mixed(out)
        }
        RuntimeSlotListShape::Int => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let RuntimeVal::Int(value) = value else {
                    unreachable!("shape scan only returns Int for int slots");
                };
                out.push(*value);
            }
            TypedList::Int(out)
        }
        RuntimeSlotListShape::Float => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let RuntimeVal::Float(value) = value else {
                    unreachable!("shape scan only returns Float for float slots");
                };
                out.push(*value);
            }
            TypedList::Float(out)
        }
        RuntimeSlotListShape::Bool => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let RuntimeVal::Bool(value) = value else {
                    unreachable!("shape scan only returns Bool for bool slots");
                };
                out.push(*value);
            }
            TypedList::Bool(out)
        }
        RuntimeSlotListShape::String => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                match value {
                    RuntimeVal::ShortStr(value) => out.push(Arc::<str>::from(value.as_str())),
                    RuntimeVal::Obj(handle) => match heap.get(*handle) {
                        Some(HeapValue::String(value)) => out.push(Arc::clone(value)),
                        _ => unreachable!("shape scan only returns String for string slots"),
                    },
                    _ => unreachable!("shape scan only returns String for string slots"),
                }
            }
            TypedList::String(out)
        }
    }
}

fn take_typed_list_from_runtime_slots(values: &mut [RuntimeVal], heap: &HeapStore) -> TypedList {
    match runtime_slot_list_shape(values, heap) {
        RuntimeSlotListShape::Mixed => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(std::mem::take(value));
            }
            TypedList::Mixed(out)
        }
        RuntimeSlotListShape::Int => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let value = match std::mem::take(value) {
                    RuntimeVal::Int(value) => value,
                    _ => unreachable!("shape scan only returns Int for int slots"),
                };
                out.push(value);
            }
            TypedList::Int(out)
        }
        RuntimeSlotListShape::Float => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let value = match std::mem::take(value) {
                    RuntimeVal::Float(value) => value,
                    _ => unreachable!("shape scan only returns Float for float slots"),
                };
                out.push(value);
            }
            TypedList::Float(out)
        }
        RuntimeSlotListShape::Bool => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let value = match std::mem::take(value) {
                    RuntimeVal::Bool(value) => value,
                    _ => unreachable!("shape scan only returns Bool for bool slots"),
                };
                out.push(value);
            }
            TypedList::Bool(out)
        }
        RuntimeSlotListShape::String => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let value = match std::mem::take(value) {
                    RuntimeVal::ShortStr(value) => Arc::<str>::from(value.as_str()),
                    RuntimeVal::Obj(handle) => match heap.get(handle) {
                        Some(HeapValue::String(value)) => Arc::clone(value),
                        _ => unreachable!("shape scan only returns String for string slots"),
                    },
                    _ => unreachable!("shape scan only returns String for string slots"),
                };
                out.push(value);
            }
            TypedList::String(out)
        }
    }
}

enum RuntimeSlotListShape {
    Mixed,
    Int,
    Float,
    Bool,
    String,
}

fn runtime_slot_list_shape(values: &[RuntimeVal], heap: &HeapStore) -> RuntimeSlotListShape {
    if values.is_empty() {
        return RuntimeSlotListShape::Mixed;
    }
    let mut shape: Option<RuntimeSlotListShape> = None;
    for value in values {
        let next = match value {
            RuntimeVal::Int(_) => RuntimeSlotListShape::Int,
            RuntimeVal::Float(_) => RuntimeSlotListShape::Float,
            RuntimeVal::Bool(_) => RuntimeSlotListShape::Bool,
            RuntimeVal::ShortStr(_) => RuntimeSlotListShape::String,
            RuntimeVal::Obj(handle) if matches!(heap.get(*handle), Some(HeapValue::String(_))) => {
                RuntimeSlotListShape::String
            }
            _ => return RuntimeSlotListShape::Mixed,
        };
        match (&shape, next) {
            (None, next) => shape = Some(next),
            (Some(RuntimeSlotListShape::Int), RuntimeSlotListShape::Int)
            | (Some(RuntimeSlotListShape::Float), RuntimeSlotListShape::Float)
            | (Some(RuntimeSlotListShape::Bool), RuntimeSlotListShape::Bool)
            | (Some(RuntimeSlotListShape::String), RuntimeSlotListShape::String) => {}
            _ => return RuntimeSlotListShape::Mixed,
        }
    }
    shape.unwrap_or(RuntimeSlotListShape::Mixed)
}
