//! Opcode handlers extracted from the dispatch loop.
//!
//! Keep `#[cold]` only on handlers that are genuinely uncommon or dominated by
//! slow/error work. String, branch, call, and container helpers can be hot in
//! real workloads, so they intentionally avoid a cold hint unless measured.

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal, TypedList};

use super::Executor;
use crate::vm::{
    CallWindow, Function, Instr, Module, Opcode, RegisterIndex, VmContext,
    analysis::{
        PerfForLoopFact, VmCallMetric, VmContainerMetric, record_branch_op_known_enabled, record_call_op_known_enabled,
        record_container_op_known_enabled,
    },
};

impl Executor {
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

    pub(super) fn dispatch_string_starts_with(&mut self, instr: Instr) -> Result<()> {
        self.string_starts_with(instr.a(), instr.b(), instr.c())?;
        self.pc += 1;
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

    pub(super) fn dispatch_br_nil(&mut self, instr: Instr) -> Result<()> {
        let index = self.stack_index_unchecked(instr.a());
        if matches!(self.state.stack[index], RuntimeVal::Nil) {
            self.pc = self.relative_pc(instr.sbx() as i32)?;
        } else {
            self.pc += 1;
        }
        Ok(())
    }

    pub(super) fn dispatch_br_not_nil(&mut self, instr: Instr) -> Result<()> {
        let index = self.stack_index_unchecked(instr.a());
        if !matches!(self.state.stack[index], RuntimeVal::Nil) {
            self.pc = self.relative_pc(instr.sbx() as i32)?;
        } else {
            self.pc += 1;
        }
        Ok(())
    }

    #[inline]
    pub(super) fn dispatch_compare_test(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let value = self.compare_test_value(instr)?;
        if self.collect_metrics {
            record_branch_op_known_enabled(true);
        }
        if let Some(fact) = function.performance.compare_test_branch(self.pc) {
            if value == (instr.c() != 0) {
                self.pc = fact.target_pc;
            } else {
                self.pc += 2;
            }
            return Ok(());
        }
        let jmp_pc = self.pc + 1;
        let jmp = *function
            .code
            .get(jmp_pc)
            .ok_or_else(|| anyhow!("compare-test at pc {} missing Jmp", self.pc))?;
        if jmp.opcode() != Opcode::Jmp {
            bail!("compare-test at pc {} expected Jmp at pc {jmp_pc}", self.pc);
        }
        if value == (instr.c() != 0) {
            self.pc = self.relative_pc_from(jmp_pc, jmp.sj_arg())?;
        } else {
            self.pc += 2;
        }
        Ok(())
    }

    #[inline]
    fn compare_test_value(&self, instr: Instr) -> Result<bool> {
        let lhs_idx = self.stack_index_unchecked(instr.a());
        let rhs_idx = self.stack_index_unchecked(instr.b());
        if let (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) = (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
            Ok(match instr.opcode() {
                Opcode::TestEqInt => lhs == rhs,
                Opcode::TestNeInt => lhs != rhs,
                Opcode::TestLtInt => lhs < rhs,
                Opcode::TestLeInt => lhs <= rhs,
                Opcode::TestGtInt => lhs > rhs,
                Opcode::TestGeInt => lhs >= rhs,
                _ => unreachable!("opcode matched by caller"),
            })
        } else {
            self.compare_test_value_slow(instr, lhs_idx, rhs_idx)
        }
    }

    #[cold]
    #[inline(never)]
    fn compare_test_value_slow(&self, instr: Instr, lhs_idx: usize, rhs_idx: usize) -> Result<bool> {
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

    pub(super) fn dispatch_jmp(&mut self, instr: Instr) -> Result<()> {
        self.pc = self.relative_pc(instr.sj_arg())?;
        Ok(())
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

    pub(super) fn dispatch_call(
        &mut self,
        function: &Function,
        module: Option<&Module>,
        instr: Instr,
        ctx: &mut Option<&mut VmContext>,
        collect_metrics: bool,
    ) -> Result<()> {
        self.collect_pending_garbage();
        if collect_metrics {
            record_call_op_known_enabled(VmCallMetric::Generic);
        }
        let call_fact = self.call_fact_from_static_cache_or_instr(function, instr, false);
        let window = CallWindow::new(RegisterIndex::new(call_fact.call_base), call_fact.positional_count, 1);
        let call_pc = self.pc;
        let value = self.call_function(module, window, Some(call_fact.target_kind), ctx)?;
        if self.pc != call_pc {
            return Ok(());
        }
        self.clear_call_window_temps(window, 0)?;
        self.write_returns(window, [value])?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_call_named(
        &mut self,
        function: &Function,
        module: Option<&Module>,
        instr: Instr,
        ctx: &mut Option<&mut VmContext>,
        collect_metrics: bool,
    ) -> Result<()> {
        self.collect_pending_garbage();
        if collect_metrics {
            record_call_op_known_enabled(VmCallMetric::Named);
        }
        let call_fact = self.call_fact_from_static_cache_or_instr(function, instr, true);
        let window = CallWindow::new(RegisterIndex::new(call_fact.call_base), call_fact.positional_count, 1);
        let call_pc = self.pc;
        let value =
            self.call_function_named(module, window, call_fact.named_count, Some(call_fact.target_kind), ctx)?;
        if self.pc != call_pc {
            return Ok(());
        }
        self.clear_call_window_temps(window, call_fact.named_count)?;
        self.write_returns(window, [value])?;
        self.pc += 1;
        Ok(())
    }

    #[cold]
    pub(super) fn dispatch_set_global(&mut self, function: &Function, instr: Instr) -> Result<()> {
        let global_fact = function.performance.global_op(self.pc).copied();
        let value = if global_fact.is_some_and(|fact| fact.move_source) {
            self.take(instr.a())?
        } else {
            self.read(instr.a())?.clone()
        };
        let slot = self.global_slot_from_fact_cache_or_instr(function, instr);
        self.write_global(slot, value)?;
        self.pc += 1;
        Ok(())
    }
}
