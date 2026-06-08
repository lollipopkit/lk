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

pub fn init_runtime() -> Result<()> {
    Ok(())
}

pub fn shutdown_runtime() {}

pub fn with_runtime<F, R>(_f: F) -> Result<R>
where
    F: FnOnce(&Runtime) -> Result<R>,
{
    Err(anyhow!("LK async runtime is not available in this build"))
}

#[derive(Debug)]
pub struct Runtime;
