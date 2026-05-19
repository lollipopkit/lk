use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn try_defer_string_const_length(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
        text: &str,
    ) -> Result<bool> {
        if !self.string_length_can_defer(instr_idx, block_end, dst) {
            return Ok(false);
        }
        self.writer
            .line(format!("store i64 {}, i64* %r{dst}, align 8", encoding::NIL_VALUE));
        self.set_known(
            dst,
            Some(KnownReg::StringLength {
                len: text.chars().count().to_string(),
                ascii: text.is_ascii(),
            }),
        );
        Ok(true)
    }

    pub(super) fn try_defer_string_length(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
        a: u16,
        b: u16,
    ) -> Result<bool> {
        if !self.string_length_can_defer(instr_idx, block_end, dst) {
            return Ok(false);
        }
        let Some((lhs, lhs_is_string, lhs_ascii)) = self.string_length_part(a)? else {
            return Ok(false);
        };
        let Some((rhs, rhs_is_string, rhs_ascii)) = self.string_length_part(b)? else {
            return Ok(false);
        };
        if !lhs_is_string && !rhs_is_string {
            return Ok(false);
        }
        let len = self.fresh("strlenadd");
        self.writer.line(format!("{len} = add i64 {lhs}, {rhs}"));
        self.writer
            .line(format!("store i64 {}, i64* %r{dst}, align 8", encoding::NIL_VALUE));
        self.set_known(
            dst,
            Some(KnownReg::StringLength {
                len,
                ascii: lhs_ascii && rhs_ascii,
            }),
        );
        Ok(true)
    }

    fn string_length_can_defer(&self, instr_idx: usize, block_end: usize, dst: u16) -> bool {
        let mut alias = dst;
        let mut future_string_regs = BTreeSet::new();
        let mut future_int_regs = BTreeSet::new();
        let _ = block_end;
        for op in &self.function.code[instr_idx + 1..] {
            match *op {
                Op::Len { src, .. } if src == alias => return true,
                Op::LoadK(reg, kidx) if self.const_is_string_like(kidx) => {
                    future_string_regs.insert(reg);
                }
                Op::AddInt(dst, _, _)
                | Op::SubInt(dst, _, _)
                | Op::MulInt(dst, _, _)
                | Op::ModInt(dst, _, _)
                | Op::AddIntImm(dst, _, _)
                | Op::Len { dst, .. }
                | Op::Floor { dst, .. } => {
                    future_int_regs.insert(dst);
                }
                Op::ToStr(dst, src)
                    if self.string_length_operand_can_part(src, &future_string_regs, &future_int_regs) =>
                {
                    future_string_regs.insert(dst);
                }
                Op::Add(new_alias, a, b)
                    if a == alias && self.string_length_operand_can_part(b, &future_string_regs, &future_int_regs) =>
                {
                    alias = new_alias;
                }
                Op::Add(new_alias, a, b)
                    if b == alias && self.string_length_operand_can_part(a, &future_string_regs, &future_int_regs) =>
                {
                    alias = new_alias;
                }
                Op::Move(new_alias, src) | Op::LoadLocal(new_alias, src) | Op::StoreLocal(new_alias, src)
                    if src == alias =>
                {
                    alias = new_alias;
                }
                Op::ToIter { dst: new_alias, src } if src == alias => {
                    alias = new_alias;
                }
                Op::Move(dst, src) | Op::LoadLocal(dst, src) | Op::StoreLocal(dst, src)
                    if self.string_length_operand_is_string_part(src, &future_string_regs) =>
                {
                    future_string_regs.insert(dst);
                }
                Op::Move(dst, src) | Op::LoadLocal(dst, src) | Op::StoreLocal(dst, src)
                    if future_int_regs.contains(&src) =>
                {
                    future_int_regs.insert(dst);
                }
                _ if string_length_op_reads_reg(op, alias) || string_length_op_writes_reg(op, alias) => {
                    return false;
                }
                _ if string_length_op_stops_scan(op) => return false,
                _ => {}
            }
        }
        false
    }

    fn string_length_part(&mut self, operand: u16) -> Result<Option<(String, bool, bool)>> {
        if rk_is_const(operand) {
            let Some(value) = self.function.consts.get(rk_index(operand) as usize) else {
                return Ok(None);
            };
            if let Some(text) = value.as_str() {
                return Ok(Some((text.chars().count().to_string(), true, text.is_ascii())));
            }
            if matches!(value, Val::Int(_) | Val::Bool(_) | Val::Nil) {
                let raw = encoding::encode_immediate(value)?.to_string();
                return Ok(Some((self.emit_int_decimal_len_expr(&raw), false, true)));
            }
            return Ok(None);
        }
        match self.known(operand).cloned() {
            Some(KnownReg::StringHandle { text, len, .. }) => Ok(Some((len.to_string(), true, text.is_ascii()))),
            Some(KnownReg::StringLength { len, ascii }) => Ok(Some((len, true, ascii))),
            _ if self.operand_known_int(operand) => {
                let value = self.load_rk(operand)?;
                Ok(Some((self.emit_int_decimal_len_expr(&value), false, true)))
            }
            _ => Ok(None),
        }
    }

    fn string_length_operand_can_part(
        &self,
        operand: u16,
        future_string_regs: &BTreeSet<u16>,
        future_int_regs: &BTreeSet<u16>,
    ) -> bool {
        if rk_is_const(operand) {
            return self.const_is_string_like(rk_index(operand) as u16);
        }
        if future_string_regs.contains(&operand) {
            return true;
        }
        if future_int_regs.contains(&operand) {
            return true;
        }
        matches!(
            self.known(operand),
            Some(KnownReg::StringHandle { .. }) | Some(KnownReg::StringLength { .. })
        ) || self.operand_known_int(operand)
            || register_is_string_constant_source(self.function, operand)
    }

    fn string_length_operand_is_string_part(&self, operand: u16, future_string_regs: &BTreeSet<u16>) -> bool {
        if rk_is_const(operand) {
            return self
                .function
                .consts
                .get(rk_index(operand) as usize)
                .is_some_and(|value| value.as_str().is_some());
        }
        future_string_regs.contains(&operand)
            || matches!(
                self.known(operand),
                Some(KnownReg::StringHandle { .. }) | Some(KnownReg::StringLength { .. })
            )
            || register_is_string_constant_source(self.function, operand)
    }

    fn const_is_string_like(&self, kidx: u16) -> bool {
        self.function
            .consts
            .get(kidx as usize)
            .is_some_and(|value| value.as_str().is_some() || matches!(value, Val::Int(_) | Val::Bool(_) | Val::Nil))
    }

    pub(super) fn emit_int_decimal_len_expr(&mut self, value: &str) -> String {
        self.require_helper(RuntimeHelper::IntDecimalLen);
        let len = self.fresh("intstrlen");
        self.writer.line(format!(
            "{len} = call i64 @{}(i64 {value})",
            RuntimeHelper::IntDecimalLen.symbol()
        ));
        len
    }
}

