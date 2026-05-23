use anyhow::{Result, anyhow};

use super::{CallableValue, Val};
use crate::vm::VmContext;

impl Val {
    /// Call this value as a function with the given arguments.
    #[inline]
    fn call_with_mode(&self, args: &[Val], ctx: &mut VmContext, force_vm: bool) -> Result<Val> {
        let _ = force_vm;
        match self {
            Val::Obj(value) => call_callable(value.as_ref(), args, &[], ctx),
            _ => Err(anyhow!("{} is not a function", self.type_name())),
        }
    }

    #[inline]
    pub fn call(&self, args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        self.call_with_mode(args, ctx, false)
    }

    #[inline]
    pub fn call_vm(&self, args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        self.call_with_mode(args, ctx, true)
    }

    /// Call a function value with positional and named arguments.
    pub(super) fn call_named_with_mode(
        &self,
        pos: &[Val],
        named: &[(String, Val)],
        ctx: &mut VmContext,
        force_vm: bool,
    ) -> Result<Val> {
        let _ = force_vm;
        match self {
            Val::Obj(value) => call_callable(value.as_ref(), pos, named, ctx),
            _ => Err(anyhow!("{} is not a function", self.type_name())),
        }
    }

    pub fn call_named(&self, pos: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val> {
        self.call_named_with_mode(pos, named, ctx, false)
    }

    pub fn call_named_vm(&self, pos: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val> {
        self.call_named_with_mode(pos, named, ctx, true)
    }
}

fn call_callable(
    value: &crate::val::HeapValue,
    pos: &[Val],
    named: &[(String, Val)],
    ctx: &mut VmContext,
) -> Result<Val> {
    let _ = (pos, named, ctx);
    let crate::val::HeapValue::Callable(function) = value else {
        return Err(anyhow!("{} is not a function", value.type_name()));
    };
    match function {
        CallableValue::Runtime32(_) => Err(anyhow!(
            "Runtime32 callable cannot be called through Val::call; execute it in Executor32"
        )),
        CallableValue::RuntimeNative32 { .. } => Err(anyhow!(
            "RuntimeNative32 callable cannot be called through Val::call; execute it in Executor32"
        )),
        CallableValue::Aot(_) => Err(anyhow!("AOT callable is disabled during the Instr32 VM migration")),
        CallableValue::Closure { .. } => Err(anyhow!(
            "Instr32 callable cannot be called through Val::call; execute it in Executor32"
        )),
    }
}
