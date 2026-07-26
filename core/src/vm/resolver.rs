//! Module resolution and import execution.
//!
//! This is runtime behaviour — loading a module means parsing it, running it,
//! and binding its exports into a [`VmContext`] — so it belongs to the VM
//! layer. It used to live in `stmt::import` next to the `use`-statement AST
//! types, which made the AST layer depend on the executor: a cycle
//! (`stmt` ↔ `vm`) that blocked separating the front end from the VM.
//! `stmt::import` now holds only the syntax (`ImportStmt` and friends) and the
//! AST walk that collects them.

use crate::compat::path::{Path, PathBuf};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::compat::shared_map::SharedMap;
use crate::{
    module::ModuleRegistry,
    stmt::{ImportSource, ImportStmt},
    syntax::{ParseOptions, parse_program_source},
    val::{HeapValue, RuntimeVal},
    vm::{ProgramExec, RuntimeExport, VmContext},
};
use alloc::sync::Arc;
use anyhow::{Result, anyhow};
// File-based module resolution (fs + path normalization) is std-only; the
// no_std VM core keeps only in-memory registry resolution (plan M0.7/8).
#[cfg(feature = "std")]
use std::path::Component;

/// Module resolver - handles finding and loading modules
#[derive(Debug, Clone)]
pub struct ModuleResolver {
    /// Standard library registry
    stdlib_registry: Arc<ModuleRegistry>,
    /// Loaded file modules as new VM runtime exports.
    runtime_file_modules: Arc<SharedMap<PathBuf, RuntimeExport>>,
    /// Search paths for module resolution
    search_paths: Vec<PathBuf>,
    /// Package modules resolved from Lk.toml dependencies/workspace members
    package_modules: Arc<SharedMap<String, PathBuf>>,
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
            runtime_file_modules: Arc::new(SharedMap::new()),
            // Prefer current directory; also allow `core/` for workspace runs.
            search_paths: vec![PathBuf::from("."), PathBuf::from("core")],
            package_modules: Arc::new(SharedMap::new()),
        }
    }

    pub fn runtime_builtin_iter(&self) -> impl Iterator<Item = (&Arc<str>, &RuntimeExport)> {
        self.stdlib_registry.get_all_runtime_builtins().iter()
    }

    pub fn get_runtime_builtin(&self, name: &str) -> Option<&RuntimeExport> {
        self.stdlib_registry.get_runtime_builtin(name)
    }

    /// Add a search path for file resolution
    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.search_paths.contains(&path) {
            self.search_paths.push(path);
        }
    }

    /// Set the default base directory for relative file imports.
    ///
    /// Additive and idempotent: a nested load re-runs this for each file it
    /// walks through, and appending unconditionally made `search_paths` grow
    /// with duplicates on every hop (each of which is then re-`exists()`-probed
    /// per candidate).
    #[cfg(feature = "std")]
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
        self.add_search_path(base.clone());
        self.add_search_path(base.join("lib"));
        self.add_search_path(base.join("modules"));
    }

    /// Register a package root module. `use name;` resolves to this file when
    /// no stdlib module with the same name exists.
    pub fn register_package_module(&self, name: impl Into<String>, root: impl Into<PathBuf>) {
        self.package_modules.insert(name.into(), root.into());
    }

    pub fn package_module_path(&self, name: &str) -> Option<PathBuf> {
        self.package_modules.get(name).map(|root| root.value().clone())
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "std")]
    pub fn resolve_runtime_file(&self, path: &str) -> Result<RuntimeExport> {
        let resolved_path = self.resolve_file_path(path)?;
        self.resolve_resolved_runtime_file(&resolved_path)
    }

    pub fn resolve_runtime_module(&self, name: &str) -> Result<RuntimeExport> {
        if let Ok(module) = self.stdlib_registry.get_module(name) {
            return module.runtime_exports();
        }
        // Package/file-backed module resolution needs the filesystem, which the
        // no_std VM core lacks; only in-memory registry modules resolve (M0.7/8).
        #[cfg(feature = "std")]
        {
            let Some(root) = self.package_modules.get(name) else {
                return Err(anyhow!("Module '{}' not found", name));
            };
            self.resolve_resolved_runtime_file(root.value())
        }
        #[cfg(not(feature = "std"))]
        {
            Err(anyhow!("Module '{}' not found", name))
        }
    }

    #[cfg(feature = "std")]
    fn resolve_resolved_runtime_file(&self, resolved_path: &Path) -> Result<RuntimeExport> {
        let resolved_path = Self::normalize_path(resolved_path.to_path_buf());
        if let Some(module) = self.runtime_file_modules.get(&resolved_path) {
            return Ok(module.value().shallow_clone_shared());
        }
        let module = self.load_file_runtime_module(&resolved_path)?;
        self.runtime_file_modules
            .insert(resolved_path.clone(), module.shallow_clone_shared());
        Ok(module)
    }

    pub fn resolve_source_runtime(&self, src: &str) -> Result<RuntimeExport> {
        self.resolve_source_runtime_with_base(src, None)
    }

    fn resolve_source_runtime_with_base(&self, src: &str, base_dir: Option<PathBuf>) -> Result<RuntimeExport> {
        let program = parse_program_source(
            src,
            ParseOptions {
                base_dir,
                ..ParseOptions::default()
            },
        )
        .map_err(|e| anyhow!(e.to_string()))?;
        let resolver = Arc::new(self.clone());
        let mut ctx = VmContext::new().with_resolver(resolver);
        let result = program.execute_with_ctx(&mut ctx)?;
        Ok(result.into_exports())
    }

    /// Resolve file path using search paths
    #[cfg(feature = "std")]
    pub fn resolve_file_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        // Only relative import paths are accepted. `..` is *not* rejected —
        // `use "../general/fib";` is supported and used by the examples — so
        // the `starts_with(root)` checks below are a normalization preference,
        // not containment: a candidate that escapes its root is still returned.
        // Tightening that into real containment needs a decision about which
        // root a `..` import is allowed to escape into. TODO(security): decide.
        if !path.is_relative() {
            return Err(anyhow!(
                "Absolute paths are not allowed for imports: {}",
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
                    if let Ok(canon) = p.canonicalize()
                        && canon.starts_with(root)
                    {
                        return Ok(Self::normalize_path(canon));
                    }
                    return Ok(Self::normalize_path(p));
                }
            }

            // Try ${MOD_NAME}.lk
            let candidate1 = root.join(base.with_extension("lk"));
            if candidate1.exists() {
                if let Ok(canon) = candidate1.canonicalize()
                    && canon.starts_with(root)
                {
                    return Ok(Self::normalize_path(canon));
                }
                return Ok(Self::normalize_path(candidate1));
            }

            // Try ${MOD_NAME}/mod.lk
            let candidate2 = root.join(base.join("mod.lk"));
            if candidate2.exists() {
                if let Ok(canon) = candidate2.canonicalize()
                    && canon.starts_with(root)
                {
                    return Ok(Self::normalize_path(canon));
                }
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

    #[cfg(feature = "std")]
    fn load_file_runtime_module(&self, path: &Path) -> Result<RuntimeExport> {
        let src = std::fs::read_to_string(path)?;
        let mut resolver = self.clone();
        if let Some(parent) = path.parent() {
            resolver.set_base_dir(parent.to_path_buf());
        }
        resolver.resolve_source_runtime_with_base(&src, path.parent().map(Path::to_path_buf))
    }
}

impl Default for ModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}

