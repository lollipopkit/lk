use crate::{
    module::ModuleRegistry,
    stmt::{Program, Stmt, stmt_parser::StmtParser},
    token::Tokenizer,
    val::Val,
    vm::{BytecodeModule, Vm, VmContext},
};
use anyhow::{Context, Result, anyhow};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Import system for LKR - supports various import syntaxes and plugin-style module resolution
///
/// Supported import syntaxes:
/// 1. `import math;` - imports stdlib module 'math' with all exports
/// 2. `import "path/to/file.lkr";` - imports file with all exports  
/// 3. `import { abs, sqrt } from math;` - imports specific items from stdlib module
/// 4. `import { func as alias } from "file.lkr";` - imports with alias
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
    /// Loaded file modules (path -> module)
    file_modules: Arc<DashMap<PathBuf, Val>>,
    /// Precompiled modules bundled alongside the executable
    embedded_modules: Arc<DashMap<String, Arc<BytecodeModule>>>,
    /// Search paths for module resolution
    search_paths: Vec<PathBuf>,
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
            file_modules: Arc::new(DashMap::new()),
            embedded_modules: Arc::new(DashMap::new()),
            // Prefer current directory; also allow `core/` for workspace runs.
            search_paths: vec![PathBuf::from("."), PathBuf::from("core")],
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

    /// Register a precompiled module that should be resolved from memory instead of disk.
    pub fn register_embedded_module(&self, path: impl Into<PathBuf>, module: BytecodeModule) {
        let normalized = Self::normalize_path(path.into());
        let key = normalized.to_string_lossy().to_string();
        self.embedded_modules.insert(key, Arc::new(module));
    }

    /// Register multiple precompiled modules at once.
    pub fn register_embedded_modules<I, P>(&self, modules: I)
    where
        I: IntoIterator<Item = (P, BytecodeModule)>,
        P: Into<PathBuf>,
    {
        for (path, module) in modules {
            self.register_embedded_module(path, module);
        }
    }

    fn has_embedded_module(&self, path: &Path) -> bool {
        let normalized = Self::normalize_path(path.to_path_buf());
        let key = normalized.to_string_lossy();
        self.embedded_modules.contains_key(key.as_ref())
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

        Err(anyhow!("Module '{}' not found", name))
    }

    /// Resolve a file module - loads if not already cached
    pub fn resolve_file(&self, path: &str) -> Result<Val> {
        let resolved_path = self.resolve_file_path(path)?;

        // Check cache first
        if let Some(module) = self.file_modules.get(&resolved_path) {
            return Ok(module.value().clone());
        }

        let key_path = Self::normalize_path(resolved_path.clone());
        let key_str = key_path.to_string_lossy().to_string();

        // Embedded modules are executed from the bundled bytecode
        if let Some(embedded) = self.embedded_modules.get(&key_str) {
            let module_arc = Arc::clone(embedded.value());
            drop(embedded);
            let exports = self
                .execute_embedded_module(&resolved_path, module_arc)
                .with_context(|| format!("Failed to execute embedded module '{}'", resolved_path.display()))?;
            self.file_modules.insert(resolved_path.clone(), exports.clone());
            return Ok(exports);
        }

        // Load and parse the file
        let module = self.load_file_module(&resolved_path)?;

        // Cache the loaded module
        self.file_modules.insert(resolved_path.clone(), module.clone());

        Ok(module)
    }

    /// Resolve a module directly from source code string.
    /// Parses, compiles, then executes the source in a fresh VmContext that shares this resolver,
    /// and returns the map of top-level definitions as the module exports.
    pub fn resolve_source(&self, src: &str) -> Result<Val> {
        // Tokenize with spans for better diagnostics
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(src).map_err(|e| anyhow!(e.to_string()))?;

        // Parse program with enhanced errors
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program: Program = parser
            .parse_program_with_enhanced_errors(src)
            .map_err(|e| anyhow!(e.to_string()))?;

        // Execute program using the VM runtime
        let resolver = Arc::new(self.clone());
        let mut ctx = VmContext::new().with_resolver(resolver);
        let func = crate::vm::compile_program(&program);
        let mut vm = crate::vm::Vm::new();
        let _ = vm.exec_with(&func, &mut ctx, None)?;

        // Collect top-level definitions as exports
        let exports = ctx.export_symbols();
        Ok(Val::from(exports))
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
        // 1) ${MOD_NAME}.lkr
        // 2) ${MOD_NAME}/mod.lkr
        // If the input already contains an extension, also allow it directly.
        let base = PathBuf::from(path);

        for root in &self.search_paths {
            // If the input already includes .lkr and exists under this root, accept it
            if base.extension().and_then(|s| s.to_str()) == Some("lkr") {
                let p = root.join(&base);
                if self.has_embedded_module(&p) {
                    return Ok(Self::normalize_path(p));
                }
                if p.exists() {
                    return Ok(Self::normalize_path(p));
                }
            }

            // Try ${MOD_NAME}.lkr
            let candidate1 = root.join(base.with_extension("lkr"));
            if self.has_embedded_module(&candidate1) {
                return Ok(Self::normalize_path(candidate1));
            }
            if candidate1.exists() {
                return Ok(Self::normalize_path(candidate1));
            }

            // Try ${MOD_NAME}/mod.lkr
            let candidate2 = root.join(base.join("mod.lkr"));
            if self.has_embedded_module(&candidate2) {
                return Ok(Self::normalize_path(candidate2));
            }
            if candidate2.exists() {
                return Ok(Self::normalize_path(candidate2));
            }
        }

        Err(anyhow!(
            "File not found for module '{}': expected '{}.lkr' or '{}/mod.lkr'",
            path.display(),
            path.display(),
            path.display()
        ))
    }

    /// Load and parse a file module into a namespace map
    fn load_file_module(&self, path: &Path) -> Result<Val> {
        // Read source then delegate to resolve_source
        let src = std::fs::read_to_string(path)?;
        self.resolve_source(&src)
    }

    fn execute_embedded_module(&self, path: &Path, module: Arc<BytecodeModule>) -> Result<Val> {
        let resolver = Arc::new(self.clone());
        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver));

        if let Some(meta) = module.meta.as_ref()
            && let Some(imports_json) = meta.tags.get("imports")
        {
            let imports = deserialize_imports(imports_json)
                .with_context(|| format!("Failed to parse serialized imports for {}", path.display()))?;
            execute_imports(&imports, resolver.as_ref(), &mut ctx)
                .with_context(|| format!("Failed to replay imports for {}", path.display()))?;
        }

        let mut vm = Vm::new();
        vm.exec_with(&module.entry, &mut ctx, None)
            .with_context(|| format!("VM execution failed for embedded module '{}'", path.display()))?;
        let exports = ctx.export_symbols();
        Ok(Val::from(exports))
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

                if let Val::Map(exports) = mod_def {
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

pub fn execute_imports(imports: &[ImportStmt], resolver: &ModuleResolver, env: &mut VmContext) -> Result<()> {
    for import in imports {
        env.import_context_mut().execute_import(import, resolver)?;
        match import {
            ImportStmt::Module { module } => {
                if let Some(val) = env.import_context().get_symbol(module) {
                    env.define(module.clone(), val.clone());
                }
            }
            ImportStmt::File { path } => {
                let module_name = Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("module")
                    .to_string();
                if let Some(val) = env.import_context().get_symbol(&module_name) {
                    env.define(module_name, val.clone());
                }
            }
            ImportStmt::Items { items, .. } => {
                for item in items {
                    let symbol_name = item.alias.as_ref().unwrap_or(&item.name);
                    if let Some(val) = env.import_context().get_symbol(symbol_name) {
                        env.define(symbol_name.clone(), val.clone());
                    }
                }
            }
            ImportStmt::Namespace { alias, .. } | ImportStmt::ModuleAlias { alias, .. } => {
                if let Some(val) = env.import_context().get_symbol(alias) {
                    env.define(alias.clone(), val.clone());
                }
            }
        }
    }
    Ok(())
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
    use super::{collect_program_imports, *};
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
        assert!(resolver.resolve_file_path("../foo.lkr").is_err());

        // Relative simple path that likely does not exist should return not found
        // (error message still OK but not due to security check)
        let rel = PathBuf::from("does_not_exist.lkr");
        assert!(resolver.resolve_file_path(&rel.to_string_lossy()).is_err());
    }

    #[test]
    fn test_resolve_source_basic() -> Result<()> {
        let resolver = ModuleResolver::new();
        let src = r#"
            let answer = 7;
            fn inc(x) { return x + 1; }
            let data = [1, 2, 3];
        "#;
        let module_val = resolver.resolve_source(src)?;

        match module_val {
            Val::Map(map) => {
                assert!(map.contains_key("answer"));
                assert!(map.contains_key("inc"));
                assert!(map.contains_key("data"));
                assert!(matches!(map.get("answer"), Some(Val::Int(7))));
            }
            other => panic!("Expected module map, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_resolve_examples_fib_exports_iterative() -> Result<()> {
        let mut resolver = ModuleResolver::new();
        resolver.add_search_path("..");
        let module_val = resolver.resolve_file("examples/fib")?;

        match module_val {
            Val::Map(map) => {
                assert!(
                    map.contains_key("iterative"),
                    "examples/fib should export iterative function"
                );
            }
            other => panic!("Expected module map, got {:?}", other.type_name()),
        }

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
        let imports = collect_program_imports(&program);
        execute_imports(&imports, resolver.as_ref(), &mut ctx)?;
        let func = crate::vm::compile_program(&program);
        let mut vm = crate::vm::Vm::new();
        let result = vm.exec_with(&func, &mut ctx, None)?;

        assert_eq!(result, Val::Int(55));
        Ok(())
    }
}
