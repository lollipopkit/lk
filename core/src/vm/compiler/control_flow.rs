use super::*;

impl Compiler {
    pub(super) fn lower_if(&mut self, condition: &Expr, then_stmt: &Stmt, else_stmt: Option<&Stmt>) -> Result<()> {
        if self.try_lower_min_max_if(condition, then_stmt, else_stmt)? {
            return Ok(());
        }
        let watermark = self.next_reg;
        let false_jumps = self.emit_condition_false_jumps(condition)?;

        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(then_stmt)?;
        self.local_rebind_suppression -= 1;
        let then_returns = self.emitted_return;
        self.next_reg = watermark; // recycle registers from then-branch

        if let Some(else_stmt) = else_stmt {
            let jmp_end = (!then_returns).then(|| self.emit_jmp_placeholder());
            let else_start = self.function.code.len();
            self.patch_condition_false_jumps(false_jumps, else_start)?;

            self.emitted_return = false;
            self.local_rebind_suppression += 1;
            self.lower_stmt(else_stmt)?;
            self.local_rebind_suppression -= 1;
            let else_returns = self.emitted_return;
            self.next_reg = watermark; // recycle registers from else-branch

            if let Some(jmp_end) = jmp_end {
                let end = self.function.code.len();
                self.patch_jmp(jmp_end, end)?;
            }
            self.emitted_return = then_returns && else_returns;
        } else {
            let end = self.function.code.len();
            self.patch_condition_false_jumps(false_jumps, end)?;
            self.emitted_return = false;
        }

        Ok(())
    }

    pub(super) fn try_lower_min_max_if(
        &mut self,
        condition: &Expr,
        then_stmt: &Stmt,
        else_stmt: Option<&Stmt>,
    ) -> Result<bool> {
        if else_stmt.is_some() {
            return Ok(false);
        }
        let Some((name, value)) = single_assign_stmt(then_stmt) else {
            return Ok(false);
        };
        if self.cell_locals.contains(name) {
            return Ok(false);
        }
        let Some(dst) = self.locals.get(name).copied() else {
            return Ok(false);
        };
        if self.function.performance.value_kind(dst) != PerfValueKind::Int {
            return Ok(false);
        }
        let Some(opcode) = min_max_update_opcode(condition, name, value) else {
            return Ok(false);
        };
        let candidate = self.lower_readonly_operand(value)?;
        if self.function.performance.value_kind(candidate) != PerfValueKind::Int {
            return Ok(false);
        }
        self.emit(Instr::abc(
            opcode,
            checked_u8("min/max dst", dst)?,
            checked_u8("min/max current", dst)?,
            checked_u8("min/max candidate", candidate)?,
        ));
        self.set_register_kind(dst, PerfValueKind::Int);
        self.emitted_return = false;
        Ok(true)
    }

    /// Promotes every local a closure inside the loop captures to a cell
    /// *now*, before any loop code is emitted. A promotion emitted mid-body
    /// re-executes each iteration (re-boxing an outer variable and orphaning
    /// the shared cell) and leaves earlier-emitted reads — the condition and
    /// increment, re-executed on the back edge — reading the raw register
    /// that meanwhile holds the cell.
    pub(super) fn pre_promote_loop_captures(&mut self, condition: Option<&Expr>, body: &Stmt) -> Result<()> {
        let mut captured = Vec::new();
        if let Some(condition) = condition {
            collect_expr_closure_captures(condition, &mut captured);
        }
        collect_stmt_closure_captures(body, &mut captured);
        for name in captured {
            // Inside the loop body the pattern names lexically bind the loop
            // variables, so a name-level skip is exact here.
            if self.loop_snapshot_vars.iter().any(|v| v.name == name) {
                continue;
            }
            self.promote_captured_local(&name)?;
        }
        Ok(())
    }

    /// Promotes `name` (if it is a plain local) to a capture cell right now:
    /// box the current value and re-bind the register to the cell.
    pub(super) fn promote_captured_local(&mut self, name: &str) -> Result<()> {
        let Some(local) = self.locals.get(name).copied() else {
            return Ok(());
        };
        if self.cell_locals.insert(name.to_string()) {
            let cell = self.emit_upval_cell(local)?;
            self.emit_move(local, cell, "box captured local")?;
        }
        Ok(())
    }