fn runtime_export_field(module: &RuntimeExport, name: &str) -> Result<RuntimeExport> {
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
        return Ok(RuntimeExport::new(value, module.shared_state(), module.shared_module()));
    }
    Err(anyhow!("Export '{}' not found in runtime module", name))
}

pub fn execute_imports(imports: &[ImportStmt], resolver: &ModuleResolver, env: &mut VmContext) -> Result<()> {
    for import in imports {
        if let ImportStmt::Items { items, source } = import {
            let module = resolve_runtime_import_source(source, resolver)?;
            // An imported module's `impl` blocks must dispatch here too; the
            // methods stay bound to their own module and heap.
            env.register_imported_types(&module);
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
                env.register_imported_types(&module_export);
                env.define_runtime_global(default_module_binding(module), module_export);
            }
            ImportStmt::File { path } => {
                // File imports read `.lk` files off disk — std-only (M0.7/8).
                #[cfg(feature = "std")]
                {
                    let module_name = Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("module")
                        .to_string();
                    let module = resolver.resolve_runtime_file(path)?;
                    env.register_imported_types(&module);
                    env.define_runtime_global(module_name, module);
                }
                #[cfg(not(feature = "std"))]
                {
                    let _ = env;
                    return Err(anyhow!("File imports require the std feature: '{}'", path));
                }
            }
            ImportStmt::Items { .. } => unreachable!("items imports are handled before runtime use binding"),
            ImportStmt::Namespace { alias, source } => {
                let module = resolve_runtime_import_source(source, resolver)?;
                env.register_imported_types(&module);
                env.define_runtime_global(alias.clone(), module);
            }
            ImportStmt::ModuleAlias { module, alias } => {
                let module_export = resolver.resolve_runtime_module(module)?;
                // Same order as every other variant: an aliased module's trait
                // impls have to be registered too, or `use shape as s;`
                // dispatches worse than `use shape;`.
                env.register_imported_types(&module_export);
                env.define_runtime_global(alias.clone(), module_export);
            }
        }
    }
    Ok(())
}

