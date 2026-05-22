use crate::{
    module::ModuleRegistry,
    stmt::{Program, Stmt, stmt_parser::StmtParser},
    token::Tokenizer,
    val::{CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, TypedMap, Val, val_to_runtime_val},
    vm::{Module32, RuntimeExport32, RuntimeModuleState32, VmContext},
};
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Import system for LK - supports various import syntaxes and plugin-style module resolution
///
/// Supported import syntaxes:
/// 1. `import math;` - imports stdlib module 'math' with all exports
/// 2. `import "path/to/file.lk";` - imports file with all exports  
/// 3. `import { abs, sqrt } from math;` - imports specific items from stdlib module
/// 4. `import { func as alias } from "file.lk";` - imports with alias
/// 5. `import * as math from math;` - imports all as namespace
/// 6. `import math as m;` - imports entire module with alias
///
/// Import statement variants
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportStmt {
    /// `import module;` - import entire module
    Module { module: String },
    /// `import "path";` - import from file path
    File { path: String },
    /// `import { items } from source;` - import specific items
    Items {
        items: Vec<ImportItem>,
        source: ImportSource,
    },
    /// `import * as alias from source;` - import all as namespace
    Namespace { alias: String, source: ImportSource },
    /// `import module as alias;` - import module with alias
    ModuleAlias { module: String, alias: String },
}

/// Import source - either stdlib module or file path
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ImportSource {
    Module(String),
    File(String),
}

/// Individual import item with optional alias
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportItem {
    pub name: String,
    pub alias: Option<String>,
}

// Note: The Module trait and related functionality have been moved to module.rs
// This file now provides compatibility layer and file-based import functionality

/// Module resolver - handles finding and loading modules
#[derive(Debug, Clone)]
pub struct ModuleResolver {
    /// Standard library registry
    stdlib_registry: Arc<ModuleRegistry>,
    /// Standard library modules cache
    stdlib_modules: Arc<DashMap<String, Val>>,
    /// Loaded file modules as new VM runtime exports.
    runtime_file_modules: Arc<DashMap<PathBuf, RuntimeExport32>>,
    /// Search paths for module resolution
    search_paths: Vec<PathBuf>,
    /// Package modules resolved from Lk.toml dependencies/workspace members
    package_modules: Arc<DashMap<String, PathBuf>>,
}

impl PartialEq for ModuleResolver {
    fn eq(&self, other: &Self) -> bool {
        // Compare only registry and search paths, ignoring caches
        self.stdlib_registry == other.stdlib_registry && self.search_paths == other.search_paths
    }
}

impl ModuleResolver {
    pub fn new() -> Self {
        Self::with_registry(ModuleRegistry::new())
    }

    /// Create a new resolver with a specific module registry
    pub fn with_registry(registry: ModuleRegistry) -> Self {
        Self {
            stdlib_registry: Arc::new(registry),
            stdlib_modules: Arc::new(DashMap::new()),
            runtime_file_modules: Arc::new(DashMap::new()),
            // Prefer current directory; also allow `core/` for workspace runs.
            search_paths: vec![PathBuf::from("."), PathBuf::from("core")],
            package_modules: Arc::new(DashMap::new()),
        }
    }

    pub fn builtin_iter(&self) -> impl Iterator<Item = (&String, &Val)> {
        self.stdlib_registry.get_all_builtins().iter()
    }

    /// Get a globally registered builtin function (if any)
    pub fn get_builtin(&self, name: &str) -> Option<&Val> {
        self.stdlib_registry.get_builtin(name)
    }

