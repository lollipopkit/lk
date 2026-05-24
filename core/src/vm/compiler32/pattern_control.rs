use anyhow::{Result, bail};

use crate::{expr::Pattern, stmt::Stmt, val::LiteralVal};

use super::{
    Compiler32, Instr32, Opcode32,
    support::{ast_literal_kind, checked_u8, pattern_kind},
};

impl Compiler32 {
    pub(super) fn lower_if_let(
        &mut self,
        pattern: &Pattern,
        value: &crate::expr::Expr,
        then_stmt: &Stmt,
        else_stmt: Option<&Stmt>,
    ) -> Result<()> {
        let watermark = self.next_reg;
        let value = self.lower_expr(value)?;
        let (condition, previous) = self.lower_pattern_match(pattern, value)?;
        let test_pc = self.emit_test_placeholder(condition)?;

        self.emitted_return = false;
        self.lower_stmt(then_stmt)?;
        let then_returns = self.emitted_return;
        self.restore_pattern_bindings(previous);
        self.next_reg = watermark; // recycle registers from then-branch

        if let Some(else_stmt) = else_stmt {
            let jmp_end = (!then_returns).then(|| self.emit_jmp_placeholder());
            let else_start = self.function.code.len();
            self.patch_test_false_jump(test_pc, else_start)?;

            self.emitted_return = false;
            self.lower_stmt(else_stmt)?;
            let else_returns = self.emitted_return;
            self.next_reg = watermark; // recycle registers from else-branch

            if let Some(jmp_end) = jmp_end {
                let end = self.function.code.len();
                self.patch_jmp(jmp_end, end)?;
            }
            self.emitted_return = then_returns && else_returns;
        } else {
            let end = self.function.code.len();
            self.patch_test_false_jump(test_pc, end)?;
            self.emitted_return = false;
        }

        Ok(())
    }

    pub(super) fn lower_while_let(&mut self, pattern: &Pattern, value: &crate::expr::Expr, body: &Stmt) -> Result<()> {
        let watermark = self.next_reg;
        let loop_start = self.function.code.len();
        let value = self.lower_expr(value)?;
        let (condition, previous) = match pattern {
            Pattern::List { patterns, .. } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                let previous = Vec::new();
                (condition, previous)
            }
            Pattern::Map { patterns, .. } => {
                let condition = self.lower_map_pattern_condition(value, patterns)?;
                let previous = Vec::new();
                (condition, previous)
            }
            _ => self.lower_pattern_match(pattern, value)?,
        };
        let exit_test = self.emit_test_placeholder(condition)?;

        let previous = match pattern {
            Pattern::List { .. } | Pattern::Map { .. } => {
                let mut previous = previous;
                self.bind_irrefutable_pattern(pattern, value, &mut previous)?;
                previous
            }
            _ => previous,
        };

        self.loops.push(super::support::LoopPatch32::default());
        self.emitted_return = false;
        self.lower_stmt(body)?;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");
        self.restore_pattern_bindings(previous);

        if !self.emitted_return {
            let jmp_back = self.emit_jmp_placeholder();
            self.patch_jmp(jmp_back, loop_start)?;
        }

