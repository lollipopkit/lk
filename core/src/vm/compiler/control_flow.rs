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

    /// Lowers `try { body } catch e { handler }` into a protected region:
    ///
    /// ```text
    ///     TryBegin catch_reg, →HANDLER
    ///     <body>
    ///     TryEnd
    ///     Jmp →END
    /// HANDLER:
    ///     <handler>          ; the caught value is already in catch_reg
    /// END:
    /// ```
    ///
    /// The opcodes have been in the executor (and the bytecode verifier) all
    /// along; nothing emitted them, because try/catch was rewritten in the
    /// parser into `try$call(|| body)` instead. Emitting them is what makes
    /// `return` inside the body return from *this* function — with a closure in
    /// the way it returned from the closure, silently — and what removes the
    /// cell-capture of outer locals that made a top-level `try` writing an outer
    /// variable fail at runtime.
    /// `try { body } catch e { handler }` used for effect — no value register,
    /// so the region is exactly what it was before `try` became an expression.
    pub(super) fn lower_try_stmt(&mut self, body: &[Box<Stmt>], catch_var: &str, handler: &[Box<Stmt>]) -> Result<()> {
        self.lower_try_region(body, catch_var, handler, None)
    }

    /// `try { body } catch e { handler }`, producing a value: each half's tail
    /// expression lands in one shared register, which is what makes
    /// `let r = try { … } catch e { … }` work. In statement position the
    /// register is simply never read. The register is allocated *before* the region opens, so both the
    /// body's writes and the handler's are visible after it closes — and it is
    /// the same shape the AOT's `written_registers` scan already looks for, so
    /// native lowering needs to know nothing new about it.
    pub(super) fn lower_try_expr(&mut self, body: &[Box<Stmt>], catch_var: &str, handler: &[Box<Stmt>]) -> Result<u16> {
        // Only when a half actually ends in an expression. A `try` used as a
        // statement — both halves ending in `;` — then emits exactly the code
        // it always did, with no register reserved and nothing written before
        // the region opens.
        let needs_value = ends_in_expression(body) || ends_in_expression(handler);
        let value_reg = if needs_value {
            let reg = self.alloc_reg();
            self.emit(Instr::abc(Opcode::LoadNil, checked_u8("try value", reg)?, 0, 0));
            Some(reg)
        } else {
            None
        };
        self.lower_try_region(body, catch_var, handler, value_reg)?;
        match value_reg {
            Some(reg) => Ok(reg),
            // Neither half has a value, so the answer is nil — the same rule an
            // `if` branch that ends in a statement follows.
            None => {
                let reg = self.alloc_reg();
                self.emit(Instr::abc(Opcode::LoadNil, checked_u8("try value", reg)?, 0, 0));
                Ok(reg)
            }
        }
    }

    fn lower_try_region(
        &mut self,
        body: &[Box<Stmt>],
        catch_var: &str,
        handler: &[Box<Stmt>],
        value_reg: Option<u16>,
    ) -> Result<()> {
        // Allocated before the region opens: the handler reads it after the
        // body's registers have been recycled, so it must sit below them.
        let catch_reg = self.alloc_reg();
        let region = self.emit_try_begin_placeholder(catch_reg)?;

        let body_returns = self.lower_scoped_stmt_sequence_valued(body, catch_reg, value_reg)?;
        self.emit(Instr::ax(Opcode::TryEnd, 0));
        // A body that always returns never reaches the jump over the handler.
        let jmp_end = (!body_returns).then(|| self.emit_jmp_placeholder());

        let handler_start = self.function.code.len();
        self.patch_try_begin(region, handler_start)?;
        // The caught name is its own scope, and both halves of the restore
        // matter. `insert_fresh_local` (not `insert_local`) drops a cell mark
        // inherited from a same-named outer local it shadows — keeping it made
        // the handler read its plain string binding through `LoadCellVal`
        // ("expected UpvalCell, got String"). And the outer mark has to come
        // back afterwards, or the shadowed local reads as the raw cell object.
        let locals = self.locals.clone();
        let cell_locals = self.cell_locals.clone();
        let scopes = self.enter_scope();
        self.insert_fresh_local(catch_var.to_string(), catch_reg);
        let handler_returns = self.lower_scoped_stmt_sequence_valued(handler, catch_reg, value_reg)?;
        self.cell_locals = self.scope_restored_cell_locals(&locals, cell_locals);
        self.locals = locals;
        self.exit_scope(scopes);

        if let Some(jmp_end) = jmp_end {
            let end = self.function.code.len();
            self.patch_jmp(jmp_end, end)?;
        }
        // Only if *both* paths return does control never fall through.
        self.emitted_return = body_returns && handler_returns;
        Ok(())
    }

    /// Lowers `statements` as their own scope, restoring the enclosing bindings
    /// and register floor afterwards. Returns whether the sequence always
    /// returned. `keep_reg` stays allocated across the restore.
    ///
    /// When `value_reg` is given, the sequence's trailing
    /// expression is moved into it — the sequence's *value*, by the same rule a
    /// block expression uses. A sequence that ends in a statement leaves the
    /// register alone, so it keeps the nil it was initialized with; that is what
    /// `if` does for a branch that ends in a statement too.
    fn lower_scoped_stmt_sequence_valued(
        &mut self,
        statements: &[Box<Stmt>],
        keep_reg: u16,
        value_reg: Option<u16>,
    ) -> Result<bool> {
        let (statements, tail) = match (value_reg, statements.split_last()) {
            (Some(_), Some((last, leading))) => match last.as_ref() {
                Stmt::Expr(expr) => (leading, Some(expr.as_ref())),
                _ => (statements, None),
            },
            _ => (statements, None),
        };
        let locals = self.locals.clone();
        let cell_locals = self.cell_locals.clone();
        let const_map_locals = self.const_map_locals.clone();
        let scopes = self.enter_scope();
        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt_sequence(statements)?;
        if let (Some(value_reg), Some(tail)) = (value_reg, tail)
            && !self.emitted_return
        {
            // Straight into the value register, not through a scratch one.
            // A scratch register inside a protected region is a register the
            // native back end sees the body write, and registers are recycled
            // once the region ends — so the scratch collides with a *later*
            // region's body-local and the whole function stops lowering. This
            // is also what a hand-written `try { r = …; }` does, and it is why
            // that shape lowered when this one did not.
            self.lower_expr_to_register(value_reg, tail, "try value")?;
        }
        self.local_rebind_suppression -= 1;
        let returns = self.emitted_return;
        // Same restore as `Stmt::Block`: an in-region promotion of an *outer*
        // local must survive, or later reads load the raw cell object.
        self.cell_locals = self.scope_restored_cell_locals(&locals, cell_locals);
        self.locals = locals;
        self.const_map_locals = const_map_locals;
        self.exit_scope(scopes);
        if !returns {
            self.next_reg = self.live_register_floor().max(keep_reg + 1);
        }
        Ok(returns)
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
        // A zero step never advances the index, so the loop is either infinite
        // or empty depending on which comparison you write. `NewRange` and
        // `iter.range` both refuse it; a `for` header is the same absurdity and
        // gets the same answer, just earlier because the step is right there.
        if matches!(step_sign, RangeStepSign::Zero) {
            bail!("Range step cannot be zero");
        }
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
            RangeStepSign::Zero => unreachable!("a zero step is refused above"),
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

/// Whether a statement sequence ends in an expression — its *value*, by the
/// same rule a block expression uses.
fn ends_in_expression(statements: &[Box<Stmt>]) -> bool {
    matches!(statements.last().map(|stmt| stmt.as_ref()), Some(Stmt::Expr(_)))
}
