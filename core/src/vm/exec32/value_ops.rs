use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal, ShortStr};

use super::{Executor32, heap_kind};

impl Executor32 {
    pub(super) fn to_runtime_string(&self, register: u8) -> Result<String> {
        match self.read(register)? {
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

    pub(super) fn write_string(&mut self, register: u8, value: String) -> Result<()> {
        let value = if let Some(short) = ShortStr::new(&value) {
            RuntimeVal::ShortStr(short)
        } else {
            RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value.into())))
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

    pub(super) fn runtime_value_to_list_values(&mut self, value: &RuntimeVal) -> Result<Option<Vec<RuntimeVal>>> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(None);
        };
        let list = match self
            .state
            .heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(list) => list.clone(),
            _ => return Ok(None),
        };
        Ok(Some(list.materialize_mixed(&mut self.state.heap)))
    }

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

    pub(super) fn runtime_value_from_string(&mut self, value: Arc<str>) -> RuntimeVal {
        if let Some(short) = ShortStr::new(&value) {
            RuntimeVal::ShortStr(short)
        } else {
            RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value)))
        }
    }
}
