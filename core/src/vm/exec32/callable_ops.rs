use anyhow::{Result, anyhow, bail};

use crate::val::{CallableValue, HeapValue, RuntimeVal};
use crate::vm::Module32;

use super::{Executor32, checked_u8_count};

impl Executor32 {
    pub(super) fn load_function_value(
        &mut self,
        dst: u8,
        function_index: u16,
        module: Option<&Module32>,
    ) -> Result<()> {
        let function_index = function_index as u32;
        let module = module.ok_or_else(|| anyhow!("LoadFunction requires Module32 execution"))?;
        if module.functions.get(function_index as usize).is_none() {
            bail!("LoadFunction index {} out of bounds", function_index);
        }
        let value = RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures: Vec::new(),
        })));
        self.write(dst, value)
    }

    pub(super) fn make_closure_value(
        &mut self,
        dst: u8,
        function_index: u8,
        capture_base: u8,
        module: Option<&Module32>,
    ) -> Result<()> {
        let function_index = function_index as u32;
        let module = module.ok_or_else(|| anyhow!("MakeClosure requires Module32 execution"))?;
        let function = module
            .functions
            .get(function_index as usize)
            .ok_or_else(|| anyhow!("MakeClosure index {} out of bounds", function_index))?;
        let captures = self.read_register_range_owned(capture_base, checked_u8_count(function.capture_count)?)?;
        let value = RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
            function_index,
            captures,
        })));
        self.write(dst, value)
    }

    pub(super) fn load_native_value(&mut self, dst: u8, native_index: u16, module: Option<&Module32>) -> Result<()> {
        let native_index = native_index as usize;
        let module = module.ok_or_else(|| anyhow!("LoadNative requires Module32 execution"))?;
        let native = module
            .natives
            .get(native_index)
            .ok_or_else(|| anyhow!("LoadNative index {} out of bounds", native_index))?;
        let value = RuntimeVal::Obj(
            self.state
                .heap
                .alloc(HeapValue::Callable(CallableValue::RuntimeNative32 {
                    arity: native.arity,
                    function: native.function.clone(),
                })),
        );
        self.write(dst, value)
    }
}