fn string_length_op_stops_scan(op: &Op) -> bool {
    matches!(
        op,
        Op::Jmp(_)
            | Op::JmpFalse(_, _)
            | Op::BoolBranch(_, _)
            | Op::CmpLtImmJmp { .. }
            | Op::JmpNilOrFalseJmp { .. }
            | Op::AddIntImmJmp { .. }
            | Op::AddRangeCountImm { .. }
            | Op::CmpLeImmJmp { .. }
            | Op::CmpNeImmJmp { .. }
            | Op::Break(_)
            | Op::Continue(_)
            | Op::ForRangePrep { .. }
            | Op::ForRangeLoop { .. }
            | Op::RangeLoopI { .. }
            | Op::ForRangeStep { .. }
            | Op::Ret { .. }
    )
}

fn string_length_op_reads_reg(op: &Op, reg: u16) -> bool {
    match *op {
        Op::Move(_, src)
        | Op::StoreLocal(_, src)
        | Op::LoadLocal(_, src)
        | Op::Not(_, src)
        | Op::ToStr(_, src)
        | Op::ToBool(_, src)
        | Op::Len { src, .. }
        | Op::Floor { src, .. }
        | Op::StartsWithK(_, src, _)
        | Op::ContainsK(_, src, _)
        | Op::JmpFalse(src, _)
        | Op::BoolBranch(src, _)
        | Op::JmpIfNil(src, _)
        | Op::JmpIfNotNil(src, _) => src == reg,
        Op::Add(_, a, b)
        | Op::StrConcatKnownCap(_, a, b)
        | Op::StrConcatToStr(_, a, b)
        | Op::Sub(_, a, b)
        | Op::Mul(_, a, b)
        | Op::Div(_, a, b)
        | Op::Mod(_, a, b)
        | Op::AddInt(_, a, b)
        | Op::SubInt(_, a, b)
        | Op::MulInt(_, a, b)
        | Op::ModInt(_, a, b)
        | Op::AddFloat(_, a, b)
        | Op::SubFloat(_, a, b)
        | Op::MulFloat(_, a, b)
        | Op::DivFloat(_, a, b)
        | Op::ModFloat(_, a, b)
        | Op::CmpEq(_, a, b)
        | Op::CmpNe(_, a, b)
        | Op::CmpLt(_, a, b)
        | Op::CmpLe(_, a, b)
        | Op::CmpGt(_, a, b)
        | Op::CmpGe(_, a, b)
        | Op::CmpI { a, b, .. }
        | Op::In(_, a, b)
        | Op::Access(_, a, b)
        | Op::Index { base: a, idx: b, .. } => a == reg || b == reg,
        Op::AddIntImm(_, src, _)
        | Op::CmpEqImm(_, src, _)
        | Op::CmpNeImm(_, src, _)
        | Op::CmpLtImm(_, src, _)
        | Op::CmpLeImm(_, src, _)
        | Op::CmpGtImm(_, src, _)
        | Op::CmpGeImm(_, src, _)
        | Op::CmpLtImmJmp { r: src, .. }
        | Op::AddIntImmJmp { r: src, .. }
        | Op::AccessK(_, src, _)
        | Op::IndexK(_, src, _) => src == reg,
        Op::ListPush { list, val } => list == reg || val == reg,
        Op::MapSet { map, key, val } | Op::MapSetMove { map, key, val } => map == reg || key == reg || val == reg,
        Op::Ret { base, retc } => retc > 0 && base == reg,
        _ => false,
    }
}

