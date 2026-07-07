//! Opcode handlers extracted from the dispatch loop.
//!
//! Keep `#[cold]` only on handlers that are genuinely uncommon or dominated by
//! slow/error work. String, branch, call, and container helpers can be hot in
//! real workloads, so they intentionally avoid a cold hint unless measured.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::{Result, anyhow, bail};

use core::fmt::Write;

use crate::val::{HeapValue, RuntimeVal, ShortStr, TypedList};

use super::{Executor, call::CallOutcome};
use crate::vm::{
    CallWindow, Function, Instr, Module, Opcode, RegisterIndex, VmContext,
    analysis::{
        PerfForLoopFact, VmCallMetric, VmContainerMetric, record_call_op_known_enabled,
        record_container_op_known_enabled,
    },
};

impl Executor {
    /// `CallMethodK`: boxing-free positional method call — receiver at the
    /// window base, args at `[base+1, base+1+c)`, result written to the base.
    /// The method name comes straight from the string constant pool; no
    /// `__lk_call_method` global load and no argument list allocation.
    pub(super) fn dispatch_call_method_k(
        &mut self,
        function: &Function,
        module: Option<&Module>,
        instr: Instr,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<()> {
        self.collect_pending_garbage();
        let base = instr.a();
        let argc = instr.c() as usize;
        // Borrowed from `function` (not `self`), so it stays valid across the
        // runtime construction below.
        let name = function
            .consts
            .string(u16::from(instr.b()))
            .ok_or_else(|| anyhow!("CallMethodK method-name const {} out of bounds", instr.b()))?;
        let receiver = *self.read(base)?;
        let mut inline: [RuntimeVal; 8] = core::array::from_fn(|_| RuntimeVal::Nil);
        let mut spill: Vec<RuntimeVal>;
        let args: &[RuntimeVal] = if argc <= inline.len() {
            for (i, slot) in inline.iter_mut().take(argc).enumerate() {
                *slot = *self.read(base.wrapping_add(1).wrapping_add(i as u8))?;
            }
            &inline[..argc]
        } else {
            spill = Vec::with_capacity(argc);
            for i in 0..argc {
                spill.push(*self.read(base.wrapping_add(1).wrapping_add(i as u8))?);
            }
            &spill
        };
        let result = {
            let mut runtime = crate::vm::NativeRuntime::new(&mut self.state, ctx.as_deref_mut(), module);
            crate::vm::context::core_call_method_windowed(receiver, name, args, &mut runtime)?
        };
        self.write(base, result)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_load_capture(&mut self, instr: Instr) -> Result<()> {
        let value = self
            .captures
            .get(instr.bx() as usize)
            .cloned()
            .ok_or_else(|| anyhow!("LoadCapture index {} out of bounds", instr.bx()))?;
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_load_cell_val(&mut self, instr: Instr) -> Result<()> {
        let value = self.load_cell_value(instr.b())?;
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_store_cell_val(&mut self, function: &Function, instr: Instr) -> Result<()> {
        self.store_cell_value(
            instr.a(),
            instr.b(),
            function
                .performance
                .cell_move(self.pc)
                .is_some_and(|fact| fact.move_value),
        )?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_load_function(&mut self, instr: Instr, module: Option<&Module>) -> Result<()> {
        self.load_function_value(instr.a(), instr.bx(), module)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_make_closure(&mut self, instr: Instr, module: Option<&Module>) -> Result<()> {
        self.collect_pending_garbage();
        self.make_closure_value(instr.a(), instr.b(), instr.c(), module)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_load_native(&mut self, instr: Instr, module: Option<&Module>) -> Result<()> {
        self.load_native_value(instr.a(), instr.bx(), module)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_not(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let index = self.stack_index_unchecked(instr.b());
        let value = match &self.state.stack[index] {
            RuntimeVal::Bool(b) => !b,
            RuntimeVal::Nil => true,
            other => bail!("Not expected Bool or Nil, got {:?}", other.kind()),
        };
        if self.try_fused_bool_branch(function, instr.a(), value, self.collect_metrics)? {
            return Ok(());
        }
        self.write_unchecked(instr.a(), RuntimeVal::Bool(value));
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_is_nil(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let index = self.stack_index_unchecked(instr.b());
        let value = matches!(self.state.stack[index], RuntimeVal::Nil);
        if self.try_fused_bool_branch(function, instr.a(), value, self.collect_metrics)? {
            return Ok(());
        }
        self.write_unchecked(instr.a(), RuntimeVal::Bool(value));
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_is_list(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let value = self.runtime_value_is_list(self.read(instr.b())?)?;
        if self.try_fused_bool_branch(function, instr.a(), value, self.collect_metrics)? {
            return Ok(());
        }
        self.write(instr.a(), RuntimeVal::Bool(value))?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_is_map(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let value = self.runtime_value_is_map(self.read(instr.b())?)?;
        if self.try_fused_bool_branch(function, instr.a(), value, self.collect_metrics)? {
            return Ok(());
        }
        self.write(instr.a(), RuntimeVal::Bool(value))?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn dispatch_to_string(
        &mut self,
        instr: Instr,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<()> {
        let value = self.to_runtime_string_with_display(instr.b(), module, ctx)?;
        self.write_string(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn dispatch_concat_string(
        &mut self,
        instr: Instr,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<()> {
        self.collect_pending_garbage();
        let lhs_val = self.read_unchecked(instr.b());
        let rhs_val = self.read_unchecked(instr.c());
        match (lhs_val, rhs_val) {
            (RuntimeVal::ShortStr(l), RuntimeVal::ShortStr(r)) => {
                match l.concat(*r) {
                    crate::val::ShortStrOrStr::Short(short) => {
                        self.write_unchecked(instr.a(), RuntimeVal::ShortStr(short));
                    }
                    crate::val::ShortStrOrStr::Str(combined) => {
                        self.write_string(instr.a(), combined)?;
                    }
                }
                self.pc += 1;
            }
            (RuntimeVal::ShortStr(s), RuntimeVal::Int(n)) => {
                match s.concat_int(*n) {
                    crate::val::ShortStrOrStr::Short(short) => {
                        self.write_unchecked(instr.a(), RuntimeVal::ShortStr(short));
                    }
                    crate::val::ShortStrOrStr::Str(combined) => {
                        self.write_string(instr.a(), combined)?;
                    }
                }
                self.pc += 1;
            }
            (RuntimeVal::Int(n), RuntimeVal::ShortStr(s)) => {
                match crate::val::ShortStr::concat_int_prefix(*n, *s) {
                    crate::val::ShortStrOrStr::Short(short) => {
                        self.write_unchecked(instr.a(), RuntimeVal::ShortStr(short));
                    }
                    crate::val::ShortStrOrStr::Str(combined) => {
                        self.write_string(instr.a(), combined)?;
                    }
                }
                self.pc += 1;
            }
            (_, _) => {
                let lhs = self.to_runtime_string_with_display(instr.b(), module, ctx)?;
                let rhs = self.to_runtime_string_with_display(instr.c(), module, ctx)?;
                self.write_string(instr.a(), format!("{lhs}{rhs}"))?;
                self.pc += 1;
            }
        };
        Ok(())
    }

    pub(super) fn dispatch_string_split(&mut self, instr: Instr) -> Result<()> {
        self.collect_pending_garbage();
        self.string_split(instr.a(), instr.b(), instr.c())?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn dispatch_list_join(&mut self, instr: Instr) -> Result<()> {
        self.collect_pending_garbage();
        self.list_join(instr.a(), instr.b(), instr.c())?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn dispatch_contains(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let value = self.contains_value(instr.b(), instr.c())?;
        if self.try_fused_bool_branch(function, instr.a(), value, self.collect_metrics)? {
            return Ok(());
        }
        self.write(instr.a(), RuntimeVal::Bool(value))?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_slice_from(&mut self, instr: Instr) -> Result<()> {
        let value = self.slice_from(instr.b(), instr.c())?;
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_map_rest(&mut self, instr: Instr) -> Result<()> {
        let value = self.map_rest(instr.b(), instr.c())?;
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_raise(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let message = function
            .consts
            .strings
            .get(instr.bx() as usize)
            .ok_or_else(|| anyhow!("Raise const index {} out of bounds", instr.bx()))?;
        self.raise_language_message(message)
    }

    #[cold]
    pub(super) fn dispatch_try_begin(&mut self, instr: Instr) -> Result<()> {
        self.begin_try(instr.a(), instr.sbx() as i32)
    }

    #[cold]
    pub(super) fn dispatch_try_end(&mut self) {
        self.end_try();
    }

    pub(super) fn dispatch_test(&mut self, instr: Instr) -> Result<()> {
        let truthy = self.truthy_unchecked(instr.a());
        if truthy == (instr.b() != 0) {
            self.pc += 1;
        } else {
            self.pc = self.relative_pc(instr.c() as i8 as i32)?;
        }
        Ok(())
    }

    pub(super) fn dispatch_br_false(&mut self, instr: Instr) -> Result<()> {
        if self.truthy_unchecked(instr.a()) {
            self.pc += 1;
        } else {
            self.pc = self.relative_pc(instr.sbx() as i32)?;
        }
        Ok(())
    }

    pub(super) fn dispatch_br_true(&mut self, instr: Instr) -> Result<()> {
        if self.truthy_unchecked(instr.a()) {
            self.pc = self.relative_pc(instr.sbx() as i32)?;
        } else {
            self.pc += 1;
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(super) fn compare_test_value_slow(&self, instr: Instr, lhs_idx: usize, rhs_idx: usize) -> Result<bool> {
        Ok(match instr.opcode() {
            Opcode::TestEqInt | Opcode::TestNeInt => {
                let equal = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                    (RuntimeVal::Int(l), RuntimeVal::Int(r)) => l == r,
                    (RuntimeVal::Int(l), RuntimeVal::Float(r)) => (*l as f64) == *r,
                    (RuntimeVal::Float(l), RuntimeVal::Int(r)) => *l == (*r as f64),
                    (RuntimeVal::Float(l), RuntimeVal::Float(r)) => l == r,
                    (RuntimeVal::Bool(l), RuntimeVal::Bool(r)) => l == r,
                    (RuntimeVal::ShortStr(l), RuntimeVal::ShortStr(r)) => l == r,
                    (RuntimeVal::Nil, RuntimeVal::Nil) => true,
                    (RuntimeVal::Nil, _) | (_, RuntimeVal::Nil) => false,
                    (RuntimeVal::Obj(l), RuntimeVal::Obj(r)) if l == r => true,
                    _ => self.values_equal(instr.a(), instr.b())?,
                };
                if instr.opcode() == Opcode::TestEqInt {
                    equal
                } else {
                    !equal
                }
            }
            Opcode::TestLtInt | Opcode::TestLeInt | Opcode::TestGtInt | Opcode::TestGeInt => {
                let lhs = self.number_value(&self.state.stack[lhs_idx])?;
                let rhs = self.number_value(&self.state.stack[rhs_idx])?;
                match instr.opcode() {
                    Opcode::TestLtInt => lhs < rhs,
                    Opcode::TestLeInt => lhs <= rhs,
                    Opcode::TestGtInt => lhs > rhs,
                    Opcode::TestGeInt => lhs >= rhs,
                    _ => unreachable!("opcode matched above"),
                }
            }
            _ => unreachable!("opcode matched by caller"),
        })
    }

    #[cold]
    #[inline(never)]
    pub(super) fn compare_test_immediate_value_slow(&self, instr: Instr, lhs_idx: usize) -> Result<bool> {
        let rhs = i64::from(instr.sc());
        Ok(match instr.opcode() {
            Opcode::TestEqIntI | Opcode::TestNeIntI => {
                let equal = match &self.state.stack[lhs_idx] {
                    RuntimeVal::Int(lhs) => *lhs == rhs,
                    RuntimeVal::Float(lhs) => *lhs == rhs as f64,
                    _ => false,
                };
                if instr.opcode() == Opcode::TestEqIntI {
                    equal
                } else {
                    !equal
                }
            }
            Opcode::TestLtIntI | Opcode::TestLeIntI | Opcode::TestGtIntI | Opcode::TestGeIntI => {
                let lhs = self.number_value(&self.state.stack[lhs_idx])?;
                let rhs = rhs as f64;
                match instr.opcode() {
                    Opcode::TestLtIntI => lhs < rhs,
                    Opcode::TestLeIntI => lhs <= rhs,
                    Opcode::TestGtIntI => lhs > rhs,
                    Opcode::TestGeIntI => lhs >= rhs,
                    _ => unreachable!("opcode matched above"),
                }
            }
            _ => unreachable!("opcode matched by caller"),
        })
    }

    #[cold]
    pub(super) fn dispatch_for_loop_i(&mut self, instr: Instr, fact: PerfForLoopFact) -> Result<()> {
        let index = self.read_int(instr.a())?;
        let end = self.read_int(instr.b())?;
        let step = self.read_int(instr.c())?;
        let next = index.wrapping_add(step);
        self.write_unchecked(instr.a(), RuntimeVal::Int(next));
        let keep_going = match (fact.positive_step, fact.inclusive) {
            (true, true) => next <= end,
            (true, false) => next < end,
            (false, true) => next >= end,
            (false, false) => next > end,
        };
        if keep_going {
            self.pc = self.relative_pc(fact.jump_offset)?;
        } else {
            self.pc += 1;
        }
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_new_object(&mut self, instr: Instr, collect_metrics: bool) -> Result<()> {
        self.collect_pending_garbage();
        if collect_metrics {
            record_container_op_known_enabled(VmContainerMetric::Generic);
        }
        let object = self.read_object_fields(instr.b(), instr.c())?;
        let handle = self.alloc_heap_value(HeapValue::Object(object));
        self.write(instr.a(), RuntimeVal::Obj(handle))?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_new_range(&mut self, instr: Instr, collect_metrics: bool) -> Result<()> {
        self.collect_pending_garbage();
        if collect_metrics {
            record_container_op_known_enabled(VmContainerMetric::List);
        }
        let list = self.build_int_range(instr.b(), instr.c() != 0)?;
        let handle = self.alloc_heap_value(HeapValue::List(TypedList::Int(list)));
        self.write(instr.a(), RuntimeVal::Obj(handle))?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn dispatch_len(&mut self, instr: Instr, collect_metrics: bool) -> Result<()> {
        if collect_metrics {
            record_container_op_known_enabled(VmContainerMetric::Generic);
        }
        let len = self.len_value(instr.b())?;
        self.write(instr.a(), RuntimeVal::Int(len as i64))?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn dispatch_to_iter(&mut self, instr: Instr, collect_metrics: bool) -> Result<()> {
        if collect_metrics {
            record_container_op_known_enabled(VmContainerMetric::Generic);
        }
        let iter = self.to_iter(instr.b())?;
        self.write(instr.a(), iter)?;
        self.pc += 1;
        Ok(())
    }

    /// Dispatch the generic `Call` opcode. Returns `Some(function_index)`
    /// when the target was a closure and a `Frame` was pushed (plan M2.5 sub-
    /// step ①) — the caller (the `Opcode::Call` arm in `exec.rs`) must stop
    /// dispatching this activation and switch to it. Returns `None` when the
    /// call already ran to completion synchronously (native/runtime target,
    /// still recursive) and the result is already written — the caller keeps
    /// looping.
    pub(super) fn dispatch_call(
        &mut self,
        function: &Function,
        module: Option<&Module>,
        instr: Instr,
        ctx: &mut Option<&mut VmContext>,
        collect_metrics: bool,
    ) -> Result<Option<u32>> {
        self.collect_pending_garbage();
        if collect_metrics {
            record_call_op_known_enabled(VmCallMetric::Generic);
        }
        let call_fact = self.call_fact_from_static_cache_or_instr(function, instr, false);
        let window = CallWindow::new(RegisterIndex::new(call_fact.call_base), call_fact.positional_count, 1);
        let call_pc = self.pc;
        match self.call_function(module, window, Some(call_fact.target_kind), ctx)? {
            CallOutcome::Pushed(function_index) => Ok(Some(function_index)),
            CallOutcome::Value(value) => {
                if self.pc != call_pc {
                    return Ok(None);
                }
                self.clear_call_window_temps(window, 0)?;
                self.write_returns(window, [value])?;
                self.pc += 1;
                Ok(None)
            }
        }
    }

    /// Dispatch `CallNamed`. Same `Some`/`None` protocol as `dispatch_call`
    /// (plan M2.5 sub-step ②): `Some(function_index)` means a `CallFrame` was
    /// pushed and the caller must switch dispatch to it.
    #[cold]
    pub(super) fn dispatch_call_named(
        &mut self,
        function: &Function,
        module: Option<&Module>,
        instr: Instr,
        ctx: &mut Option<&mut VmContext>,
        collect_metrics: bool,
    ) -> Result<Option<u32>> {
        self.collect_pending_garbage();
        if collect_metrics {
            record_call_op_known_enabled(VmCallMetric::Named);
        }
        let call_fact = self.call_fact_from_static_cache_or_instr(function, instr, true);
        let window = CallWindow::new(RegisterIndex::new(call_fact.call_base), call_fact.positional_count, 1);
        let call_pc = self.pc;
        match self.call_function_named(module, window, call_fact.named_count, Some(call_fact.target_kind), ctx)? {
            CallOutcome::Pushed(function_index) => Ok(Some(function_index)),
            CallOutcome::Value(value) => {
                if self.pc != call_pc {
                    return Ok(None);
                }
                self.clear_call_window_temps(window, call_fact.named_count)?;
                self.write_returns(window, [value])?;
                self.pc += 1;
                Ok(None)
            }
        }
    }

    #[cold]
    pub(super) fn dispatch_set_global(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let global_fact = function.performance.global_op(self.pc).copied();
        let value = if global_fact.is_some_and(|fact| fact.move_source) {
            self.take(instr.a())?
        } else {
            *self.read(instr.a())?
        };
        let slot = self.global_slot_from_fact_cache_or_instr(function, instr);
        self.write_global(slot, value)?;
        self.pc += 1;
        Ok(())
    }

    /// Dispatch cold opcodes that are rarely executed in hot loops.
    /// Moving these to a separate `#[inline(never)]` function reduces the main
    /// dispatch loop code size, improving I-cache behavior for the hot path.
    #[inline(never)]
    pub(super) fn dispatch_cold(
        &mut self,
        opcode: Opcode,
        function: &Function,
        module: Option<&Module>,
        instr: Instr,
        ctx: &mut Option<&mut VmContext>,
        collect_metrics: bool,
    ) -> Result<()> {
        match opcode {
            Opcode::LoadCapture => {
                self.dispatch_load_capture(instr)?;
            }
            Opcode::LoadCellVal => {
                self.dispatch_load_cell_val(instr)?;
            }
            Opcode::StoreCellVal => self.dispatch_store_cell_val(function, instr)?,
            Opcode::LoadFunction => {
                self.dispatch_load_function(instr, module)?;
            }
            Opcode::MakeClosure => {
                self.dispatch_make_closure(instr, module)?;
            }
            Opcode::LoadNative => {
                self.dispatch_load_native(instr, module)?;
            }
            Opcode::Not => {
                self.dispatch_not(function, instr)?;
            }
            Opcode::IsNil => {
                self.dispatch_is_nil(function, instr)?;
            }
            Opcode::IsList => {
                self.dispatch_is_list(function, instr)?;
            }
            Opcode::IsMap => {
                self.dispatch_is_map(function, instr)?;
            }
            Opcode::ToString => {
                self.dispatch_to_string(instr, module, ctx)?;
            }
            Opcode::StringSplit => {
                self.dispatch_string_split(instr)?;
            }
            Opcode::ListJoin => {
                self.dispatch_list_join(instr)?;
            }
            Opcode::Contains => {
                self.dispatch_contains(function, instr)?;
            }
            Opcode::SliceFrom => {
                self.dispatch_slice_from(instr)?;
            }
            Opcode::MapRest => {
                self.dispatch_map_rest(instr)?;
            }
            Opcode::Raise => self.dispatch_raise(function, instr)?,
            Opcode::TryBegin => self.dispatch_try_begin(instr)?,
            Opcode::TryEnd => self.dispatch_try_end(),
            Opcode::Test => self.dispatch_test(instr)?,
            Opcode::BrFalse => self.dispatch_br_false(instr)?,
            Opcode::BrTrue => self.dispatch_br_true(instr)?,
            Opcode::NewObject => {
                self.dispatch_new_object(instr, collect_metrics)?;
            }
            Opcode::NewRange => {
                self.dispatch_new_range(instr, collect_metrics)?;
            }
            Opcode::SetGlobal => self.dispatch_set_global(function, instr)?,
            _ => unreachable!("dispatch_cold called for non-cold opcode: {:?}", opcode),
        }
        Ok(())
    }

    /// Dispatch ConcatN: concatenate C values from registers B to B+C-1 into A.
    /// Like Lua's OP_CONCAT, this first measures total length, then allocates once.
    /// Fast paths: all ShortStr/Int/Float parts that fit in ShortStr (7 bytes or fewer).
    pub(super) fn dispatch_concat_n(
        &mut self,
        instr: Instr,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<()> {
        let start = instr.b() as usize;
        let count = instr.c() as usize;
        if count == 0 {
            self.write_string(instr.a(), String::new())?;
            self.pc += 1;
            return Ok(());
        }
        if count == 1 {
            let s = self.to_runtime_string_with_display(start as u8, module, ctx)?;
            self.write_string(instr.a(), s)?;
            self.pc += 1;
            return Ok(());
        }
        self.collect_pending_garbage();

        // Fast path: all ShortStr/Int/Float and result fits ShortStr (7 bytes or fewer)
        let mut short_buf = [0u8; 7];
        let mut short_len: usize = 0;
        let mut all_short = true;
        'a: for i in 0..count {
            let reg = (start + i) as u8;
            let val = self.read_unchecked(reg);
            match &val {
                RuntimeVal::ShortStr(s) => {
                    let bytes = s.as_str().as_bytes();
                    if short_len + bytes.len() <= 7 {
                        short_buf[short_len..short_len + bytes.len()].copy_from_slice(bytes);
                        short_len += bytes.len();
                    } else {
                        all_short = false;
                        break 'a;
                    }
                }
                RuntimeVal::Int(n) => {
                    let n_str = n.to_string();
                    if short_len + n_str.len() <= 7 {
                        short_buf[short_len..short_len + n_str.len()].copy_from_slice(n_str.as_bytes());
                        short_len += n_str.len();
                    } else {
                        all_short = false;
                        break 'a;
                    }
                }
                _ => {
                    all_short = false;
                    break 'a;
                }
            }
        }

        if all_short && short_len > 0 {
            let result_str = core::str::from_utf8(&short_buf[..short_len]).unwrap_or("");
            if let Some(short) = ShortStr::new(result_str) {
                self.write_unchecked(instr.a(), RuntimeVal::ShortStr(short));
                self.pc += 1;
                return Ok(());
            }
        }

        // General path: build result string with pre-allocated buffer
        let mut result = String::with_capacity(64);
        for i in 0..count {
            let reg = (start + i) as u8;
            let val = self.read_unchecked(reg);
            match &val {
                RuntimeVal::ShortStr(s) => result.push_str(s.as_str()),
                RuntimeVal::Int(n) => write!(result, "{}", n)?,
                RuntimeVal::Float(f) => write!(result, "{}", f)?,
                RuntimeVal::Bool(b) => result.push_str(if *b { "true" } else { "false" }),
                RuntimeVal::Nil => result.push_str("nil"),
                RuntimeVal::Obj(handle) => {
                    if let Some(HeapValue::String(s)) = self.state.heap.get(*handle) {
                        result.push_str(s.as_ref());
                    } else {
                        let s = self.to_runtime_string_with_display(reg, module, ctx)?;
                        result.push_str(&s);
                    }
                }
            }
        }

        self.write_string(instr.a(), result)?;
        self.pc += 1;
        Ok(())
    }
}
