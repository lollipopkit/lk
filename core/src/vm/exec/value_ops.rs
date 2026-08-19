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
            None => bail!("object cannot be converted to string: {}", self.value_type_name(value)),
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
            bail!(
                "StringSplit target must be string, got {}",
                self.value_type_name(&target)
            );
        };
        let delimiter = *self.read(delimiter)?;
        let Some(delimiter) = self.runtime_value_to_string(&delimiter)? else {
            bail!(
                "StringSplit delimiter must be string, got {}",
                self.value_type_name(&delimiter)
            );
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
            bail!("ListJoin target must be list, got {}", self.value_type_name(&target));
        };
        let separator = *self.read(separator)?;
        let Some(separator) = self.runtime_value_to_string(&separator)? else {
            bail!(
                "ListJoin separator must be string, got {}",
                self.value_type_name(&separator)
            );
        };
        // Every element is written the way the language writes it anywhere else.
        //
        // This used to raise "ListJoin list must contain only strings" for any
        // carrier but `String` — so `[1, 2].join(",")` type-checked and then
        // failed at run time, while `"${[1, 2]}"` had been printing `[1,2]` all
        // along. The restriction was arbitrary in a language that renders every
        // value, and it did not stay put: the AOT lowering refuses `join` on
        // numeric carriers *because the VM refuses*, so one arbitrary rule became
        // a second one in another back end.
        //
        // `display_runtime_value` is that one renderer, so there is no second
        // spelling of "how does an Int look" to drift. The `String` carrier keeps
        // its direct path: it is already what the renderer would produce (a bare
        // string renders unquoted; only *inside* a container is it quoted), and
        // it avoids an allocation per element.
        let heap = &self.state.heap;
        let joined = match heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(values) => join_typed_list(values, heap, separator.as_ref()),
            // The other two sequence carriers. `join` reaching only `List` is
            // the same shape the comment above describes — one carrier's
            // arbitrary limit becoming the operator's.
            HeapValue::Bytes(bytes) => bytes
                .iter()
                .map(|byte| crate::vm::display_runtime_value(&RuntimeVal::Int(i64::from(*byte)), heap))
                .collect::<Vec<_>>()
                .join(separator.as_ref()),
            // A window joins the range it windows, through the same function
            // the list arm uses.
            HeapValue::Slice(slice) => match slice.source {
                RuntimeVal::Obj(source) => match heap.get(source) {
                    Some(HeapValue::List(values)) => {
                        let window = values.window(slice.start, slice.live_len(heap));
                        join_typed_list(&window, heap, separator.as_ref())
                    }
                    _ => String::new(),
                },
                _ => String::new(),
            },
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

    /// The executor's spelling of [`RuntimeVal::type_name_in`] — it has the
    /// heap, so callers do not thread it through.
    #[cold]
    pub(super) fn value_type_name(&self, value: &RuntimeVal) -> &str {
        value.type_name_in(&self.state.heap)
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
            // A container renders the way `print` renders it — the same
            // correction interpolation already took, and for the same reason.
            // The comment above `runtime_value_to_display_string` counts three
            // ways to print one value, two of which worked; `+` is a fourth,
            // and it was the one still failing:
            //
            //     println(xs)          → [1,2]
            //     println("{}", xs)    → [1,2]
            //     println("${xs}")     → [1,2]
            //     println("" + xs)     → failed, at run time
            //
            // A list operand never reaches here — a list wins over a string and
            // the answer is a list — so what this changes is `Set`, `Bytes`, a
            // window, a struct and a callable, none of which had any meaning
            // under `+` at all.
            //
            // The other caller is the "X is not a function" message, where
            // raising replaced the diagnostic with a worse one.
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.to_string()),
                _ => crate::vm::exec::display::runtime_display_value(value, &self.state.heap),
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

/// `xs.join(sep)` over a list's elements, whatever carrier holds them.
///
/// Its own function because three receivers need it — a list, a window over a
/// list, and (through its own byte rendering) `Bytes`. Every element is written
/// the way the language writes it anywhere else, through the one renderer, so
/// there is no second spelling of "how does an Int look". The `String` carrier
/// keeps its direct path: it is already what the renderer would produce (a bare
/// string renders unquoted; only *inside* a container is it quoted), and it
/// avoids an allocation per element.
fn join_typed_list(values: &TypedList, heap: &crate::val::HeapStore, separator: &str) -> String {
    match values {
        TypedList::String(values) => values
            .iter()
            .map(|value| value.as_ref())
            .collect::<Vec<_>>()
            .join(separator),
        TypedList::Int(values) => values
            .iter()
            .map(|value| crate::vm::display_runtime_value(&RuntimeVal::Int(*value), heap))
            .collect::<Vec<_>>()
            .join(separator),
        TypedList::Float(values) => values
            .iter()
            .map(|value| crate::vm::display_runtime_value(&RuntimeVal::Float(*value), heap))
            .collect::<Vec<_>>()
            .join(separator),
        TypedList::Bool(values) => values
            .iter()
            .map(|value| crate::vm::display_runtime_value(&RuntimeVal::Bool(*value), heap))
            .collect::<Vec<_>>()
            .join(separator),
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| crate::vm::display_runtime_value(value, heap))
            .collect::<Vec<_>>()
            .join(separator),
    }
}
