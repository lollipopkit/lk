use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn try_defer_string_int_key_fact(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
    ) -> Result<bool> {
        let Some(fact) = self
            .function
            .analysis
            .as_ref()
            .and_then(|analysis| analysis.perf.key_ops.get(instr_idx))
            .and_then(Option::as_ref)
            .and_then(|fact| fact.string_int)
        else {
            return Ok(false);
        };
        if !self.string_int_key_can_defer(instr_idx, block_end, dst) {
            return Ok(false);
        }
        let Some(prefix) = self
            .function
            .consts
            .get(fact.prefix_key as usize)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
        else {
            return Ok(false);
        };
        let suffix = self.load_reg(fact.suffix_reg)?;
        self.writer
            .line(format!("store i64 {}, i64* %r{dst}, align 8", encoding::NIL_VALUE));
        self.set_known(dst, Some(KnownReg::StringIntKey { prefix, suffix }));
        Ok(true)
    }

    pub(super) fn try_defer_string_int_key(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
        a: u16,
        b: u16,
    ) -> Result<bool> {
        let Some((prefix, suffix_operand)) = self.string_int_key_parts(a, b) else {
            return Ok(false);
        };
        if !self.string_int_key_can_defer(instr_idx, block_end, dst) {
            return Ok(false);
        }
        let suffix = self.load_rk(suffix_operand)?;
        self.writer
            .line(format!("store i64 {}, i64* %r{dst}, align 8", encoding::NIL_VALUE));
        self.set_known(dst, Some(KnownReg::StringIntKey { prefix, suffix }));
        Ok(true)
    }

    pub(super) fn try_defer_str_concat_to_str_key(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
        lhs: u16,
        src: u16,
    ) -> Result<bool> {
        if !self.operand_known_int(src) {
            return Ok(false);
        }
        let Some(KnownReg::StringHandle { text, .. }) = self.known(lhs).cloned() else {
            return Ok(false);
        };
        if !self.string_int_key_can_defer(instr_idx, block_end, dst) {
            return Ok(false);
        }
        let suffix = self.load_reg(src)?;
        self.writer
            .line(format!("store i64 {}, i64* %r{dst}, align 8", encoding::NIL_VALUE));
        self.set_known(dst, Some(KnownReg::StringIntKey { prefix: text, suffix }));
        Ok(true)
    }

    pub(super) fn materialize_string_int_key(&mut self, reg: u16) -> Result<Option<String>> {
        let Some(KnownReg::StringIntKey { prefix, suffix }) = self.known(reg).cloned() else {
            return Ok(None);
        };
        let const_data = self.make_string_constant(&prefix);
        self.anonymous_string_constants.push(const_data.clone());
        let ptr = self.emit_string_pointer(&const_data);
        self.require_helper(RuntimeHelper::StringIntKey);
        let result = self.fresh("strintkey");
        self.writer.line(format!(
            "{result} = call i64 @{}(i8* {ptr}, i64 {len}, i64 {suffix})",
            RuntimeHelper::StringIntKey.symbol(),
            len = const_data.len
        ));
        Ok(Some(result))
    }

    fn string_int_key_parts(&self, a: u16, b: u16) -> Option<(String, u16)> {
        if !rk_is_const(a)
            && self.operand_known_int(b)
            && let Some(KnownReg::StringHandle { text, .. }) = self.known(a)
        {
            return Some((text.clone(), b));
        }
        if !rk_is_const(b)
            && self.operand_known_int(a)
            && let Some(KnownReg::StringHandle { text, .. }) = self.known(b)
        {
            return Some((text.clone(), a));
        }
        None
    }

    fn string_int_key_can_defer(&self, instr_idx: usize, block_end: usize, dst: u16) -> bool {
        let mut alias = dst;
        let mut consumed = false;
        for op in &self.function.code[instr_idx + 1..block_end] {
            match *op {
                Op::Access(_, _, field) if field == alias => consumed = true,
                Op::MapGetDynamic(_, _, key) if key == alias => consumed = true,
                Op::MapHas(_, _, key) if key == alias => consumed = true,
                Op::MapSet { key, .. } | Op::MapSetMove { key, .. } if key == alias => consumed = true,
                Op::ListPush { val, .. } | Op::ListPushMove { val, .. } if val == alias => consumed = true,
                Op::BuildList { base, len, .. } if reg_in_range(alias, base, len) => consumed = true,
                Op::Call { base, argc, .. }
                | Op::CallExact { base, argc, .. }
                | Op::CallClosureExact { base, argc, .. }
                | Op::CallNativeFast { base, argc, .. }
                    if reg_in_range(alias, base, argc.into()) =>
                {
                    consumed = true
                }
                Op::Move(new_alias, src) | Op::LoadLocal(new_alias, src) | Op::StoreLocal(new_alias, src)
                    if src == alias =>
                {
                    alias = new_alias;
                }
                _ if string_key_op_reads_reg(op, alias) => return false,
                _ if string_key_op_writes_reg(op, alias) => return consumed,
                _ => {}
            }
        }
        consumed && !self.string_key_alias_read_after_block(block_end, alias)
    }

    fn string_key_alias_read_after_block(&self, block_end: usize, mut alias: u16) -> bool {
        for op in &self.function.code[block_end..] {
            match *op {
                Op::Access(_, _, field)
                | Op::MapGetDynamic(_, _, field)
                | Op::MapHas(_, _, field)
                | Op::MapSet { key: field, .. }
                | Op::MapSetMove { key: field, .. }
                | Op::ListPush { val: field, .. }
                | Op::ListPushMove { val: field, .. }
                    if field == alias => {}
                Op::Move(new_alias, src) | Op::LoadLocal(new_alias, src) | Op::StoreLocal(new_alias, src)
                    if src == alias =>
                {
                    alias = new_alias;
                }
                _ if string_key_op_reads_reg(op, alias) => return true,
                _ if string_key_op_writes_reg(op, alias) => return false,
                _ => {}
            }
        }
        false
    }
}

