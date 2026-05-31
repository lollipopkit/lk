use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal};
use crate::vm::analysis::{PerfIndexFact, PerfIndexTargetKind};

use super::{Executor32, IndexTargetKind, heap_kind, runtime_map_string_key};

impl Executor32 {
    pub(in crate::vm::exec32) fn get_index(
        &mut self,
        pc: usize,
        target_reg: u8,
        key_reg: u8,
        known_string_key: Option<Arc<str>>,
        index_fact: Option<PerfIndexFact>,
    ) -> Result<RuntimeVal> {
        if let RuntimeVal::Obj(h) = self.read(key_reg)?
            && let Some(HeapValue::List(list)) = self.state.heap.get(*h)
        {
            let items = list.collect_owned();
            if !items.is_empty() && items.len() <= 3 {
                let start = match &items[0] {
                    RuntimeVal::Int(i) => *i,
                    _ => 0i64,
                };
                let last = items
                    .last()
                    .and_then(|v| if let RuntimeVal::Int(i) = v { Some(*i) } else { None });
                return self.get_index_slice(target_reg, start, last.map(|i| i + 1), None);
            }
        }
        match self.read(target_reg)? {
            RuntimeVal::ShortStr(value) => {
                let idx_val = self.read(key_reg)?;
                let idx = match &idx_val {
                    RuntimeVal::Int(n) => {
                        let len = value.as_str().len() as i64;
                        if *n < 0 { (len + *n) as usize } else { *n as usize }
                    }
                    _ => bail!("String index must be Int"),
                };
                self.index_string_at(value.as_str(), idx)
            }
            RuntimeVal::Obj(handle) => self.get_heap_index(pc, *handle, key_reg, known_string_key.as_ref(), index_fact),
            other => bail!("GetIndex target expected Obj, got {:?}", other.kind()),
        }
    }

    fn get_heap_index(
        &mut self,
        pc: usize,
        handle: crate::val::HeapRef,
        key_reg: u8,
        known_string_key: Option<&Arc<str>>,
        index_fact: Option<PerfIndexFact>,
    ) -> Result<RuntimeVal> {
        let index_cache = match index_fact {
            Some(_) => None,
            None => self.cached_or_observed_index_cache(pc, handle, known_string_key)?,
        };
        let index_fact = index_fact.or_else(|| index_cache.map(|cache| cache.fact));
        let observed_kind = self.index_target_kind(handle)?;
        let target_kind = match index_fact.map(|fact| fact.target_kind) {
            Some(PerfIndexTargetKind::List) => IndexTargetKind::List,
            Some(PerfIndexTargetKind::Map) => IndexTargetKind::Map,
            Some(PerfIndexTargetKind::Object) => IndexTargetKind::Object,
            Some(PerfIndexTargetKind::String) => IndexTargetKind::String,
            Some(PerfIndexTargetKind::Unknown) | None => observed_kind,
        };
        let target_kind = if target_kind == observed_kind {
            target_kind
        } else {
            observed_kind
        };

        match target_kind {
            IndexTargetKind::List => {
                if let Some(pos) = self.negative_list_index(handle, key_reg) {
                    let orig_val = self.read(key_reg)?.clone();
                    self.write(key_reg, RuntimeVal::Int(pos as i64))?;
                    let result = self.index_list_handle(handle, key_reg, index_fact.map(|fact| fact.value_kind));
                    self.write(key_reg, orig_val)?;
                    return result;
                }
                self.index_list_handle(handle, key_reg, index_fact.map(|fact| fact.value_kind))
            }
            IndexTargetKind::Map => {
                if let Some(key) = known_string_key
                    && let Some(value) =
                        self.lookup_string_map_handle(handle, key, index_fact.map(|fact| fact.value_kind))?
                {
                    return Ok(value);
                }
                let key = match known_string_key {
                    Some(key) => runtime_map_string_key(key.clone()),
                    None => self.map_key_from_register(key_reg)?,
                };
                Ok(self.lookup_map_handle(handle, &key)?.unwrap_or(RuntimeVal::Nil))
            }
            IndexTargetKind::Object => {
                let key = match known_string_key {
                    Some(key) => key.clone(),
                    None => self.object_key_from_register(key_reg)?,
                };
                let field_slot = index_cache.and_then(|cache| cache.object_field_slot);
                Ok(self
                    .index_object_handle(handle, &key, field_slot)?
                    .unwrap_or(RuntimeVal::Nil))
            }
            IndexTargetKind::String => self.get_heap_string_index(handle, key_reg),
        }
    }

    fn negative_list_index(&self, handle: crate::val::HeapRef, key_reg: u8) -> Option<usize> {
        let n = self.read_int(key_reg).ok()?;
        if n >= 0 {
            return None;
        }
        let len = self.state.heap.get(handle).and_then(|v| match v {
            HeapValue::List(l) => Some(l.len()),
            _ => None,
        })?;
        Some(((len as i64) + n) as usize)
    }

    fn get_heap_string_index(&mut self, handle: crate::val::HeapRef, key_reg: u8) -> Result<RuntimeVal> {
        if let Some(pos) = self.negative_string_index(handle, key_reg) {
            let orig_val = self.read(key_reg)?.clone();
            self.write(key_reg, RuntimeVal::Int(pos as i64))?;
            let result = self.index_heap_string_at_key(handle, key_reg);
            self.write(key_reg, orig_val)?;
            return result;
        }
        self.index_heap_string_at_key(handle, key_reg)
    }

    fn negative_string_index(&self, handle: crate::val::HeapRef, key_reg: u8) -> Option<usize> {
        let n = self.read_int(key_reg).ok()?;
        if n >= 0 {
            return None;
        }
        let s = match self.state.heap.get(handle)? {
            HeapValue::String(value) => value,
            _ => return None,
        };
        Some(((s.len() as i64) + n) as usize)
    }

    fn index_heap_string_at_key(&self, handle: crate::val::HeapRef, key_reg: u8) -> Result<RuntimeVal> {
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => {
                let idx_val = self.read(key_reg)?;
                let idx = match &idx_val {
                    RuntimeVal::Int(n) => {
                        let len = value.len() as i64;
                        if *n < 0 { (len + *n) as usize } else { *n as usize }
                    }
                    _ => bail!("String index must be Int"),
                };
                self.index_string_at(value, idx)
            }
            other => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
        }
    }
}
