use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn emit_build_list(&mut self, dst: u16, base: u16, len: u16) -> Result<()> {
        if len == 0 {
            self.require_helper(RuntimeHelper::BuildList);
            let list = self.fresh("list");
            self.writer.line(format!(
                "{list} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildList.symbol()
            ));
            self.store_reg(dst, &list)?;
            self.set_known(dst, Some(KnownReg::List { base, len }));
            return Ok(());
        }

        let base_idx = base as usize;
        let len_usize = len as usize;
        if base_idx + len_usize > self.function.n_regs as usize {
            return Err(anyhow!("BuildList reads out of bounds registers"));
        }

        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("listbuf");
        self.writer
            .line(format!("{array} = alloca [{len} x i64], align 8", len = len));
        for i in 0..len_usize {
            let reg = base + i as u16;
            let value = match self.materialize_string_int_key(reg)? {
                Some(value) => value,
                None => self.load_reg(reg)?,
            };
            let slot = self.fresh("listelt");
            self.writer.line(format!(
                "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}",
                len = len,
                idx = i
            ));
            self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
        }

        let ptr = self.fresh("listptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0",
            len = len
        ));
        self.require_helper(RuntimeHelper::BuildList);
        let list = self.fresh("list");
        self.writer.line(format!(
            "{list} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildList.symbol(),
            len = len
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        self.store_reg(dst, &list)?;
        self.set_known(dst, Some(KnownReg::List { base, len }));
        Ok(())
    }

    pub(super) fn emit_list_push(&mut self, list: u16, val: u16) -> Result<()> {
        let list_value = self.load_reg(list)?;
        let item_value = match self.materialize_string_int_key(val)? {
            Some(value) => value,
            None => self.load_reg(val)?,
        };
        let helper = if self.operand_known_int(val) {
            RuntimeHelper::ListPushInt
        } else {
            RuntimeHelper::ListPush
        };
        self.require_helper(helper);
        let updated = self.fresh("listpush");
        self.writer.line(format!(
            "{updated} = call i64 @{}(i64 {list_value}, i64 {item_value})",
            helper.symbol()
        ));
        self.store_reg(list, &updated)?;
        self.set_known(list, None);
        Ok(())
    }

    pub(super) fn emit_build_map(&mut self, dst: u16, base: u16, len: u16) -> Result<()> {
        if len == 0 {
            self.require_helper(RuntimeHelper::BuildMap);
            let map = self.fresh("map");
            self.writer.line(format!(
                "{map} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildMap.symbol()
            ));
            self.store_reg(dst, &map)?;
            return Ok(());
        }

        let pair_count = len as usize;
        let base_idx = base as usize;
        if base_idx + pair_count * 2 > self.function.n_regs as usize {
            return Err(anyhow!("BuildMap reads out of bounds registers"));
        }

        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("mapbuf");
        let total = pair_count * 2;
        self.writer.line(format!("{array} = alloca [{total} x i64], align 8"));
        for i in 0..pair_count {
            let key = self.load_reg(base + (2 * i) as u16)?;
            let val = self.load_reg(base + (2 * i + 1) as u16)?;

            let key_slot = self.fresh("mapkey");
            self.writer.line(format!(
                "{key_slot} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 {idx}",
                total = total,
                idx = 2 * i
            ));
            self.writer.line(format!("store i64 {key}, i64* {key_slot}, align 8"));

            let val_slot = self.fresh("mapval");
            self.writer.line(format!(
                "{val_slot} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 {idx}",
                total = total,
                idx = 2 * i + 1
            ));
            self.writer.line(format!("store i64 {val}, i64* {val_slot}, align 8"));
        }

        let ptr = self.fresh("mapptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 0",
            total = total
        ));

        self.require_helper(RuntimeHelper::BuildMap);
        let map = self.fresh("map");
        self.writer.line(format!(
            "{map} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildMap.symbol(),
            len = pair_count
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        self.store_reg(dst, &map)?;
        Ok(())
    }

    pub(super) fn emit_map_set(&mut self, map: u16, key: u16, val: u16) -> Result<()> {
        if let Some(KnownReg::StringIntKey { prefix, suffix }) = self.known(key).cloned() {
            let map_value = self.load_reg(map)?;
            let val_value = self.load_reg(val)?;
            let const_data = self.make_string_constant(&prefix);
            self.anonymous_string_constants.push(const_data.clone());
            let ptr = self.emit_string_pointer(&const_data);
            self.require_helper(RuntimeHelper::MapSetStrInt);
            let updated = self.fresh("mapsetkey");
            self.writer.line(format!(
                "{updated} = call i64 @{}(i64 {map}, i8* {ptr}, i64 {len}, i64 {suffix}, i64 {value})",
                RuntimeHelper::MapSetStrInt.symbol(),
                map = map_value,
                len = const_data.len,
                value = val_value
            ));
            self.store_reg(map, &updated)?;
            self.set_known(map, None);
            return Ok(());
        }
        let map_value = self.load_reg(map)?;
        let key_value = self.load_reg(key)?;
        let val_value = self.load_reg(val)?;
        self.require_helper(RuntimeHelper::MapSet);
        let updated = self.fresh("mapset");
        self.writer.line(format!(
            "{updated} = call i64 @{}(i64 {map_value}, i64 {key_value}, i64 {val_value})",
            RuntimeHelper::MapSet.symbol()
        ));
        self.store_reg(map, &updated)?;
        self.set_known(map, None);
        Ok(())
    }

    pub(super) fn emit_map_set_const(&mut self, map: u16, kidx: u16, val: u16) -> Result<()> {
        let key = self
            .function
            .consts
            .get(kidx as usize)
            .and_then(Val::as_str)
            .ok_or_else(|| anyhow!("MapSetInterned expects string constant k{}", kidx))?
            .to_string();
        let map_value = self.load_reg(map)?;
        let key_value = self.intern_string_constant(kidx, &key)?;
        let val_value = self.load_reg(val)?;
        self.require_helper(RuntimeHelper::MapSet);
        let updated = self.fresh("mapsetk");
        self.writer.line(format!(
            "{updated} = call i64 @{}(i64 {map_value}, i64 {key_value}, i64 {val_value})",
            RuntimeHelper::MapSet.symbol()
        ));
        self.store_reg(map, &updated)?;
        self.set_known(map, None);
        Ok(())
    }

    pub(super) fn emit_map_has(&mut self, dst: u16, map: u16, key: u16) -> Result<()> {
        let map_value = self.load_reg(map)?;
        let key_value = match self.materialize_string_int_key(key)? {
            Some(value) => value,
            None => self.load_reg(key)?,
        };
        self.emit_map_has_with_key(dst, &map_value, &key_value)
    }

    pub(super) fn emit_map_has_const(&mut self, dst: u16, map: u16, kidx: u16) -> Result<()> {
        let key = self
            .function
            .consts
            .get(kidx as usize)
            .and_then(Val::as_str)
            .ok_or_else(|| anyhow!("MapHasK expects string constant k{}", kidx))?
            .to_string();
        let map_value = self.load_reg(map)?;
        let key_value = self.intern_string_constant(kidx, &key)?;
        self.emit_map_has_with_key(dst, &map_value, &key_value)
    }

    fn emit_map_has_with_key(&mut self, dst: u16, map_value: &str, key_value: &str) -> Result<()> {
        self.require_helper(RuntimeHelper::MapHas);
        let out = self.fresh("maphas");
        self.writer.line(format!(
            "{out} = call i64 @{}(i64 {map_value}, i64 {key_value})",
            RuntimeHelper::MapHas.symbol()
        ));
        self.store_reg(dst, &out)?;
        Ok(())
    }

    pub(super) fn emit_list_slice(&mut self, dst: u16, src: u16, start: u16) -> Result<()> {
        let list = self.load_reg(src)?;
        let start_idx = self.load_reg(start)?;
        self.require_helper(RuntimeHelper::ListSlice);
        let result = self.fresh("listslice");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {list}, i64 {start})",
            RuntimeHelper::ListSlice.symbol(),
            list = list,
            start = start_idx
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    pub(super) fn emit_access_or_defer_value(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
        base: u16,
        field: u16,
    ) -> Result<()> {
        if let Some(KnownReg::StringHandle { text, .. }) = self.known(field).cloned()
            && self.try_emit_const_map_access(dst, base, &text)?
        {
            return Ok(());
        }
        if self.try_emit_access_str_int(dst, base, field)? {
            return Ok(());
        }
        let base_val = self.load_reg(base)?;
        let key = self.load_reg(field)?;
        if self.access_result_can_defer(instr_idx, block_end, dst) {
            self.set_known(dst, Some(KnownReg::AccessedValue { base: base_val, key }));
            return Ok(());
        }
        self.emit_access_with_values(dst, base_val, key)
    }

    pub(super) fn emit_access(&mut self, dst: u16, base: u16, field: u16) -> Result<()> {
        if let Some(KnownReg::StringHandle { text, .. }) = self.known(field).cloned()
            && self.try_emit_const_map_access(dst, base, &text)?
        {
            return Ok(());
        }
        if self.try_emit_access_str_int(dst, base, field)? {
            return Ok(());
        }

        let base_val = self.load_reg(base)?;
        let key = self.load_reg(field)?;
        self.emit_access_with_values(dst, base_val, key)
    }

    fn emit_access_with_values(&mut self, dst: u16, base_val: String, key: String) -> Result<()> {
        self.require_helper(RuntimeHelper::Access);
        let result = self.fresh("access");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {key})",
            RuntimeHelper::Access.symbol(),
            base = base_val,
            key = key
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    fn access_result_can_defer(&self, instr_idx: usize, block_end: usize, dst: u16) -> bool {
        let mut alias = dst;
        let mut consumed = false;
        let scan_end = block_end.min(instr_idx + 8);
        for op in &self.function.code[instr_idx + 1..scan_end] {
            match *op {
                Op::Add(_, a, b) | Op::StrConcatKnownCap(_, a, b) | Op::StrConcatToStr(_, a, b) | Op::Sub(_, a, b)
                    if a == alias || b == alias =>
                {
                    consumed = true;
                }
                Op::Move(new_alias, src) | Op::LoadLocal(new_alias, src) | Op::StoreLocal(new_alias, src)
                    if src == alias =>
                {
                    if consumed {
                        return false;
                    }
                    alias = new_alias;
                }
                _ if op_reads_reg(op, alias) => return false,
                _ if op_writes_reg(op, alias) => return consumed,
                _ if !is_len_feed_neutral_op(op) => return consumed,
                _ => {}
            }
        }
        consumed
    }

    pub(super) fn emit_access_const(&mut self, dst: u16, base: u16, kidx: u16) -> Result<()> {
        let name = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let name_str = match name.as_str() {
            Some(s) => s.to_string(),
            None => return Err(anyhow!("AccessK expects string constant; found {:?}", name)),
        };
        if self.try_emit_const_map_access(dst, base, &name_str)? {
            return Ok(());
        }

        let key = self.intern_string_constant(kidx, &name_str)?;
        self.emit_access_with_key(dst, base, key.as_str())
    }

    fn try_emit_const_map_access(&mut self, dst: u16, base: u16, key: &str) -> Result<bool> {
        let Some(KnownReg::ConstMap { entries }) = self.known(base).cloned() else {
            return Ok(false);
        };
        let value = entries.get(key).cloned().unwrap_or(Val::Nil);
        let raw = self.emit_const_value(&value)?;
        self.store_reg(dst, &raw)?;
        self.set_known_const_value(dst, &value, &raw);
        Ok(true)
    }

    fn try_emit_access_str_int(&mut self, dst: u16, base: u16, field: u16) -> Result<bool> {
        let Some(KnownReg::StringIntKey { prefix, suffix }) = self.known(field).cloned() else {
            return Ok(false);
        };
        let base_val = self.load_reg(base)?;
        let const_data = self.make_string_constant(&prefix);
        self.anonymous_string_constants.push(const_data.clone());
        let ptr = self.emit_string_pointer(&const_data);
        self.require_helper(RuntimeHelper::AccessStrInt);
        let result = self.fresh("accesskey");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i8* {ptr}, i64 {len}, i64 {suffix})",
            RuntimeHelper::AccessStrInt.symbol(),
            base = base_val,
            len = const_data.len
        ));
        self.store_reg(dst, &result)?;
        Ok(true)
    }

    pub(super) fn emit_access_with_key(&mut self, dst: u16, base: u16, key: &str) -> Result<()> {
        let base_val = self.load_reg(base)?;
        self.require_helper(RuntimeHelper::Access);
        let result = self.fresh("access");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {key})",
            RuntimeHelper::Access.symbol(),
            base = base_val,
            key = key
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    pub(super) fn emit_index_or_defer_len(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
        base: u16,
        idx: u16,
    ) -> Result<()> {
        let base_val = self.load_reg(base)?;
        let index_val = self.load_reg(idx)?;
        if let Some(KnownReg::StringLength { len, ascii: true }) = self.known(base).cloned()
            && self.index_result_feeds_only_len(instr_idx, block_end, dst)
        {
            self.set_known(
                dst,
                Some(KnownReg::IndexedAsciiCharLength {
                    base_len: len,
                    index: index_val,
                }),
            );
            return Ok(());
        }
        if self.index_result_feeds_only_len(instr_idx, block_end, dst) {
            self.set_known(
                dst,
                Some(KnownReg::IndexedValue {
                    base: base_val,
                    index: index_val,
                }),
            );
            return Ok(());
        }
        self.emit_index_with_values(dst, base_val, index_val)
    }

    fn emit_index_with_values(&mut self, dst: u16, base_val: String, index_val: String) -> Result<()> {
        self.require_helper(RuntimeHelper::Index);
        let result = self.fresh("index");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {index})",
            RuntimeHelper::Index.symbol(),
            base = base_val,
            index = index_val
        ));
        self.store_reg(dst, &result)?;
        self.set_known(
            dst,
            Some(KnownReg::IndexedValue {
                base: base_val.clone(),
                index: index_val.clone(),
            }),
        );
        Ok(())
    }

    fn index_result_feeds_only_len(&self, instr_idx: usize, block_end: usize, dst: u16) -> bool {
        let mut alias = dst;
        let scan_end = block_end.min(instr_idx + 8);
        for op in &self.function.code[instr_idx + 1..scan_end] {
            match *op {
                Op::Len { src, .. } if src == alias => return true,
                Op::Move(new_alias, src) | Op::LoadLocal(new_alias, src) | Op::StoreLocal(new_alias, src)
                    if src == alias =>
                {
                    alias = new_alias;
                }
                _ if op_reads_reg(op, alias) || op_writes_reg(op, alias) => return false,
                _ if !is_len_feed_neutral_op(op) => return false,
                _ => {}
            }
        }
        false
    }

    pub(super) fn emit_index_const(&mut self, dst: u16, base: u16, kidx: u16) -> Result<()> {
        let value = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let literal = match value {
            Val::Int(i) => i.to_string(),
            other => {
                return Err(anyhow!("IndexK expects integer constant; found {:?}", other));
            }
        };
        let base_val = self.load_reg(base)?;
        self.require_helper(RuntimeHelper::Index);
        let result = self.fresh("index");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {base}, i64 {literal})",
            RuntimeHelper::Index.symbol(),
            base = base_val,
            literal = literal
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }

    pub(super) fn emit_in(&mut self, dst: u16, needle: u16, haystack: u16) -> Result<()> {
        let needle_val = self.load_reg(needle)?;
        let haystack_val = self.load_reg(haystack)?;
        self.require_helper(RuntimeHelper::In);
        let result = self.fresh("in");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {needle}, i64 {haystack})",
            RuntimeHelper::In.symbol(),
            needle = needle_val,
            haystack = haystack_val
        ));
        self.store_reg(dst, &result)?;
        Ok(())
    }
}

fn op_reads_reg(op: &Op, reg: u16) -> bool {
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

fn op_writes_reg(op: &Op, reg: u16) -> bool {
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

fn is_len_feed_neutral_op(op: &Op) -> bool {
    matches!(
        op,
        Op::LoadK(..)
            | Op::Move(..)
            | Op::StoreLocal(..)
            | Op::LoadLocal(..)
            | Op::Add(..)
            | Op::StrConcatKnownCap(..)
            | Op::StrConcatToStr(..)
            | Op::Sub(..)
            | Op::Mul(..)
            | Op::Div(..)
            | Op::Mod(..)
            | Op::AddInt(..)
            | Op::SubInt(..)
            | Op::MulInt(..)
            | Op::ModInt(..)
            | Op::AddFloat(..)
            | Op::SubFloat(..)
            | Op::MulFloat(..)
            | Op::DivFloat(..)
            | Op::ModFloat(..)
            | Op::AddIntImm(..)
    )
}
