use anyhow::{Result, anyhow};

use crate::{
    val::{HeapStore, RuntimeVal},
    vm::copy_runtime_value,
};

#[derive(Clone, Debug)]
pub struct RuntimePayload {
    pub value: RuntimeVal,
    pub heap: HeapStore,
}

impl RuntimePayload {
    pub fn new(value: RuntimeVal, heap: HeapStore) -> Self {
        Self { value, heap }
    }

    pub fn copy_from_value(value: &RuntimeVal, heap: &HeapStore) -> Result<Self> {
        let mut payload_heap = HeapStore::new();
        let value = copy_runtime_value(value, heap, &mut payload_heap)?;
        Ok(Self::new(value, payload_heap))
    }

    pub fn into_value(self, heap: &mut HeapStore) -> Result<RuntimeVal> {
        copy_runtime_value(&self.value, &self.heap, heap)
    }

    pub fn clone_value_into(&self, heap: &mut HeapStore) -> Result<RuntimeVal> {
        copy_runtime_value(&self.value, &self.heap, heap)
    }

    pub fn nil() -> Self {
        Self {
            value: RuntimeVal::Nil,
            heap: HeapStore::new(),
        }
    }
}

/// Stub async-runtime handle for builds without the `async-runtime` feature.
///
/// Mirrors the real `AsyncRuntimeHandle` API so `VmContext` and native call
/// sites are feature-agnostic; every operation reports that async is
/// unavailable rather than reaching for a global.
#[derive(Clone, Default, Debug)]
pub struct AsyncRuntimeHandle;

impl AsyncRuntimeHandle {
    pub fn new() -> Self {
        Self
    }

    pub fn init(&self) -> Result<()> {
        Ok(())
    }

    pub fn with<F, R>(&self, _f: F) -> Result<R>
    where
        F: FnOnce(&Runtime) -> Result<R>,
    {
        Err(anyhow!("LK async runtime is not available in this build"))
    }

    pub fn shutdown(&self) {}
}

#[derive(Debug)]
pub struct Runtime;
