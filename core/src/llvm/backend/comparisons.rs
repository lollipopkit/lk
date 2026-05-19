use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn emit_int_compare_kind(
        &mut self,
        dst: u16,
        a: u16,
        b: u16,
        kind: crate::vm::IntCmpKind,
    ) -> Result<()> {
        let op = match kind {
            crate::vm::IntCmpKind::Eq => "eq",
            crate::vm::IntCmpKind::Ne => "ne",
            crate::vm::IntCmpKind::Lt => "slt",
            crate::vm::IntCmpKind::Le => "sle",
            crate::vm::IntCmpKind::Gt => "sgt",
            crate::vm::IntCmpKind::Ge => "sge",
        };
        self.emit_compare(dst, a, b, op)
    }

    pub(super) fn emit_compare(&mut self, dst: u16, a: u16, b: u16, op: &str) -> Result<()> {
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
}
