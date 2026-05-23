use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, ShortStr, TypedList, TypedMap};

use super::{Executor32, heap_kind, remove_runtime_entry, set_list_value};

impl Executor32 {
    pub(super) fn build_int_range(&self, base: u8, inclusive: bool) -> Result<Vec<i64>> {
        let start = self.read_int(base)?;
        let end = self.read_int(base.checked_add(1).ok_or_else(|| anyhow!("range base overflow"))?)?;
        let step = self.read_int(base.checked_add(2).ok_or_else(|| anyhow!("range base overflow"))?)?;
        if step == 0 {
            bail!("Range step cannot be zero");
        }

        let mut out = Vec::new();
        let mut current = start;
        if step > 0 {
            while if inclusive { current <= end } else { current < end } {
                out.push(current);
                current = current
                    .checked_add(step)
                    .ok_or_else(|| anyhow!("Range step overflow"))?;
            }
        } else {
            while if inclusive { current >= end } else { current > end } {
                out.push(current);
                current = current
                    .checked_add(step)
                    .ok_or_else(|| anyhow!("Range step overflow"))?;
            }
        }
        Ok(out)
    }

    pub(super) fn read_map_entries(&self, base: u8, count: u8) -> Result<BTreeMap<RuntimeMapKey, RuntimeVal>> {
        let mut values = BTreeMap::new();
        for entry in 0..count {
            let key_reg = base
                .checked_add(entry.checked_mul(2).expect("map entry register overflow"))
                .ok_or_else(|| anyhow!("map key register overflow"))?;
            let value_reg = key_reg
                .checked_add(1)
                .ok_or_else(|| anyhow!("map value register overflow"))?;
            let key = self.map_key_from_register(key_reg)?;
            let value = self.read(value_reg)?.clone();
            values.insert(key, value);
        }
        Ok(values)
    }

    pub(super) fn read_object_fields(&self, base: u8, count: u8) -> Result<RuntimeObject> {
        let type_name = Arc::<str>::from(self.to_runtime_string(base)?);
        let field_base = base
            .checked_add(1)
            .ok_or_else(|| anyhow!("object field base overflow"))?;
        let mut fields = BTreeMap::new();
        for entry in 0..count {
            let offset = entry
                .checked_mul(2)
                .ok_or_else(|| anyhow!("object field register overflow"))?;
            let key_reg = field_base
                .checked_add(offset)
                .ok_or_else(|| anyhow!("object key register overflow"))?;
            let value_reg = key_reg
                .checked_add(1)
                .ok_or_else(|| anyhow!("object value register overflow"))?;
            fields.insert(
                Arc::<str>::from(self.to_runtime_string(key_reg)?),
                self.read(value_reg)?.clone(),
            );
        }
        Ok(RuntimeObject { type_name, fields })
    }

    pub(super) fn get_index(&mut self, target_reg: u8, key_reg: u8) -> Result<RuntimeVal> {
        match self.read(target_reg)?.clone() {
            RuntimeVal::ShortStr(value) => {
                let value = Arc::<str>::from(value.as_str());
                self.index_string(&value, key_reg)
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::List(list) => self.index_list(&list, key_reg),
                HeapValue::Map(map) => {
                    let key = self.map_key_from_register(key_reg)?;
                    Ok(self.lookup_map(&map, &key)?.unwrap_or(RuntimeVal::Nil))
                }
                HeapValue::Object(object) => {
                    let key = self.object_key_from_register(key_reg)?;
                    Ok(object.fields.get(&key).cloned().unwrap_or(RuntimeVal::Nil))
                }
                HeapValue::String(value) => self.index_string(&value, key_reg),
                other => bail!("GetIndex target object is not indexable: {:?}", heap_kind(&other)),
            },
            other => bail!("GetIndex target expected Obj, got {:?}", other.kind()),
        }
    }

