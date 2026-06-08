use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};
use lk_core::{
    rt::RuntimePayload,
    val::{HeapStore, HeapValue, ResourceHandle, ResourceValue, RuntimeVal, TaskValue},
};

pub(crate) fn resource_value(kind: &'static str, handle: ResourceHandle, heap: &mut HeapStore) -> RuntimeVal {
    RuntimeVal::Obj(heap.alloc(HeapValue::Resource(Arc::new(ResourceValue {
        kind,
        handle: Arc::new(Mutex::new(handle)),
    }))))
}

pub(crate) fn resource_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Arc<ResourceValue>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects a resource argument");
    };
    match heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Resource(resource) => Ok(resource.clone()),
        other => bail!("{context} expects a resource argument, got {}", other.type_name()),
    }
}

pub(crate) fn close_resource(resource: &ResourceValue) -> Result<bool> {
    let mut handle = resource.handle.lock().map_err(|_| anyhow!("resource lock poisoned"))?;
    let was_open = !matches!(*handle, ResourceHandle::Closed);
    *handle = ResourceHandle::Closed;
    Ok(was_open)
}

pub(crate) fn task_value(task_id: u64, heap: &mut HeapStore) -> RuntimeVal {
    RuntimeVal::Obj(heap.alloc(HeapValue::Task(Arc::new(TaskValue {
        id: task_id,
        value: None,
    }))))
}

pub(crate) fn payload_resource(kind: &'static str, handle: ResourceHandle) -> RuntimePayload {
    let mut heap = HeapStore::new();
    let value = resource_value(kind, handle, &mut heap);
    RuntimePayload::new(value, heap)
}

pub(crate) fn payload_string(value: String) -> RuntimePayload {
    let mut heap = HeapStore::new();
    let value = crate::runtime_native::runtime_string_value(&value, &mut heap);
    RuntimePayload::new(value, heap)
}

pub(crate) fn payload_int(value: i64) -> RuntimePayload {
    RuntimePayload::new(RuntimeVal::Int(value), HeapStore::new())
}