        let loop_end = self.function.code.len();
        self.patch_test_false_jump(exit_test, loop_end)?;
        for pc in loop_patch.breaks {
            self.patch_jmp(pc, loop_end)?;
        }
        for pc in loop_patch.continues {
            self.patch_jmp(pc, loop_start)?;
        }
        self.emitted_return = false;
        self.next_reg = watermark; // recycle all while-let loop registers
        Ok(())
    }

    pub(super) fn lower_pattern_match(
        &mut self,
        pattern: &Pattern,
        value: u16,
    ) -> Result<(u16, Vec<(String, Option<u16>)>)> {
        let mut previous = Vec::new();
        let condition = match pattern {
            Pattern::Variable(name) => {
                previous.push((name.clone(), self.insert_local(name.clone(), value)));
                let is_nil = self.alloc_reg();
                self.emit(Instr32::abc(
                    Opcode32::IsNil,
                    checked_u8("pattern variable nil check", is_nil)?,
                    checked_u8("pattern variable value", value)?,
                    0,
                ));
                let condition = self.alloc_reg();
                self.emit(Instr32::abc(
                    Opcode32::Not,
                    checked_u8("pattern variable condition", condition)?,
                    checked_u8("pattern variable nil", is_nil)?,
                    0,
                ));
                condition
            }
            Pattern::Wildcard => self.lower_val(&LiteralVal::Bool(true))?,
            Pattern::List { patterns, .. } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.bind_irrefutable_pattern(pattern, value, &mut previous)?;
                condition
            }
            Pattern::Map { patterns, .. } => {
                let condition = self.lower_map_pattern_condition(value, patterns)?;
                self.bind_irrefutable_pattern(pattern, value, &mut previous)?;
                condition
            }
            Pattern::Literal(literal) => {
                let expected = self.lower_pattern_literal(literal)?;
                let condition = self.alloc_reg();
                self.emit(Instr32::abc(
                    Opcode32::CmpInt,
                    checked_u8("pattern condition", condition)?,
                    checked_u8("pattern value", value)?,
                    checked_u8("pattern literal", expected)?,
                ));
                condition
            }
            Pattern::Range { start, end, inclusive } => {
                self.lower_range_pattern_condition(value, start, end, *inclusive)?
            }
            Pattern::Guard { pattern, guard } => {
                let (pattern_condition, nested_previous) = self.lower_pattern_match(pattern, value)?;
                previous.extend(nested_previous);
                self.lower_guard_condition(pattern_condition, guard)?
            }
            Pattern::Or(patterns) => self.lower_or_pattern_condition(patterns, value)?,
        };
        Ok((condition, previous))
    }

    fn lower_range_pattern_condition(
        &mut self,
        value: u16,
        start: &crate::expr::Expr,
        end: &crate::expr::Expr,
        inclusive: bool,
    ) -> Result<u16> {
        let start = self.lower_expr(start)?;
        let end = self.lower_expr(end)?;
        let ge_start = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::CmpGeInt,
            checked_u8("pattern range lower condition", ge_start)?,
            checked_u8("pattern range value", value)?,
            checked_u8("pattern range start", start)?,
        ));
        let before_end = self.alloc_reg();
        self.emit(Instr32::abc(
            if inclusive {
                Opcode32::CmpLeInt
            } else {
                Opcode32::CmpLtInt
            },
            checked_u8("pattern range upper condition", before_end)?,
            checked_u8("pattern range value", value)?,
            checked_u8("pattern range end", end)?,
        ));
        self.lower_and_condition(ge_start, before_end)
    }

    pub(super) fn lower_list_pattern_condition(&mut self, value: u16, fixed_len: usize) -> Result<u16> {
        let is_list = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::IsList,
            checked_u8("pattern list shape dst", is_list)?,
            checked_u8("pattern list shape value", value)?,
            0,
        ));
        let result = self.lower_val(&LiteralVal::Bool(false))?;
        let skip_len = self.emit_test_placeholder(is_list)?;
        let len = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::Len,
            checked_u8("pattern list len dst", len)?,
            checked_u8("pattern list value", value)?,
            0,
        ));
        let expected = self.lower_val(&LiteralVal::Int(fixed_len as i64))?;
        let condition = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::CmpGeInt,
            checked_u8("pattern list condition", condition)?,
            checked_u8("pattern list len", len)?,
            checked_u8("pattern list expected", expected)?,
        ));
        self.emit_move(result, condition, "pattern list condition")?;
        let end = self.function.code.len();
        self.patch_test_false_jump(skip_len, end)?;
        Ok(result)
    }

    pub(super) fn lower_map_pattern_condition(&mut self, value: u16, patterns: &[(String, Pattern)]) -> Result<u16> {
        self.lower_map_pattern_key_condition(value, patterns.iter().map(|(key, _)| key.as_str()))
    }

    pub(super) fn lower_map_pattern_key_condition<'a>(
        &mut self,
        value: u16,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Result<u16> {
        let is_map = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::IsMap,
            checked_u8("pattern map shape dst", is_map)?,
            checked_u8("pattern map shape value", value)?,
            0,
        ));
        let result = self.lower_val(&LiteralVal::Bool(false))?;
        let skip_contains = self.emit_test_placeholder(is_map)?;
        let mut condition = self.lower_val(&LiteralVal::Bool(true))?;
        for key in keys {
            let key = self.lower_val(&LiteralVal::from_str(key))?;
            let contains = self.alloc_reg();
            self.emit(Instr32::abc(
                Opcode32::Contains,
                checked_u8("pattern map contains", contains)?,
                checked_u8("pattern map key", key)?,
                checked_u8("pattern map value", value)?,
            ));
            condition = self.lower_and_condition(condition, contains)?;
        }
        self.emit_move(result, condition, "pattern map condition")?;
        let end = self.function.code.len();
        self.patch_test_false_jump(skip_contains, end)?;
        Ok(result)
    }

    fn lower_guard_condition(&mut self, pattern_condition: u16, guard: &crate::expr::Expr) -> Result<u16> {
        let result = self.lower_val(&LiteralVal::Bool(false))?;
        let skip_guard = self.emit_test_placeholder(pattern_condition)?;
        let guard = self.lower_expr(guard)?;
        self.emit_move(result, guard, "pattern guard condition")?;
        let end = self.function.code.len();
        self.patch_test_false_jump(skip_guard, end)?;
        Ok(result)
    }

    fn lower_or_pattern_condition(&mut self, patterns: &[Pattern], value: u16) -> Result<u16> {
        let result = self.lower_val(&LiteralVal::Bool(false))?;
        let true_value = self.lower_val(&LiteralVal::Bool(true))?;
        let mut end_jumps = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let (condition, previous) = self.lower_pattern_match(pattern, value)?;
            if !previous.is_empty() {
                self.restore_pattern_bindings(previous);
                bail!("Compiler32 does not support binding variables inside or-pattern yet");
            }
            let next_pattern = self.emit_test_placeholder(condition)?;
            self.emit_move(result, true_value, "or-pattern matched")?;
            end_jumps.push(self.emit_jmp_placeholder());
            let next = self.function.code.len();
            self.patch_test_false_jump(next_pattern, next)?;
        }
        let end = self.function.code.len();
        for pc in end_jumps {
            self.patch_jmp(pc, end)?;
        }
        Ok(result)
    }

    fn lower_and_condition(&mut self, lhs: u16, rhs: u16) -> Result<u16> {
        let result = self.lower_val(&LiteralVal::Bool(false))?;
        let skip_rhs = self.emit_test_placeholder(lhs)?;
        self.emit_move(result, rhs, "and-pattern condition")?;
        let end = self.function.code.len();
        self.patch_test_false_jump(skip_rhs, end)?;
        Ok(result)
    }

    fn bind_irrefutable_pattern(
        &mut self,
        pattern: &Pattern,
        value: u16,
        previous: &mut Vec<(String, Option<u16>)>,
    ) -> Result<()> {
        match pattern {
            Pattern::Variable(name) => {
                previous.push((name.clone(), self.insert_local(name.clone(), value)));
                Ok(())
            }
            Pattern::Wildcard => Ok(()),
            Pattern::List { patterns, rest } => {
                for (index, pattern) in patterns.iter().enumerate() {
                    let index =
                        i64::try_from(index).map_err(|_| anyhow::anyhow!("Compiler32 pattern index overflow"))?;
                    let key = self.lower_val(&LiteralVal::Int(index))?;
                    let field = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::GetIndex,
                        checked_u8("pattern sequence field", field)?,
                        checked_u8("pattern sequence value", value)?,
                        checked_u8("pattern sequence index", key)?,
                    ));
                    self.bind_irrefutable_pattern(pattern, field, previous)?;
                }
                if let Some(rest) = rest {
                    let start = self.lower_val(&LiteralVal::Int(patterns.len() as i64))?;
                    let slice = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::SliceFrom,
                        checked_u8("pattern rest slice", slice)?,
                        checked_u8("pattern rest value", value)?,
                        checked_u8("pattern rest start", start)?,
                    ));
                    previous.push((rest.clone(), self.insert_local(rest.clone(), slice)));
                }
                Ok(())
            }
            Pattern::Map { patterns, rest } => {
                for (key, pattern) in patterns {
                    let key = self.lower_val(&LiteralVal::from_str(key))?;
                    let field = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::GetIndex,
                        checked_u8("pattern map field", field)?,
                        checked_u8("pattern map value", value)?,
                        checked_u8("pattern map key", key)?,
                    ));
                    self.bind_irrefutable_pattern(pattern, field, previous)?;
                }
                if let Some(rest) = rest {
                    let map = self.lower_map_rest(value, patterns)?;
                    previous.push((rest.clone(), self.insert_local(rest.clone(), map)));
                }
                Ok(())
            }
            other => bail!(
                "Compiler32 does not support nested refutable pattern yet: {:?}",
                pattern_kind(other)
            ),
        }
    }

    fn lower_pattern_literal(&mut self, literal: &LiteralVal) -> Result<u16> {
        match literal {
            LiteralVal::Nil
            | LiteralVal::Bool(_)
            | LiteralVal::Int(_)
            | LiteralVal::Float(_)
            | LiteralVal::ShortStr(_) => self.lower_val(literal),
            value if value.as_str().is_some() => self.lower_val(value),
            other => bail!(
                "Compiler32 cannot lower pattern literal with AST value kind {}",
                ast_literal_kind(other)
            ),
        }
    }

    pub(super) fn restore_pattern_bindings(&mut self, previous: Vec<(String, Option<u16>)>) {
        for (name, old) in previous.into_iter().rev() {
            if let Some(old) = old {
                self.insert_local(name, old);
            } else {
                self.locals.remove(&name);
            }
        }
    }
}
