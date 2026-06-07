use std::{ops::Range, sync::Arc};

use anyhow::{Result, anyhow, bail};

use crate::{
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{
        Function, Instr, Module, NativeArgs, NativeEntry, NativeFunction, NativeRuntime, Opcode, RuntimeModuleState,
        VmContext,
        analysis::{PerfCallFact, PerfFusedBoolBranchFact, PerfIndexFact, record_branch_op_known_enabled},
    },
};

use super::Executor;

pub(super) fn set_list_value(list: &mut TypedList, index: usize, value: RuntimeVal) -> Result<()> {
    match list {
        TypedList::Mixed(values) => {
            let Some(slot) = values.get_mut(index) else {
                bail!("list index {} out of bounds", index);
            };
            *slot = value;
        }
        TypedList::Int(values) => match value {
            RuntimeVal::Int(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
            }
            value => {
                if index >= values.len() {
                    bail!("list index {} out of bounds", index);
                }
                let mixed = copy_numeric_list_with_replacement(values, index, value, RuntimeVal::Int);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::Float(values) => match value {
            RuntimeVal::Float(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
            }
            value => {
                if index >= values.len() {
                    bail!("list index {} out of bounds", index);
                }
                let mixed = copy_numeric_list_with_replacement(values, index, value, RuntimeVal::Float);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::Bool(values) => match value {
            RuntimeVal::Bool(value) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = value;
            }
            value => {
                if index >= values.len() {
                    bail!("list index {} out of bounds", index);
                }
                let mixed = copy_numeric_list_with_replacement(values, index, value, RuntimeVal::Bool);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::String(_) => bail!("internal error: typed string list write must be handled before mutable borrow"),
    }
    Ok(())
}

pub(super) fn push_list_value(list: &mut TypedList, value: RuntimeVal, string_value: Option<Arc<str>>) -> Result<()> {
    match list {
        TypedList::Mixed(values) if values.is_empty() => match (value, string_value) {
            (RuntimeVal::Int(value), _) => *list = TypedList::Int(vec![value]),
            (RuntimeVal::Float(value), _) => *list = TypedList::Float(vec![value]),
            (RuntimeVal::Bool(value), _) => *list = TypedList::Bool(vec![value]),
            (value, Some(string_value)) if matches!(value, RuntimeVal::ShortStr(_) | RuntimeVal::Obj(_)) => {
                *list = TypedList::String(vec![string_value]);
            }
            (value, _) => values.push(value),
        },
        TypedList::Mixed(values) => values.push(value),
        TypedList::Int(values) => match value {
            RuntimeVal::Int(value) => values.push(value),
            value => {
                let mut mixed = copy_numeric_list(values, RuntimeVal::Int);
                mixed.push(value);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::Float(values) => match value {
            RuntimeVal::Float(value) => values.push(value),
            value => {
                let mut mixed = copy_numeric_list(values, RuntimeVal::Float);
                mixed.push(value);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::Bool(values) => match value {
            RuntimeVal::Bool(value) => values.push(value),
            value => {
                let mut mixed = copy_numeric_list(values, RuntimeVal::Bool);
                mixed.push(value);
                *list = TypedList::Mixed(mixed);
            }
        },
        TypedList::String(values) => match string_value {
            Some(value) => values.push(value),
            None => bail!("internal error: typed string list push must be materialized before mutable borrow"),
        },
    }
    Ok(())
}

fn copy_numeric_list<T: Copy>(values: &[T], wrap: impl Fn(T) -> RuntimeVal) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len() + 1);
    mixed.extend(values.iter().copied().map(wrap));
    mixed
}

fn copy_numeric_list_with_replacement<T: Copy>(
    values: &[T],
    index: usize,
    value: RuntimeVal,
    wrap: impl Fn(T) -> RuntimeVal,
) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len());
    for value in &values[..index] {
        mixed.push(wrap(*value));
    }
    mixed.push(value);
    for value in &values[index + 1..] {
        mixed.push(wrap(*value));
    }
    mixed
}

pub(super) fn call_native_entry(
    native: &NativeEntry,
    args: &[RuntimeVal],
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    shared_module: Option<Arc<Module>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_native_entry_with_args(native, NativeArgs::new(args), state, module, shared_module, ctx)
}

pub(super) fn call_native_entry_with_args(
    native: &NativeEntry,
    native_args: NativeArgs<'_>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    shared_module: Option<Arc<Module>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let result = match &native.function {
        NativeFunction::Plain(function) | NativeFunction::Context(function) | NativeFunction::FullState(function) => {
            let mut runtime = match shared_module {
                Some(module) => NativeRuntime::new_with_shared_module(state, ctx, module),
                None => NativeRuntime::new(state, ctx, module),
            };
            function(native_args, &mut runtime)
        }
    };
    map_native_error(native, result)
}

pub(super) enum InlineNativeArgs {
    Zero,
    One([RuntimeVal; 1]),
    Two([RuntimeVal; 2]),
    Three([RuntimeVal; 3]),
    Four([RuntimeVal; 4]),
    Five([RuntimeVal; 5]),
    Six([RuntimeVal; 6]),
    Seven([RuntimeVal; 7]),
    Eight([RuntimeVal; 8]),
}

impl InlineNativeArgs {
    #[inline]
    pub(super) fn as_slice(&self) -> &[RuntimeVal] {
        match self {
            Self::Zero => &[],
            Self::One(values) => values,
            Self::Two(values) => values,
            Self::Three(values) => values,
            Self::Four(values) => values,
            Self::Five(values) => values,
            Self::Six(values) => values,
            Self::Seven(values) => values,
            Self::Eight(values) => values,
        }
    }
}

pub(super) fn move_inline_native_args_from_stack(
    native: &NativeEntry,
    stack: &mut [RuntimeVal],
    args: Range<usize>,
) -> Result<InlineNativeArgs> {
    move_inline_native_slots_from_stack(native, stack, args, "argument")
}

pub(super) fn move_inline_native_slots_from_stack(
    native: &NativeEntry,
    stack: &mut [RuntimeVal],
    slots: Range<usize>,
    label: &str,
) -> Result<InlineNativeArgs> {
    if slots.end > stack.len() {
        bail!("{} {} window out of bounds", native.name, label);
    }
    Ok(match slots.len() {
        0 => InlineNativeArgs::Zero,
        1 => InlineNativeArgs::One([std::mem::take(&mut stack[slots.start])]),
        2 => InlineNativeArgs::Two([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
        ]),
        3 => InlineNativeArgs::Three([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
        ]),
        4 => InlineNativeArgs::Four([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
        ]),
        5 => InlineNativeArgs::Five([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
        ]),
        6 => InlineNativeArgs::Six([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
            std::mem::take(&mut stack[slots.start + 5]),
        ]),
        7 => InlineNativeArgs::Seven([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
            std::mem::take(&mut stack[slots.start + 5]),
            std::mem::take(&mut stack[slots.start + 6]),
        ]),
        8 => InlineNativeArgs::Eight([
            std::mem::take(&mut stack[slots.start]),
            std::mem::take(&mut stack[slots.start + 1]),
            std::mem::take(&mut stack[slots.start + 2]),
            std::mem::take(&mut stack[slots.start + 3]),
            std::mem::take(&mut stack[slots.start + 4]),
            std::mem::take(&mut stack[slots.start + 5]),
            std::mem::take(&mut stack[slots.start + 6]),
            std::mem::take(&mut stack[slots.start + 7]),
        ]),
        len => bail!(
            "{} FullState native {} count {} exceeds inline buffer",
            native.name,
            label,
            len
        ),
    })
}

pub(super) fn call_native_entry_parts(
    native: &NativeEntry,
    args: NativeArgs<'_>,
    heap: &mut HeapStore,
    globals: &[RuntimeVal],
    module: Option<&Module>,
    shared_module: Option<Arc<Module>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    call_native_entry_parts_with_args(native, args, heap, globals, module, shared_module, ctx)
}

pub(super) fn call_native_entry_parts_with_args(
    native: &NativeEntry,
    native_args: NativeArgs<'_>,
    heap: &mut HeapStore,
    globals: &[RuntimeVal],
    module: Option<&Module>,
    shared_module: Option<Arc<Module>>,
    ctx: Option<&mut VmContext>,
) -> Result<RuntimeVal> {
    let result = match &native.function {
        NativeFunction::Plain(function) | NativeFunction::Context(function) => {
            let mut runtime = match shared_module {
                Some(module) => NativeRuntime::from_parts_with_shared_module(heap, globals, ctx, module),
                None => NativeRuntime::from_parts(heap, globals, ctx, module),
            };
            function(native_args, &mut runtime)
        }
        NativeFunction::FullState(_) => {
            bail!("{} requires full runtime state", native.name);
        }
    };
    map_native_error(native, result)
}

fn map_native_error(native: &NativeEntry, result: Result<RuntimeVal>) -> Result<RuntimeVal> {
    result.map_err(|err| {
        if err.is::<super::LanguageRaise>() {
            err
        } else {
            anyhow!("native `{}` failed: {err}", native.name)
        }
    })
}

pub(super) fn heap_kind(value: &HeapValue) -> &'static str {
    match value {
        HeapValue::String(_) => "String",
        HeapValue::List(_) => "List",
        HeapValue::Map(_) => "Map",
        HeapValue::Callable(_) => "Callable",
        HeapValue::Task(_) => "Task",
        HeapValue::Channel(_) => "Channel",
        HeapValue::Stream(_) => "Stream",
        HeapValue::StreamCursor(_) => "StreamCursor",
        HeapValue::Object(_) => "Object",
        HeapValue::UpvalCell(_) => "UpvalCell",
        HeapValue::ErrorVal(_) => "Error",
    }
}

impl Executor {
    #[inline]
    pub(super) fn read_int(&self, register: u8) -> Result<i64> {
        let index = self.stack_index(register)?;
        match &self.state.stack[index] {
            RuntimeVal::Int(value) => Ok(*value),
            other => bail!("register {} expected Int, got {:?}", register, other.kind()),
        }
    }

    #[inline]
    pub(super) fn read_number(&self, register: u8) -> Result<f64> {
        let index = self.stack_index(register)?;
        self.number_value(&self.state.stack[index])
            .map_err(|err| anyhow!("register {} expected Int or Float: {err}", register))
    }

    #[inline(always)]
    pub(super) fn read_number_unchecked(&self, register: u8) -> f64 {
        let index = self.stack_index_unchecked(register);
        match &self.state.stack[index] {
            RuntimeVal::Int(value) => *value as f64,
            RuntimeVal::Float(value) => *value,
            _ => panic!("register {} expected Int or Float", register),
        }
    }

    pub(super) fn number_value(&self, value: &RuntimeVal) -> Result<f64> {
        match value {
            RuntimeVal::Int(value) => Ok(*value as f64),
            RuntimeVal::Float(value) => Ok(*value),
            other => bail!("got {:?}", other.kind()),
        }
    }

    #[inline(always)]
    pub(super) fn truthy_unchecked(&self, register: u8) -> bool {
        let index = self.stack_index_unchecked(register);
        !matches!(&self.state.stack[index], RuntimeVal::Nil | RuntimeVal::Bool(false))
    }

    #[inline]
    #[allow(dead_code)]
    pub(super) fn truthy(&self, register: u8) -> Result<bool> {
        let index = self.stack_index(register)?;
        Ok(!matches!(
            &self.state.stack[index],
            RuntimeVal::Nil | RuntimeVal::Bool(false)
        ))
    }

    #[inline]
    pub(super) fn try_fused_compare_branch(
        &mut self,
        function: &Function,
        instr: Instr,
        collect_metrics: bool,
    ) -> Result<bool> {
        let Some(fact) = self.fused_bool_branch_fact(function, instr.a()) else {
            return Ok(false);
        };
        let value = self.number_compare_value(instr.opcode(), instr.b(), instr.c())?;
        self.apply_fused_bool_branch_fact(fact, value, collect_metrics)
    }

    #[inline]
    pub(super) fn try_fused_bool_branch(
        &mut self,
        function: &Function,
        result_reg: u8,
        value: bool,
        collect_metrics: bool,
    ) -> Result<bool> {
        let Some(fact) = self.fused_bool_branch_fact(function, result_reg) else {
            return Ok(false);
        };

        self.apply_fused_bool_branch_fact(fact, value, collect_metrics)
    }

    #[inline]
    pub(super) fn apply_fused_bool_branch_fact(
        &mut self,
        fact: PerfFusedBoolBranchFact,
        value: bool,
        collect_metrics: bool,
    ) -> Result<bool> {
        if collect_metrics {
            record_branch_op_known_enabled(true);
        }
        if value == fact.jump_when {
            self.pc = self.relative_pc_from(self.pc + fact.jump_base_pc_delta, fact.jump_offset)?;
        } else {
            self.pc += fact.fallthrough_pc_delta;
        }
        Ok(true)
    }

    #[inline(always)]
    pub(super) fn apply_compare_test_branch_unchecked(
        &mut self,
        function: &Function,
        code: &[Instr],
        instr: Instr,
        value: bool,
    ) {
        if let Some(fact) = function.performance.compare_test_branch(self.pc) {
            self.pc = if value == (instr.c() != 0) {
                fact.target_pc
            } else {
                self.pc + 2
            };
        } else {
            let jmp = code[self.pc + 1];
            if value == (instr.c() != 0) {
                self.pc = self.relative_pc_unchecked(jmp.sj_arg());
            } else {
                self.pc += 2;
            }
        }
    }

    #[inline]
    pub(super) fn fused_bool_branch_fact(
        &self,
        function: &Function,
        result_reg: u8,
    ) -> Option<PerfFusedBoolBranchFact> {
        if let Some(fact) = function.performance.fused_bool_branch(self.pc)
            && fact.result_reg == result_reg
        {
            return Some(fact);
        }
        if function.performance.has_control_flow_fact_slot(self.pc) {
            return None;
        }
        let branch = function.code.get(self.pc + 1).copied()?;
        if branch.a() != result_reg {
            return None;
        }
        if branch.opcode() == Opcode::BrFalse || branch.opcode() == Opcode::BrTrue {
            return Some(PerfFusedBoolBranchFact {
                result_reg,
                jump_when: branch.opcode() == Opcode::BrTrue,
                jump_offset: branch.sbx() as i32,
                jump_base_pc_delta: 1,
                fallthrough_pc_delta: 2,
            });
        }
        if branch.opcode() != Opcode::Test || branch.c() != 1 {
            return None;
        }
        let jmp = function.code.get(self.pc + 2).copied()?;
        (jmp.opcode() == Opcode::Jmp).then_some(PerfFusedBoolBranchFact {
            result_reg,
            jump_when: branch.b() != 0,
            jump_offset: jmp.sj_arg(),
            jump_base_pc_delta: 2,
            fallthrough_pc_delta: 3,
        })
    }

    #[inline]
    pub(super) fn relative_pc(&self, offset: i32) -> Result<usize> {
        self.relative_pc_from(self.pc, offset)
    }

    #[inline]
    pub(super) fn relative_pc_from(&self, pc: usize, offset: i32) -> Result<usize> {
        let next = pc as i64 + 1 + offset as i64;
        if next < 0 {
            return Self::jump_before_start_error();
        }
        Ok(next as usize)
    }

    /// Unchecked relative PC — elides bounds check. Use only when the offset
    /// is compiler-generated and known-valid.
    #[inline(always)]
    pub(super) fn relative_pc_unchecked(&self, offset: i32) -> usize {
        (self.pc as i64 + 1 + offset as i64) as usize
    }

    #[cold]
    #[inline(never)]
    pub(super) fn jump_before_start_error<T>() -> Result<T> {
        bail!("jump before start of function")
    }

    #[inline]
    pub(super) fn call_fact_from_static_cache_or_instr(
        &mut self,
        function: &Function,
        instr: Instr,
        named: bool,
    ) -> PerfCallFact {
        if let Some(fact) = function.performance.call_site(self.pc).copied()
            && (named || fact.named_count == 0)
        {
            self.state.inline_caches.set_call(self.pc, fact);
            return fact;
        }
        if let Some(fact) = self.state.inline_caches.call(self.pc) {
            return fact;
        }
        let (positional_count, named_count) = if named {
            let payload = instr.bx();
            ((payload & 0x7f) as u16, (payload >> 7) as u16)
        } else {
            (instr.c() as u16, 0)
        };
        let fact = PerfCallFact {
            // A holds the call-window base. B is only 7 bits and would truncate call_base >= 128.
            call_base: instr.a() as u16,
            positional_count,
            named_count,
            target_kind: self.observe_call_target_kind(instr.a() as u16),
        };
        self.state.inline_caches.set_call(self.pc, fact);
        fact
    }

    #[inline]
    pub(super) fn global_slot_from_fact_cache_or_instr(&mut self, function: &Function, instr: Instr) -> u16 {
        let slot = function
            .performance
            .global_op(self.pc)
            .map(|fact| fact.slot)
            .or_else(|| self.state.inline_caches.global(self.pc))
            .unwrap_or_else(|| instr.bx());
        self.state.inline_caches.set_global(self.pc, slot);
        slot
    }

    #[inline(always)]
    pub(super) fn static_index_fact(&self, function: &Function) -> Option<PerfIndexFact> {
        function.performance.index_op(self.pc).copied()
    }
}
