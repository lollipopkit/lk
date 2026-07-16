use super::*;

impl Compiler {
    pub(super) fn lower_access(&mut self, target: &Expr, key: &Expr) -> Result<u16> {
        let dst = self.alloc_reg();
        self.lower_access_to_register(dst, target, key)?;
        Ok(dst)
    }

    pub(super) fn lower_access_to_register(&mut self, dst: u16, target: &Expr, key: &Expr) -> Result<()> {
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
    pub(super) fn lower_unary(&mut self, op: &UnaryOp, inner: &Expr) -> Result<u16> {
        let src = self.lower_readonly_operand(inner)?;
        let dst = self.alloc_reg();
        let opcode = match op {
            UnaryOp::Not => Opcode::Not,
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
                if let Some((opcode, value, immediate)) = self.lower_mod_zero_i4_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_i4_branch_placeholder(opcode, value, immediate)?]);
                }
                if let Some((opcode, value)) = self.lower_zero_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_branch_placeholder(opcode, value)?]);
                }
                if let Some((opcode, value, immediate)) = self.lower_i4_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_i4_branch_placeholder(opcode, value, immediate)?]);
                }
                if ENABLE_COMPARE_TEST_IMMEDIATE_LOWERING
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