fn reg_in_range(reg: u16, base: u16, len: u16) -> bool {
    reg >= base && reg < base.saturating_add(len)
}

fn string_key_op_reads_reg(op: &Op, reg: u16) -> bool {
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
        | Op::JmpIfNotNil(src, _)
        | Op::FloorDivImm { src, .. } => src == reg,
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
        | Op::CmpIntJmp { a, b, .. }
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
        | Op::CmpLeImmJmp { r: src, .. }
        | Op::CmpEqImmJmp { r: src, .. }
        | Op::CmpGtImmJmp { r: src, .. }
        | Op::CmpGeImmJmp { r: src, .. }
        | Op::CmpNeImmJmp { r: src, .. }
        | Op::AddIntImmJmp { r: src, .. }
        | Op::AccessK(_, src, _)
        | Op::IndexK(_, src, _) => src == reg,
        Op::ListPush { list, val } | Op::ListPushMove { list, val } => list == reg || val == reg,
        Op::MapSet { map, key, val } | Op::MapSetMove { map, key, val } => map == reg || key == reg || val == reg,
        Op::CallMethod0 { receiver, .. } => receiver == reg,
        Op::CallGlobalMethod0 { .. } => false,
        Op::Ret { base, retc } => retc > 0 && base == reg,
        _ => false,
    }
}

fn string_key_op_writes_reg(op: &Op, reg: u16) -> bool {
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
        | Op::FloorDivImm { dst, .. }
        | Op::StartsWithK(dst, _, _)
        | Op::ContainsK(dst, _, _)
        | Op::BuildMap { dst, .. }
        | Op::BuildList { dst, .. }
        | Op::MakeClosure { dst, .. } => dst == reg,
        Op::CallMethod0 { dst, .. } => dst == reg,
        Op::CallGlobalMethod0 { dst, .. } => dst == reg,
        Op::NullishPick { dst, .. } | Op::JmpFalseSet { dst, .. } | Op::JmpTrueSet { dst, .. } => dst == reg,
        _ => false,
    }
}
