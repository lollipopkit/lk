#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal, ShortStr, TypedList};
use crate::vm::{Module, VmContext};

use super::Executor;

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
        // A container renders the way `print` renders it. It used to be an
        // error — "object cannot be converted to string" — so
        //
        //     println(xs)            → [1,2]
        //     println("{}", xs)      → [1,2]
        //     println("${xs}")       → failed, at run time, after the type
        //                              checker had approved it
        //
        // Three ways to print one value, two of which worked. The reason on
        // record was a map's iteration order not being portable between the two
        // backends — but the other two paths already print maps, so the rule
        // was not buying that, and the AOT declines to *lower* an interpolated
        // container anyway, which is where portability is actually decided.
        if matches!(value, RuntimeVal::Obj(_)) {
            return crate::vm::exec::display::runtime_display_value(&value, &self.state.heap);
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
        let type_name = Arc::clone(object.type_name());
        let type_scope = object.type_scope().clone();
        let Some(ctx_ref) = ctx.as_deref_mut() else {
            return Ok(None);
        };
        let Some(impl_ref) = ctx_ref.trait_method(&type_scope, &type_name, "show").cloned() else {
            return Ok(None);
        };
        let result = crate::vm::call_trait_method(
            &impl_ref,
            crate::vm::TraitMethodRef {
                type_name: &type_name,
                method: "show",
            },
            value,
            None,
            &mut self.state,
            module,
            Some(ctx_ref),
        )?;
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
            other => bail!("ListJoin target must be list, got {:?}", HeapValue::type_name(other)),
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

    /// The **language type** name of a value, for an error message a program
    /// can print.
    ///
    /// `RuntimeVal::kind().type_name()` cannot answer this: a handle is
    /// `Object`, and its own doc says a caller that has the heap should reach
    /// for `HeapValue::type_name` instead. Every arithmetic/compare error
    /// message formatted the kind, so `"ab" - 1` said `String` and
    /// `"aaaaaaaaaa" - 1` said `Object` — the same type, two names, decided by
    /// whether the string fit in seven bytes. A list, map and set were all
    /// `Object` too.
    ///
    /// The representation is not secret — `RuntimeValKind::repr_name` exists and
    /// names itself — it is just not what an error about a *type* should say.
    #[cold]
    pub(super) fn value_type_name(&self, value: &RuntimeVal) -> &'static str {
        match value {
            RuntimeVal::Obj(handle) => self.state.heap.get(*handle).map_or("Object", |value| value.type_name()),
            other => other.kind().type_name(),
        }
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
                other => bail!(
                    "object cannot be converted to string: {:?}",
                    HeapValue::type_name(other)
                ),
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
