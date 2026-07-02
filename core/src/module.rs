use crate::util::fast_map::fast_hash_map_new;
use crate::{
    val::{CallableValue, HeapStore, HeapValue, RuntimeVal, TypedMap},
    vm::{ContextNativeFunction, Module, NativeFunction, PlainNativeFunction, RuntimeExport, RuntimeModuleState},
};
use anyhow::{Result, anyhow};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Central module registry inspired by Lua's linit.c
///
/// This registry manages all standard library modules and provides
/// a Lua-like module loading system with feature-based compilation.
#[derive(Debug)]
pub struct ModuleRegistry {
    modules: HashMap<String, Box<dyn ModuleProvider>>,
    runtime_builtin_functions: HashMap<Arc<str>, RuntimeExport>,
}

impl PartialEq for ModuleRegistry {
    fn eq(&self, other: &Self) -> bool {
        // RuntimeExport carries shared state and is not value-comparable.
        // Resolver equality only needs stable registry shape for cache/debug use.
        self.modules.len() == other.modules.len()
            && self.modules.keys().all(|name| other.modules.contains_key(name))
            && self.runtime_builtin_functions.len() == other.runtime_builtin_functions.len()
            && self
                .runtime_builtin_functions
                .keys()
                .all(|name| other.runtime_builtin_functions.contains_key(name))
    }
}

impl ModuleRegistry {
    /// Create a new module registry with all core modules registered
    pub fn new() -> Self {
        let mut registry = Self {
            modules: HashMap::new(),
            runtime_builtin_functions: HashMap::new(),
        };

        registry.register_core_modules();
        registry
    }

    /// Register core modules based on enabled features
    /// Similar to Lua's linit.c which opens standard libraries
    fn register_core_modules(&mut self) {
        // Note: stdlib modules are now in a separate crate
        // They will be registered by the stdlib crate itself
        // This method is kept for future builtin modules
    }

    /// Register a module with the registry
    pub fn register_module(&mut self, name: &str, module: Box<dyn ModuleProvider>) -> Result<()> {
        if module.enabled() {
            module.register(self)?;
        }

        self.modules.insert(name.to_string(), module);
        Ok(())
    }

    /// Get a module by name
    pub fn get_module(&self, name: &str) -> Result<&dyn ModuleProvider> {
        self.modules
            .get(name)
            .map(|boxed| boxed.as_ref())
            .ok_or_else(|| anyhow!("Module '{}' not found", name))
    }

    /// Resolve a registered module as a VM runtime export.
    pub fn get_runtime_module(&self, name: &str) -> Result<RuntimeExport> {
        self.get_module(name)?.runtime_exports()
    }

    /// Get all registered module names
    pub fn get_module_names(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }

    /// Register a VM-native builtin globally.
    pub fn register_runtime_builtin(&mut self, name: &str, function: NativeFunction, arity: u16) {
        let value = runtime_export_from_runtime_native(name, function.clone(), arity);
        self.runtime_builtin_functions.insert(Arc::<str>::from(name), value);
    }

    pub fn get_runtime_builtin(&self, name: &str) -> Option<&RuntimeExport> {
        self.runtime_builtin_functions.get(name)
    }

    pub fn get_all_runtime_builtins(&self) -> &HashMap<Arc<str>, RuntimeExport> {
        &self.runtime_builtin_functions
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Module trait inspired by Lua's library pattern
///
/// Each module implements this trait to provide its functionality
/// in a standardized way, similar to how Lua's standard libraries work.
pub trait ModuleProvider: Send + Sync + std::fmt::Debug {
    /// Get the module name
    fn name(&self) -> &str;

    /// Get the module version
    fn version(&self) -> &str {
        "1.0.0"
    }

    /// Get module description
    fn description(&self) -> &str {
        ""
    }

    /// Check if the module is enabled
    fn enabled(&self) -> bool {
        true
    }

    /// Register the module's exports with the registry
    fn register(&self, registry: &mut ModuleRegistry) -> Result<()>;

    /// Get exports in the canonical VM runtime representation.
    fn runtime_exports(&self) -> Result<RuntimeExport>;

    /// Initialize the module (called once when loaded)
    fn init(&self) -> Result<()> {
        Ok(())
    }

    /// Cleanup the module (called when unloading)
    fn cleanup(&self) -> Result<()> {
        Ok(())
    }

    /// Get module metadata
    fn metadata(&self) -> HashMap<String, String> {
        let mut meta = HashMap::new();
        meta.insert("name".to_string(), self.name().to_string());
        meta.insert("version".to_string(), self.version().to_string());
        meta.insert("description".to_string(), self.description().to_string());
        meta.insert("enabled".to_string(), self.enabled().to_string());
        meta
    }
}

#[derive(Clone)]
pub struct RuntimeNativeExport {
    pub name: &'static str,
    pub function: NativeFunction,
    pub arity: u16,
}

impl RuntimeNativeExport {
    pub const fn plain(name: &'static str, function: PlainNativeFunction, arity: u16) -> Self {
        Self {
            name,
            function: NativeFunction::Plain(function),
            arity,
        }
    }

    pub const fn full_state(name: &'static str, function: ContextNativeFunction, arity: u16) -> Self {
        Self {
            name,
            function: NativeFunction::FullState(function),
            arity,
        }
    }
}

#[derive(Clone)]
pub struct RuntimeValueExport {
    pub name: &'static str,
    pub value: RuntimeVal,
}

impl RuntimeValueExport {
    pub const fn new(name: &'static str, value: RuntimeVal) -> Self {
        Self { name, value }
    }
}

pub fn runtime_export_from_plain_native_entries(
    natives: &[RuntimeNativeExport],
    values: &[RuntimeValueExport],
) -> RuntimeExport {
    let mut heap = HeapStore::new();
    let mut entries = fast_hash_map_new();
    for native in natives {
        let value = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
            name: Arc::<str>::from(native.name),
            arity: native.arity,
            function: native.function.clone(),
        })));
        entries.insert(Arc::<str>::from(native.name), value);
    }
    for value in values {
        entries.insert(Arc::<str>::from(value.name), value.value);
    }
    let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(entries))));
    RuntimeExport::new(
        value,
        Arc::new(Mutex::new(RuntimeModuleState::new(heap, Vec::new()))),
        Arc::new(Module::default()),
    )
}

pub fn runtime_export_from_runtime_native(name: &str, function: NativeFunction, arity: u16) -> RuntimeExport {
    let mut heap = HeapStore::new();
    let value = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
        name: Arc::<str>::from(name),
        arity,
        function,
    })));
    RuntimeExport::from_value(value, heap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_registry_creation() {
        let registry = ModuleRegistry::new();
        // Just test that the registry can be created without panicking
        // The stdlib modules are now registered externally
        assert!(registry.get_module_names().is_empty());
    }
}
