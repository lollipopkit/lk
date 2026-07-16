use super::*;

impl Compiler {
    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) -> Result<()> {
        match stmt {
            Stmt::Attributed { item, .. } => self.lower_stmt(item)?,
            Stmt::Empty => {}
            Stmt::Expr(expr) => {
                let watermark = self.next_reg;
                if !self.try_lower_rewritten_set_index_expr(expr)?
                    && !self.try_lower_builtin_method_statement(expr)?
                    && !self.try_lower_dead_literal_expr(expr)?
                {
                    self.lower_readonly_operand(expr)?;
                }
                self.next_reg = watermark;
            }
            Stmt::Return { value } => {
                if let Some(value) = value {
                    let value = self.lower_readonly_operand(value)?;
                    self.emit_return(value)?;
                } else {
                    self.emit_empty_return();
                }
            }
            Stmt::Let { pattern, value, .. } => self.lower_let(pattern, value)?,
            Stmt::Define { name, value } => self.lower_define(name, value)?,
            Stmt::Assign { name, value, .. } => {
                let watermark = self.next_reg;
                self.lower_assign(name, value)?;
                self.next_reg = self.live_register_floor().max(watermark);
            }
            Stmt::CompoundAssign { name, op, value, .. } => {
                let watermark = self.next_reg;
                self.lower_compound_assign(name, op, value)?;
                self.next_reg = self.live_register_floor().max(watermark);
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => self.lower_if(condition, then_stmt, else_stmt.as_deref())?,
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => self.lower_if_let(pattern, value, then_stmt, else_stmt.as_deref())?,
            Stmt::While { condition, body } => self.lower_while(condition, body)?,
            Stmt::WhileLet { pattern, value, body } => self.lower_while_let(pattern, value, body)?,
            Stmt::For {
                pattern,
                iterable,
                body,
            } => self.lower_for(pattern, iterable, body)?,
            Stmt::Break => self.lower_break()?,
            Stmt::Continue => self.lower_continue()?,
            Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {}
            Stmt::Trait { name, methods } => self.lower_trait_decl(name, methods)?,
            Stmt::Impl {
                trait_name,
                target_type,
                methods,
            } => self.lower_impl_decl(trait_name, target_type, methods)?,
            Stmt::Function { name, .. } => self.lower_function_decl(name)?,
            Stmt::Block { statements } => {
                let watermark = self.next_reg;
                let locals = self.locals.clone();
                let cell_locals = self.cell_locals.clone();
                let const_map_locals = self.const_map_locals.clone();
                self.local_rebind_suppression += 1;
                self.lower_stmt_sequence(statements)?;
                self.local_rebind_suppression -= 1;
                // In-block promotions of *outer* locals must survive the
                // scope restore (the register now holds the cell); dropping
                // them left later reads loading the raw cell object.
                self.cell_locals = self.scope_restored_cell_locals(&locals, cell_locals);
                self.locals = locals;
                self.const_map_locals = const_map_locals;
                if !self.emitted_return {
                    self.next_reg = self.live_register_floor().max(watermark);
                }
            }
        }
        Ok(())
    }

    pub(super) fn lower_stmt_sequence(&mut self, statements: &[Box<Stmt>]) -> Result<()> {
        let mut index = 0;
        while index < statements.len() {
            if index + 1 < statements.len()
                && (self
                    .try_lower_default_assign_if_chain(statements[index].as_ref(), statements[index + 1].as_ref())?
                    || self.try_lower_move2_assign_pair(statements[index].as_ref(), statements[index + 1].as_ref())?)
            {
                index += 2;
            } else {
                self.lower_stmt(statements[index].as_ref())?;
                index += 1;
            }
            if self.emitted_return {
                break;
            }
        }
        Ok(())
    }

    pub(super) fn try_lower_default_assign_if_chain(&mut self, first: &Stmt, second: &Stmt) -> Result<bool> {
        let Some((name, default_value, is_let)) = default_assign_candidate(first) else {
            return Ok(false);
        };
        if self.cell_locals.contains(name)
            || !pure_default_expr(default_value)
            || expr_mentions_name(default_value, name)
            || !if_chain_assigns_only_target(second, name)
            || if_chain_condition_mentions_name(second, name)
        {
            return Ok(false);
        }
        let Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } = second
        else {
            return Ok(false);
        };

        let watermark = self.next_reg;
        let target = if let Some(reg) = self.locals.get(name).copied() {
            // A re-`let` over a promoted cell or over a live loop counter
            // must not write the old register in place — the generic path
            // allocates the fresh binding.
            if is_let && (self.cell_locals.contains(name) || self.active_loop_binding_slot(name) == Some(reg)) {
                return Ok(false);
            }
            reg
        } else if is_let {
            let reg = self.alloc_reg();
            self.insert_fresh_local(name.to_string(), reg);
            reg
        } else {
            return Ok(false);
        };
        self.function
            .performance
            .set_register_kind(target, facts::expr_static_value_kind(default_value));
        self.lower_defaulted_if_chain(name, default_value, condition, then_stmt, else_stmt.as_deref())?;
        self.next_reg = self.live_register_floor().max(watermark).max(target + 1);
        Ok(true)
    }

    pub(super) fn lower_defaulted_if_chain(
        &mut self,
        name: &str,
        default_value: &Expr,
        condition: &Expr,
        then_stmt: &Stmt,
        else_stmt: Option<&Stmt>,
    ) -> Result<()> {
        let false_jumps = self.emit_condition_false_jumps(condition)?;

        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(then_stmt)?;
        self.local_rebind_suppression -= 1;
        let then_returns = self.emitted_return;

        let jmp_end = (!then_returns).then(|| self.emit_jmp_placeholder());
        let else_start = self.function.code.len();
        self.patch_condition_false_jumps(false_jumps, else_start)?;

        self.emitted_return = false;
        if let Some(Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        }) = else_stmt
        {
            self.lower_defaulted_if_chain(name, default_value, condition, then_stmt, else_stmt.as_deref())?;
        } else {
            debug_assert!(else_stmt.is_none());
            self.local_rebind_suppression += 1;
            self.lower_assign(name, default_value)?;
            self.local_rebind_suppression -= 1;
        }
        let else_returns = self.emitted_return;

        if let Some(jmp_end) = jmp_end {
            let end = self.function.code.len();
            self.patch_jmp(jmp_end, end)?;
        }
        self.emitted_return = then_returns && else_returns;
        Ok(())
    }

    pub(super) fn try_lower_move2_assign_pair(&mut self, first: &Stmt, second: &Stmt) -> Result<bool> {
        let (
            Stmt::Assign {
                name: first_dst,
                value: first_value,
                ..
            },
            Stmt::Assign {
                name: second_dst,
                value: second_value,
                ..
            },
        ) = (first, second)
        else {
            return Ok(false);
        };
        let Some(first_src) = simple_local_expr_name(first_value) else {
            return Ok(false);
        };
        if first_src != second_dst {
            return Ok(false);
        }
        let Some(second_src) = simple_local_expr_name(second_value) else {
            return Ok(false);
        };
        if first_dst == first_src
            || self.cell_locals.contains(first_dst)
            || self.cell_locals.contains(first_src)
            || self.cell_locals.contains(second_src)
            || self.const_map_locals.contains_key(first_dst)
            || self.const_map_locals.contains_key(first_src)
            || self.const_map_locals.contains_key(second_src)
        {
            return Ok(false);
        }
        let Some(first_dst_reg) = self.locals.get(first_dst).copied() else {
            return Ok(false);
        };
        let Some(first_src_reg) = self.locals.get(first_src).copied() else {
            return Ok(false);
        };
        let Some(second_src_reg) = self.locals.get(second_src).copied() else {
            return Ok(false);
        };

        self.emit(Instr::abc(
            Opcode::Move2,
            checked_u8("move2 first dst", first_dst_reg)?,
            checked_u8("move2 shared slot", first_src_reg)?,
            checked_u8("move2 second src", second_src_reg)?,
        ));
        self.function
            .performance
            .copy_register_fact(first_dst_reg, first_src_reg);
        self.function
            .performance
            .copy_register_fact(first_src_reg, second_src_reg);
        self.clear_const_map_local(first_dst);
        self.clear_const_map_local(first_src);
        Ok(true)
    }

    pub(super) fn lower_define(&mut self, name: &str, value: &Expr) -> Result<()> {
        // NOTE: a `let` bound to a loop-cached literal must NOT alias the
        // shared cache register as the variable's home (the old fast path
        // here). A later reassignment (`let i = 1; … i += 1;`) re-binds the
        // local to a fresh register, but loop-body reads emitted *before*
        // the assignment keep loading the cache register on every back edge
        // — a silent miscompile (`sort_words`' inner scan never advanced).
        // The general path below still consumes the cache: the literal store
        // becomes a register move instead of a constant load.
        let watermark = self.next_reg;
        let slot = if let Some(slot) = self.locals.get(name).copied() {
            if self.active_loop_binding_slot(name) == Some(slot) || self.cell_locals.contains(name) {
                // A fresh binding must not write the old register in place:
                // it would clobber the counter the fused loop opcodes drive
                // (`for i { let i = …; }`), or overwrite a promoted cell that
                // earlier-emitted reads (a loop condition or a statement
                // before this `let`, re-executed on the back edge) still
                // load through.
                self.alloc_reg()
            } else {
                self.local_write_slot(slot).0
            }
        } else {
            self.alloc_reg()
        };
        if !self.try_lower_expr_to_register(slot, value)? {
            let value = self.lower_expr(value)?;
            let move_source = !self.is_current_local_slot(value);
            self.emit_move_with_policy(slot, value, "define local", move_source)?;
        }
        if self.top_level
            && let Some(global_slot) = self.global_names.get(name).copied()
        {
            self.emit_set_global(slot, global_slot)?;
        }
        self.record_const_map_local_from_expr(name, value)?;
        self.insert_fresh_local(name.to_string(), slot);
        self.next_reg = self.live_register_floor().max(watermark).max(slot + 1);
        Ok(())
    }

    pub(super) fn lower_val(&mut self, value: &LiteralVal) -> Result<u16> {
        if let Some(reg) = self.cached_loop_literal(value) {
            return Ok(reg);
        }
        let dst = self.alloc_reg();
        match value {
            LiteralVal::Nil => {
                self.emit(Instr::abc(Opcode::LoadNil, checked_u8("dst", dst)?, 0, 0));
                self.set_register_kind(dst, PerfValueKind::Nil);
            }
            LiteralVal::Bool(value) => self.emit(Instr::abc(
                Opcode::LoadBool,
                checked_u8("dst", dst)?,
                u8::from(*value),
                0,
            )),
            LiteralVal::Int(value) => {
                let k = self.push_int(*value)?;
                self.emit(Instr::abx(Opcode::LoadInt, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Int);
            }
            LiteralVal::Float(value) => {
                let k = self.push_float(*value)?;
                self.emit(Instr::abx(Opcode::LoadFloat, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Float);
            }
            value if value.as_str().is_some() => {
                let value = value.as_str().expect("checked string");
                if ShortStr::new(value).is_some() {
                    let k = self.push_string(value)?;
                    self.emit(Instr::abx(Opcode::LoadString, checked_u8("dst", dst)?, k));
                } else {
                    let k = self.push_heap_value(ConstHeapValue::LongString(value.into()))?;
                    self.emit(Instr::abx(Opcode::LoadHeapConst, checked_u8("dst", dst)?, k));
                }
                self.set_register_kind(dst, PerfValueKind::String);
            }
            other => {
                bail!(
                    "Compiler cannot materialize AST literal value yet: {}",
                    ast_literal_kind(other)
                );
            }
        }
        if matches!(value, LiteralVal::Bool(_)) {
            self.set_register_kind(dst, PerfValueKind::Bool);
        }
        Ok(dst)
    }

    pub(super) fn try_lower_dead_literal_expr(&mut self, expr: &Expr) -> Result<bool> {
        match expr {
            Expr::Paren(inner) => self.try_lower_dead_literal_expr(inner),
            Expr::Literal(value) if literal_dead_write_is_safe(value) => {
                self.lower_val(value)?;
                self.mark_last_dead_write();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
