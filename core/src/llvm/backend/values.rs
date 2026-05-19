use std::{collections::BTreeMap, fmt::Write as _};

use arcstr::ArcStr;

use crate::util::fast_map::FastHashMap;

use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn fresh(&mut self, prefix: &str) -> String {
        let tmp = format!("%{}_{}", prefix, self.tmp_counter);
        self.tmp_counter += 1;
        tmp
    }

    pub(super) fn fresh_label(&mut self, prefix: &str) -> String {
        let label = format!("{}_{}", prefix, self.tmp_counter);
        self.tmp_counter += 1;
        label
    }

    pub(super) fn set_known(&mut self, reg: u16, value: Option<KnownReg>) {
        if let Some(slot) = self.known_regs.get_mut(reg as usize) {
            *slot = value;
        }
    }

    pub(super) fn known(&self, reg: u16) -> Option<&KnownReg> {
        self.known_regs.get(reg as usize).and_then(Option::as_ref)
    }

    pub(super) fn ensure_reg(&self, reg: u16) -> Result<()> {
        if reg as usize >= self.function.n_regs as usize {
            Err(anyhow!("register {} out of bounds", reg))
        } else {
            Ok(())
        }
    }

    pub(super) fn load_reg(&mut self, reg: u16) -> Result<String> {
        self.ensure_reg(reg)?;
        let tmp = self.fresh("load");
        self.writer.line(format!("{tmp} = load i64, i64* %r{reg}, align 8"));
        Ok(tmp)
    }

    pub(super) fn load_rk(&mut self, operand: u16) -> Result<String> {
        if rk_is_const(operand) {
            self.load_const_value(rk_index(operand))
        } else {
            self.load_reg(operand)
        }
    }

    pub(super) fn operand_known_int(&self, operand: u16) -> bool {
        if rk_is_const(operand) {
            matches!(self.function.consts.get(rk_index(operand) as usize), Some(Val::Int(_)))
        } else {
            matches!(self.known(operand), Some(KnownReg::Int)) || self.integer_regs.contains(&operand)
        }
    }

    pub(super) fn emit_binary(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        let lhs = self.load_rk(a)?;
        let rhs = self.load_rk(b)?;
        let tmp = self.fresh(op);
        self.writer.line(format!("{tmp} = {op} i64 {lhs}, {rhs}"));
        self.store_reg(dst, &tmp)?;
        self.set_known(dst, Some(KnownReg::Int));
        Ok(())
    }

    pub(super) fn emit_int_binary(&mut self, dst: u16, a: u16, b: u16, op: &str, helper: RuntimeHelper) -> Result<()> {
        if self.operand_known_int(a) && self.operand_known_int(b) {
            return self.emit_binary(dst, a, b, op);
        }
        self.emit_value_binary(dst, a, b, helper)
    }

    pub(super) fn emit_copy(&mut self, dst: u16, src: u16) -> Result<()> {
        let known = self.known(src).cloned();
        let value = self.load_reg(src)?;
        self.store_reg(dst, &value)?;
        self.set_known(dst, known);
        Ok(())
    }

    pub(super) fn emit_store_local(&mut self, idx: u16, src: u16) -> Result<()> {
        let known = self.known(src).cloned();
        let value = self.load_reg(src)?;
        self.store_reg(idx, &value)?;
        self.set_known(idx, known);
        Ok(())
    }

    pub(super) fn emit_load_local(&mut self, dst: u16, idx: u16) -> Result<()> {
        let known = self.known(idx).cloned();
        let value = self.load_reg(idx)?;
        self.store_reg(dst, &value)?;
        self.set_known(dst, known);
        Ok(())
    }

    pub(super) fn emit_add_int_imm(&mut self, dst: u16, src: u16, imm: i16) -> Result<()> {
        let lhs = self.load_reg(src)?;
        let tmp = self.fresh("addi");
        self.writer.line(format!("{tmp} = add i64 {lhs}, {}", imm as i64));
        self.store_reg(dst, &tmp)?;
        self.set_known(dst, Some(KnownReg::Int));
        Ok(())
    }

    pub(super) fn emit_cmp_int_imm(&mut self, dst: u16, src: u16, imm: i16, op: &str) -> Result<()> {
        let lhs = self.load_reg(src)?;
        let literal = encoding::encode_immediate(&Val::Int(imm as i64))?;
        self.emit_bool_compare(dst, &lhs, &literal.to_string(), op, "cmpimm")
    }

    pub(super) fn emit_add_value(
        &mut self,
        instr_idx: usize,
        block_end: usize,
        dst: u16,
        a: u16,
        b: u16,
    ) -> Result<()> {
        if self.try_defer_string_int_key(instr_idx, block_end, dst, a, b)? {
            return Ok(());
        }
        if self.try_defer_string_length(instr_idx, block_end, dst, a, b)? {
            return Ok(());
        }
        self.emit_value_binary(dst, a, b, RuntimeHelper::AddValue)
    }

    pub(super) fn emit_value_binary(&mut self, dst: u16, a: u16, b: u16, helper: RuntimeHelper) -> Result<()> {
        if self.try_emit_access_binary(dst, a, b, helper)? {
            return Ok(());
        }
        let lhs = self.load_rk(a)?;
        let rhs = self.load_rk(b)?;
        if self.operand_known_int(a) && self.operand_known_int(b) {
            let op = match helper {
                RuntimeHelper::AddValue => Some("add"),
                RuntimeHelper::SubValue => Some("sub"),
                RuntimeHelper::MulValue => Some("mul"),
                RuntimeHelper::DivValue => Some("sdiv"),
                RuntimeHelper::ModValue => Some("srem"),
                _ => None,
            };
            if let Some(op) = op {
                let tmp = self.fresh(helper.temp_prefix());
                self.writer.line(format!("{tmp} = {op} i64 {lhs}, {rhs}"));
                self.store_reg(dst, &tmp)?;
                self.set_known(dst, Some(KnownReg::Int));
                return Ok(());
            }
        }
        self.require_helper(helper);
        let tmp = self.fresh(helper.temp_prefix());
        self.writer
            .line(format!("{tmp} = call i64 @{}(i64 {lhs}, i64 {rhs})", helper.symbol()));
        self.store_reg(dst, &tmp)?;
        Ok(())
    }

    pub(super) fn emit_str_concat_to_str(&mut self, dst: u16, lhs: u16, src: u16) -> Result<()> {
        let lhs_value = self.load_rk(lhs)?;
        let src_value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::ToString);
        self.require_helper(RuntimeHelper::AddValue);
        let rhs = self.fresh("tostr");
        self.writer.line(format!(
            "{rhs} = call i64 @{}(i64 {src_value})",
            RuntimeHelper::ToString.symbol()
        ));
        let out = self.fresh(RuntimeHelper::AddValue.temp_prefix());
        self.writer.line(format!(
            "{out} = call i64 @{}(i64 {lhs_value}, i64 {rhs})",
            RuntimeHelper::AddValue.symbol()
        ));
        self.store_reg(dst, &out)?;
        Ok(())
    }

    fn try_emit_access_binary(&mut self, dst: u16, a: u16, b: u16, helper: RuntimeHelper) -> Result<bool> {
        let (lhs, base, key, access_helper) = match helper {
            RuntimeHelper::AddValue => {
                if let Some(KnownReg::AccessedValue { base, key }) = self.known(b).cloned() {
                    (self.load_rk(a)?, base, key, RuntimeHelper::AddAccess)
                } else if let Some(KnownReg::AccessedValue { base, key }) = self.known(a).cloned() {
                    (self.load_rk(b)?, base, key, RuntimeHelper::AddAccess)
                } else {
                    return Ok(false);
                }
            }
            RuntimeHelper::SubValue => {
                let Some(KnownReg::AccessedValue { base, key }) = self.known(b).cloned() else {
                    return Ok(false);
                };
                (self.load_rk(a)?, base, key, RuntimeHelper::SubAccess)
            }
            _ => return Ok(false),
        };
        self.require_helper(access_helper);
        let out = self.fresh(access_helper.temp_prefix());
        self.writer.line(format!(
            "{out} = call i64 @{}(i64 {lhs}, i64 {base}, i64 {key})",
            access_helper.symbol()
        ));
        self.store_reg(dst, &out)?;
        Ok(true)
    }

    pub(super) fn emit_to_str(&mut self, dst: u16, src: u16) -> Result<()> {
        let value = self.load_reg(src)?;
        let known = self.known(src).cloned();
        let known_len = if let Some(KnownReg::StringHandle { text, len, .. }) = known {
            Some((len.to_string(), text.is_ascii()))
        } else if let Some(KnownReg::StringLength { len, ascii }) = known {
            Some((len, ascii))
        } else if self.operand_known_int(src) {
            Some((self.emit_int_decimal_len_expr(&value), true))
        } else {
            None
        };
        self.require_helper(RuntimeHelper::ToString);
        let tmp = self.fresh("tostr");
        self.writer.line(format!(
            "{tmp} = call i64 @{}(i64 {value})",
            RuntimeHelper::ToString.symbol()
        ));
        self.store_reg(dst, &tmp)?;
        if let Some((len, ascii)) = known_len {
            self.set_known(dst, Some(KnownReg::StringLength { len, ascii }));
        }
        Ok(())
    }

    pub(super) fn load_const_value(&mut self, kidx: u16) -> Result<String> {
        let val = self
            .function
            .consts
            .get(kidx as usize)
            .cloned()
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        match &val {
            Val::Int(_) | Val::Bool(_) | Val::Nil => Ok(encoding::encode_immediate(&val)?.to_string()),
            Val::Float(f) => {
                let literal = Self::format_double(*f);
                Ok(self.emit_float_value(&literal))
            }
            val if val.as_str().is_some() => self.intern_string_constant(kidx, val.as_str().unwrap()),
            Val::List(items) => self.emit_const_list(items),
            Val::Map(map) => self.emit_const_map(map),
            other => Err(anyhow!(
                "unsupported constant {:?} in LLVM backend; only primitive/List/Map constants are accepted",
                other
            )),
        }
    }

    pub(super) fn store_reg(&mut self, reg: u16, value: impl AsRef<str>) -> Result<()> {
        self.ensure_reg(reg)?;
        self.writer
            .line(format!("store i64 {}, i64* %r{reg}, align 8", value.as_ref()));
        self.set_known(reg, None);
        Ok(())
    }

    pub(super) fn store_bool(&mut self, reg: u16, value: bool) -> Result<()> {
        self.store_reg(reg, encoding::bool_literal(value))
    }

    pub(super) fn emit_float_value(&mut self, literal: &str) -> String {
        self.require_helper(RuntimeHelper::MakeFloat);
        let tmp = self.fresh("constf");
        self.writer.line(format!(
            "{tmp} = call i64 @{}(double {literal})",
            RuntimeHelper::MakeFloat.symbol()
        ));
        tmp
    }

    pub(super) fn emit_floor(&mut self, dst: u16, src: u16) -> Result<()> {
        if self.operand_known_int(src) {
            let value = self.load_reg(src)?;
            self.store_reg(dst, value)?;
            self.set_known(dst, Some(KnownReg::Int));
            return Ok(());
        }
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::Floor);
        let out = self.fresh("floor");
        self.writer.line(format!(
            "{out} = call i64 @{}(i64 {value})",
            RuntimeHelper::Floor.symbol()
        ));
        self.store_reg(dst, out)?;
        self.set_known(dst, Some(KnownReg::Int));
        Ok(())
    }

    pub(super) fn emit_len(&mut self, dst: u16, src: u16) -> Result<()> {
        if let Some(KnownReg::StringLength { len, .. }) = self.known(src).cloned() {
            self.store_reg(dst, &len)?;
            self.set_known(dst, Some(KnownReg::Int));
            return Ok(());
        }
        if let Some(KnownReg::StringHandle { len, .. }) = self.known(src).cloned() {
            self.store_reg(dst, len.to_string())?;
            self.set_known(dst, Some(KnownReg::Int));
            return Ok(());
        }
        if let Some(KnownReg::IndexedValue { base, index }) = self.known(src).cloned() {
            self.require_helper(RuntimeHelper::IndexLen);
            let result = self.fresh("indexlen");
            self.writer.line(format!(
                "{result} = call i64 @{}(i64 {base}, i64 {index})",
                RuntimeHelper::IndexLen.symbol()
            ));
            self.store_reg(dst, &result)?;
            self.set_known(dst, Some(KnownReg::Int));
            return Ok(());
        }
        if let Some(KnownReg::IndexedAsciiCharLength { base_len, index }) = self.known(src).cloned() {
            let in_range = self.fresh("asciicharinrange");
            self.writer
                .line(format!("{in_range} = icmp ult i64 {index}, {base_len}"));
            let result = self.fresh("asciicharlen");
            self.writer
                .line(format!("{result} = select i1 {in_range}, i64 1, i64 0"));
            self.store_reg(dst, &result)?;
            self.set_known(dst, Some(KnownReg::Int));
            return Ok(());
        }
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::Len);
        let result = self.fresh("len");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {value})",
            RuntimeHelper::Len.symbol()
        ));
        self.store_reg(dst, &result)?;
        self.set_known(dst, Some(KnownReg::Int));
        Ok(())
    }

    pub(super) fn emit_string_predicate_k(
        &mut self,
        dst: u16,
        src: u16,
        kidx: u16,
        helper: RuntimeHelper,
    ) -> Result<()> {
        let needle = self
            .function
            .consts
            .get(kidx as usize)
            .and_then(Val::as_str)
            .ok_or_else(|| anyhow!("string predicate expects string constant k{}", kidx))?
            .to_string();
        if let Some(KnownReg::StringHandle { text, .. }) = self.known(src).cloned() {
            let result = match helper {
                RuntimeHelper::StartsWith => text.starts_with(&needle),
                RuntimeHelper::Contains => text.contains(&needle),
                _ => return Err(anyhow!("unsupported string predicate helper {:?}", helper)),
            };
            self.store_bool(dst, result)?;
            return Ok(());
        }
        let value = self.load_reg(src)?;
        if helper == RuntimeHelper::StartsWith {
            let const_data = self.make_string_constant(&needle);
            self.anonymous_string_constants.push(const_data.clone());
            let ptr = self.emit_string_pointer(&const_data);
            self.require_helper(RuntimeHelper::StartsWithConst);
            let out = self.fresh(RuntimeHelper::StartsWithConst.temp_prefix());
            self.writer.line(format!(
                "{out} = call i64 @{}(i64 {value}, i8* {ptr}, i64 {len})",
                RuntimeHelper::StartsWithConst.symbol(),
                len = const_data.len
            ));
            return self.store_reg(dst, out);
        }
        let needle = self.intern_string_constant(kidx, &needle)?;
        self.require_helper(helper);
        let out = self.fresh(helper.temp_prefix());
        self.writer.line(format!(
            "{out} = call i64 @{}(i64 {value}, i64 {needle})",
            helper.symbol()
        ));
        self.store_reg(dst, out)
    }

    pub(super) fn try_emit_direct_aot_call(&mut self, rf: u16, base: u16, argc: u8, retc: u8) -> Result<bool> {
        let Some(KnownReg::AotClosure {
            symbol,
            proto_index,
            arity,
            integer_params,
        }) = self.known(rf).cloned()
        else {
            return Ok(false);
        };
        if arity != argc as usize {
            return Ok(false);
        }
        let base_idx = base as usize;
        if base_idx + arity > self.function.n_regs as usize {
            return Err(anyhow!("AOT direct call reads out of bounds registers"));
        }
        if integer_params
            .iter()
            .any(|idx| !self.operand_known_int(base + *idx as u16))
        {
            return Ok(false);
        }
        let symbol = self.specialized_aot_symbol(proto_index, &symbol, base, arity)?;
        let mut args = Vec::with_capacity(arity);
        for i in 0..arity {
            let value = self.load_reg(base + i as u16)?;
            args.push(format!("i64 {value}"));
        }
        let params = args.join(", ");
        if retc == 1 {
            let result = self.fresh("aotdirect");
            self.writer.line(format!("{result} = call i64 @{symbol}({params})"));
            self.store_reg(base, result)?;
        } else {
            self.writer.line(format!("call i64 @{symbol}({params})"));
        }
        Ok(true)
    }

    fn specialized_aot_symbol(
        &mut self,
        proto_index: u16,
        fallback_symbol: &str,
        base: u16,
        arity: usize,
    ) -> Result<String> {
        let mut known_params = BTreeMap::new();
        for idx in 0..arity {
            match self.known(base + idx as u16).cloned() {
                Some(KnownReg::ConstMap { entries }) => {
                    known_params.insert(idx, KnownReg::ConstMap { entries });
                }
                Some(KnownReg::StringHandle { text, len, .. })
                    if register_is_string_constant_source(self.function, base + idx as u16) =>
                {
                    known_params.insert(
                        idx,
                        KnownReg::StringHandle {
                            handle: format!("%arg{idx}"),
                            text,
                            len,
                        },
                    );
                }
                _ => {}
            }
        }
        if known_params.is_empty() {
            return Ok(fallback_symbol.to_string());
        }
        let key = known_specialization_key(proto_index as usize, &known_params);
        if let Some(symbol) = self.specialized_native_closures.get(&key) {
            return Ok(symbol.clone());
        }
        let proto = self
            .function
            .protos
            .get(proto_index as usize)
            .ok_or_else(|| anyhow!("AOT proto {} out of range", proto_index))?;
        let Some(func) = proto.func.as_ref() else {
            return Ok(fallback_symbol.to_string());
        };
        let symbol = format!(
            "{}_proto_{}_spec_{}",
            self.function_name,
            proto_index,
            self.specialized_native_closures.len()
        );
        let translator = function_translator_with_captures(func, &symbol, self.options, Some(proto.captures.as_ref()))
            .with_initial_known_params(known_params);
        let ir = translator
            .translate()
            .with_context(|| format!("compile specialised native closure proto {}", proto_index))?;
        self.merge_runtime_helpers_from_ir(&ir);
        self.native_closure_ir.push(strip_nested_module_header(&ir));
        self.specialized_native_closures.insert(key, symbol.clone());
        Ok(symbol)
    }

    pub(super) fn emit_make_closure(&mut self, dst: u16, proto: u16) -> Result<()> {
        let binding = self
            .native_closures
            .get(&proto)
            .cloned()
            .ok_or_else(|| anyhow!("unsupported closure proto in LLVM backend: p{}", proto))?;
        self.require_helper(RuntimeHelper::MakeAotFunction);
        let closure = self.fresh("aotclosure");
        let params = std::iter::repeat_n("i64", binding.arity).collect::<Vec<_>>().join(", ");
        self.writer.line(format!(
            "{closure} = call i64 @{}(i8* bitcast (i64 ({params})* @{} to i8*), i64 {})",
            RuntimeHelper::MakeAotFunction.symbol(),
            binding.symbol,
            binding.arity
        ));
        self.store_reg(dst, &closure)?;
        self.set_known(
            dst,
            Some(KnownReg::AotClosure {
                symbol: binding.symbol,
                proto_index: binding.proto_index,
                arity: binding.arity,
                integer_params: binding.integer_params,
            }),
        );
        Ok(())
    }

    pub(super) fn require_helper(&mut self, helper: RuntimeHelper) {
        self.runtime_helpers.insert(helper);
    }

    pub(super) fn intern_string_constant(&mut self, kidx: u16, value: &str) -> Result<String> {
        let const_data = self.ensure_string_constant(kidx, value).clone();
        let ptr = self.emit_string_pointer(&const_data);
        self.require_helper(RuntimeHelper::InternString);
        let handle = self.fresh("conststr");
        self.writer.line(format!(
            "{handle} = call i64 @{}(i8* {ptr}, i64 {})",
            RuntimeHelper::InternString.symbol(),
            const_data.len
        ));
        Ok(handle)
    }

    pub(super) fn intern_anonymous_string(&mut self, value: &str) -> Result<String> {
        let const_data = self.make_string_constant(value);
        self.anonymous_string_constants.push(const_data.clone());
        let ptr = self.emit_string_pointer(&const_data);
        self.require_helper(RuntimeHelper::InternString);
        let handle = self.fresh("conststr");
        self.writer.line(format!(
            "{handle} = call i64 @{}(i8* {ptr}, i64 {})",
            RuntimeHelper::InternString.symbol(),
            const_data.len
        ));
        Ok(handle)
    }

    pub(super) fn emit_string_pointer(&mut self, const_data: &StringConstant) -> String {
        let ptr = self.fresh("strptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i8], [{len} x i8]* @{label}, i64 0, i64 0",
            len = const_data.array_len,
            label = const_data.label
        ));
        ptr
    }

    pub(super) fn ensure_string_constant(&mut self, kidx: u16, value: &str) -> &StringConstant {
        if !self.string_constants.contains_key(&kidx) {
            let const_data = self.make_string_constant(value);
            self.string_constants.insert(kidx, const_data);
        }
        self.string_constants.get(&kidx).expect("string constant inserted")
    }

    pub(super) fn make_string_constant(&mut self, value: &str) -> StringConstant {
        let function = self
            .function_name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let label = format!(".{function}.str{}", self.string_const_counter);
        self.string_const_counter += 1;
        let bytes = value.as_bytes();
        let len = bytes.len();
        let encoded = Self::encode_string_literal(bytes);
        StringConstant {
            label,
            encoded,
            len,
            array_len: len + 1,
        }
    }

    pub(super) fn encode_string_literal(bytes: &[u8]) -> String {
        let mut encoded = String::with_capacity(bytes.len() * 4 + 4);
        for &b in bytes {
            let _ = write!(&mut encoded, "\\{:02X}", b);
        }
        encoded.push_str("\\00");
        encoded
    }

    pub(super) fn emit_load_const(&mut self, instr_idx: usize, block_end: usize, dst: u16, kidx: u16) -> Result<()> {
        let val = self
            .function
            .consts
            .get(kidx as usize)
            .cloned()
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        match &val {
            Val::Int(_) | Val::Bool(_) | Val::Nil => {
                let encoded = encoding::encode_immediate(&val)?;
                self.store_reg(dst, encoded.to_string())?;
                if matches!(val, Val::Int(_)) {
                    self.set_known(dst, Some(KnownReg::Int));
                }
            }
            Val::Float(f) => {
                let literal = Self::format_double(*f);
                let tmp = self.emit_float_value(&literal);
                self.store_reg(dst, &tmp)?;
            }
            val if val.as_str().is_some() => {
                let text = val.as_str().unwrap();
                if self.try_defer_string_const_length(instr_idx, block_end, dst, text)? {
                    return Ok(());
                }
                let handle = self.intern_string_constant(kidx, text)?;
                self.store_reg(dst, &handle)?;
                self.set_known(
                    dst,
                    Some(KnownReg::StringHandle {
                        handle,
                        text: text.to_string(),
                        len: text.len(),
                    }),
                );
            }
            Val::List(items) => {
                let handle = self.emit_const_list(items)?;
                self.store_reg(dst, &handle)?;
                self.set_known(dst, None);
            }
            Val::Map(map) => {
                let handle = self.emit_const_map(map)?;
                self.store_reg(dst, &handle)?;
                if !map.is_empty() {
                    self.set_known(
                        dst,
                        Some(KnownReg::ConstMap {
                            entries: Self::const_map_entries(map),
                        }),
                    );
                }
            }
            other => {
                return Err(anyhow!(
                    "unsupported constant {:?} in LLVM backend; only primitive/List/Map constants are accepted",
                    other
                ));
            }
        }
        Ok(())
    }

    pub(super) fn set_known_const_value(&mut self, dst: u16, value: &Val, raw: &str) {
        let known = match value {
            Val::Int(_) => Some(KnownReg::Int),
            val if val.as_str().is_some() => {
                let text = val.as_str().unwrap().to_string();
                Some(KnownReg::StringHandle {
                    handle: raw.to_string(),
                    len: text.len(),
                    text,
                })
            }
            Val::Map(map) if !map.is_empty() => Some(KnownReg::ConstMap {
                entries: Self::const_map_entries(map),
            }),
            _ => None,
        };
        self.set_known(dst, known);
    }

    pub(super) fn emit_const_value(&mut self, val: &Val) -> Result<String> {
        match val {
            Val::Int(_) | Val::Bool(_) | Val::Nil => Ok(encoding::encode_immediate(val)?.to_string()),
            Val::Float(f) => {
                let literal = Self::format_double(*f);
                Ok(self.emit_float_value(&literal))
            }
            val if val.as_str().is_some() => self.intern_anonymous_string(val.as_str().unwrap()),
            Val::List(items) => self.emit_const_list(items),
            Val::Map(map) => self.emit_const_map(map),
            other => Err(anyhow!("unsupported nested constant {:?} in LLVM backend", other)),
        }
    }

    pub(super) fn emit_const_list(&mut self, items: &[Val]) -> Result<String> {
        self.require_helper(RuntimeHelper::BuildList);
        if items.is_empty() {
            let list = self.fresh("constlist");
            self.writer.line(format!(
                "{list} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildList.symbol()
            ));
            return Ok(list);
        }
        let len = items.len();
        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("constlistbuf");
        self.writer.line(format!("{array} = alloca [{len} x i64], align 8"));
        for (idx, item) in items.iter().enumerate() {
            let value = self.emit_const_value(item)?;
            let slot = self.fresh("constlistelt");
            self.writer.line(format!(
                "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}"
            ));
            self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
        }
        let ptr = self.fresh("constlistptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0"
        ));
        let list = self.fresh("constlist");
        self.writer.line(format!(
            "{list} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildList.symbol()
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        Ok(list)
    }

    pub(super) fn emit_const_map(&mut self, map: &FastHashMap<ArcStr, Val>) -> Result<String> {
        self.require_helper(RuntimeHelper::BuildMap);
        if map.is_empty() {
            let out = self.fresh("constmap");
            self.writer.line(format!(
                "{out} = call i64 @{}(i64* null, i64 0)",
                RuntimeHelper::BuildMap.symbol()
            ));
            return Ok(out);
        }
        let len = map.len();
        let total = len * 2;
        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("constmapbuf");
        self.writer.line(format!("{array} = alloca [{total} x i64], align 8"));
        for (idx, (key, value)) in map.iter().enumerate() {
            let key_value = self.intern_anonymous_string(key.as_str())?;
            let val_value = self.emit_const_value(value)?;
            for (offset, raw) in [(idx * 2, key_value), (idx * 2 + 1, val_value)] {
                let slot = self.fresh("constmapelt");
                self.writer.line(format!(
                    "{slot} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 {offset}"
                ));
                self.writer.line(format!("store i64 {raw}, i64* {slot}, align 8"));
            }
        }
        let ptr = self.fresh("constmapptr");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{total} x i64], [{total} x i64]* {array}, i64 0, i64 0"
        ));
        let out = self.fresh("constmap");
        self.writer.line(format!(
            "{out} = call i64 @{}(i64* {ptr}, i64 {len})",
            RuntimeHelper::BuildMap.symbol()
        ));
        self.writer
            .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        Ok(out)
    }

    fn const_map_entries(map: &FastHashMap<ArcStr, Val>) -> BTreeMap<String, Val> {
        map.iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    pub(super) fn format_double(value: f64) -> String {
        let bits = value.to_bits();
        format!("0x{:016X}", bits)
    }
}
