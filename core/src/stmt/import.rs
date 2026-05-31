use crate::{
    module::ModuleRegistry,
    stmt::{Program, Stmt, stmt_parser::StmtParser},
    token::Tokenizer,
    val::{HeapValue, RuntimeVal},
    vm::{RuntimeExport32, VmContext},
};
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

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

// Note: The Module trait and registry live in module.rs; this file owns source import resolution.

/// Module resolver - handles finding and loading modules
#[derive(Debug, Clone)]
pub struct ModuleResolver {
    /// Standard library registry
    stdlib_registry: Arc<ModuleRegistry>,
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
            runtime_file_modules: Arc::new(DashMap::new()),
            // Prefer current directory; also allow `core/` for workspace runs.
            search_paths: vec![PathBuf::from("."), PathBuf::from("core")],
            package_modules: Arc::new(DashMap::new()),
        }
    }

    pub fn runtime_builtin_iter(&self) -> impl Iterator<Item = (&Arc<str>, &RuntimeExport32)> {
        self.stdlib_registry.get_all_runtime_builtins().iter()
    }

    pub fn get_runtime_builtin(&self, name: &str) -> Option<&RuntimeExport32> {
        self.stdlib_registry.get_runtime_builtin(name)
    }

    /// Add a search path for file resolution
    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        self.search_paths.push(path.into());
    }

    /// Set the default base directory for relative file imports.
    pub fn set_base_dir(&mut self, path: impl Into<PathBuf>) {
        let base = path.into();
        // Keep current directory as a search path; add the file's directory
        if !self
            .search_paths
            .iter()
            .any(|p| p.as_os_str() == PathBuf::from(".").as_os_str())
        {
            self.search_paths.insert(0, PathBuf::from("."));
        }
        self.search_paths.push(base.clone());
        self.search_paths.push(base.join("lib"));
        self.search_paths.push(base.join("modules"));
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

    pub fn resolve_runtime_file(&self, path: &str) -> Result<RuntimeExport32> {
        let resolved_path = self.resolve_file_path(path)?;
        self.resolve_resolved_runtime_file(&resolved_path)
    }

    pub fn resolve_runtime_module(&self, name: &str) -> Result<RuntimeExport32> {
        if let Ok(module) = self.stdlib_registry.get_runtime_module(name) {
            return Ok(module);
        }
        let Some(root) = self.package_modules.get(name) else {
            return Err(anyhow!("Module '{}' not found", name));
        };
        self.resolve_resolved_runtime_file(root.value())
    }

    fn resolve_resolved_runtime_file(&self, resolved_path: &Path) -> Result<RuntimeExport32> {
        let resolved_path = Self::normalize_path(resolved_path.to_path_buf());
        if let Some(module) = self.runtime_file_modules.get(&resolved_path) {
            return Ok(module.value().shallow_clone_shared());
        }
        let module = self.load_file_runtime_module(&resolved_path)?;
        self.runtime_file_modules
            .insert(resolved_path.clone(), module.shallow_clone_shared());
        Ok(module)
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
        let result = program.execute32_with_ctx(&mut ctx)?;
        Ok(result.into_exports())
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

impl Default for ModuleResolver {
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

fn runtime_export_field(module: &RuntimeExport32, name: &str) -> Result<RuntimeExport32> {
    let state = module.state_lock()?;
    let RuntimeVal::Obj(handle) = module.value() else {
        return Err(anyhow!("runtime module export is not a map"));
    };
    let Some(value) = state.heap.get(*handle) else {
        return Err(anyhow!("heap object {} out of bounds", handle.index()));
    };
    let HeapValue::Map(map) = value else {
        return Err(anyhow!("runtime module export is not a map"));
    };
    if let Some(value) = map.get_str(name) {
        return Ok(RuntimeExport32::new(
            value,
            module.shared_state(),
            module.shared_module(),
        ));
    }
    Err(anyhow!("Export '{}' not found in runtime module", name))
}

pub fn execute_imports(imports: &[ImportStmt], resolver: &ModuleResolver, env: &mut VmContext) -> Result<()> {
    for import in imports {
        if let ImportStmt::Items { items, source } = import {
            let module = resolve_runtime_import_source(source, resolver)?;
            for item in items {
                let symbol_name = item.alias.as_ref().unwrap_or(&item.name);
                let export = runtime_export_field(&module, &item.name)?;
                env.define_runtime_global(symbol_name.clone(), export);
            }
            continue;
        }

        match import {
            ImportStmt::Module { module } => {
                let module_export = resolver.resolve_runtime_module(module)?;
                env.define_runtime_global(module.clone(), module_export);
            }
            ImportStmt::File { path } => {
                let module_name = Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("module")
                    .to_string();
                let module = resolver.resolve_runtime_file(path)?;
                env.define_runtime_global(module_name, module);
            }
            ImportStmt::Items { .. } => unreachable!("items imports are handled before runtime import binding"),
            ImportStmt::Namespace { alias, source } => {
                let module = resolve_runtime_import_source(source, resolver)?;
                env.define_runtime_global(alias.clone(), module);
            }
            ImportStmt::ModuleAlias { module, alias } => {
                let module_export = resolver.resolve_runtime_module(module)?;
                env.define_runtime_global(alias.clone(), module_export);
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

    fn parse_program(source: &str) -> Result<Program> {
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source).map_err(|e| anyhow!(e.to_string()))?;
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        parser
            .parse_program_with_enhanced_errors(source)
            .map_err(|e| anyhow!(e.to_string()))
    }

    fn execute_import_source(source: &str, resolver: Arc<ModuleResolver>) -> Result<RuntimeVal> {
        let program = parse_program(source)?;
        let mut ctx = VmContext::new().with_resolver(resolver);
        Ok(program.execute32_with_ctx(&mut ctx)?.first_return().clone())
    }

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
        assert!(resolver.resolve_runtime_module("nonexistent").is_err());

        // Note: stdlib modules are now registered externally
        // The resolver starts with an empty registry
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
        let runtime = resolver.resolve_source_runtime(src)?;
        let RuntimeVal::Obj(handle) = runtime.value() else {
            panic!("Expected runtime module map");
        };
        let state = runtime.state_lock().expect("runtime module state");
        let Some(HeapValue::Map(map)) = state.heap.get(*handle) else {
            panic!("Expected runtime module map");
        };
        assert_eq!(map.get_str("answer"), Some(RuntimeVal::Int(7)));
        assert!(matches!(map.get_str("data"), Some(RuntimeVal::Obj(_))));
        assert!(matches!(
            map.get_str("inc"),
            Some(RuntimeVal::Obj(handle)) if matches!(state.heap.get(handle), Some(HeapValue::Callable(_)))
        ));
        Ok(())
    }

    #[test]
    fn test_resolve_examples_fib_exports_iterative() -> Result<()> {
        let mut resolver = ModuleResolver::new();
        resolver.add_search_path("..");
        let runtime = resolver.resolve_runtime_file("examples/fib")?;
        let RuntimeVal::Obj(handle) = runtime.value() else {
            panic!("Expected runtime module map");
        };
        let state = runtime.state_lock().expect("runtime module state");
        let Some(value) = state.heap.get(*handle) else {
            panic!("Expected runtime module heap object");
        };
        let HeapValue::Map(map) = value else {
            panic!("Expected runtime module map");
        };
        assert!(
            matches!(map.get_str("iterative"), Some(RuntimeVal::Obj(_))),
            "examples/fib should export iterative function"
        );

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

        let result = execute_import_source(src, Arc::clone(&resolver))?;

        assert_eq!(result, RuntimeVal::Int(55));
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

        let result = execute_import_source(src, Arc::clone(&resolver))?;

        assert_eq!(result, RuntimeVal::Int(55));
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

        let result = execute_import_source(src, Arc::clone(&resolver))?;

        assert_eq!(result, RuntimeVal::Int(55));
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

        let result = execute_import_source(src, Arc::clone(&resolver));

        let _ = std::fs::remove_dir_all(base);
        assert_eq!(result?, RuntimeVal::Int(42));
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

        let result = execute_import_source(src, Arc::clone(&resolver));

        let _ = std::fs::remove_dir_all(base);
        assert_eq!(result?, RuntimeVal::Int(21));
        Ok(())
    }
}
