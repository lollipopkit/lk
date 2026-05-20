use super::*;

impl<'a> FunctionTranslator<'a> {
    #[inline]
    fn int_cmp_op(kind: crate::vm::IntCmpKind) -> &'static str {
        match kind {
            crate::vm::IntCmpKind::Eq => "eq",
            crate::vm::IntCmpKind::Ne => "ne",
            crate::vm::IntCmpKind::Lt => "slt",
            crate::vm::IntCmpKind::Le => "sle",
            crate::vm::IntCmpKind::Gt => "sgt",
            crate::vm::IntCmpKind::Ge => "sge",
        }
    }

    pub(super) fn emit_int_compare_kind(
        &mut self,
        dst: u16,
        a: u16,
        b: u16,
        kind: crate::vm::IntCmpKind,
    ) -> Result<()> {
        self.emit_compare(dst, a, b, Self::int_cmp_op(kind))
    }

    pub(super) fn emit_cmp_int_jmp(
        &mut self,
        block_idx: usize,
        instr_idx: usize,
        a: u16,
        b: u16,
        kind: crate::vm::IntCmpKind,
        ofs: i16,
    ) -> Result<()> {
        let target = Self::compute_target(instr_idx, ofs, self.function.code.len())?;
        let target_label = self.block_label_for_index(target)?;
        let fallthrough = self
            .blocks
            .get(block_idx + 1)
            .map(|block| block.label.clone())
            .unwrap_or_else(|| DEFAULT_RETURN_LABEL.to_string());
        let lhs = self.load_reg(a)?;
        let rhs = self.load_reg(b)?;
        let lhs_is_sentinel = self.fresh("cmpint_lhs_sentinel");
        self.writer.line(format!(
            "{lhs_is_sentinel} = icmp sle i64 {lhs}, {sentinel_max}",
            sentinel_max = encoding::BOOL_TRUE_VALUE
        ));
        let rhs_is_sentinel = self.fresh("cmpint_rhs_sentinel");
        self.writer.line(format!(
            "{rhs_is_sentinel} = icmp sle i64 {rhs}, {sentinel_max}",
            sentinel_max = encoding::BOOL_TRUE_VALUE
        ));
        let any_sentinel = self.fresh("cmpint_any_sentinel");
        self.writer
            .line(format!("{any_sentinel} = or i1 {lhs_is_sentinel}, {rhs_is_sentinel}"));
        let cmp = self.fresh("cmpint");
        self.writer
            .line(format!("{cmp} = icmp {} i64 {lhs}, {rhs}", Self::int_cmp_op(kind)));
        let not_sentinel = self.fresh("cmpint_not_sentinel");
        self.writer
            .line(format!("{not_sentinel} = xor i1 {any_sentinel}, true"));
        let is_int_match = self.fresh("cmpint_match");
        self.writer
            .line(format!("{is_int_match} = and i1 {cmp}, {not_sentinel}"));
        let should_jump = self.fresh("cmpint_jump");
        self.writer.line(format!("{should_jump} = xor i1 {is_int_match}, true"));
        self.writer.line(format!(
            "br i1 {should_jump}, label %{}, label %{}",
            target_label, fallthrough
        ));
        Ok(())
    }

    pub(super) fn emit_compare(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        if matches!(op, "eq" | "ne") {
            if self.compare_operand_is_nil(a) {
                let rhs = if self.compare_operand_is_nil(b) {
                    encoding::NIL_VALUE.to_string()
                } else {
                    self.load_rk(b)?
                };
                return self.emit_bool_compare(dst, &encoding::NIL_VALUE.to_string(), &rhs, op, "cmpnil");
            }
            if self.compare_operand_is_nil(b) {
                let lhs = self.load_rk(a)?;
                return self.emit_bool_compare(dst, &lhs, &encoding::NIL_VALUE.to_string(), op, "cmpnil");
            }
        }
        let lhs = self.load_rk(a)?;
        let rhs = self.load_rk(b)?;
        if self.operand_known_int(a) && self.operand_known_int(b) {
            return self.emit_bool_compare(dst, &lhs, &rhs, op, "cmpint");
        }
        let code = match op {
            "eq" => 0,
            "ne" => 1,
            "slt" => 2,
            "sle" => 3,
            "sgt" => 4,
            "sge" => 5,
            _ => return Err(anyhow!("unsupported LLVM compare op {op}")),
        };
        self.require_helper(RuntimeHelper::Compare);
        let select = self.fresh("cmpval");
        self.writer.line(format!(
            "{select} = call i64 @{}(i64 {lhs}, i64 {rhs}, i64 {code})",
            RuntimeHelper::Compare.symbol()
        ));
        self.store_reg(dst, &select)?;
        Ok(())
    }

    fn compare_operand_is_nil(&self, operand: u16) -> bool {
        rk_is_const(operand) && matches!(self.function.consts.get(rk_index(operand) as usize), Some(Val::Nil))
    }

    pub(super) fn emit_to_bool(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        let is_false = self.fresh("isfalse");
        self.writer.line(format!(
            "{is_false} = icmp eq i64 {value}, {false_val}",
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        let is_nil = self.fresh("isnil");
        self.writer.line(format!(
            "{is_nil} = icmp eq i64 {value}, {nil_val}",
            nil_val = encoding::NIL_VALUE
        ));
        let falsy = self.fresh("falsy");
        self.writer.line(format!("{falsy} = or i1 {is_false}, {is_nil}"));
        let result = self.fresh("tobool");
        self.writer.line(format!(
            "{result} = select i1 {falsy}, i64 {false_val}, i64 {true_val}",
            false_val = encoding::BOOL_FALSE_VALUE,
            true_val = encoding::BOOL_TRUE_VALUE
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    pub(super) fn emit_not(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        let is_false = self.fresh("not_is_false");
        self.writer.line(format!(
            "{is_false} = icmp eq i64 {value}, {false_val}",
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        let result = self.fresh("not");
        self.writer.line(format!(
            "{result} = select i1 {is_false}, i64 {true_val}, i64 {false_val}",
            true_val = encoding::BOOL_TRUE_VALUE,
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    pub(super) fn emit_bool_compare(&mut self, dst: u16, lhs: &str, rhs: &str, op: &str, prefix: &str) -> Result<()> {
        match op {
            "eq" | "ne" | "slt" | "sle" | "sgt" | "sge" => {}
            _ => return Err(anyhow!("unsupported LLVM compare op {op}")),
        }
        let cmp = self.fresh(prefix);
        self.writer.line(format!("{cmp} = icmp {op} i64 {lhs}, {rhs}"));
        let select = self.fresh("boolsel");
        self.writer.line(format!(
            "{select} = select i1 {cmp}, i64 {true_val}, i64 {false_val}",
            true_val = encoding::BOOL_TRUE_VALUE,
            false_val = encoding::BOOL_FALSE_VALUE
        ));
        self.store_reg(dst, &select)?;
        Ok(())
    }
}