    /// Add a search path for file resolution
    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        self.search_paths.push(path.into());
    }

    /// Set the default base directory for relative file imports.
    pub fn set_base_dir(&mut self, path: impl Into<PathBuf>) {
        let base = path.into();
        self.search_paths = vec![base.clone(), base.join("lib"), base.join("modules")];
    }

    /// Register a package root module. `import name;` resolves to this file when
    /// no stdlib module with the same name exists.
    pub fn register_package_module(&self, name: impl Into<String>, root: impl Into<PathBuf>) {
        self.package_modules.insert(name.into(), root.into());
    }

    pub fn package_module_path(&self, name: &str) -> Option<PathBuf> {
        self.package_modules.get(name).map(|root| root.value().clone())
    }

    fn normalize_path(path: PathBuf) -> PathBuf {
        let mut normalized = PathBuf::new();
        for comp in path.components() {
            if matches!(comp, Component::CurDir) {
                continue;
            }
            normalized.push(comp.as_os_str());
        }
        if normalized.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            normalized
        }
    }

    /// Resolve a module by name (stdlib modules)
    pub fn resolve_module(&self, name: &str) -> Result<Val> {
        // Check cache first to avoid cloning exports repeatedly
        if let Some(value) = self.stdlib_modules.get(name) {
            return Ok(value.value().clone());
        }

        // Try to get from stdlib registry and populate cache
        if let Ok(module) = self.stdlib_registry.get_module(name) {
            let exports = module.exports();
            let module_val = Val::from(exports);

            self.stdlib_modules.insert(name.to_string(), module_val.clone());

            return Ok(module_val);
        }

        if let Some(root) = self.package_modules.get(name) {
            return self.resolve_resolved_file(root.value());
        }

        Err(anyhow!("Module '{}' not found", name))
    }

    /// Resolve only modules already present in the registry.
    ///
    /// This is used by native AOT import replay, where falling back to package
    /// or file execution would pull the VM into otherwise native executables.
    pub fn resolve_registered_module(&self, name: &str) -> Result<Val> {
        if let Some(value) = self.stdlib_modules.get(name) {
            return Ok(value.value().clone());
        }

        let module = self.stdlib_registry.get_module(name)?;
        let exports = module.exports();
        let module_val = Val::from(exports);
        self.stdlib_modules.insert(name.to_string(), module_val.clone());
        Ok(module_val)
    }

    /// Resolve a file module - loads if not already cached
    pub fn resolve_file(&self, path: &str) -> Result<Val> {
        let resolved_path = self.resolve_file_path(path)?;
        self.resolve_resolved_file(&resolved_path)
    }

    pub fn resolve_runtime_file(&self, path: &str) -> Result<RuntimeExport32> {
        let resolved_path = self.resolve_file_path(path)?;
        self.resolve_resolved_runtime_file(&resolved_path)
    }

    pub fn resolve_runtime_module(&self, name: &str) -> Result<RuntimeExport32> {
        if let Ok(module) = self.stdlib_registry.get_module(name) {
            return stdlib_module_runtime_export(module.exports());
        }
        let Some(root) = self.package_modules.get(name) else {
            return Err(anyhow!("Module '{}' not found", name));
        };
        self.resolve_resolved_runtime_file(root.value())
    }

    fn resolve_resolved_file(&self, resolved_path: &Path) -> Result<Val> {
        let resolved_path = Self::normalize_path(resolved_path.to_path_buf());
        let runtime = self.resolve_resolved_runtime_file(&resolved_path)?;
        runtime_export_to_legacy_map(&runtime)
    }

    fn resolve_resolved_runtime_file(&self, resolved_path: &Path) -> Result<RuntimeExport32> {
        let resolved_path = Self::normalize_path(resolved_path.to_path_buf());
        if let Some(module) = self.runtime_file_modules.get(&resolved_path) {
            return Ok(module.value().clone());
        }
        let module = self.load_file_runtime_module(&resolved_path)?;
        self.runtime_file_modules.insert(resolved_path.clone(), module.clone());
        Ok(module)
    }

    /// Resolve a module directly from source code string.
    /// Parses, compiles, then executes the source in a fresh VmContext that shares this resolver,
    /// and returns the map of top-level definitions as the module exports.
    pub fn resolve_source(&self, src: &str) -> Result<Val> {
        let runtime = self.resolve_source_runtime(src)?;
        runtime_export_to_legacy_map(&runtime)
    }

    pub fn resolve_source_runtime(&self, src: &str) -> Result<RuntimeExport32> {
        // Tokenize with spans for better diagnostics
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(src).map_err(|e| anyhow!(e.to_string()))?;

        // Parse program with enhanced errors
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program: Program = parser
            .parse_program_with_enhanced_errors(src)
            .map_err(|e| anyhow!(e.to_string()))?;

        let resolver = Arc::new(self.clone());
        let mut ctx = VmContext::new_without_core_vm_builtins().with_resolver(resolver);
        let result = program.execute32_raw_with_ctx(&mut ctx)?;
        Ok(result.exports())
    }

    /// Resolve file path using search paths
    pub fn resolve_file_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        // Enforce security: only allow relative, sanitized paths (no absolute, no `..`).
        if !path.is_relative() {
            return Err(anyhow!(
                "Absolute paths are not allowed for imports: {}",
                path.display()
            ));
        }

        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(anyhow!(
                "Parent directory components ('..') are not allowed in imports: {}",
                path.display()
            ));
        }

        // Candidate patterns (searched under each `search_paths` root):
        // 1) ${MOD_NAME}.lk
        // 2) ${MOD_NAME}/mod.lk
        // If the input already contains an extension, also allow it directly.
        let base = PathBuf::from(path);

        for root in &self.search_paths {
            // If the input already includes .lk and exists under this root, accept it
            if base.extension().and_then(|s| s.to_str()) == Some("lk") {
                let p = root.join(&base);
                if p.exists() {
                    return Ok(Self::normalize_path(p));
                }
            }

            // Try ${MOD_NAME}.lk
            let candidate1 = root.join(base.with_extension("lk"));
            if candidate1.exists() {
                return Ok(Self::normalize_path(candidate1));
            }

            // Try ${MOD_NAME}/mod.lk
            let candidate2 = root.join(base.join("mod.lk"));
            if candidate2.exists() {
                return Ok(Self::normalize_path(candidate2));
            }
        }

        Err(anyhow!(
            "File not found for module '{}': expected '{}.lk' or '{}/mod.lk'",
            path.display(),
            path.display(),
            path.display()
        ))
    }

    fn load_file_runtime_module(&self, path: &Path) -> Result<RuntimeExport32> {
        let src = std::fs::read_to_string(path)?;
        let mut resolver = self.clone();
        if let Some(parent) = path.parent() {
            resolver.set_base_dir(parent.to_path_buf());
        }
        resolver.resolve_source_runtime(&src)
    }
}

