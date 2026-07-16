use super::*;

impl Compiler {
    pub(super) fn lower_function_decl(&mut self, name: &str) -> Result<()> {
        let function = self.load_function_by_name(name)?;
        if self.top_level
            && let Some(slot) = self.global_names.get(name).copied()
        {
            self.emit_set_global(function, slot)?;
            return Ok(());
        }
        self.insert_local(name.to_string(), function);
        Ok(())
    }

    pub(super) fn lower_trait_decl(&mut self, name: &str, methods: &[(String, Type)]) -> Result<()> {
        let Some(helper) = self.try_load_callable_by_name("__lk_register_trait")? else {
            return Ok(());
        };
        let name = self.lower_val(&LiteralVal::from_str(name))?;
        let mut entries = Vec::with_capacity(methods.len());
        for (method_name, method_type) in methods {
            let method_name = self.lower_val(&LiteralVal::from_str(method_name))?;
            let method_type = self.lower_val(&LiteralVal::from_str(&method_type.display()))?;
            entries.push(self.materialize_list(vec![method_name, method_type])?);
        }
        let methods = self.materialize_list(entries)?;
        self.lower_call_window_regs(helper, &[name, methods])?;
        Ok(())
    }

    pub(super) fn lower_impl_decl(&mut self, trait_name: &str, target_type: &Type, methods: &[Stmt]) -> Result<()> {
        let Some(helper) = self.try_load_callable_by_name("__lk_register_trait_impl")? else {
            return Ok(());
        };
        let trait_name = self.lower_val(&LiteralVal::from_str(trait_name))?;
        let target_type_text = target_type.display();
        let target_type_reg = self.lower_val(&LiteralVal::from_str(&target_type_text))?;
        let mut entries = Vec::with_capacity(methods.len());
        for method in methods {
            let Stmt::Function {
                name,
                params,
                param_types,
                named_params,
                return_type,
                body,
            } = method
            else {
                bail!("Compiler impl block only supports function methods");
            };
            let method_name = self.lower_val(&LiteralVal::from_str(name))?;
            let method_value = self.compile_impl_method_function(params, named_params, body)?;
            let method_type = impl_method_type(target_type, params, param_types, named_params, return_type);
            let method_type = self.lower_val(&LiteralVal::from_str(&method_type.display()))?;
            entries.push(self.materialize_list(vec![method_name, method_value, method_type])?);
        }
        let methods = self.materialize_list(entries)?;
        self.lower_call_window_regs(helper, &[trait_name, target_type_reg, methods])?;
        Ok(())
    }

    pub(super) fn compile_impl_method_function(
        &mut self,
        params: &[String],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
    ) -> Result<u16> {
        let function_index = self
            .dynamic_function_base
            .checked_add(self.pending_functions.len() as u32)
            .ok_or_else(|| anyhow!("Compiler dynamic impl method index overflow"))?;
        let mut compiled = Self::compile_function_body(
            params,
            named_params,
            body,
            self.function_names.clone(),
            self.function_signatures.clone(),
            self.function_bodies.clone(),
            self.native_names.clone(),
            self.global_names.clone(),
            self.user_let_globals.clone(),
            HashMap::new(),
            function_index + 1,
        )?;
        let dst = self.alloc_reg();
        self.emit(Instr::abx(
            Opcode::LoadFunction,
            checked_u8("impl method function dst", dst)?,
            u16::try_from(function_index)
                .map_err(|_| anyhow!("Compiler impl method index {function_index} exceeds u16"))?,
        ));
        self.function.performance.set_register_fact(
            dst,
            PerfRegisterFact {
                callable: PerfCallTargetKind::Closure,
                ..PerfRegisterFact::default()
            },
        );
        self.pending_functions.push(compiled.function);
        self.pending_functions.append(&mut compiled.pending_functions);
        Ok(dst)
    }

    pub(super) fn load_callable_by_name(&mut self, name: &str) -> Result<u16> {
        self.try_load_callable_by_name(name)?
            .ok_or_else(|| anyhow!("Compiler undefined callable `{name}`"))
    }

    pub(super) fn try_load_callable_by_name(&mut self, name: &str) -> Result<Option<u16>> {
        if self.function_names.contains_key(name) {
            return self.load_function_by_name(name).map(Some);
        }
        if self.native_names.contains_key(name) {
            return self.load_native_by_name(name).map(Some);
        }
        if let Some(slot) = self.global_names.get(name).copied() {
            return self.emit_get_global(slot).map(Some);
        }
        Ok(None)
    }

    pub(super) fn load_function_by_name(&mut self, name: &str) -> Result<u16> {
        let function_index = *self
            .function_names
            .get(name)
            .ok_or_else(|| anyhow!("Compiler undefined function `{name}`"))?;
        let dst = self.alloc_reg();
        let function_index = u16::try_from(function_index)
            .map_err(|_| anyhow!("Compiler function index {function_index} exceeds u16"))?;
        self.emit(Instr::abx(
            Opcode::LoadFunction,
            checked_u8("function dst", dst)?,
            function_index,
        ));
        self.function.performance.set_register_fact(
            dst,
            PerfRegisterFact {
                callable: PerfCallTargetKind::Closure,
                ..PerfRegisterFact::default()
            },
        );
        Ok(dst)
    }

