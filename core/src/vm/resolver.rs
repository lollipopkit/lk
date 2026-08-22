//! Module resolution and import execution.
//!
//! This is runtime behaviour — loading a module means parsing it, running it,
//! and binding its exports into a [`VmContext`] — so it belongs to the VM
//! layer. It used to live in `stmt::import` next to the `use`-statement AST
//! types, which made the AST layer depend on the executor: a cycle
//! (`stmt` ↔ `vm`) that blocked separating the front end from the VM.
//! `stmt::import` now holds only the syntax (`ImportStmt` and friends) and the
//! AST walk that collects them.

use crate::compat::path::PathBuf;
// File loading — and therefore every `Path` use — is a `std` surface.
#[cfg(feature = "std")]
use crate::compat::path::Path;
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
    /// Loaded file modules as new VM runtime exports. Only ever read by the
    /// `std`-gated file-loading path; without `std` there are no files to load,
    /// so the map stays empty rather than the field being conditional (that
    /// would have to be threaded through every constructor and clone).
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    runtime_file_modules: Arc<SharedMap<PathBuf, RuntimeExport>>,
    /// Search paths for module resolution
    search_paths: Vec<PathBuf>,
    /// Package modules resolved from Lk.toml dependencies/workspace members
    package_modules: Arc<SharedMap<String, PathBuf>>,
    /// The directory a file import may not escape: the package root (the nearest
    /// ancestor with an `Lk.toml`), falling back to the importing file's own
    /// directory when there is no manifest.
    ///
    /// `..` in an import path is *allowed* — `use "../general/fib";` is used by
    /// the examples — so the boundary is containment, not a ban on `..`. Set per
    /// loaded file (`set_base_dir`), which is what makes it "a package's imports
    /// cannot leave that package": a dependency loaded from elsewhere gets its
    /// own root, not the importer's.
    #[cfg(feature = "std")]
    containment_root: Option<PathBuf>,
    /// The file modules currently being loaded, innermost last.
    ///
    /// The cache above is only written once a module has finished loading, so
    /// without this a circular import recurses until the stack runs out — the
    /// process aborts with "stack overflow" and says nothing about which files
    /// are involved. Shared through every clone down an import chain, which is
    /// what makes it see the whole chain rather than one link.
    #[cfg(feature = "std")]
    loading: Arc<crate::compat::sync::Mutex<Vec<PathBuf>>>,
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
            #[cfg(feature = "std")]
            containment_root: None,
            #[cfg(feature = "std")]
            loading: Arc::new(crate::compat::sync::Mutex::new(Vec::new())),
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
        // Canonicalized *before* the manifest search: `find_manifest` walks
        // parents, and a relative base like `.` has none to walk — the package
        // root would silently come back as `.` itself.
        let anchor = base.canonicalize().unwrap_or(base);
        let root = crate::package::find_manifest(&anchor)
            .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
            .unwrap_or(anchor);
        self.containment_root = Some(root.canonicalize().unwrap_or(root));
    }

    /// Whether `candidate` is inside the containment root (see the field docs).
    /// Unset root — a resolver that was never given a base directory, e.g. the
    /// in-memory registry-only one — contains everything.
    #[cfg(feature = "std")]
    fn within_containment_root(&self, candidate: &Path) -> bool {
        let Some(root) = &self.containment_root else {
            return true;
        };
        let resolved = candidate.canonicalize();
        resolved.as_deref().unwrap_or(candidate).starts_with(root)
    }

    /// `candidate` is expected already canonicalized — the raw join (`././x.lk`)
    /// says nothing about *which* directory was left.
    #[cfg(feature = "std")]
    fn escaped_containment_root(&self, requested: &Path, candidate: &Path) -> anyhow::Error {
        anyhow!(
            "import '{}' resolves to '{}', which is outside '{}' — an import may not leave its package \
             (the nearest directory with an Lk.toml, or the importing file's directory when there is none)",
            requested.display(),
            candidate.display(),
            self.containment_root.as_deref().unwrap_or(Path::new(".")).display()
        )
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
        {
            let mut loading = self
                .loading
                .lock()
                .map_err(|_| anyhow!("module loading state poisoned"))?;
            if let Some(start) = loading.iter().position(|path| path == &resolved_path) {
                let mut chain: Vec<String> = loading[start..].iter().map(|path| path.display().to_string()).collect();
                chain.push(resolved_path.display().to_string());
                return Err(anyhow!("circular import: {}", chain.join(" -> ")));
            }
            loading.push(resolved_path.clone());
        }
        let loaded = self.load_file_runtime_module(&resolved_path);
        if let Ok(mut loading) = self.loading.lock() {
            loading.pop();
        }
        let module = loaded?;
        self.runtime_file_modules
            .insert(resolved_path.clone(), module.shallow_clone_shared());
        Ok(module)
    }

    pub fn resolve_source_runtime(&self, src: &str) -> Result<RuntimeExport> {
        self.resolve_source_runtime_with_base(src, None, crate::val::TypeScope::anonymous())
    }

    fn resolve_source_runtime_with_base(
        &self,
        src: &str,
        base_dir: Option<PathBuf>,
        type_scope: crate::val::TypeScope,
    ) -> Result<RuntimeExport> {
        let seed_dir = base_dir.clone();
        let program = parse_program_source(
            src,
            ParseOptions {
                base_dir,
                ..ParseOptions::default()
            },
        )
        .map_err(|e| anyhow!(e.to_string()))?;
        let resolver = Arc::new(self.clone());
        let mut ctx = VmContext::new().with_resolver(resolver).with_type_scope(type_scope);
        // The loaded module's own directory, so *its* imports are seeded too:
        // a type crossing one more module boundary is still a type this file
        // names.
        let result = program.execute_with_ctx_from(&mut ctx, seed_dir.as_deref())?;
        Ok(result.into_exports())
    }

    /// Every file module loaded so far, in load order.
    ///
    /// The cache behind this is shared by every resolver clone down an import
    /// chain, so it is the *transitive* closure of what a program pulled in —
    /// which is what `execute_imports` registers, so that a value built by a
    /// module its own dependency imported still finds its methods.
    #[cfg(feature = "std")]
    pub fn loaded_file_modules(&self) -> Vec<RuntimeExport> {
        self.runtime_file_modules
            .iter()
            .map(|entry| entry.value().shallow_clone_shared())
            .collect()
    }

    /// Resolve file path using search paths
    #[cfg(feature = "std")]
    pub fn resolve_file_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        // Only relative import paths are accepted, and a relative one must still
        // resolve *inside* the containment root — `..` is allowed as a way to
        // reach a sibling directory of the same package, not as a way out of it
        // (see `containment_root`).
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

        // Containment is enforced per candidate, and an out-of-root candidate
        // *skips* rather than failing the search: `search_paths` always starts
        // with `.` and `core`, which have nothing to do with the importing file's
        // directory whenever cwd differs from it. Returning an error on the first
        // escape meant a name that also existed in cwd shadowed — and hard-failed
        // — an import whose real target sat under a later root.
        //
        // (The `starts_with(root)` tests this replaced were a *normalization*
        // preference, never a boundary: the fallback returned the path anyway.)
        let mut escaped: Option<PathBuf> = None;
        for root in &self.search_paths {
            let candidates = [
                // The input already includes `.lk`.
                (base.extension().and_then(|s| s.to_str()) == Some("lk")).then(|| root.join(&base)),
                Some(root.join(base.with_extension("lk"))),
                Some(root.join(base.join("mod.lk"))),
            ];
            for candidate in candidates.into_iter().flatten() {
                if !candidate.exists() {
                    continue;
                }
                let resolved = candidate.canonicalize().unwrap_or(candidate);
                if !self.within_containment_root(&resolved) {
                    escaped.get_or_insert(resolved);
                    continue;
                }
                return Ok(Self::normalize_path(resolved));
            }
        }

        // Nothing in-root matched. If some candidate *did* exist but sat outside,
        // that is the useful diagnosis — a plain "not found" would hide it.
        if let Some(escaped) = escaped {
            return Err(self.escaped_containment_root(path, &escaped));
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
        // The normalized path is this module's type identity: the compiler has
        // no idea what file it is compiling, so the loader is the only place
        // that can supply it (`val::TypeScope`).
        resolver.resolve_source_runtime_with_base(
            &src,
            path.parent().map(Path::to_path_buf),
            crate::val::TypeScope::from_path(&path.to_string_lossy()),
        )
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
    // A type: bind the constructor the declaring module generates beside it
    // (`crate::stmt::struct_ctors`), which is an ordinary top-level `fn` and so
    // an ordinary export. Binding *that* is what makes the imported name build
    // the declaring module's type rather than a same-named one — the call runs
    // in `m`, so the scope, the field order and trait dispatch are all right,
    // exactly as `m.P { … }` already is.
    //
    // It used to refuse here, with a message that said an imported type "cannot
    // be named or constructed directly" — which `m.P { … }` had been doing all
    // along.
    if let Some(ctor) = map.get_str(&crate::stmt::struct_ctors::constructor_name(name)) {
        return Ok(RuntimeExport::new(ctor, module.shared_state(), module.shared_module()));
    }
    if name.starts_with(char::is_uppercase) {
        // Say which of the three it is, rather than asserting what the module
        // holds. The old message ended "and this module declares neither",
        // which it had not checked: `type Pair = List<Int>;` is declared, and
        // got told it was not.
        if module
            .shared_module()
            .type_info
            .traits
            .iter()
            .any(|decl| decl.name == name)
        {
            return Err(anyhow!(
                "'{name}' is a `trait`, and a trait has no constructor to bind, so it cannot be imported as \
                 a name. Import the type that implements it instead — the `impl` travels with the type."
            ));
        }
        return Err(anyhow!(
            "'{name}' is not an export of this module — no value, and no `struct` by that name to bind a \
             constructor for. A `trait` and a `type` alias are both compile-time only and neither can be \
             imported as a name."
        ));
    }
    Err(anyhow!("'{}' is not an export of this module", name))
}