fn stdlib_module_runtime_export(exports: HashMap<String, Val>) -> Result<RuntimeExport32> {
    let mut heap = HeapStore::new();
    let mut entries = std::collections::BTreeMap::new();
    for (name, value) in exports {
        entries.insert(
            RuntimeMapKey::String(Arc::<str>::from(name.as_str())),
            stdlib_export_to_runtime_value(&value, &mut heap)?,
        );
    }
    let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))));
    let state = Arc::new(Mutex::new(RuntimeModuleState32 {
        heap,
        globals: Vec::new(),
    }));
    Ok(RuntimeExport32 {
        value,
        state,
        module: Arc::new(Module32::default()),
    })
}

fn stdlib_export_to_runtime_value(value: &Val, heap: &mut HeapStore) -> Result<RuntimeVal> {
    match value {
        Val::Obj(object) => match object.as_ref() {
            HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) => Ok(RuntimeVal::Obj(heap.alloc(
                HeapValue::Callable(CallableValue::RuntimeNative32 {
                    arity: *arity,
                    function: function.clone(),
                }),
            ))),
            HeapValue::Callable(_) => Err(anyhow!("stdlib export still uses legacy callable")),
            _ => val_to_runtime_val(value, heap),
        },
        _ => val_to_runtime_val(value, heap),
    }
}

impl Default for ModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Import context - manages imported symbols in current scope
#[derive(Debug, Clone, PartialEq)]
pub struct ImportContext {
    /// Imported symbols: name -> value
    symbols: HashMap<String, Val>,
}

