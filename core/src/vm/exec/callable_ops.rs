use anyhow::{Result, anyhow, bail};
use std::sync::Arc;

use crate::val::{CallableValue, HeapValue, RuntimeVal};
use crate::vm::Module;

use super::Executor;

impl Executor {
    #[cold]
    pub(super) fn load_function_value(&mut self, dst: u8, function_index: u16, module: Option<&Module>) -> Result<()> {
        let function_index = function_index as u32;
        let module = module.ok_or_else(|| anyhow!("LoadFunction requires Module execution"))?;
        if module.functions.get(function_index as usize).is_none() {
            bail!("LoadFunction index {} out of bounds", function_index);
        }
        let value = RuntimeVal::Obj(self.alloc_heap_value(HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures: Arc::new(Vec::new()),
        })));
        self.write(dst, value)
    }

    #[cold]
    pub(super) fn make_closure_value(
        &mut self,
        dst: u8,
        function_index: u8,
        capture_base: u8,
        module: Option<&Module>,
    ) -> Result<()> {
        let function_index = function_index as u32;
        let module = module.ok_or_else(|| anyhow!("MakeClosure requires Module execution"))?;
        let function = module
            .functions
            .get(function_index as usize)
            .ok_or_else(|| anyhow!("MakeClosure index {} out of bounds", function_index))?;
        let captures = self.capture_values(capture_base, function.capture_count)?;
        let value = RuntimeVal::Obj(self.alloc_heap_value(HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures: Arc::new(captures),
        })));
        self.write(dst, value)
    }

    #[cold]
    pub(super) fn load_native_value(&mut self, dst: u8, native_index: u16, module: Option<&Module>) -> Result<()> {
        let native_index = native_index as usize;
        let module = module.ok_or_else(|| anyhow!("LoadNative requires Module execution"))?;
        let native = module
            .natives
            .get(native_index)
            .ok_or_else(|| anyhow!("LoadNative index {} out of bounds", native_index))?;
        let value = RuntimeVal::Obj(self.alloc_heap_value(HeapValue::Callable(CallableValue::RuntimeNative {
            name: Arc::<str>::from(native.name.as_str()),
            arity: native.arity,
            function: native.function.clone(),
        })));
        self.write(dst, value)
    }

    fn capture_values(&self, base: u8, count: u16) -> Result<Vec<RuntimeVal>> {
        let count = usize::from(count);
        if usize::from(base) + count > usize::from(self.register_count) {
            bail!("capture range {}..{} out of bounds", base, usize::from(base) + count);
        }
        let start = self.frame_base + usize::from(base);
        let mut captures = Vec::with_capacity(count);
        for value in &self.state.stack[start..start + count] {
            captures.push(value.clone());
        }
        Ok(captures)
    }
}
