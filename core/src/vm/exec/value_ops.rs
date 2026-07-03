#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal, ShortStr, Type, TypedList};
use crate::vm::{Module, VmContext, call_runtime_value_runtime_with_receiver};

use super::{Executor, heap_kind};

impl Executor {
    pub(super) fn to_runtime_string(&self, register: u8) -> Result<String> {
        self.runtime_value_to_plain_string(self.read(register)?)
    }

    #[allow(clippy::wrong_self_convention)] // display conversion may allocate heap strings
    pub(super) fn to_runtime_string_with_display(
        &mut self,
        register: u8,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<String> {
        let value = *self.read(register)?;
        if let Some(text) = self.runtime_value_to_plain_string_maybe(&value)? {
            return Ok(text);
        }
        if let Some(text) = self.try_runtime_display_show(&value, module, ctx)? {
            return Ok(text);
        }
        self.runtime_value_to_plain_string(&value)
    }

    fn runtime_value_to_plain_string(&self, value: &RuntimeVal) -> Result<String> {
        match self.runtime_value_to_plain_string_maybe(value)? {
            Some(value) => Ok(value),
            None => bail!("object cannot be converted to string: {:?}", value.kind()),
        }
    }

    fn runtime_value_to_plain_string_maybe(&self, value: &RuntimeVal) -> Result<Option<String>> {
        match value {
            RuntimeVal::Nil => Ok(Some("nil".to_string())),
            RuntimeVal::Bool(value) => Ok(Some(value.to_string())),
            RuntimeVal::Int(value) => Ok(Some(value.to_string())),
            RuntimeVal::Float(value) => Ok(Some(value.to_string())),
            RuntimeVal::ShortStr(value) => Ok(Some(value.as_str().to_string())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(Some(value.to_string())),
                _ => Ok(None),
            },
        }
    }

    fn try_runtime_display_show(
        &mut self,
        value: &RuntimeVal,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<Option<String>> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(None);
        };
        let Some(HeapValue::Object(object)) = self.state.heap.get(*handle) else {
            return Ok(None);
        };
        let receiver_type = Type::Named(object.type_name.to_string());
        let Some(ctx_ref) = ctx.as_deref_mut() else {
            return Ok(None);
        };
        let Some(method) = ctx_ref
            .type_checker()
            .as_ref()
            .and_then(|tc| tc.registry().get_method(&receiver_type, "show").cloned())
        else {
            return Ok(None);
        };
        let result =
            call_runtime_value_runtime_with_receiver(method, value, &[], &mut self.state, module, Some(ctx_ref))?;
        self.runtime_value_to_plain_string_maybe(&result)
    }

    pub(super) fn write_string(&mut self, register: u8, value: String) -> Result<()> {
        let value = if let Some(short) = ShortStr::new(&value) {
            RuntimeVal::ShortStr(short)
        } else {
            RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(value.into())))
        };
        self.write(register, value)
    }

    pub(super) fn runtime_value_to_string(&self, value: &RuntimeVal) -> Result<Option<Arc<str>>> {
        match value {
            RuntimeVal::ShortStr(value) => Ok(Some(Arc::<str>::from(value.as_str()))),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(Some(value.clone())),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    pub(super) fn string_split(&mut self, dst: u8, target: u8, delimiter: u8) -> Result<()> {
        let target = *self.read(target)?;
        let Some(target) = self.runtime_value_to_string(&target)? else {
            bail!("StringSplit target must be string, got {:?}", target.kind());
        };
        let delimiter = *self.read(delimiter)?;
        let Some(delimiter) = self.runtime_value_to_string(&delimiter)? else {
            bail!("StringSplit delimiter must be string, got {:?}", delimiter.kind());
        };
        let values = target
            .split(delimiter.as_ref())
            .map(Arc::<str>::from)
            .collect::<Vec<_>>();
        let handle = self.alloc_heap_value(HeapValue::List(TypedList::String(values)));
        self.write(dst, RuntimeVal::Obj(handle))
    }

    pub(super) fn list_join(&mut self, dst: u8, target: u8, separator: u8) -> Result<()> {
        let target = *self.read(target)?;
        let RuntimeVal::Obj(handle) = target else {
            bail!("ListJoin target must be list, got {:?}", target.kind());
        };
        let separator = *self.read(separator)?;
        let Some(separator) = self.runtime_value_to_string(&separator)? else {
            bail!("ListJoin separator must be string, got {:?}", separator.kind());
        };
        let joined = match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(TypedList::String(values)) => values
                .iter()
                .map(|value| value.as_ref())
                .collect::<Vec<_>>()
                .join(separator.as_ref()),
            HeapValue::List(TypedList::Mixed(values)) => {
                let mut parts = Vec::with_capacity(values.len());
                for value in values {
                    let Some(value) = self.runtime_value_to_string(value)? else {
                        bail!("ListJoin list must contain only strings");
                    };
                    parts.push(value.to_string());
                }
                parts.join(separator.as_ref())
            }
            HeapValue::List(_) => bail!("ListJoin list must contain only strings"),
            other => bail!("ListJoin target must be list, got {:?}", heap_kind(other)),
        };
        self.write_string(dst, joined)
    }

    #[cold]
    pub(super) fn runtime_value_is_list(&self, value: &RuntimeVal) -> Result<bool> {
        if matches!(value, RuntimeVal::ShortStr(_)) {
            return Ok(true);
        }
        let RuntimeVal::Obj(handle) = value else {
            return Ok(false);
        };
        Ok(matches!(
            self.state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            HeapValue::List(_) | HeapValue::String(_)
        ))
    }

    #[cold]
    pub(super) fn runtime_value_is_heap_list(&self, value: &RuntimeVal) -> Result<bool> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(false);
        };
        Ok(matches!(
            self.state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            HeapValue::List(_)
        ))
    }

    #[cold]
    pub(super) fn runtime_value_is_map(&self, value: &RuntimeVal) -> Result<bool> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(false);
        };
        Ok(matches!(
            self.state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            HeapValue::Map(_)
        ))
    }

    #[cold]
    pub(super) fn runtime_value_display_string(&self, value: &RuntimeVal) -> Result<String> {
        match value {
            RuntimeVal::Nil => Ok("nil".to_string()),
            RuntimeVal::Bool(value) => Ok(value.to_string()),
            RuntimeVal::Int(value) => Ok(value.to_string()),
            RuntimeVal::Float(value) => Ok(value.to_string()),
            RuntimeVal::ShortStr(value) => Ok(value.as_str().to_string()),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.to_string()),
                other => bail!("object cannot be converted to string: {:?}", heap_kind(other)),
            },
        }
    }

    #[cold]
    pub(super) fn runtime_value_from_string(&mut self, value: Arc<str>) -> RuntimeVal {
        if let Some(short) = ShortStr::new(&value) {
            RuntimeVal::ShortStr(short)
        } else {
            RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(value)))
        }
    }
}