impl ImportContext {
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
        }
    }

    /// Execute an import statement
    pub fn execute_import(&mut self, import: &ImportStmt, resolver: &ModuleResolver) -> Result<()> {
        match import {
            ImportStmt::Module { module } => {
                let mod_def = resolver.resolve_module(module)?;
                // Import module as namespace - don't pollute global scope
                self.symbols.insert(module.clone(), mod_def);
            }
            ImportStmt::File { path } => {
                let mod_def = resolver.resolve_file(path)?;
                // Import file module as namespace using filename (without extension)
                let module_name = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("module");
                self.symbols.insert(module_name.to_string(), mod_def);
            }
            ImportStmt::Items { items, source } => {
                let mod_def = match source {
                    ImportSource::Module(name) => resolver.resolve_module(name)?,
                    ImportSource::File(path) => resolver.resolve_file(path)?,
                };

                if let Some(exports) = mod_def.as_map() {
                    for item in items {
                        let export_value = exports
                            .get(item.name.as_str())
                            .ok_or_else(|| anyhow!("Export '{}' not found in module", item.name))?;

                        let symbol_name = item.alias.as_ref().unwrap_or(&item.name);
                        self.symbols.insert(symbol_name.clone(), export_value.clone());
                    }
                }
            }
            ImportStmt::Namespace { alias, source } => {
                let mod_def = match source {
                    ImportSource::Module(name) => resolver.resolve_module(name)?,
                    ImportSource::File(path) => resolver.resolve_file(path)?,
                };

                // The module is already a map, so we can use it directly
                self.symbols.insert(alias.clone(), mod_def);
            }
            ImportStmt::ModuleAlias { module, alias } => {
                let mod_def = resolver.resolve_module(module)?;
                // The module is already a map, so we can use it directly
                self.symbols.insert(alias.clone(), mod_def);
            }
        }
        Ok(())
    }

    /// Get imported symbol
    pub fn get_symbol(&self, name: &str) -> Option<&Val> {
        self.symbols.get(name)
    }

    /// Check if symbol exists
    pub fn has_symbol(&self, name: &str) -> bool {
        self.symbols.contains_key(name)
    }

    /// Get all symbols
    pub fn get_all_symbols(&self) -> &HashMap<String, Val> {
        &self.symbols
    }
}

impl Default for ImportContext {
    fn default() -> Self {
        Self::new()
    }
}

pub fn serialize_imports(imports: &[ImportStmt]) -> serde_json::Result<String> {
    serde_json::to_string(imports)
}

pub fn deserialize_imports(json: &str) -> serde_json::Result<Vec<ImportStmt>> {
    serde_json::from_str(json)
}

fn runtime_export_to_legacy_map(module: &RuntimeExport32) -> Result<Val> {
    let state = module
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeExport32 state lock poisoned"))?
        .clone();
    let RuntimeVal::Obj(handle) = module.value else {
        return crate::val::runtime_val_to_val(&module.value, &state.heap);
    };
    let Some(value) = state.heap.get(handle) else {
        return Err(anyhow!("heap object {} out of bounds", handle.index()));
    };
    let HeapValue::Map(map) = value else {
        return crate::val::runtime_val_to_val(&module.value, &state.heap);
    };
    let mut exports = HashMap::new();
    for (key, value) in runtime_map_entries(map) {
        let Some(key) = runtime_key_to_string(&key) else {
            continue;
        };
        if let Ok(value) = crate::val::runtime_val_to_val(&value, &state.heap) {
            exports.insert(key, value);
        }
    }
    Ok(Val::from(exports))
}

fn runtime_export_field(module: &RuntimeExport32, name: &str) -> Result<RuntimeExport32> {
    let state = module
        .state
        .lock()
        .map_err(|_| anyhow!("RuntimeExport32 state lock poisoned"))?
        .clone();
    let RuntimeVal::Obj(handle) = module.value else {
        return Err(anyhow!("runtime module export is not a map"));
    };
    let Some(value) = state.heap.get(handle) else {
        return Err(anyhow!("heap object {} out of bounds", handle.index()));
    };
    let HeapValue::Map(map) = value else {
        return Err(anyhow!("runtime module export is not a map"));
    };
    for (key, value) in runtime_map_entries(map) {
        if runtime_key_to_string(&key).as_deref() == Some(name) {
            return Ok(RuntimeExport32 {
                value,
                state: module.state.clone(),
                module: Arc::clone(&module.module),
            });
        }
    }
    Err(anyhow!("Export '{}' not found in runtime module", name))
}

fn runtime_map_entries(map: &TypedMap) -> Vec<(RuntimeMapKey, RuntimeVal)> {
    match map {
        TypedMap::Mixed(values) => values.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
        TypedMap::StringMixed(values) => values
            .iter()
            .map(|(key, value)| (RuntimeMapKey::String(key.clone()), value.clone()))
            .collect(),
        TypedMap::StringInt(values) => values
            .iter()
            .map(|(key, value)| (RuntimeMapKey::String(key.clone()), RuntimeVal::Int(*value)))
            .collect(),
        TypedMap::StringFloat(values) => values
            .iter()
            .map(|(key, value)| (RuntimeMapKey::String(key.clone()), RuntimeVal::Float(*value)))
            .collect(),
        TypedMap::StringBool(values) => values
            .iter()
            .map(|(key, value)| (RuntimeMapKey::String(key.clone()), RuntimeVal::Bool(*value)))
            .collect(),
    }
}

