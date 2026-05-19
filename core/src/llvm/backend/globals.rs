use super::*;

impl<'a> FunctionTranslator<'a> {
    pub(super) fn emit_load_global(&mut self, dst: u16, kidx: u16) -> Result<()> {
        let name = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let name_str = match name.as_str() {
            Some(s) => s,
            None => return Err(anyhow!("LoadGlobal expects string constant; found {:?}", name)),
        };
        if matches!(name_str, "__lk_call_method" | "__lk_call_method_named") {
            self.store_reg(dst, encoding::NIL_LITERAL)?;
            self.set_known(dst, Some(KnownReg::Global(name_str.to_string())));
            return Ok(());
        }
        let handle = self.intern_string_constant(kidx, name_str)?;
        self.require_helper(RuntimeHelper::LoadGlobal);
        let global = self.fresh("loadglobal");
        self.writer.line(format!(
            "{global} = call i64 @{}(i64 {handle})",
            RuntimeHelper::LoadGlobal.symbol()
        ));
        self.store_reg(dst, &global)?;
        let known = self
            .known_globals
            .get(name_str)
            .cloned()
            .unwrap_or_else(|| KnownReg::Global(name_str.to_string()));
        self.set_known(dst, Some(known));
        Ok(())
    }

    pub(super) fn emit_load_capture(&mut self, dst: u16, idx: u16) -> Result<()> {
        let specs = self
            .capture_specs
            .ok_or_else(|| anyhow!("LoadCapture c{} has no capture metadata in LLVM backend", idx))?;
        let spec = specs
            .get(idx as usize)
            .ok_or_else(|| anyhow!("capture index {} out of range in LLVM backend", idx))?;
        match spec {
            CaptureSpec::Global { name } => {
                let handle = self.intern_anonymous_string(name.as_str())?;
                self.require_helper(RuntimeHelper::LoadGlobal);
                let global = self.fresh("loadcapture_global");
                self.writer.line(format!(
                    "{global} = call i64 @{}(i64 {handle})",
                    RuntimeHelper::LoadGlobal.symbol()
                ));
                self.store_reg(dst, &global)?;
                self.set_known(dst, Some(KnownReg::Global(name.clone())));
                Ok(())
            }
            CaptureSpec::Const { kidx, .. } => {
                let value = self.load_const_value(*kidx)?;
                self.store_reg(dst, &value)?;
                Ok(())
            }
            CaptureSpec::Register { name, .. } => Err(anyhow!(
                "unsupported register capture `{}` in LLVM native closure p{}",
                name,
                idx
            )),
        }
    }

    pub(super) fn emit_define_global(&mut self, kidx: u16, src: u16) -> Result<()> {
        if self.capture_specs.is_some() {
            return Ok(());
        }
        let name = self
            .function
            .consts
            .get(kidx as usize)
            .ok_or_else(|| anyhow!("constant index {} out of range", kidx))?;
        let name_str = match name.as_str() {
            Some(s) => s,
            None => return Err(anyhow!("DefineGlobal expects string constant; found {:?}", name)),
        };
        let handle = self.intern_string_constant(kidx, name_str)?;
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::DefineGlobal);
        self.writer.line(format!(
            "call void @{}(i64 {handle}, i64 {value})",
            RuntimeHelper::DefineGlobal.symbol()
        ));
        if let Some(known) = self.known(src).cloned()
            && !matches!(known, KnownReg::ConstMap { .. })
        {
            self.known_globals.insert(name_str.to_string(), known);
        }
        Ok(())
    }

    pub(super) fn emit_to_iter(&mut self, dst: u16, src: u16) -> Result<()> {
        let known = self.known(src).cloned();
        if matches!(known, Some(KnownReg::StringLength { .. })) {
            self.store_reg(dst, encoding::NIL_LITERAL)?;
            self.set_known(dst, known);
            return Ok(());
        }
        let value = self.load_reg(src)?;
        self.require_helper(RuntimeHelper::ToIter);
        let result = self.fresh("toiter");
        self.writer.line(format!(
            "{result} = call i64 @{}(i64 {value})",
            RuntimeHelper::ToIter.symbol()
        ));
        self.store_reg(dst, &result)?;
        if matches!(
            known,
            Some(KnownReg::StringHandle { .. } | KnownReg::StringLength { .. })
        ) {
            self.set_known(dst, known);
        }
        Ok(())
    }
}