    pub(super) fn len_value(&self, register: u8) -> Result<usize> {
        match self.read(register)? {
            RuntimeVal::ShortStr(value) => Ok(value.as_str().chars().count()),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.chars().count()),
                HeapValue::List(value) => Ok(value.len()),
                HeapValue::Map(value) => Ok(value.len()),
                other => bail!("Len target object is not sized: {:?}", heap_kind(other)),
            },
            other => bail!("Len target expected string/list/map, got {:?}", other.kind()),
        }
    }

    pub(super) fn contains_value(&self, needle_reg: u8, haystack_reg: u8) -> Result<bool> {
        let needle = self.read(needle_reg)?.clone();
        match self.read(haystack_reg)?.clone() {
            RuntimeVal::ShortStr(haystack) => {
                let Some(needle) = self.runtime_value_to_string(&needle)? else {
                    return Ok(false);
                };
                Ok(haystack.as_str().contains(needle.as_ref()))
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(haystack) => {
                    let Some(needle) = self.runtime_value_to_string(&needle)? else {
                        return Ok(false);
                    };
                    Ok(haystack.contains(needle.as_ref()))
                }
                HeapValue::List(values) => self.list_contains(values, &needle),
                HeapValue::Map(values) => self.map_contains(values, &needle),
                other => bail!("Contains haystack object is not searchable: {:?}", heap_kind(other)),
            },
            other => bail!("Contains haystack expected string/list/map, got {:?}", other.kind()),
        }
    }

    pub(super) fn slice_from(&mut self, target_reg: u8, start_reg: u8) -> Result<RuntimeVal> {
        let start = usize::try_from(self.read_int(start_reg)?)
            .map_err(|_| anyhow!("SliceFrom start index must be non-negative"))?;
        match self.read(target_reg)?.clone() {
            RuntimeVal::ShortStr(value) => self.slice_string_from(Arc::<str>::from(value.as_str()), start),
            RuntimeVal::Obj(handle) => {
                let value = self
                    .state
                    .heap
                    .get(handle)
                    .cloned()
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
                match value {
                    HeapValue::List(values) => Ok(RuntimeVal::Obj(
                        self.state.heap.alloc(HeapValue::List(values.slice_from(start))),
                    )),
                    HeapValue::String(value) => self.slice_string_from(value.clone(), start),
                    other => bail!("SliceFrom target object is not sliceable: {:?}", heap_kind(&other)),
                }
            }
            other => bail!("SliceFrom target expected string/list object, got {:?}", other.kind()),
        }
    }

    fn slice_string_from(&mut self, value: Arc<str>, start: usize) -> Result<RuntimeVal> {
        let suffix = value.chars().skip(start).collect::<String>();
        Ok(self.runtime_value_from_string(Arc::<str>::from(suffix)))
    }

    pub(super) fn map_rest(&mut self, base: u8, key_count: u8) -> Result<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read(base)?.clone() else {
            bail!("MapRest base expected map object");
        };
        let source = self
            .state
            .heap
            .get(handle)
            .cloned()
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        let HeapValue::Map(map) = &source else {
            bail!("MapRest source object is not a map: {:?}", heap_kind(&source));
        };

        let mut entries = map
            .entries_into_heap(&mut self.state.heap)?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        for offset in 0..key_count {
            let key_reg = base
                .checked_add(1)
                .and_then(|reg| reg.checked_add(offset))
                .ok_or_else(|| anyhow!("MapRest key register overflow"))?;
            let key = self.map_key_from_register(key_reg)?;
            remove_runtime_entry(&mut entries, &key);
        }
        Ok(RuntimeVal::Obj(
            self.state
                .heap
                .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
        ))
    }

    fn list_contains(&self, values: &TypedList, needle: &RuntimeVal) -> Result<bool> {
        Ok(match values {
            TypedList::Mixed(values) => values.iter().any(|value| value == needle),
            TypedList::Int(values) => matches!(needle, RuntimeVal::Int(needle) if values.contains(needle)),
            TypedList::Float(values) => matches!(needle, RuntimeVal::Float(needle) if values.contains(needle)),
            TypedList::Bool(values) => matches!(needle, RuntimeVal::Bool(needle) if values.contains(needle)),
            TypedList::String(values) => {
                let Some(needle) = self.runtime_value_to_string(needle)? else {
                    return Ok(false);
                };
                values.iter().any(|value| value.as_ref() == needle.as_ref())
            }
        })
    }

    fn map_contains(&self, values: &TypedMap, needle: &RuntimeVal) -> Result<bool> {
        Ok(match values {
            TypedMap::Mixed(values) => {
                let key = self.runtime_map_key_from_value(needle)?;
                values.contains_key(&key)
            }
            TypedMap::StringMixed(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringInt(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringFloat(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringBool(values) => self.string_map_contains_key(values, needle)?,
        })
    }

    pub(super) fn to_iter(&mut self, register: u8) -> Result<RuntimeVal> {
        match self.read(register)?.clone() {
            RuntimeVal::ShortStr(value) => {
                let list = value
                    .as_str()
                    .chars()
                    .map(|ch| Arc::<str>::from(ch.to_string()))
                    .collect();
                Ok(RuntimeVal::Obj(
                    self.state.heap.alloc(HeapValue::List(TypedList::String(list))),
                ))
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::List(_) => Ok(RuntimeVal::Obj(handle)),
                HeapValue::String(value) => {
                    let list = value.chars().map(|ch| Arc::<str>::from(ch.to_string())).collect();
                    Ok(RuntimeVal::Obj(
                        self.state.heap.alloc(HeapValue::List(TypedList::String(list))),
                    ))
                }
                HeapValue::Map(map) => self.map_to_iter_list(&map),
                other => bail!("ToIter target object is not iterable: {:?}", heap_kind(&other)),
            },
            other => bail!("ToIter target expected string/list/map, got {:?}", other.kind()),
        }
    }

    fn map_to_iter_list(&mut self, map: &TypedMap) -> Result<RuntimeVal> {
        let mut pairs = Vec::with_capacity(map.len());
        for (key, value) in map.entries_into_heap(&mut self.state.heap)? {
            let key = self.runtime_map_key_to_value(key);
            let pair = HeapValue::List(TypedList::from_runtime_values(vec![key, value], &self.state.heap));
            pairs.push(RuntimeVal::Obj(self.state.heap.alloc(pair)));
        }
        Ok(RuntimeVal::Obj(
            self.state.heap.alloc(HeapValue::List(TypedList::Mixed(pairs))),
        ))
    }

    fn runtime_map_key_to_value(&mut self, key: RuntimeMapKey) -> RuntimeVal {
        match key {
            RuntimeMapKey::Nil => RuntimeVal::Nil,
            RuntimeMapKey::Bool(value) => RuntimeVal::Bool(value),
            RuntimeMapKey::Int(value) => RuntimeVal::Int(value),
            RuntimeMapKey::ShortStr(value) => RuntimeVal::ShortStr(value),
            RuntimeMapKey::String(value) => {
                if let Some(short) = ShortStr::new(&value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value)))
                }
            }
            RuntimeMapKey::Obj(value) => RuntimeVal::Obj(value),
        }
    }

    pub(super) fn set_index(&mut self, target_reg: u8, key_reg: u8, value_reg: u8) -> Result<()> {
        let target = self.read(target_reg)?.clone();
        let value = self.read(value_reg)?.clone();
        let RuntimeVal::Obj(handle) = target else {
            bail!("SetIndex target expected Obj, got {:?}", target.kind());
        };
        let key = self.map_key_from_register(key_reg)?;

        if let Some(done) = self.try_set_string_list(handle, &key, value.clone())? {
            return Ok(done);
        }

        match self
            .state
            .heap
            .get_mut(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(list) => {
                let RuntimeMapKey::Int(index) = key else {
                    bail!("SetIndex list key must be Int");
                };
                let index = usize::try_from(index).map_err(|_| anyhow!("list index must be non-negative"))?;
                set_list_value(list, index, value)
            }
            HeapValue::Map(map) => {
                map.set(key, value);
                Ok(())
            }
            HeapValue::Object(object) => {
                let Some(key) = key.as_arc_str() else {
                    bail!("SetIndex object key must be string");
                };
                object.fields.insert(key, value);
                Ok(())
            }
            other => bail!("SetIndex target object is not writable: {:?}", heap_kind(other)),
        }
    }

    fn object_key_from_register(&self, register: u8) -> Result<Arc<str>> {
        match self.read(register)? {
            RuntimeVal::ShortStr(value) => Ok(Arc::<str>::from(value.as_str())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.clone()),
                other => bail!("object field key cannot be object: {:?}", heap_kind(other)),
            },
            other => bail!("object field key must be string, got {:?}", other.kind()),
        }
    }

    fn try_set_string_list(
        &mut self,
        handle: crate::val::HeapRef,
        key: &RuntimeMapKey,
        value: RuntimeVal,
    ) -> Result<Option<()>> {
        let RuntimeMapKey::Int(index) = key else {
            return Ok(None);
        };
        let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get(handle) else {
            return Ok(None);
        };
        let index = usize::try_from(*index).map_err(|_| anyhow!("list index must be non-negative"))?;
        if index >= values.len() {
            bail!("list index {} out of bounds", index);
        }

        if let Some(value) = self.runtime_value_to_string(&value)? {
            let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get_mut(handle) else {
                bail!("heap object {} changed while writing string list", handle.index());
            };
            values[index] = value;
            return Ok(Some(()));
        }

        let strings = values.clone();
        let mut mixed = Vec::with_capacity(strings.len());
        for value in strings {
            mixed.push(self.runtime_value_from_string(value));
        }
        mixed[index] = value;
        let Some(HeapValue::List(list)) = self.state.heap.get_mut(handle) else {
            bail!("heap object {} changed while materializing string list", handle.index());
        };
        *list = TypedList::Mixed(mixed);
        Ok(Some(()))
    }

    fn index_list(&mut self, list: &TypedList, key_reg: u8) -> Result<RuntimeVal> {
        let index = usize::try_from(self.read_int(key_reg)?).map_err(|_| anyhow!("list index must be non-negative"))?;
        Ok(match list {
            TypedList::Mixed(values) => values.get(index).cloned(),
            TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int),
            TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float),
            TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool),
            TypedList::String(values) => values.get(index).map(|value| {
                if let Some(short) = ShortStr::new(value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value.clone())))
                }
            }),
        }
        .unwrap_or(RuntimeVal::Nil))
    }

    fn index_string(&self, value: &Arc<str>, key_reg: u8) -> Result<RuntimeVal> {
        let index =
            usize::try_from(self.read_int(key_reg)?).map_err(|_| anyhow!("string index must be non-negative"))?;
        let Some(ch) = value.chars().nth(index) else {
            return Ok(RuntimeVal::Nil);
        };
        let mut buf = [0_u8; 4];
        let ch = ch.encode_utf8(&mut buf);
        if let Some(short) = ShortStr::new(ch) {
            Ok(RuntimeVal::ShortStr(short))
        } else {
            Ok(RuntimeVal::Nil)
        }
    }

    fn lookup_map(&mut self, map: &TypedMap, key: &RuntimeMapKey) -> Result<Option<RuntimeVal>> {
        map.get_into_heap(key, &mut self.state.heap)
    }

    fn map_key_from_register(&self, register: u8) -> Result<RuntimeMapKey> {
        self.runtime_map_key_from_value(self.read(register)?)
    }

    pub(super) fn runtime_map_key_from_value(&self, value: &RuntimeVal) -> Result<RuntimeMapKey> {
        match value {
            RuntimeVal::Nil => Ok(RuntimeMapKey::Nil),
            RuntimeVal::Bool(value) => Ok(RuntimeMapKey::Bool(*value)),
            RuntimeVal::Int(value) => Ok(RuntimeMapKey::Int(*value)),
            RuntimeVal::ShortStr(value) => Ok(RuntimeMapKey::ShortStr(*value)),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(RuntimeMapKey::String(value.clone())),
                other => bail!("object cannot be used as map key: {:?}", heap_kind(other)),
            },
            RuntimeVal::Float(_) => bail!("Float cannot be used as RuntimeMapKey"),
        }
    }

    fn runtime_value_to_key_string(&self, value: &RuntimeVal) -> Result<Option<Arc<str>>> {
        Ok(match value {
            RuntimeVal::Bool(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::Int(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::Float(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::ShortStr(value) => Some(Arc::<str>::from(value.as_str())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Some(value.clone()),
                _ => None,
            },
            RuntimeVal::Nil => None,
        })
    }

    fn string_map_contains_key<T>(&self, values: &BTreeMap<Arc<str>, T>, needle: &RuntimeVal) -> Result<bool> {
        let Some(key) = self.runtime_value_to_key_string(needle)? else {
            return Ok(false);
        };
        Ok(values.contains_key(key.as_ref()))
    }
}