fn string_length_op_writes_reg(op: &Op, reg: u16) -> bool {
    match *op {
        Op::LoadK(dst, _)
        | Op::Move(dst, _)
        | Op::StoreLocal(dst, _)
        | Op::LoadLocal(dst, _)
        | Op::Not(dst, _)
        | Op::ToStr(dst, _)
        | Op::ToBool(dst, _)
        | Op::Add(dst, _, _)
        | Op::StrConcatKnownCap(dst, _, _)
        | Op::StrConcatToStr(dst, _, _)
        | Op::Sub(dst, _, _)
        | Op::Mul(dst, _, _)
        | Op::Div(dst, _, _)
        | Op::Mod(dst, _, _)
        | Op::AddInt(dst, _, _)
        | Op::SubInt(dst, _, _)
        | Op::MulInt(dst, _, _)
        | Op::ModInt(dst, _, _)
        | Op::AddFloat(dst, _, _)
        | Op::SubFloat(dst, _, _)
        | Op::MulFloat(dst, _, _)
        | Op::DivFloat(dst, _, _)
        | Op::ModFloat(dst, _, _)
        | Op::CmpEq(dst, _, _)
        | Op::CmpNe(dst, _, _)
        | Op::CmpLt(dst, _, _)
        | Op::CmpLe(dst, _, _)
        | Op::CmpGt(dst, _, _)
        | Op::CmpGe(dst, _, _)
        | Op::CmpI { dst, .. }
        | Op::In(dst, _, _)
        | Op::Access(dst, _, _)
        | Op::AccessK(dst, _, _)
        | Op::Index { dst, .. }
        | Op::IndexK(dst, _, _)
        | Op::Len { dst, .. }
        | Op::Floor { dst, .. }
        | Op::StartsWithK(dst, _, _)
        | Op::ContainsK(dst, _, _)
        | Op::BuildMap { dst, .. }
        | Op::BuildList { dst, .. }
        | Op::MakeClosure { dst, .. } => dst == reg,
        Op::NullishPick { dst, .. } | Op::JmpFalseSet { dst, .. } | Op::JmpTrueSet { dst, .. } => dst == reg,
        _ => false,
    }
}
