use crate::val::Val;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Central module registry inspired by Lua's linit.c
///
/// This registry manages all standard library modules and provides
/// a Lua-like module loading system with feature-based compilation.
#[derive(Debug)]
pub struct ModuleRegistry {
    modules: HashMap<String, Box<dyn Module>>,
    builtin_functions: HashMap<String, Val>,
    cache: Mutex<HashMap<String, Val>>,
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
            cache: Mutex::new(HashMap::new()),
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

    /// Cache a module value for performance
    pub fn cache_module(&self, name: &str, value: Val) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(name.to_string(), value);
        }
    }

    /// Get cached module value
    pub fn get_cached_module(&self, name: &str) -> Option<Val> {
        if let Ok(cache) = self.cache.lock() {
            cache.get(name).cloned()
        } else {
            None
        }
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

/// Enhanced import context with Lua-like module loading
///
/// Provides a Lua-like `require` function for loading modules
/// with caching and search path support.
#[derive(Debug, Clone)]
pub struct ImportContext {
    registry: Arc<ModuleRegistry>,
    loaded_modules: HashMap<String, Val>,
    search_paths: Vec<String>,
}

impl ImportContext {
    /// Create a new import context
    pub fn new(registry: Arc<ModuleRegistry>) -> Self {
        Self {
            registry,
            loaded_modules: HashMap::new(),
            search_paths: vec!["./modules".to_string(), "./lib".to_string(), ".".to_string()],
        }
    }

    /// Lua-like require function for loading modules
    pub fn require(&mut self, module_name: &str) -> Result<Val> {
        // Check cache first
        if let Some(module) = self.loaded_modules.get(module_name) {
            return Ok(module.clone());
        }

        // Check registry cache
        if let Some(cached) = self.registry.get_cached_module(module_name) {
            self.loaded_modules.insert(module_name.to_string(), cached.clone());
            return Ok(cached);
        }

        // Load the module
        let module = self.load_module(module_name)?;

        // Cache the module
        self.loaded_modules.insert(module_name.to_string(), module.clone());
        self.registry.cache_module(module_name, module.clone());

        Ok(module)
    }

    /// Load a module by name
    fn load_module(&self, module_name: &str) -> Result<Val> {
        let module_def = self.registry.get_module(module_name)?;

        // Initialize the module
        module_def.init()?;

        // Get module exports as a map
        let exports = module_def.exports();
        let module_value = Val::from(exports);

        Ok(module_value)
    }

    /// Add a search path for module resolution
    pub fn add_search_path(&mut self, path: String) {
        self.search_paths.push(path);
    }

    /// Get all loaded modules
    pub fn get_loaded_modules(&self) -> &HashMap<String, Val> {
        &self.loaded_modules
    }

    /// Check if a module is loaded
    pub fn is_module_loaded(&self, module_name: &str) -> bool {
        self.loaded_modules.contains_key(module_name)
    }

    /// Unload a module
    pub fn unload_module(&mut self, module_name: &str) -> Result<()> {
        if self.loaded_modules.remove(module_name).is_some() {
            // Note: In a real implementation, we might want to call cleanup
            // on the module here, but for now we just remove it from cache
        }
        Ok(())
    }

    /// Get module metadata
    pub fn get_module_metadata(&self, module_name: &str) -> Result<HashMap<String, String>> {
        let module = self.registry.get_module(module_name)?;
        Ok(module.metadata())
    }

    /// List all available modules
    pub fn list_modules(&self) -> Vec<String> {
        self.registry.get_module_names()
    }
}

impl Default for ImportContext {
    fn default() -> Self {
        Self::new(Arc::new(ModuleRegistry::new()))
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

    #[test]
    fn test_import_context() {
        let registry = Arc::new(ModuleRegistry::new());
        let mut ctx = ImportContext::new(registry);

        // For now, just test that the context works without stdlib modules
        let result = ctx.require("nonexistent");
        assert!(result.is_err());
    }
}
