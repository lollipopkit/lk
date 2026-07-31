use super::*;

impl Compiler {
    pub(super) fn lower_function_decl(&mut self, name: &str) -> Result<()> {
        // Publishing a top-level `fn` to its global slot goes through one
        // shared register — see `Compiler::fn_publish_reg` for why it is shared
        // rather than recycled. Every declaration overwrites it and then stores
        // it, so nothing ever reads a stale value out of it.
        if self.top_level
            && let Some(slot) = self.global_names.get(name).copied()
        {
            let dst = match self.fn_publish_reg {
                Some(reg) => reg,
                None => {
                    let reg = self.alloc_reg();
                    self.fn_publish_reg = Some(reg);
                    reg
                }
            };
            self.load_function_into(dst, name)?;
            self.emit_set_global(dst, slot)?;
            return Ok(());
        }
        // A declaration that binds a *local* — nested inside a function, or a
        // name this module does not export — needs a register of its own,
        // because the binding is the register.
        let function = self.load_function_by_name(name)?;
        self.insert_local(name.to_string(), function);
        Ok(())
    }

    pub(super) fn lower_trait_decl(&mut self, name: &str, methods: &[(String, Type)]) -> Result<()> {
        // Record the declaration structurally first: it is valid module type
        // info whether or not the runtime registration helper is available.
        self.type_info.traits.push(crate::vm::TraitDecl {
            name: name.to_string(),
            methods: methods
                .iter()
                .map(|(method_name, method_type)| (method_name.clone(), method_type.display()))
                .collect(),
        });
        // Nothing to emit: the declaration is module data, carried by
        // `Module::type_info` and read by whoever needs it.
        Ok(())
    }

    pub(super) fn lower_impl_decl(
        &mut self,
        trait_name: Option<&str>,
        target_type: &Type,
        methods: &[Stmt],
    ) -> Result<()> {
        let target_type_text = target_type.display();
        let mut decl_methods = Vec::with_capacity(methods.len());
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
            // The compiled body's index is the durable identity of this method;
            // the registration call below only re-encodes it as a runtime value.
            let function_index = self.compile_impl_method_function_indexed(
                params,
                param_types,
                named_params,
                body,
                &alloc::format!("{target_type_text}::{name}"),
            )?;
            let method_type = impl_method_type(target_type, params, param_types, named_params, return_type);
            let method_type_text = method_type.display();
            decl_methods.push(crate::vm::ImplMethod {
                name: name.clone(),
                function: function_index,
                ty: method_type_text.clone(),
                // Filled in by `record_impl_method_global_use` once every
                // function exists; a method can call one compiled after it.
                writes_globals: false,
                reads_globals: Vec::new(),
            });
        }
        self.type_info.impls.push(crate::vm::ImplDecl {
            trait_name: trait_name.map(str::to_string),
            type_name: target_type_text,
            methods: decl_methods,
        });
        Ok(())
    }

    /// Compiles an impl method body into the module's function table and
    /// returns its index — the method's identity in `Module::type_info`.
    ///
    /// Nothing is emitted into the enclosing function: an `impl` block is a
    /// declaration, so after dropping the runtime registration call it
    /// contributes no instructions at all.
    pub(super) fn compile_impl_method_function_indexed(
        &mut self,
        params: &[String],
        param_types: &[Option<crate::val::Type>],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
        debug_name: &str,
    ) -> Result<u32> {
        let function_index = self
            .dynamic_function_base
            .checked_add(self.pending_functions.len() as u32)
            .ok_or_else(|| anyhow!("Compiler dynamic impl method index overflow"))?;
        let mut compiled = Self::compile_function_body(
            params,
            param_types,
            named_params,
            body,
            self.function_names.clone(),
            self.function_signatures.clone(),
            self.function_bodies.clone(),
            self.native_names.clone(),
            self.global_names.clone(),
            self.user_let_globals.clone(),
            self.top_level_data_globals.clone(),
            self.function_machine_returns.clone(),
            self.struct_field_machine_widths.clone(),
            self.impl_method_names.clone(),
            self.global_machine_widths.clone(),
            HashMap::new(),
            function_index + 1,
        )?;
        // `Type::method`, so a diagnostic about this function can name it.
        // Impl methods carried no name at all, and every AOT blocker inside one
        // read as a bare `an operand at pc 1 …` with nothing to look up.
        compiled.function.debug_name = Some(alloc::sync::Arc::<str>::from(debug_name));
        self.pending_functions.push(compiled.function);
        self.pending_functions.append(&mut compiled.pending_functions);
        Ok(function_index)
    }

    pub(super) fn load_callable_by_name(&mut self, name: &str) -> Result<u16> {
        if let Some(loaded) = self.try_load_callable_by_name(name)? {
            return Ok(loaded);
        }
        // The one case with a rule behind it rather than a typo: the name being
        // called *is* the binding currently being initialized, so a lambda is
        // trying to call itself. `let fact = |n| … fact(n - 1) …;` reported
        // "undefined callable `fact`" — a sentence about an operand, for a rule
        // about scope, with nothing to do about it.
        if self.initializing_binding.as_deref() == Some(name) {
            bail!(
                "`{name}` is not in scope inside its own initializer, so this closure cannot call itself; \
                 write a recursive function as a top-level `fn {name}(…)`"
            );
        }
        bail!("undefined function `{name}`{}", self.suggest_known_name(name))
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
        let dst = self.alloc_reg();
        self.load_function_into(dst, name)?;
        Ok(dst)
    }

    /// As [`Self::load_function_by_name`], into a register the caller chose.
    pub(super) fn load_function_into(&mut self, dst: u16, name: &str) -> Result<()> {
        let function_index = *self
            .function_names
            .get(name)
            .ok_or_else(|| anyhow!("Compiler undefined function `{name}`"))?;
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
        Ok(())
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
        self.emit_get_global_named(slot, None)
    }

    /// As [`Self::emit_get_global`], told which name it is reading.
    ///
    /// The name is what makes a declared width usable: a top-level
    /// `const MASK: u32` reads through `GetGlobal` into a fresh register, and
    /// the register is where every machine-integer rule looks. Callers that do
    /// not have a name pass `None` and get the old behaviour.
    pub(super) fn emit_get_global_named(&mut self, slot: u32, name: Option<&str>) -> Result<u16> {
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
        // The declared width of the global, onto the register it landed in.
        if let Some(kind) = name.and_then(|name| self.global_machine_widths.get(name).copied()) {
            self.machine_regs.insert(dst, kind);
        }
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