    pub(super) fn lower_while(&mut self, condition: &Expr, body: &Stmt) -> Result<()> {
        self.pre_promote_loop_captures(Some(condition), body)?;
        let watermark = self.next_reg;
        self.begin_loop_scalar_const_scope(condition, body)?;
        let condition_start = self.function.code.len();
        let false_jumps = self.emit_condition_false_jumps(condition)?;
        // Scalar constant loads in the condition can run once before the first
        // iteration; loop-back jumps resume at the first real condition op.
        let condition_end = self.function.code.len();
        let loop_start = self.function.code[condition_start..condition_end]
            .iter()
            .enumerate()
            .find_map(|(i, instr)| {
                if !instr.opcode().is_scalar_const_load() {
                    Some(condition_start + i)
                } else {
                    None
                }
            })
            .unwrap_or(condition_start);

        self.loops.push(LoopPatch::default());
        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(body)?;
        self.local_rebind_suppression -= 1;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");
        if !self.emitted_return {
            let jmp_back = self.emit_jmp_placeholder();
            self.patch_jmp(jmp_back, loop_start)?;
        }

        let end = self.function.code.len();
        self.patch_condition_false_jumps(false_jumps, end)?;
        for pc in loop_patch.breaks {
            self.patch_jmp(pc, end)?;
        }
        for pc in loop_patch.continues {
            self.patch_jmp(pc, loop_start)?;
        }
        self.emitted_return = false;
        self.end_loop_scalar_const_scope();
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    pub(super) fn lower_for(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> Result<()> {
        // The pattern names register *before* the body prescan: inside the
        // body they lexically refer to the loop variables, so the prescan
        // must not pre-promote a same-named outer local.
        let snapshot_mark = self.loop_snapshot_vars.len();
        collect_for_pattern_names(pattern, &mut self.loop_snapshot_vars);
        let result = self
            .pre_promote_loop_captures(None, body)
            .and_then(|()| self.lower_for_dispatch(pattern, iterable, body));
        self.loop_snapshot_vars.truncate(snapshot_mark);
        result
    }

    pub(super) fn lower_for_dispatch(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> Result<()> {
        match iterable {
            Expr::Range {
                start,
                end,
                inclusive,
                step,
            } => self.lower_for_range(
                pattern,
                start.as_deref(),
                end.as_deref(),
                *inclusive,
                step.as_deref(),
                body,
            ),
            iterable => self.lower_for_indexed(pattern, iterable, body),
        }
    }

    pub(super) fn lower_for_indexed(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> Result<()> {
        let watermark = self.next_reg;
        let iterable_value = self.lower_readonly_access_target(iterable)?;
        let iterable_kind = self.function.performance.value_kind(iterable_value);
        let direct_iterable = matches!(iterable_kind, PerfValueKind::List | PerfValueKind::String);
        let iterable = if direct_iterable {
            iterable_value
        } else {
            let iterable = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::ToIter,
                checked_u8("for indexed iter dst", iterable)?,
                checked_u8("for indexed iter src", iterable_value)?,
                0,
            ));
            self.set_register_kind(iterable, PerfValueKind::List);
            iterable
        };
        let len = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::Len,
            checked_u8("for indexed len dst", len)?,
            checked_u8("for indexed iterable", iterable)?,
            0,
        ));
        self.set_register_kind(len, PerfValueKind::Int);
        let index = self.lower_val(&LiteralVal::Int(0))?;
        let step = self.lower_val(&LiteralVal::Int(1))?;
        let skip_value_load = matches!(iterable_kind, PerfValueKind::String)
            && matches!(pattern, ForPattern::Variable(name) if !stmt_uses_for_binding_value(body, name) && !stmt_shadows_name_deep(body, name));
        let value = if skip_value_load { step } else { self.alloc_reg() };

        let loop_start = self.function.code.len();
        let condition = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::CmpLtInt,
            checked_u8("for indexed condition dst", condition)?,
            checked_u8("for indexed index", index)?,
            checked_u8("for indexed len", len)?,
        ));
        let exit_test = self.emit_test_placeholder(condition)?;
        if !skip_value_load {
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("for indexed value", value)?,
                checked_u8("for indexed iterable", iterable)?,
                checked_u8("for indexed index", index)?,
            ));
            if let Some(fact) = index_fact_from_target(&self.function.performance, iterable) {
                let pc = self.function.code.len() - 1;
                self.function.performance.set_index_fact(pc, fact);
            }
        }
        let previous_binding = self.bind_for_pattern(pattern, value)?;
        let previous_single_char_locals = self.single_char_string_locals.clone();
        if matches!(iterable_kind, PerfValueKind::String)
            && let ForPattern::Variable(name) = pattern
        {
            self.single_char_string_locals.insert(name.clone(), step);
        }

        self.loops.push(LoopPatch::default());
        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(body)?;
        self.local_rebind_suppression -= 1;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");

        let step_start = self.function.code.len();
        if !self.emitted_return {
            self.emit_bin_op_to_register(index, &BinOp::Add, index, step)?;
            let jmp_back = self.emit_jmp_placeholder();
            self.patch_jmp(jmp_back, loop_start)?;
        }

        let loop_end = self.function.code.len();
        self.patch_test_false_jump(exit_test, loop_end)?;
        for pc in loop_patch.breaks {
            self.patch_jmp(pc, loop_end)?;
        }
        for pc in loop_patch.continues {
            self.patch_jmp(pc, step_start)?;
        }
        self.restore_for_pattern(previous_binding);
        self.single_char_string_locals = previous_single_char_locals;
        self.emitted_return = false;
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    pub(super) fn lower_for_range(
        &mut self,
        pattern: &ForPattern,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        step: Option<&Expr>,
        body: &Stmt,
    ) -> Result<()> {
        let watermark = self.next_reg;
        self.begin_loop_scalar_const_scope_for_exprs(&[], body)?;
        let step_sign = range_step_sign(step);
        let index = self.alloc_reg();
        match start {
            Some(start) => self.lower_expr_to_register(index, start, "for range initial index")?,
            None => self.emit_literal_to_register(index, &LiteralVal::Int(0))?,
        }
        let end = end.ok_or_else(|| anyhow!("Compiler open-ended range for loop is not supported"))?;
        let body_mutations = mutated_names_in_stmt(body);
        let end = self.lower_loop_snapshot_operand(end, &body_mutations)?;

        let step = match step {
            Some(step) => self.lower_loop_snapshot_operand(step, &body_mutations)?,
            None => self.lower_val(&LiteralVal::Int(1))?,
        };

        let previous_binding = self.bind_for_pattern(pattern, index)?;

        match step_sign {
            RangeStepSign::Positive => self.lower_for_range_static_loop(index, end, step, inclusive, true, body)?,
            RangeStepSign::Negative => self.lower_for_range_static_loop(index, end, step, inclusive, false, body)?,
            RangeStepSign::Dynamic => self.lower_for_range_dynamic_loop(index, end, step, inclusive, body)?,
        }

        self.restore_for_pattern(previous_binding);
        self.emitted_return = false;
        self.end_loop_scalar_const_scope();
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    pub(super) fn bind_for_pattern(&mut self, pattern: &ForPattern, value: u16) -> Result<Vec<ForPatternBinding>> {
        let mut previous = Vec::new();
        self.bind_for_pattern_inner(pattern, value, &mut previous)?;
        Ok(previous)
    }

    /// A loop binding is fresh (never a cell), so binding clears any stale
    /// cell mark of a same-named outer local; the restore re-instates both
    /// the previous slot and its mark.
    pub(super) fn bind_for_name(&mut self, name: &str, value: u16, previous: &mut Vec<ForPatternBinding>) {
        let was_cell = self.cell_locals.contains(name);
        previous.push(ForPatternBinding {
            name: name.to_string(),
            slot: self.insert_fresh_local(name.to_string(), value),
            was_cell,
        });
        // Fill the innermost pending snapshot entry: captures and re-`let`s
        // recognize the loop variable by this slot, not by name alone.
        if let Some(entry) = self
            .loop_snapshot_vars
            .iter_mut()
            .rev()
            .find(|entry| entry.name == name && entry.slot.is_none())
        {
            entry.slot = Some(value);
        }
    }

    /// The binding slot of the innermost enclosing `for` variable named
    /// `name`, if that loop has already bound its pattern.
    pub(super) fn active_loop_binding_slot(&self, name: &str) -> Option<u16> {
        self.loop_snapshot_vars
            .iter()
            .rev()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.slot)
    }

    pub(super) fn bind_for_pattern_inner(
        &mut self,
        pattern: &ForPattern,
        value: u16,
        previous: &mut Vec<ForPatternBinding>,
    ) -> Result<()> {
        match pattern {
            ForPattern::Variable(name) => {
                self.bind_for_name(name, value, previous);
                Ok(())
            }
            ForPattern::Ignore => Ok(()),
            ForPattern::Tuple(patterns) => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_for_sequence_pattern(patterns, value, previous)
            }
            ForPattern::Array { patterns, rest: None } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_for_sequence_pattern(patterns, value, previous)
            }
            ForPattern::Array {
                patterns,
                rest: Some(rest),
            } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_for_sequence_pattern(patterns, value, previous)?;
                let start = self.lower_val(&LiteralVal::Int(patterns.len() as i64))?;
                let slice = self.alloc_reg();
                self.emit(Instr::abc(
                    Opcode::SliceFrom,
                    checked_u8("for rest slice", slice)?,
                    checked_u8("for rest value", value)?,
                    checked_u8("for rest start", start)?,
                ));
                self.bind_for_name(rest, slice, previous);
                Ok(())
            }
            ForPattern::Object(entries) => {
                let condition =
                    self.lower_map_pattern_key_condition(value, entries.iter().map(|(key, _)| key.as_str()))?;
                self.emit_pattern_assert(condition)?;
                for (key, pattern) in entries {
                    let key = self.lower_val(&LiteralVal::from_str(key))?;
                    let field = self.alloc_reg();
                    self.emit(Instr::abc(
                        Opcode::GetIndex,
                        checked_u8("for object field", field)?,
                        checked_u8("for object value", value)?,
                        checked_u8("for object key", key)?,
                    ));
                    self.bind_for_pattern_inner(pattern, field, previous)?;
                }
                Ok(())
            }
        }
    }

    pub(super) fn bind_for_sequence_pattern(
        &mut self,
        patterns: &[ForPattern],
        value: u16,
        previous: &mut Vec<ForPatternBinding>,
    ) -> Result<()> {
        for (index, pattern) in patterns.iter().enumerate() {
            let index = i64::try_from(index).map_err(|_| anyhow!("Compiler for pattern index overflow"))?;
            let key = self.lower_val(&LiteralVal::Int(index))?;
            let field = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("for sequence field", field)?,
                checked_u8("for sequence value", value)?,
                checked_u8("for sequence index", key)?,
            ));
            self.bind_for_pattern_inner(pattern, field, previous)?;
        }
        Ok(())
    }

    pub(super) fn restore_for_pattern(&mut self, previous: Vec<ForPatternBinding>) {
        for binding in previous.into_iter().rev() {
            if let Some(old) = binding.slot {
                self.insert_local(binding.name.clone(), old);
            } else {
                self.locals.remove(&binding.name);
            }
            if binding.was_cell {
                self.cell_locals.insert(binding.name);
            }
        }
    }

    pub(super) fn lower_break(&mut self) -> Result<()> {
        let pc = self.emit_jmp_placeholder();
        let Some(loop_patch) = self.loops.last_mut() else {
            bail!("break statement outside of loop");
        };
        loop_patch.breaks.push(pc);
        Ok(())
    }

    pub(super) fn lower_continue(&mut self) -> Result<()> {
        let pc = self.emit_jmp_placeholder();
        let Some(loop_patch) = self.loops.last_mut() else {
            bail!("continue statement outside of loop");
        };
        loop_patch.continues.push(pc);
        Ok(())
    }
}
