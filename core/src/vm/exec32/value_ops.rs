use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal, ShortStr, TypedList};

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

    pub(super) fn string_starts_with(&mut self, dst: u8, target: u8, prefix: u8) -> Result<()> {
        let target = self.read(target)?.clone();
        let Some(target) = self.runtime_value_to_string(&target)? else {
            bail!("StringStartsWith target must be string, got {:?}", target.kind());
        };
        let prefix = self.read(prefix)?.clone();
        let Some(prefix) = self.runtime_value_to_string(&prefix)? else {
            bail!("StringStartsWith prefix must be string, got {:?}", prefix.kind());
        };
        self.write(dst, RuntimeVal::Bool(target.starts_with(prefix.as_ref())))
    }

    pub(super) fn string_split(&mut self, dst: u8, target: u8, delimiter: u8) -> Result<()> {
        let target = self.read(target)?.clone();
        let Some(target) = self.runtime_value_to_string(&target)? else {
            bail!("StringSplit target must be string, got {:?}", target.kind());
        };
        let delimiter = self.read(delimiter)?.clone();
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
        let target = self.read(target)?.clone();
        let RuntimeVal::Obj(handle) = target else {
            bail!("ListJoin target must be list, got {:?}", target.kind());
        };
        let separator = self.read(separator)?.clone();
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
            RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(value)))
        }
    }
}
