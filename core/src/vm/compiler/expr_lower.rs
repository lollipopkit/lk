use super::*;

impl Compiler {
    pub(super) fn lower_access(&mut self, target: &Expr, key: &Expr) -> Result<u16> {
        let dst = self.alloc_reg();
        self.lower_access_to_register(dst, target, key)?;
        Ok(dst)
    }

    pub(super) fn lower_access_to_register(&mut self, dst: u16, target: &Expr, key: &Expr) -> Result<()> {
        // A field's declared width, onto the register it lands in.
        //
        // The binary paths ask `machine_regs` about *registers*, not about the
        // expression that filled them, so knowing `r.value` is a `u32` is only
        // useful once it is written down here. Without it `r.value + 1` on a
        // `u32` field added at 64 bits and answered 4294967296.
        //
        // Recorded before the access lowers rather than after: the lowering
        // below has several returns, and one of them is a fused opcode.
        match self.access_register_width_of(target, key) {
            Some(width) => {
                self.machine_regs.insert(dst, width);
            }
            None => {
                self.machine_regs.remove(&dst);
            }
        }
        self.lower_access_to_register_inner(dst, target, key)
    }

    fn lower_access_to_register_inner(&mut self, dst: u16, target: &Expr, key: &Expr) -> Result<()> {
        let target = self.lower_readonly_access_target(target)?;
        let index_fact = index_fact_from_target(&self.function.performance, target);
        if let Some((suffix, key_fact)) = self.try_lower_string_int_key_for_map(index_fact, key)? {
            let pc = self.function.code.len();
            self.emit(Instr::abc(
                Opcode::GetIndexStrI,
                checked_u8("string-int index dst", dst)?,
                checked_u8("string-int index target", target)?,
                checked_u8("string-int index suffix", suffix)?,
            ));
            self.function.performance.set_key_fact(pc, key_fact);
            self.function.performance.clear_register(dst);
            if let Some(fact) = index_fact {
                self.function.performance.set_index_fact(pc, fact);
            }
            return Ok(());
        }
        let (key, key_fact) = self.lower_index_key_for_target(target, index_fact, key)?;
        let pc = self.function.code.len();
        if list_int_key(index_fact, &self.function.performance, key) {
            self.emit(Instr::abc(
                Opcode::GetList,
                checked_u8("list get dst", dst)?,
                checked_u8("list get target", target)?,
                checked_u8("list get key", key)?,
            ));
        } else if let Some(const_key) = get_field_key(index_fact, key_fact) {
            self.emit(Instr::abc(
                Opcode::GetFieldK,
                checked_u8("field dst", dst)?,
                checked_u8("field target", target)?,
                checked_u8("field key", const_key)?,
            ));
        } else {
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("index dst", dst)?,
                checked_u8("index target", target)?,
                checked_u8("index key", key)?,
            ));
            if let Some(fact) = key_fact {
                self.function.performance.set_key_fact(pc, fact);
            }
        }
        self.function.performance.clear_register(dst);
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        Ok(())
    }

    pub(super) fn lower_readonly_access_target(&mut self, target: &Expr) -> Result<u16> {
        if let Expr::Var(name) = target
            && let Some(local) = self.locals.get(name).copied()
            && !self.cell_locals.contains(name)
        {
            return Ok(local);
        }
        self.lower_expr(target)
    }

    pub(super) fn lower_index_key_for_target(
        &mut self,
        target: u16,
        index_fact: Option<crate::vm::analysis::PerfIndexFact>,
        key: &Expr,
    ) -> Result<(u16, Option<crate::vm::analysis::PerfKeyFact>)> {
        if let Some(text) = short_string_literal_key(key) {
            let const_key = self.push_string(text)?;
            let key_fact = Some(crate::vm::analysis::PerfKeyFact {
                const_key: Some(const_key),
                string_int: None,
            });
            if index_fact.is_some_and(|fact| {
                matches!(
                    fact.target_kind,
                    crate::vm::analysis::PerfIndexTargetKind::Map | crate::vm::analysis::PerfIndexTargetKind::Object
                )
            }) {
                return Ok((target, key_fact));
            }
            let dst = self.alloc_reg();
            self.emit(Instr::abx(Opcode::LoadString, checked_u8("index key", dst)?, const_key));
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok((dst, key_fact));
        }
        Ok((self.lower_readonly_operand(key)?, None))
    }

    pub(super) fn try_lower_string_int_key_for_map(
        &mut self,
        index_fact: Option<crate::vm::analysis::PerfIndexFact>,
        key: &Expr,
    ) -> Result<Option<(u16, PerfKeyFact)>> {
        if !index_fact.is_some_and(|fact| fact.target_kind == crate::vm::analysis::PerfIndexTargetKind::Map) {
            return Ok(None);
        }
        let Some((prefix, suffix_expr)) = string_int_template_key(key) else {
            return Ok(None);
        };
        if !string_int_key_suffix_is_int_like(suffix_expr, &self.locals, &self.function.performance) {
            return Ok(None);
        }
        let suffix = self.lower_readonly_operand(suffix_expr)?;
        if self.function.performance.value_kind(suffix) != PerfValueKind::Int {
            return Ok(None);
        }
        let prefix_key = self.push_string(prefix)?;
        Ok(Some((
            suffix,
            PerfKeyFact {
                const_key: None,
                string_int: Some(PerfStringIntKeyFact {
                    prefix_key,
                    suffix_reg: suffix,
                }),
            },
        )))
    }

    pub(super) fn lower_optional_access(&mut self, target: &Expr, key: &Expr) -> Result<u16> {
        let target = self.lower_readonly_access_target(target)?;
        let dst = self.alloc_reg();
        self.emit(Instr::abc(Opcode::LoadNil, checked_u8("optional dst", dst)?, 0, 0));

        let is_nil = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::IsNil,
            checked_u8("optional test dst", is_nil)?,
            checked_u8("optional target", target)?,
            0,
        ));
        let skip_get = self.emit_test_placeholder(is_nil)?;

        let index_fact = index_fact_from_target(&self.function.performance, target);
        let (key, key_fact) = self.lower_index_key_for_target(target, index_fact, key)?;
        let pc = self.function.code.len();
        if list_int_key(index_fact, &self.function.performance, key) {
            self.emit(Instr::abc(
                Opcode::GetList,
                checked_u8("optional list dst", dst)?,
                checked_u8("optional list target", target)?,
                checked_u8("optional list key", key)?,
            ));
        } else if let Some(const_key) = get_field_key(index_fact, key_fact) {
            self.emit(Instr::abc(
                Opcode::GetFieldK,
                checked_u8("optional field dst", dst)?,
                checked_u8("optional field target", target)?,
                checked_u8("optional field key", const_key)?,
            ));
        } else {
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("optional get dst", dst)?,
                checked_u8("optional get target", target)?,
                checked_u8("optional get key", key)?,
            ));
            if let Some(fact) = key_fact {
                self.function.performance.set_key_fact(pc, fact);
            }
        }
        self.function.performance.clear_register(dst);
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        let end = self.function.code.len();
        self.patch_test_true_jump(skip_get, end)?;
        Ok(dst)
    }

    /// `yield expr`: lower the value into a *fresh* register (never an
    /// existing local's own slot — `Yield` overwrites it in place with the
    /// resumed value, and aliasing a local would silently clobber it across
    /// the suspend point) and emit the single-register in/out `Yield` opcode.
    /// The register's static-type fact must be reset: after resuming, it can
    /// hold any type, not whatever `inner` produced.
    /// `expr as T`.
    ///
    /// The target is static, so it rides in the instruction's `C` byte rather
    /// than costing a constant-pool load. The type checker has already rejected
    /// targets that are not scalar, so an unencodable one here is a compiler
    /// bug rather than a user error.
    pub(super) fn lower_cast(&mut self, inner: &Expr, ty: &crate::val::Type) -> Result<u16> {
        // A pointer *is* an address, so converting to one is a type-system
        // event with no runtime content — the bits are already right.
        //
        // TODO(32-bit targets): on a 32-bit deployment target a pointer is
        // narrower than the `i64` carrier, so this will need the same
        // truncation a `u32` gets. Harmless while both backends are 64-bit,
        // and wrong the moment the AOT path cross-compiles to thumb/arm32 —
        // which it cannot: Cranelift's backend set here has no 32-bit target
        // (`no_32_bit_target_is_reachable_yet` in lk-aot-codegen fails when
        // that stops being true, and names this site).
        //
        // Note this is the *compiler*, so it cannot follow the target even in
        // principle: bytecode is target-agnostic, and the triple only appears
        // at `lk compile object:<triple>`. A truncation here would have to
        // become one the VM performs at run time, as `truncate_to_width`
        // already does for `isize`/`usize`.
        if matches!(ty, crate::val::Type::Ptr { .. }) {
            let src = self.lower_readonly_operand(inner)?;
            // The result is an address, not a machine integer of some width:
            // arithmetic on it must not inherit the operand's wrap.
            self.machine_regs.remove(&src);
            return Ok(src);
        }
        // `u64 as Float` reads the carrier as unsigned.
        //
        // The last conversion in this family. A `u64` with bit 63 set is a
        // negative `i64` carrier, and unlike a comparison or a divide the result
        // does not *look* wrong until it is compared with zero.
        if matches!(ty, crate::val::Type::Float)
            && let Some(kind) = self.expr_machine_width(inner)
            && matches!(kind, crate::val::IntKind::U64 | crate::val::IntKind::Usize)
        {
            let call = Expr::Call(
                alloc::string::String::from("__lk_u64_to_float"),
                alloc::vec![Box::new(inner.clone())],
            );
            return self.lower_expr(&call);
        }
        let Some(target) = crate::vm::ir::CastTarget::from_type(ty) else {
            anyhow::bail!("internal error: cast target {} reached lowering", ty.display());
        };
        let src = self.lower_readonly_operand(inner)?;
        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::CastTo,
            checked_u8("cast dst", dst)?,
            checked_u8("cast src", src)?,
            target as u8,
        ));
        // A freshly allocated register carries no static-type fact, so there
        // is nothing to invalidate — same as `lower_unary`. The width, though,
        // is worth remembering: it is how arithmetic downstream knows to wrap.
        self.note_machine_reg(dst, Some(ty));
        Ok(dst)
    }

    pub(super) fn lower_unary(&mut self, op: &UnaryOp, inner: &Expr) -> Result<u16> {
        let src = self.lower_readonly_operand(inner)?;
        let dst = self.alloc_reg();
        let opcode = match op {
            UnaryOp::Not => Opcode::Not,
            UnaryOp::Neg => Opcode::Neg,
        };
        self.emit(Instr::abc(
            opcode,
            checked_u8("unary dst", dst)?,
            checked_u8("unary src", src)?,
            0,
        ));
        Ok(dst)
    }

    pub(super) fn lower_short_circuit(&mut self, lhs: &Expr, rhs: &Expr, kind: ShortCircuitKind) -> Result<u16> {
        let lhs = self.lower_readonly_operand(lhs)?;
        let dst = self.alloc_reg();
        let move_source = !self.is_current_local_slot(lhs);
        self.emit_move_with_policy(dst, lhs, "short circuit lhs", move_source)?;

        let test_reg = match kind {
            ShortCircuitKind::And | ShortCircuitKind::Or => dst,
            ShortCircuitKind::Nullish => {
                let is_nil = self.alloc_reg();
                self.emit(Instr::abc(
                    Opcode::IsNil,
                    checked_u8("nullish test dst", is_nil)?,
                    checked_u8("nullish lhs", dst)?,
                    0,
                ));
                is_nil
            }
        };

        let test_pc = self.emit_test_placeholder(test_reg)?;
        match kind {
            ShortCircuitKind::And | ShortCircuitKind::Nullish => {
                self.lower_expr_to_register(dst, rhs, "short circuit rhs")?;
                let end = self.function.code.len();
                self.patch_test_false_jump(test_pc, end)?;
            }
            ShortCircuitKind::Or => {
                self.lower_expr_to_register(dst, rhs, "short circuit rhs")?;
                let end = self.function.code.len();
                self.patch_test_true_jump(test_pc, end)?;
            }
        }
        Ok(dst)
    }

    pub(super) fn emit_condition_false_jumps(&mut self, condition: &Expr) -> Result<Vec<usize>> {
        match condition {
            Expr::And(lhs, rhs) => {
                if ENABLE_COMPARE_TEST_PAIR_IMMEDIATE_LOWERING
                    && let Some(pc) = self.try_emit_compare_test_pair_immediate_placeholder(lhs, rhs)?
                {
                    return Ok(vec![pc]);
                }
                let mut jumps = self.emit_condition_false_jumps(lhs)?;
                jumps.extend(self.emit_condition_false_jumps(rhs)?);
                Ok(jumps)
            }
            Expr::Or(lhs, rhs) => {
                let lhs = self.lower_readonly_operand(lhs)?;
                let skip_rhs = self.emit_test_placeholder(lhs)?;
                let jumps = self.emit_condition_false_jumps(rhs)?;
                let end = self.function.code.len();
                self.patch_test_true_jump(skip_rhs, end)?;
                Ok(jumps)
            }
            Expr::Bin(lhs, BinOp::Eq, rhs) if expr_is_nil_literal(lhs) => {
                let value = self.lower_readonly_operand(rhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNotNil, value)?])
            }
            Expr::Bin(lhs, BinOp::Eq, rhs) if expr_is_nil_literal(rhs) => {
                let value = self.lower_readonly_operand(lhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNotNil, value)?])
            }
            Expr::Bin(lhs, BinOp::Ne, rhs) if expr_is_nil_literal(lhs) => {
                let value = self.lower_readonly_operand(rhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNil, value)?])
            }
            Expr::Bin(lhs, BinOp::Ne, rhs) if expr_is_nil_literal(rhs) => {
                let value = self.lower_readonly_operand(lhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNil, value)?])
            }
            Expr::Bin(lhs, op, rhs) if compare_test_opcode(op).is_some() => {
                // A `u64` comparison is unsigned, and the fused compare-branch
                // opcodes below are not.
                //
                // This is the *third* path the same rewrite has to reach:
                // `lower_bin` for a comparison producing a value, the
                // lower-into-register path for one feeding a call argument, and
                // this one for a condition. Each was found by a test the
                // previous fix left failing — `println(a < b)` after
                // `let c = a / b`, and `if (a > b)` after both.
                if let Some(value) = self.lower_unsigned_bin(lhs, op, rhs)? {
                    return Ok(vec![self.emit_branch_placeholder(Opcode::BrFalse, value)?]);
                }
                // Each fused shape below lowers an operand to decide, and leaves
                // those instructions behind when it declines — so they are only
                // tried over operands that are free to lower twice. See
                // [`Self::is_free_to_lower_twice`]: `if (a > b)` and
                // `if (x % 2 == 0)` still fuse, `if (1 + f(x) > 0)` no longer
                // calls `f` three times.
                let speculate = Self::is_free_to_lower_twice(lhs) && Self::is_free_to_lower_twice(rhs);
                if speculate
                    && let Some((opcode, value, immediate)) = self.lower_mod_zero_i4_branch_operands(lhs, op, rhs)?
                {
                    return Ok(vec![self.emit_i4_branch_placeholder(opcode, value, immediate)?]);
                }
                if speculate && let Some((opcode, value)) = self.lower_zero_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_branch_placeholder(opcode, value)?]);
                }
                if speculate && let Some((opcode, value, immediate)) = self.lower_i4_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_i4_branch_placeholder(opcode, value, immediate)?]);
                }
                if speculate
                    && ENABLE_COMPARE_TEST_IMMEDIATE_LOWERING
                    && let Some((opcode, lhs, rhs)) = self.lower_compare_test_immediate_operands(lhs, op, rhs)?
                {
                    return Ok(vec![
                        self.emit_compare_test_immediate_placeholder(opcode, lhs, rhs, false)?,
                    ]);
                }
                let lhs = self.lower_readonly_operand(lhs)?;
                let rhs = self.lower_readonly_operand(rhs)?;
                if ENABLE_COMPARE_TEST_LOWERING && compare_test_operands_are_int(&self.function.performance, lhs, rhs) {
                    let opcode = compare_test_opcode(op).expect("checked compare-test opcode");
                    return Ok(vec![self.emit_compare_test_placeholder(opcode, lhs, rhs, false)?]);
                }
                let dst = self.alloc_reg();
                let condition = self.emit_bin_op_to_register(dst, op, lhs, rhs)?;
                Ok(vec![self.emit_test_placeholder(condition)?])
            }
            _ => {
                let condition = self.lower_readonly_operand(condition)?;
                Ok(vec![self.emit_test_placeholder(condition)?])
            }
        }
    }

    pub(super) fn try_emit_compare_test_pair_immediate_placeholder(
        &mut self,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<Option<usize>> {
        let Some((first_name, first_value)) = equality_u4_local_immediate(lhs) else {
            return Ok(None);
        };
        let Some((second_name, second_value)) = equality_u4_local_immediate(rhs) else {
            return Ok(None);
        };
        let Some(first_reg) = self.locals.get(first_name).copied() else {
            return Ok(None);
        };
        let Some(second_reg) = self.locals.get(second_name).copied() else {
            return Ok(None);
        };
        if self.cell_locals.contains(first_name)
            || self.cell_locals.contains(second_name)
            || self.function.performance.value_kind(first_reg) != PerfValueKind::Int
            || self.function.performance.value_kind(second_reg) != PerfValueKind::Int
        {
            return Ok(None);
        }
        self.emit_compare_test_pair_immediate_placeholder(first_reg, first_value, second_reg, second_value)
            .map(Some)
    }

    /// Whether lowering this expression twice is observably the same as once.
    ///
    /// The fused compare-and-branch shapes below cannot decide without a
    /// register fact (`value_kind`), so each lowers its operand and *then* asks —
    /// and a helper that declines answers `None` with its instructions already in
    /// the stream. The next attempt lowers the expression again, so an operand
    /// with a side effect runs once per attempt that looked and declined:
    /// `if (1 + f(x) > 0)` called `f` three times. Worse, the attempts do not
    /// even agree on *which* subexpression is the operand — the `%`-against-zero
    /// form takes `x` out of `x % k` while the next form takes `x % k` whole — so
    /// there is no single register to hand along.
    ///
    /// What makes the speculation sound is this: only speculate over operands
    /// that are free to lower twice. A name, a literal, and arithmetic over them
    /// re-lower to at most a dead `Move`/`LoadInt` on the path that declines,
    /// which is what that path already costs; a call re-lowers to a *call*.
    ///
    /// Deliberately a whitelist. A new `Expr` variant is not free until someone
    /// says it is, and the cost of being wrong here is a program that runs its
    /// operand twice — the exact bug this exists to prevent.
    fn is_free_to_lower_twice(expr: &Expr) -> bool {
        match expr {
            Expr::Var(_) | Expr::Literal(_) => true,
            Expr::Paren(inner) | Expr::Unsafe(inner) | Expr::Cast(inner, _) => Self::is_free_to_lower_twice(inner),
            Expr::Unary(_, inner) => Self::is_free_to_lower_twice(inner),
            Expr::Bin(lhs, _, rhs) => Self::is_free_to_lower_twice(lhs) && Self::is_free_to_lower_twice(rhs),
            _ => false,
        }
    }

    pub(super) fn lower_compare_test_immediate_operands(
        &mut self,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<Option<(Opcode, u16, i8)>> {
        if let Some(immediate) = compare_test_immediate_operand(rhs) {
            let lhs = self.lower_readonly_operand(lhs)?;
            if self.function.performance.value_kind(lhs) == PerfValueKind::Int {
                return Ok(compare_test_immediate_opcode(op).map(|opcode| (opcode, lhs, immediate)));
            }
            return Ok(None);
        }
        if let Some(immediate) = compare_test_immediate_operand(lhs) {
            let rhs = self.lower_readonly_operand(rhs)?;
            if self.function.performance.value_kind(rhs) == PerfValueKind::Int {
                return Ok(reverse_compare_test_immediate_opcode(op).map(|opcode| (opcode, rhs, immediate)));
            }
        }
        Ok(None)
    }

    pub(super) fn lower_zero_branch_operands(
        &mut self,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<Option<(Opcode, u16)>> {
        let value_expr = if zero_int_literal(rhs) {
            lhs
        } else if zero_int_literal(lhs) {
            rhs
        } else {
            return Ok(None);
        };
        let value = self.lower_readonly_operand(value_expr)?;
        if self.function.performance.value_kind(value) != PerfValueKind::Int {
            return Ok(None);
        }
        let opcode = match op {
            BinOp::Eq => Opcode::BrNeZeroInt,
            BinOp::Ne => Opcode::BrEqZeroInt,
            _ => return Ok(None),
        };
        Ok(Some((opcode, value)))
    }

    pub(super) fn lower_mod_zero_i4_branch_operands(
        &mut self,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<Option<(Opcode, u16, u8)>> {
        let Some((value_expr, divisor)) = mod_i4_zero_operands(lhs, rhs) else {
            return Ok(None);
        };
        let value = self.lower_readonly_operand(value_expr)?;
        if self.function.performance.value_kind(value) != PerfValueKind::Int {
            return Ok(None);
        }
        let opcode = match op {
            BinOp::Eq => Opcode::BrModNeZeroIntI4,
            BinOp::Ne => Opcode::BrModEqZeroIntI4,
            _ => return Ok(None),
        };
        Ok(Some((opcode, value, divisor)))
    }

    pub(super) fn lower_i4_branch_operands(
        &mut self,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<Option<(Opcode, u16, u8)>> {
        let (value_expr, immediate) = if let Some(immediate) = u4_literal(rhs) {
            (lhs, immediate)
        } else if let Some(immediate) = u4_literal(lhs) {
            (rhs, immediate)
        } else {
            return Ok(None);
        };
        let value = self.lower_readonly_operand(value_expr)?;
        if self.function.performance.value_kind(value) != PerfValueKind::Int {
            return Ok(None);
        }
        let opcode = match op {
            BinOp::Eq => Opcode::BrNeIntI4,
            BinOp::Ne => Opcode::BrEqIntI4,
            _ => return Ok(None),
        };
        Ok(Some((opcode, value, immediate)))
    }

    pub(super) fn patch_condition_false_jumps(&mut self, jumps: Vec<usize>, target: usize) -> Result<()> {
        for jump in jumps {
            match self.function.code.get(jump).copied().map(Instr::opcode) {
                Some(
                    Opcode::BrNil
                    | Opcode::BrNotNil
                    | Opcode::BrFalse
                    | Opcode::BrTrue
                    | Opcode::BrEqZeroInt
                    | Opcode::BrNeZeroInt,
                ) => {
                    self.patch_branch(jump, target)?;
                }
                Some(Opcode::BrEqIntI4 | Opcode::BrNeIntI4 | Opcode::BrModEqZeroIntI4 | Opcode::BrModNeZeroIntI4) => {
                    self.patch_i4_branch(jump, target)?
                }
                Some(opcode) if opcode.is_compare_test() => self.patch_compare_test_jump(jump, target)?,
                _ => self.patch_test_false_jump(jump, target)?,
            }
        }
        Ok(())
    }
}