pub fn default_module_binding(module: &str) -> String {
    module.rsplit('/').next().unwrap_or(module).to_string()
}

fn resolve_runtime_import_source(source: &ImportSource, resolver: &ModuleResolver) -> Result<RuntimeExport> {
    match source {
        #[cfg(feature = "std")]
        ImportSource::File(path) => resolver.resolve_runtime_file(path),
        // File imports read `.lk` files off disk — std-only (M0.7/8).
        #[cfg(not(feature = "std"))]
        ImportSource::File(path) => Err(anyhow!("File imports require the std feature: '{}'", path)),
        ImportSource::Module(name) => resolver.resolve_runtime_module(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ModuleRegistry;
    use crate::stmt::{ImportItem, Program, import::collect_program_imports};
    use crate::vm::ProgramExec;
    use std::path::PathBuf;

    fn parse_program(source: &str) -> Result<Program> {
        parse_program_source(source, ParseOptions::default()).map_err(|e| anyhow!(e.to_string()))
    }

    fn execute_import_source(source: &str, resolver: Arc<ModuleResolver>) -> Result<RuntimeVal> {
        let program = parse_program(source)?;
        let mut ctx = VmContext::new().with_resolver(resolver);
        Ok(*program.execute_with_ctx(&mut ctx)?.first_return())
    }

    #[test]
    fn test_parent_module_item_import_parses() -> Result<()> {
        let program = parse_program("use { file, std } from io;")?;
        assert_eq!(
            collect_program_imports(&program),
            vec![ImportStmt::Items {
                items: vec![
                    ImportItem {
                        name: "file".to_string(),
                        alias: None,
                    },
                    ImportItem {
                        name: "std".to_string(),
                        alias: None,
                    },
                ],
                source: ImportSource::Module("io".to_string()),
            }]
        );
        Ok(())
    }

    #[test]
    fn test_slash_module_path_is_rejected() {
        let err = parse_program("use io/file;").expect_err("slash module paths are no longer supported");
        assert!(err.to_string().contains("Expected"));
    }

    #[test]
    fn test_parent_module_item_import_binds_child_namespace() -> Result<()> {
        use crate::{
            module::{ModuleProvider, RuntimeNativeExport, runtime_export_from_plain_native_entries},
            util::fast_map::fast_hash_map_from_iter,
            val::{HeapStore, HeapValue, TypedMap},
            vm::{NativeArgs, NativeRuntime},
        };

        #[derive(Debug)]
        struct TestModule;

        impl ModuleProvider for TestModule {
            fn name(&self) -> &str {
                "io"
            }

            fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
                Ok(())
            }

            fn runtime_exports(&self) -> Result<RuntimeExport> {
                let file =
                    runtime_export_from_plain_native_entries(&[RuntimeNativeExport::plain("marker", marker, 0)], &[]);
                let mut heap = HeapStore::new();
                let file = crate::vm::import_runtime_export(&file, &mut heap)?;
                let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(
                    fast_hash_map_from_iter([(Arc::<str>::from("file"), file)]),
                ))));
                Ok(RuntimeExport::from_value(value, heap))
            }
        }

        fn marker(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
            Ok(RuntimeVal::Int(7))
        }

        let mut registry = ModuleRegistry::new();
        registry.register_module("io", Box::new(TestModule))?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let result = execute_import_source(
            r#"
            use { file } from io;
            return file.marker();
            "#,
            resolver,
        )?;
        assert_eq!(result, RuntimeVal::Int(7));
        Ok(())
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

        // Parent directory components are now allowed (relative to source file)
        // but must stay within a search_path root

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
            use "examples/fib";
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
            use { iterative as fib_iter } from "examples/fib";
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
            use * as fibs from "examples/fib";
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
            use * as calc from "calc";
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
            use * as counter from "counter";
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