fn runtime_key_to_string(key: &RuntimeMapKey) -> Option<String> {
    match key {
        RuntimeMapKey::ShortStr(value) => Some(value.as_str().to_string()),
        RuntimeMapKey::String(value) => Some(value.to_string()),
        _ => None,
    }
}

pub fn execute_imports(imports: &[ImportStmt], resolver: &ModuleResolver, env: &mut VmContext) -> Result<()> {
    for import in imports {
        if let ImportStmt::Items { items, source } = import {
            match resolve_runtime_import_source(source, resolver) {
                Ok(module) => {
                    for item in items {
                        let symbol_name = item.alias.as_ref().unwrap_or(&item.name);
                        let export = runtime_export_field(&module, &item.name)?;
                        env.define_runtime_global(symbol_name.clone(), export);
                    }
                }
                Err(runtime_err) => {
                    let legacy_module = resolve_legacy_import_source(source, resolver)?;
                    let Some(exports) = legacy_module.as_map() else {
                        return Err(runtime_err);
                    };
                    for item in items {
                        let symbol_name = item.alias.as_ref().unwrap_or(&item.name);
                        let value = exports
                            .get(item.name.as_str())
                            .ok_or_else(|| anyhow!("{}: {}", runtime_err, item.name))?;
                        env.define(symbol_name.clone(), value.clone());
                    }
                }
            }
            continue;
        }

        match import {
            ImportStmt::Module { module } => {
                if let Ok(module_export) = resolver.resolve_runtime_module(module) {
                    env.define_runtime_global(module.clone(), module_export);
                } else {
                    env.import_context_mut().execute_import(import, resolver)?;
                    if let Some(val) = env.import_context().get_symbol(module) {
                        env.define(module.clone(), val.clone());
                    }
                }
            }
            ImportStmt::File { path } => {
                let module_name = Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("module")
                    .to_string();
                if let Ok(module) = resolver.resolve_runtime_file(path) {
                    env.define_runtime_global(module_name, module);
                } else {
                    env.import_context_mut().execute_import(import, resolver)?;
                    if let Some(val) = env.import_context().get_symbol(&module_name) {
                        env.define(module_name.clone(), val.clone());
                    }
                }
            }
            ImportStmt::Items { .. } => unreachable!("items imports are handled before legacy import context"),
            ImportStmt::Namespace { alias, source } => {
                if let Ok(module) = resolve_runtime_import_source(source, resolver) {
                    env.define_runtime_global(alias.clone(), module);
                } else {
                    env.import_context_mut().execute_import(import, resolver)?;
                    if let Some(val) = env.import_context().get_symbol(alias) {
                        env.define(alias.clone(), val.clone());
                    }
                }
            }
            ImportStmt::ModuleAlias { module, alias } => {
                if let Ok(module_export) = resolver.resolve_runtime_module(module) {
                    env.define_runtime_global(alias.clone(), module_export);
                } else {
                    env.import_context_mut().execute_import(import, resolver)?;
                    if let Some(val) = env.import_context().get_symbol(alias) {
                        env.define(alias.clone(), val.clone());
                    }
                }
            }
        }
    }
    Ok(())
}

fn resolve_runtime_import_source(source: &ImportSource, resolver: &ModuleResolver) -> Result<RuntimeExport32> {
    match source {
        ImportSource::File(path) => resolver.resolve_runtime_file(path),
        ImportSource::Module(name) => resolver.resolve_runtime_module(name),
    }
}

fn resolve_legacy_import_source(source: &ImportSource, resolver: &ModuleResolver) -> Result<Val> {
    match source {
        ImportSource::File(path) => resolver.resolve_file(path),
        ImportSource::Module(name) => resolver.resolve_module(name),
    }
}

