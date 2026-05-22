use std::collections::BTreeMap;

use anyhow::{Result, bail};

use crate::val::{CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, TypedList, TypedMap, Val};
use crate::vm::{NativeEntry32, NativeFunction32};

pub(super) fn legacy_val_to_runtime_val(
    name: &str,
    value: &Val,
    heap: &mut HeapStore,
    natives: &mut Vec<NativeEntry32>,
) -> Result<RuntimeVal> {
    match value {
        Val::Nil => Ok(RuntimeVal::Nil),
        Val::Bool(value) => Ok(RuntimeVal::Bool(*value)),
        Val::Int(value) => Ok(RuntimeVal::Int(*value)),
        Val::Float(value) => Ok(RuntimeVal::Float(*value)),
        Val::ShortStr(value) => Ok(RuntimeVal::ShortStr(*value)),
        value if value.as_list().is_some() => {
            let values = value.as_list().expect("checked list");
            let values = values
                .iter()
                .enumerate()
                .map(|(index, value)| legacy_val_to_runtime_val(&format!("{name}[{index}]"), value, heap, natives))
                .collect::<Result<Vec<_>>>()?;
            Ok(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))),
            ))
        }
        value if value.as_map().is_some() => {
            let values = value.as_map().expect("checked map");
            let mut entries = BTreeMap::new();
            for (key, value) in values.iter() {
                entries.insert(
                    RuntimeMapKey::String(key.as_str().into()),
                    legacy_val_to_runtime_val(&format!("{name}.{}", key.as_str()), value, heap, natives)?,
                );
            }
            Ok(RuntimeVal::Obj(
                heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
            ))
        }
        Val::Obj(value) => match value.as_ref() {
            HeapValue::Callable(CallableValue::ParsedClosure(_)) => {
                bail!("parsed closure stub cannot be imported into Instr32")
            }
            HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) => {
                runtime_native(name, *arity, function.clone(), heap, natives)
            }
            HeapValue::Callable(CallableValue::Aot(_)) | HeapValue::Callable(CallableValue::AotHandle { .. }) => {
                bail!("AOT callable cannot be imported into Instr32 yet")
            }
            HeapValue::Callable(CallableValue::Closure {
                function_index,
                captures,
            }) => Ok(RuntimeVal::Obj(heap.alloc(HeapValue::Callable(
                CallableValue::Closure {
                    function_index: *function_index,
                    captures: captures.clone(),
                },
            )))),
            HeapValue::Callable(CallableValue::Native { function_index, arity }) => Ok(RuntimeVal::Obj(heap.alloc(
                HeapValue::Callable(CallableValue::Native {
                    function_index: *function_index,
                    arity: *arity,
                }),
            ))),
            HeapValue::Callable(CallableValue::Runtime32(function)) => Ok(RuntimeVal::Obj(
                heap.alloc(HeapValue::Callable(CallableValue::Runtime32(function.clone()))),
            )),
            value => Ok(RuntimeVal::Obj(heap.alloc(value.clone()))),
        },
    }
}

fn runtime_native(
    name: &str,
    arity: u16,
    function: NativeFunction32,
    heap: &mut HeapStore,
    natives: &mut Vec<NativeEntry32>,
) -> Result<RuntimeVal> {
    let function_index = natives.len();
    natives.push(NativeEntry32 {
        name: name.to_string(),
        arity,
        function,
    });
    Ok(RuntimeVal::Obj(heap.alloc(HeapValue::Callable(
        CallableValue::Native {
            function_index: function_index as u32,
            arity,
        },
    ))))
}
