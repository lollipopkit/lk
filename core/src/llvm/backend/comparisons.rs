use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn emit_compare(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
        if let Some((key, keys, invert)) = self.const_map_membership_compare(a, b, op) {
            return self.emit_const_map_membership_compare(dst, &key, &keys, invert);
        }
        if let Some((base, prefix, suffix, invert)) = self.str_int_map_membership_compare(a, b, op) {
            return self.emit_str_int_map_membership_compare(dst, &base, &prefix, &suffix, invert);
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

    fn const_map_membership_compare(&self, a: u16, b: u16, op: &str) -> Option<(String, Vec<String>, bool)> {
        let invert = match op {
            "ne" => false,
            "eq" => true,
            _ => return None,
        };
        if self.operand_is_nil_const(b)
            && let Some(KnownReg::ConstMapMembership { key, keys }) = self.known(a).cloned()
        {
            return Some((key, keys, invert));
        }
        if self.operand_is_nil_const(a)
            && let Some(KnownReg::ConstMapMembership { key, keys }) = self.known(b).cloned()
        {
            return Some((key, keys, invert));
        }
        None
    }

    fn str_int_map_membership_compare(&self, a: u16, b: u16, op: &str) -> Option<(String, String, String, bool)> {
        let invert = match op {
            "ne" => false,
            "eq" => true,
            _ => return None,
        };
        if self.operand_is_nil_const(b)
            && let Some(KnownReg::StrIntMapMembership { base, prefix, suffix }) = self.known(a).cloned()
        {
            return Some((base, prefix, suffix, invert));
        }
        if self.operand_is_nil_const(a)
            && let Some(KnownReg::StrIntMapMembership { base, prefix, suffix }) = self.known(b).cloned()
        {
            return Some((base, prefix, suffix, invert));
        }
        None
    }

    fn emit_str_int_map_membership_compare(
        &mut self,
        dst: u16,
        base: &str,
        prefix: &str,
        suffix: &str,
        invert: bool,
    ) -> Result<()> {
        let const_data = self.make_string_constant(prefix);
        self.anonymous_string_constants.push(const_data.clone());
        let ptr = self.emit_string_pointer(&const_data);
        self.require_helper(RuntimeHelper::MapHasStrInt);
        let matched = self.fresh("maphas");
        self.writer.line(format!(
            "{matched} = call i64 @{}(i64 {base}, i8* {ptr}, i64 {}, i64 {suffix})",
            RuntimeHelper::MapHasStrInt.symbol(),
            const_data.len
        ));
        self.store_maybe_inverted_bool(dst, &matched, invert, "maphas")
    }

    fn emit_const_map_membership_compare(&mut self, dst: u16, key: &str, keys: &[String], invert: bool) -> Result<()> {
        let mut key_constants = Vec::with_capacity(3);
        for idx in 0..3 {
            let text = keys.get(idx).map(String::as_str).unwrap_or("");
            let const_data = self.make_string_constant(text);
            self.anonymous_string_constants.push(const_data.clone());
            let ptr = self.emit_string_pointer(&const_data);
            key_constants.push((ptr, const_data.len));
        }
        self.require_helper(RuntimeHelper::StringInConst3);
        let matched = self.fresh("strin");
        self.writer.line(format!(
            "{matched} = call i64 @{}(i64 {key}, i8* {}, i64 {}, i8* {}, i64 {}, i8* {}, i64 {}, i64 {})",
            RuntimeHelper::StringInConst3.symbol(),
            key_constants[0].0,
            key_constants[0].1,
            key_constants[1].0,
            key_constants[1].1,
            key_constants[2].0,
            key_constants[2].1,
            keys.len()
        ));
        self.store_maybe_inverted_bool(dst, &matched, invert, "strin")
    }

    fn store_maybe_inverted_bool(&mut self, dst: u16, value: &str, invert: bool, prefix: &str) -> Result<()> {
        if !invert {
            self.store_reg(dst, value)?;
            return Ok(());
        }
        let is_true = self.fresh(&format!("{prefix}true"));
        self.writer.line(format!(
            "{is_true} = icmp eq i64 {value}, {}",
            encoding::BOOL_TRUE_VALUE
        ));
        let out = self.fresh(&format!("{prefix}not"));
        self.writer.line(format!(
            "{out} = select i1 {is_true}, i64 {}, i64 {}",
            encoding::BOOL_FALSE_VALUE,
            encoding::BOOL_TRUE_VALUE
        ));
        self.store_reg(dst, &out)
    }

    fn operand_is_nil_const(&self, operand: u16) -> bool {
        rk_is_const(operand)
            && self
                .function
                .consts
                .get(rk_index(operand) as usize)
                .is_some_and(|value| matches!(value, Val::Nil))
    }
}