pub fn execute_imports(imports: &[ImportStmt], resolver: &ModuleResolver, env: &mut VmContext) -> Result<()> {
    for import in imports {
        if let ImportStmt::Items { items, source } = import {
            let module = resolve_runtime_import_source(source, resolver)?;
            // An imported module's `impl` blocks must dispatch here too; the
            // methods stay bound to their own module and heap.
            env.register_imported_types(&module)?;
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
                env.register_imported_types(&module_export)?;
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
                    env.register_imported_types(&module)?;
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
                env.register_imported_types(&module)?;
                env.define_runtime_global(alias.clone(), module);
            }
            ImportStmt::ModuleAlias { module, alias } => {
                let module_export = resolver.resolve_runtime_module(module)?;
                // Same order as every other variant: an aliased module's trait
                // impls have to be registered too, or `use shape as s;`
                // dispatches worse than `use shape;`.
                env.register_imported_types(&module_export)?;
                env.define_runtime_global(alias.clone(), module_export);
            }
        }
    }
    // The per-import registrations above only reach one level. A value can
    // arrive from deeper than that — `main` imports `mid`, `mid` imports `c`,
    // and `mid.passthru()` hands back a `c` struct — and its methods live in a
    // module `main` never named, so dispatch failed outright ("Object has no
    // method"). Registering the resolver's whole loaded set closes that: it is
    // already the transitive closure, and scope-keyed entries mean the extra
    // modules cannot clobber anything (see `val::TypeScope`).
    #[cfg(feature = "std")]
    for module in resolver.loaded_file_modules() {
        env.register_imported_types(&module)?;
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

// Module resolution under test means resolving *files*; the no_std build
// has no filesystem to resolve against.
#[cfg(all(test, feature = "std"))]
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
                    crate::util::value_map::value_map_from_iter([(Arc::<str>::from("file"), file)]),
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

        // Relative simple path that likely does not exist should return not found
        // (error message still OK but not due to security check)
        let rel = PathBuf::from("does_not_exist.lk");
        assert!(resolver.resolve_file_path(&rel.to_string_lossy()).is_err());
    }

    /// A method in an imported module may call another method on `self`.
    ///
    /// `take_runtime_callable_state` moves a module's shared state out of its
    /// mutex for the duration of a call and leaves `Default::default()` behind,
    /// so the mechanism cannot be re-entered — and a method calling another
    /// method on `self` re-enters by definition. The outer call took the state,
    /// the inner call took the empty shell, and the executor refused "a module
    /// expecting 83 globals against a table of 0" for a program that never
    /// mentions a global. All three shapes below were broken; each works when
    /// the same code sits in one file, which is what made it a cross-module bug
    /// rather than a dispatch bug.
    #[test]
    fn an_imported_method_may_call_another_method_on_self() -> Result<()> {
        let cases = [
            // A trait default body reaching the impl's own method.
            (
                "trait Area { fn area(self) -> Int; fn twice(self) -> Int { return self.area() * 2; } }\n\
                 impl Area for Sq { fn area(self) -> Int { return self.side * self.side; } }",
                "twice",
            ),
            // An inherent method reaching another inherent method.
            (
                "impl Sq { fn area(self) -> Int { return self.side * self.side; } \n\
                 fn twice(self) -> Int { return self.area() * 2; } }",
                "twice",
            ),
            // An inherent method reaching a trait method.
            (
                "trait Area { fn area(self) -> Int; }\n\
                 impl Area for Sq { fn area(self) -> Int { return self.side * self.side; } }\n\
                 impl Sq { fn twice(self) -> Int { return self.area() * 2; } }",
                "twice",
            ),
        ];
        for (index, (impls, method)) in cases.iter().enumerate() {
            let temp = tempfile::tempdir()?;
            let dep = temp.path().join("shape.lk");
            std::fs::write(
                &dep,
                format!("struct Sq {{ side: Int }}\n{impls}\nfn make(n: Int) -> Sq {{ return Sq {{ side: n }}; }}\n"),
            )?;
            let mut resolver = ModuleResolver::new();
            resolver.set_base_dir(temp.path().to_path_buf());
            let value = execute_import_source(
                &format!("use {{ make }} from \"./shape.lk\";\nreturn make(3).{method}();\n"),
                Arc::new(resolver),
            )?;
            assert_eq!(value, RuntimeVal::Int(18), "case {index}");
        }
        Ok(())
    }

    /// Module A's method may call into B and have B call back into A.
    ///
    /// The step past `an_imported_method_may_call_another_method_on_self`: there
    /// the re-entered module *was* the one executing, so the call could simply
    /// use the live state. Here it is not — B is — and A's state is out on the
    /// stack, so neither borrowing it nor reusing the current one is right.
    ///
    /// Borrowing was never the only way to run a foreign body, though.
    /// `call_foreign_module_method` keeps the current heap and swaps in a global
    /// table shaped like the declaring module's, so it needs A's *module*, not
    /// A's state. Before that, the re-entering call got the empty placeholder
    /// and the failure surfaced as "module expected 84 globals, got 0".
    #[test]
    fn a_module_may_be_re_entered_through_another_module() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(temp.path().join("b.lk"), "fn helper(x) { return x.base() + 100; }\n")?;
        std::fs::write(
            temp.path().join("a.lk"),
            "use { helper } from \"./b.lk\";\n\
             struct A { v: Int }\n\
             impl A {\n\
                 fn base(self) -> Int { return self.v; }\n\
                 fn viab(self) -> Int { return helper(self); }\n\
             }\n\
             fn make(n: Int) -> A { return A { v: n }; }\n",
        )?;
        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(temp.path().to_path_buf());
        let value = execute_import_source(
            "use { make } from \"./a.lk\";\nreturn make(5).viab();\n",
            Arc::new(resolver),
        )?;

        assert_eq!(value, RuntimeVal::Int(105));
        Ok(())
    }

    /// `..` is allowed as a way to reach a sibling directory of the same package,
    /// not as a way out of it. The boundary is the package root (nearest
    /// `Lk.toml`), or the importing file's directory when there is no manifest.
    #[test]
    fn resolve_file_path_contains_parent_traversal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let outside = temp.path().join("outside.lk");
        std::fs::write(&outside, "return nil;\n")?;
        let proj = temp.path().join("proj");
        std::fs::create_dir_all(proj.join("sub"))?;
        std::fs::write(proj.join("sibling.lk"), "return nil;\n")?;
        std::fs::write(proj.join("sub").join("inner.lk"), "return nil;\n")?;

        // No manifest: the importing file's directory is the boundary.
        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(proj.join("sub"));
        assert!(
            resolver.resolve_file_path("inner").is_ok(),
            "a sibling inside the boundary resolves"
        );
        let escaped = resolver.resolve_file_path("../sibling");
        assert!(escaped.is_err(), "`..` may not leave the boundary");
        assert!(
            escaped.unwrap_err().to_string().contains("outside"),
            "the error must say what boundary was crossed"
        );
        assert!(resolver.resolve_file_path("../../outside").is_err());

        // An out-of-root candidate must *skip*, not abort the search: a name that
        // also exists in cwd (always the first search path) would otherwise
        // hard-fail an import whose real target sits under a later root.
        let cwd_shadow = std::env::current_dir()?.join("shadowed.lk");
        std::fs::write(&cwd_shadow, "return nil;\n")?;
        std::fs::write(proj.join("sub").join("shadowed.lk"), "return nil;\n")?;
        let resolved = resolver.resolve_file_path("shadowed");
        let _ = std::fs::remove_file(&cwd_shadow);
        assert!(
            resolved.is_ok(),
            "the in-root candidate must win over an out-of-root shadow: {resolved:?}"
        );

        // With a manifest above, the package root is the boundary, so the same
        // `..` import now resolves.
        std::fs::write(
            proj.join(crate::package::MANIFEST_FILE),
            "name = \"t\"\nversion = \"0.1.0\"\n",
        )?;
        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(proj.join("sub"));
        assert!(
            resolver.resolve_file_path("../sibling").is_ok(),
            "`..` inside the package is fine"
        );
        assert!(
            resolver.resolve_file_path("../../outside").is_err(),
            "but not out of the package"
        );
        Ok(())
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

    /// A circular import reports the cycle instead of overflowing the stack.
    ///
    /// The module cache is only written once a load finishes, so without the
    /// in-progress stack the resolver recurses until the process aborts with
    /// "stack overflow" — which names neither file.
    #[test]
    #[cfg(feature = "std")]
    fn circular_file_imports_are_reported() -> Result<()> {
        // Unique per run: the pid alone collides when the suite runs the same
        // test binary under more than one feature set.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("lk_cycle_{}_{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("a.lk"), "use { g } from \"b\";\nfn f() -> Int { return 1; }\n")?;
        std::fs::write(dir.join("b.lk"), "use { f } from \"a\";\nfn g() -> Int { return 2; }\n")?;

        // Imports resolve relative to a search path; absolute ones are refused
        // outright, which is a different rule from the one under test.
        let mut resolver = ModuleResolver::new();
        resolver.add_search_path(dir.clone());
        let error = resolver
            .resolve_runtime_file("a.lk")
            .expect_err("a cycle must be an error");
        let message = format!("{error:#}");
        assert!(message.contains("circular import"), "unexpected error: {message}");

        let _ = std::fs::remove_dir_all(&dir);
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

    /// An imported type can be constructed: `module.Type { … }`.
    ///
    /// A module exports *values*, and a `struct` declaration is not one, so a
    /// module that declared a type could not let its users make one — every
    /// such module hand-wrote a `make`. It cannot simply be allowed either: a
    /// type's identity carries its defining module (`TypeScope`), and a `Pt`
    /// built in the importer is not the `Pt` that `impl … for Pt` was
    /// registered against.
    ///
    /// So the *defining* module builds it: `stmt::struct_ctors` puts a
    /// named-parameter constructor beside every `struct`, and the literal is
    /// parse-time sugar for a call to it. This pins all three things that made
    /// it worth doing: the fields, the trait method, and the declaration-order
    /// display.
    #[test]
    fn an_imported_type_can_be_constructed_by_the_module_that_owns_it() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("types.lk"),
            "struct Pt { x: Int, y: Int }\n             trait Norm { fn norm(self) -> Int; }\n             impl Norm for Pt { fn norm(self) -> Int { return self.x + self.y; } }\n",
        )?;
        let program = crate::syntax::parse_program_source(
            "use \"types\";\nlet p = types.Pt { x: 3, y: 4 };\nreturn [p.x, p.norm(), \"${p}\"];\n",
            crate::syntax::ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..crate::syntax::ParseOptions::default()
            },
        )
        .expect("program should parse");
        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(temp.path().to_path_buf());
        let mut ctx = crate::vm::VmContext::new().with_resolver(alloc::sync::Arc::new(resolver));
        let result = crate::vm::ProgramExec::execute_with_ctx(&program, &mut ctx)?;
        assert_eq!(result.display_first_return(), r#"[3,7,"Pt{x:3,y:4}"]"#);
        Ok(())
    }
}
