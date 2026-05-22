use crate::val::Val;
use anyhow::{Result, anyhow};
use std::collections::HashMap;

/// Central module registry inspired by Lua's linit.c
///
/// This registry manages all standard library modules and provides
/// a Lua-like module loading system with feature-based compilation.
#[derive(Debug)]
pub struct ModuleRegistry {
    modules: HashMap<String, Box<dyn Module>>,
    builtin_functions: HashMap<String, Val>,
}

impl PartialEq for ModuleRegistry {
    fn eq(&self, other: &Self) -> bool {
        // Compare only the builtin functions, ignoring modules and cache
        self.builtin_functions == other.builtin_functions
    }
}

impl ModuleRegistry {
    /// Create a new module registry with all core modules registered
    pub fn new() -> Self {
        let mut registry = Self {
            modules: HashMap::new(),
            builtin_functions: HashMap::new(),
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
    pub fn register_module(&mut self, name: &str, module: Box<dyn Module>) -> Result<()> {
        if module.enabled() {
            module.register(self)?;
        }

        self.modules.insert(name.to_string(), module);
        Ok(())
    }

    /// Get a module by name
    pub fn get_module(&self, name: &str) -> Result<&dyn Module> {
        self.modules
            .get(name)
            .map(|boxed| boxed.as_ref())
            .ok_or_else(|| anyhow!("Module '{}' not found", name))
    }

    /// Get all registered module names
    pub fn get_module_names(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }

    /// Register a builtin function globally
    pub fn register_builtin(&mut self, name: &str, func: Val) {
        self.builtin_functions.insert(name.to_string(), func);
    }

    /// Get a builtin function by name
    pub fn get_builtin(&self, name: &str) -> Option<&Val> {
        self.builtin_functions.get(name)
    }

    /// Get all builtin functions
    pub fn get_all_builtins(&self) -> &HashMap<String, Val> {
        &self.builtin_functions
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
pub trait Module: Send + Sync + std::fmt::Debug {
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

    /// Get all exports from this module
    fn exports(&self) -> HashMap<String, Val>;

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