pub fn collect_program_imports(program: &Program) -> Vec<ImportStmt> {
    fn visit(stmt: &Stmt, acc: &mut Vec<ImportStmt>) {
        match stmt {
            Stmt::Import(import_stmt) => acc.push(import_stmt.clone()),
            Stmt::Block { statements } => {
                for stmt in statements {
                    visit(stmt, acc);
                }
            }
            Stmt::If {
                then_stmt, else_stmt, ..
            } => {
                visit(then_stmt, acc);
                if let Some(else_stmt) = else_stmt {
                    visit(else_stmt, acc);
                }
            }
            Stmt::IfLet {
                then_stmt, else_stmt, ..
            } => {
                visit(then_stmt, acc);
                if let Some(else_stmt) = else_stmt {
                    visit(else_stmt, acc);
                }
            }
            Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => visit(body, acc),
            Stmt::Function { body, .. } => visit(body, acc),
            Stmt::Impl { methods, .. } => {
                for method in methods {
                    visit(method, acc);
                }
            }
            _ => {}
        }
    }

    let mut imports = Vec::new();
    for stmt in &program.statements {
        visit(stmt, &mut imports);
    }
    imports
}

// Standard library module implementations have been moved to module.rs

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_import_stmt_variants() {
        let import = ImportStmt::Module {
            module: "math".to_string(),
        };
        assert!(matches!(import, ImportStmt::Module { .. }));

        let import = ImportStmt::Items {
            items: vec![ImportItem {
                name: "abs".to_string(),
                alias: None,
            }],
            source: ImportSource::Module("math".to_string()),
        };
        assert!(matches!(import, ImportStmt::Items { .. }));
    }

    #[test]
    fn test_module_resolver() {
        let resolver = ModuleResolver::new();

        // Test that nonexistent modules fail
        assert!(resolver.resolve_module("nonexistent").is_err());

        // Note: stdlib modules are now registered externally
        // The resolver starts with an empty registry
    }

    #[test]
    fn test_import_context() {
        let mut ctx = ImportContext::new();
        let resolver = ModuleResolver::new();

        let import = ImportStmt::Module {
            module: "nonexistent".to_string(),
        };

        // Test that nonexistent modules fail
        let result = ctx.execute_import(&import, &resolver);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_file_path_security() {
        let resolver = ModuleResolver::new();

        // Absolute paths are rejected
        let abs = std::env::current_dir().unwrap();
        let abs_str = abs.to_string_lossy().to_string();
        assert!(resolver.resolve_file_path(&abs_str).is_err());

        // Parent directory components are rejected
        assert!(resolver.resolve_file_path("../foo.lk").is_err());

        // Relative simple path that likely does not exist should return not found
        // (error message still OK but not due to security check)
        let rel = PathBuf::from("does_not_exist.lk");
        assert!(resolver.resolve_file_path(&rel.to_string_lossy()).is_err());
    }

    #[test]
    fn test_resolve_file_path_uses_base_dir() -> Result<()> {
        let mut base = std::env::temp_dir();
        base.push(format!("lk-import-base-test-{}", std::process::id()));
        let current_file_dir = base.join("examples");
        let nested_import_dir = current_file_dir.join("examples");
        std::fs::create_dir_all(&nested_import_dir)?;

        let expected = nested_import_dir.join("fib.lk");
        std::fs::write(&expected, "fn iterative(n) { return n; }\n")?;

        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(&current_file_dir);

        assert_eq!(resolver.resolve_file_path("examples/fib")?, expected);

        let _ = std::fs::remove_dir_all(base);
        Ok(())
    }

    #[test]
    fn test_resolve_source_basic() -> Result<()> {
        let resolver = ModuleResolver::new();
        let src = r#"
            answer := 7;
            fn inc(x) { return x + 1; }
            data := [1, 2, 3];
        "#;
        let module_val = resolver.resolve_source(src)?;

        match module_val.as_map() {
            Some(map) => {
                assert!(map.contains_key("answer"));
                assert!(map.contains_key("data"));
                assert!(matches!(map.get("answer"), Some(Val::Int(7))));
                assert!(
                    !map.contains_key("inc"),
                    "runtime callables are no longer exported through legacy Val maps"
                );
            }
            None => panic!("Expected module map, got {:?}", module_val),
        }
        Ok(())
    }

    #[test]
    fn test_resolve_examples_fib_exports_iterative() -> Result<()> {
        let mut resolver = ModuleResolver::new();
        resolver.add_search_path("..");
        let module_val = resolver.resolve_file("examples/fib")?;

        assert!(module_val.as_map().is_some());
        let runtime = resolver.resolve_runtime_file("examples/fib")?;
        let RuntimeVal::Obj(handle) = runtime.value else {
            panic!("Expected runtime module map");
        };
        let state = runtime.state.lock().expect("runtime module state").clone();
        let Some(value) = state.heap.get(handle) else {
            panic!("Expected runtime module heap object");
        };
        let HeapValue::Map(map) = value else {
            panic!("Expected runtime module map");
        };
        let has_iterative = runtime_map_entries(map)
            .iter()
            .any(|(key, _)| runtime_key_to_string(key).as_deref() == Some("iterative"));
        assert!(has_iterative, "examples/fib should export iterative function");

        Ok(())
    }

    #[test]
    fn test_import_executes_fib_iterative_via_vm() -> Result<()> {
        let mut resolver = ModuleResolver::new();
        resolver.add_search_path("..");
        let resolver = Arc::new(resolver);

        let src = r#"
            import "examples/fib";
            return fib.iterative(10);
        "#;

        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(src).map_err(|e| anyhow!(e.to_string()))?;
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program = parser
            .parse_program_with_enhanced_errors(src)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
        let result = program.execute32_with_ctx(&mut ctx)?;

        assert_eq!(result, Val::Int(55));
        Ok(())
    }

    #[test]
    fn test_item_import_executes_runtime_callable_via_vm() -> Result<()> {
        let mut resolver = ModuleResolver::new();
        resolver.add_search_path("..");
        let resolver = Arc::new(resolver);

        let src = r#"
            import { iterative as fib_iter } from "examples/fib";
            return fib_iter(10);
        "#;

        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(src).map_err(|e| anyhow!(e.to_string()))?;
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program = parser
            .parse_program_with_enhanced_errors(src)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
        let result = program.execute32_with_ctx(&mut ctx)?;

        assert_eq!(result, Val::Int(55));
        Ok(())
    }

    #[test]
    fn test_namespace_import_executes_runtime_callable_via_vm() -> Result<()> {
        let mut resolver = ModuleResolver::new();
        resolver.add_search_path("..");
        let resolver = Arc::new(resolver);

        let src = r#"
            import * as fibs from "examples/fib";
            return fibs.iterative(10);
        "#;

        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(src).map_err(|e| anyhow!(e.to_string()))?;
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program = parser
            .parse_program_with_enhanced_errors(src)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
        let result = program.execute32_with_ctx(&mut ctx)?;

        assert_eq!(result, Val::Int(55));
        Ok(())
    }

    #[test]
    fn test_namespace_import_executes_runtime_callable_with_named_args() -> Result<()> {
        let mut base = std::env::temp_dir();
        base.push(format!("lk-import-named-call-test-{}", std::process::id()));
        std::fs::create_dir_all(&base)?;
        std::fs::write(
            base.join("calc.lk"),
            r#"
            fn add({x: Int, y: Int}) {
                return x + y;
            }
            "#,
        )?;

        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(&base);
        let resolver = Arc::new(resolver);

        let src = r#"
            import * as calc from "calc";
            return calc.add(y: 2, x: 40);
        "#;

        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(src).map_err(|e| anyhow!(e.to_string()))?;
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program = parser
            .parse_program_with_enhanced_errors(src)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
        let result = program.execute32_with_ctx(&mut ctx);

        let _ = std::fs::remove_dir_all(base);
        assert_eq!(result?, Val::Int(42));
        Ok(())
    }

    #[test]
    fn test_imported_runtime_callable_keeps_shared_module_state() -> Result<()> {
        let mut base = std::env::temp_dir();
        base.push(format!("lk-import-runtime-state-test-{}", std::process::id()));
        std::fs::create_dir_all(&base)?;
        std::fs::write(
            base.join("counter.lk"),
            r#"
            current := 0;
            fn next() {
                current = current + 1;
                return current;
            }
            "#,
        )?;

        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(&base);
        let resolver = Arc::new(resolver);

        let src = r#"
            import * as counter from "counter";
            let first = counter.next();
            let second = counter.next();
            return second * 10 + first;
        "#;

        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(src).map_err(|e| anyhow!(e.to_string()))?;
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program = parser
            .parse_program_with_enhanced_errors(src)
            .map_err(|e| anyhow!(e.to_string()))?;

        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
        let result = program.execute32_with_ctx(&mut ctx);

        let _ = std::fs::remove_dir_all(base);
        assert_eq!(result?, Val::Int(21));
        Ok(())
    }
}
