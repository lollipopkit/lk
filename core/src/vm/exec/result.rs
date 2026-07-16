use super::format::format_runtime_val;
use super::*;

impl ProgramResult {
    pub fn first_return(&self) -> &RuntimeVal {
        self.returns.first().unwrap_or(&RuntimeVal::Nil)
    }

    pub fn first_return_list(&self) -> Result<&TypedList> {
        let RuntimeVal::Obj(handle) = self.first_return() else {
            bail!("first return is {:?}, expected list object", self.first_return().kind());
        };
        match self.state.heap.get(*handle) {
            Some(HeapValue::List(values)) => Ok(values),
            Some(other) => bail!("first return heap object is {:?}, expected list", other),
            None => bail!("first return heap object {} out of bounds", handle.index()),
        }
    }

    pub fn first_return_map(&self) -> Result<&TypedMap> {
        let RuntimeVal::Obj(handle) = self.first_return() else {
            bail!("first return is {:?}, expected map object", self.first_return().kind());
        };
        match self.state.heap.get(*handle) {
            Some(HeapValue::Map(values)) => Ok(values),
            Some(other) => bail!("first return heap object is {:?}, expected map", other),
            None => bail!("first return heap object {} out of bounds", handle.index()),
        }
    }

    pub fn into_exports(self) -> RuntimeExport {
        let mut state = self.state;
        let mut entries = fast_hash_map_new();
        for (slot, value) in self.module.globals.iter().zip(state.globals.iter()) {
            entries.insert(RuntimeMapKey::String(slot.name.clone()), *value);
        }
        let value = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(typed_map_from_entries(entries))));
        RuntimeExport::new(
            value,
            Arc::new(crate::compat::sync::Mutex::new(RuntimeModuleState::new(
                state.heap,
                state.globals,
            ))),
            self.module,
        )
    }

    /// Returns `true` if the first return value is `nil`.
    pub fn first_return_is_nil(&self) -> bool {
        matches!(self.first_return(), RuntimeVal::Nil)
    }

    /// Format the first return value as a human-readable string for REPL/CLI display.
    pub fn display_first_return(&self) -> String {
        format_runtime_val(self.first_return(), &self.state.heap, 0)
    }
}
