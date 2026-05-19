use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn emit_call(&mut self, rf: u16, base: u16, argc: u8, retc: u8) -> Result<()> {
        if retc > 1 {
            return Err(anyhow!("multiple return values are not supported by the LLVM backend"));
        }
        if self.try_emit_direct_aot_call(rf, base, argc, retc)? {
            return Ok(());
        }
        if self.try_emit_method_call(rf, base, argc, retc)? {
            return Ok(());
        }
        let use_native_call = matches!(
            self.known(rf),
            Some(KnownReg::Global(name)) if matches!(name.as_str(), "print" | "println" | "panic")
        );
        let func = self.load_reg(rf)?;

        let (args_expr, len, stack_restore) = if argc == 0 {
            (String::from("null"), 0usize, None)
        } else {
            let len = argc as usize;
            let base_idx = base as usize;
            if base_idx + len > self.function.n_regs as usize {
                return Err(anyhow!("Call reads out of bounds registers"));
            }
            let stack_guard = self.fresh("stacksp");
            self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
            let array = self.fresh("callargs");
            self.writer.line(format!("{array} = alloca [{len} x i64], align 8"));
            for i in 0..len {
                let reg = base + i as u16;
                let value = match self.materialize_string_int_key(reg)? {
                    Some(value) => value,
                    None => self.load_reg(reg)?,
                };
                let slot = self.fresh("callarg");
                self.writer.line(format!(
                    "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}",
                    len = len,
                    idx = i
                ));
                self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
            }
            let ptr = self.fresh("callargv");
            self.writer.line(format!(
                "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0",
                len = len
            ));
            (ptr, len, Some(stack_guard))
        };

        let helper = if use_native_call {
            RuntimeHelper::CallNative
        } else {
            RuntimeHelper::Call
        };
        self.require_helper(helper);
        let result = self.fresh("callres");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {func}, i64* {args}, i64 {argc}, i64 {retc})",
            helper.symbol(),
            args = args_expr,
            argc = len,
            retc = retc
        ));
        if let Some(stack_guard) = stack_restore {
            self.writer
                .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        }
        if retc == 1 {
            self.store_reg(base, &result)?;
        }
        Ok(())
    }

    fn try_emit_method_call(&mut self, rf: u16, base: u16, argc: u8, retc: u8) -> Result<bool> {
        if argc != 3 {
            return Ok(false);
        }
        if !matches!(self.known(rf), Some(KnownReg::Global(name)) if name == "__lk_call_method") {
            return Ok(false);
        }
        let Some(KnownReg::StringHandle {
            handle: method_handle,
            text: method,
            ..
        }) = self.known(base + 1).cloned()
        else {
            return Ok(false);
        };
        let Some(KnownReg::List { base: args_base, len }) = self.known(base + 2).cloned() else {
            return Ok(false);
        };
        if self.try_emit_builtin_method_call(base, args_base, len, &method)? {
            return Ok(true);
        }

        let result_known = self.known_builtin_method_result(base, &method, len);
        let receiver = self.load_reg(base)?;
        let (args_expr, stack_restore) = self.emit_arg_array(args_base, len)?;
        self.require_helper(RuntimeHelper::CallMethod);
        let result = self.fresh("methodres");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {receiver}, i64 {method}, i64* {args}, i64 {argc}, i64 {retc})",
            RuntimeHelper::CallMethod.symbol(),
            method = method_handle,
            args = args_expr,
            argc = len
        ));
        if let Some(stack_guard) = stack_restore {
            self.writer
                .line(format!("call void @llvm.stackrestore(i8* {stack_guard})"));
        }
        if retc == 1 {
            self.store_reg(base, &result)?;
            self.set_known(base, result_known);
        }
        Ok(true)
    }

    fn known_builtin_method_result(&self, receiver: u16, method: &str, argc: u16) -> Option<KnownReg> {
        if argc != 0 {
            return None;
        }
        match (self.known(receiver), method) {
            (Some(KnownReg::Global(module)), "epoch" | "time") if module == "os" => Some(KnownReg::Int),
            _ => None,
        }
    }

    fn try_emit_builtin_method_call(&mut self, base: u16, args_base: u16, len: u16, method: &str) -> Result<bool> {
        match (method, len) {
            ("get", 2) if matches!(self.known(base), Some(KnownReg::Global(name)) if name == "map") => {
                self.emit_access(base, args_base, args_base + 1)?;
                Ok(true)
            }
            ("set", 2) => {
                self.emit_map_set(base, args_base, args_base + 1)?;
                Ok(true)
            }
            ("push", 1) => {
                self.emit_list_push(base, args_base)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn emit_arg_array(&mut self, base: u16, len: u16) -> Result<(String, Option<String>)> {
        if len == 0 {
            return Ok((String::from("null"), None));
        }
        let len_usize = len as usize;
        let base_idx = base as usize;
        if base_idx + len_usize > self.function.n_regs as usize {
            return Err(anyhow!("Call reads out of bounds registers"));
        }
        let stack_guard = self.fresh("stacksp");
        self.writer.line(format!("{stack_guard} = call i8* @llvm.stacksave()"));
        let array = self.fresh("callargs");
        self.writer.line(format!("{array} = alloca [{len} x i64], align 8"));
        for i in 0..len_usize {
            let reg = base + i as u16;
            let value = match self.materialize_string_int_key(reg)? {
                Some(value) => value,
                None => self.load_reg(reg)?,
            };
            let slot = self.fresh("callarg");
            self.writer.line(format!(
                "{slot} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 {idx}",
                len = len,
                idx = i
            ));
            self.writer.line(format!("store i64 {value}, i64* {slot}, align 8"));
        }
        let ptr = self.fresh("callargv");
        self.writer.line(format!(
            "{ptr} = getelementptr inbounds [{len} x i64], [{len} x i64]* {array}, i64 0, i64 0",
            len = len
        ));
        Ok((ptr, Some(stack_guard)))
    }
}