    pub(super) fn load_native_by_name(&mut self, name: &str) -> Result<u16> {
        let native_index = *self
            .native_names
            .get(name)
            .ok_or_else(|| anyhow!("Compiler undefined native `{name}`"))?;
        let dst = self.alloc_reg();
        let native_index =
            u16::try_from(native_index).map_err(|_| anyhow!("Compiler native index {native_index} exceeds u16"))?;
        self.emit(Instr::abx(
            Opcode::LoadNative,
            checked_u8("native dst", dst)?,
            native_index,
        ));
        self.function.performance.set_register_fact(
            dst,
            PerfRegisterFact {
                callable: PerfCallTargetKind::Native,
                ..PerfRegisterFact::default()
            },
        );
        Ok(dst)
    }

    pub(super) fn emit_get_global(&mut self, slot: u32) -> Result<u16> {
        let dst = self.alloc_reg();
        let slot = u16::try_from(slot).map_err(|_| anyhow!("Compiler global slot {slot} exceeds u16"))?;
        let pc = self.function.code.len();
        self.emit(Instr::abx(Opcode::GetGlobal, checked_u8("global dst", dst)?, slot));
        self.function.performance.set_global_fact(
            pc,
            PerfGlobalFact {
                slot,
                move_source: false,
            },
        );
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    pub(super) fn emit_load_capture(&mut self, capture: u16) -> Result<u16> {
        let dst = self.alloc_reg();
        self.emit(Instr::abx(
            Opcode::LoadCapture,
            checked_u8("capture dst", dst)?,
            capture,
        ));
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    pub(super) fn emit_load_cell_value(&mut self, cell: u16) -> Result<u16> {
        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::LoadCellVal,
            checked_u8("cell value dst", dst)?,
            checked_u8("cell value src", cell)?,
            0,
        ));
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    pub(super) fn lower_capture_value(&mut self, name: &str) -> Result<(u16, bool)> {
        if let Some(local) = self.locals.get(name).copied() {
            // A `for` loop variable cannot be re-bound to a cell (the fused
            // loop opcodes drive the raw register): each capture snapshots
            // the current value into a fresh cell — per-iteration binding.
            // Only the loop's own binding slot qualifies: a same-named fresh
            // `let` in the body is an ordinary local and promotes normally.
            if self.active_loop_binding_slot(name) == Some(local) {
                let cell = self.emit_upval_cell_with_policy(local, false)?;
                return Ok((cell, true));
            }
            if self.cell_locals.insert(name.to_string()) {
                let cell = self.emit_upval_cell(local)?;
                self.emit_move(local, cell, "box captured local")?;
            }
            return Ok((local, true));
        }
        if let Some(capture) = self.capture_names.get(name).copied() {
            let value = self.emit_load_capture(capture)?;
            return Ok((value, self.capture_cells.contains(name)));
        }
        let value = self.lower_var(name)?;
        Ok((value, false))
    }

    pub(super) fn emit_upval_cell(&mut self, src: u16) -> Result<u16> {
        self.emit_upval_cell_with_policy(src, true)
    }

    /// `move_value: false` keeps `src` intact — the snapshot capture of a
    /// loop variable copies the counter into the cell (the fused loop opcode
    /// keeps driving the raw register afterwards).
    pub(super) fn emit_upval_cell_with_policy(&mut self, src: u16, move_value: bool) -> Result<u16> {
        let dst = self.alloc_reg();
        let k = self.push_heap_value(ConstHeapValue::UpvalCell(Box::new(ConstRuntimeValue::Nil)))?;
        self.emit(Instr::abx(Opcode::LoadHeapConst, checked_u8("upval cell dst", dst)?, k));
        self.emit_store_cell_value_with_policy(dst, src, "upval cell", move_value)?;
        Ok(dst)
    }

    pub(super) fn emit_set_global(&mut self, src: u16, slot: u32) -> Result<()> {
        self.emit_set_global_with_policy(src, slot, false)
    }

    pub(super) fn emit_set_global_with_policy(&mut self, src: u16, slot: u32, move_source: bool) -> Result<()> {
        let slot = u16::try_from(slot).map_err(|_| anyhow!("Compiler global slot {slot} exceeds u16"))?;
        let pc = self.function.code.len();
        self.emit(Instr::abx(Opcode::SetGlobal, checked_u8("global src", src)?, slot));
        self.function
            .performance
            .set_global_fact(pc, PerfGlobalFact { slot, move_source });
        Ok(())
    }

    pub(super) fn collect_closure_captures(&self, params: &[String], body: &Expr) -> Vec<String> {
        let mut bound = HashSet::with_capacity(params.len());
        for param in params {
            bound.insert(param.clone());
        }
        let mut free = Vec::new();
        collect_expr_free_vars(body, &mut bound, &mut free);
        let mut seen = HashSet::new();
        let mut captures = Vec::new();
        for name in free {
            let captures_local = self.locals.contains_key(&name);
            let captures_outer = self.capture_names.contains_key(&name) && !self.global_names.contains_key(&name);
            if (captures_local || captures_outer)
                && !self.function_names.contains_key(&name)
                && !self.native_names.contains_key(&name)
                && seen.insert(name.clone())
            {
                captures.push(name);
            }
        }
        captures
    }
}
