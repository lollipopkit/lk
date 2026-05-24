//! Runtime helpers exposed to LLVM-generated code.
//!
//! Native LLVM output may still ask this module to replay imports and expose
//! runtime objects, but it must not re-enter the Instr32 VM through an artifact
//! launcher.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Result, anyhow};

use crate::{
    module::ModuleRegistry,
    stmt::{ImportSource, ImportStmt, ModuleResolver, deserialize_imports, execute_imports},
    util::fast_map::{FastHashMap, fast_hash_map_new},
    val::{HeapStore, HeapValue, RuntimeVal},
    vm::{RuntimeExport32, VmContext, import_runtime_export},
};

#[cfg(test)]
mod imports;
#[cfg(test)]
mod tests;

#[cfg(test)]
use imports::{imports_need_concurrency_globals, stdlib_module_names_from_imports};

#[cfg(not(test))]
unsafe extern "Rust" {
    fn lk_stdlib_register_core_globals(registry: &mut ModuleRegistry);
    fn lk_stdlib_register_concurrency_globals(registry: &mut ModuleRegistry);
    fn lk_stdlib_register_module_io(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_json(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_yaml(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_toml(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_iter(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_math(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_string(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_list(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_map(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_datetime(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_os(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_tcp(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_stream(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_task(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_chan(registry: &mut ModuleRegistry) -> Result<()>;
    fn lk_stdlib_register_module_time(registry: &mut ModuleRegistry) -> Result<()>;
}

#[cfg(test)]
fn lk_stdlib_register_core_globals(_registry: &mut ModuleRegistry) {}

#[cfg(test)]
fn lk_stdlib_register_concurrency_globals(_registry: &mut ModuleRegistry) {}

type StdlibRegistrar = fn(&mut ModuleRegistry) -> Result<()>;

macro_rules! stdlib_registrar_bridge {
    ($bridge:ident, $extern_name:ident) => {
        #[cfg(not(test))]
        fn $bridge(registry: &mut ModuleRegistry) -> Result<()> {
            unsafe { $extern_name(registry) }
        }

        #[cfg(test)]
        fn $bridge(_registry: &mut ModuleRegistry) -> Result<()> {
            Ok(())
        }
    };
}

stdlib_registrar_bridge!(register_stdlib_io_bridge, lk_stdlib_register_module_io);
stdlib_registrar_bridge!(register_stdlib_json_bridge, lk_stdlib_register_module_json);
stdlib_registrar_bridge!(register_stdlib_yaml_bridge, lk_stdlib_register_module_yaml);
stdlib_registrar_bridge!(register_stdlib_toml_bridge, lk_stdlib_register_module_toml);
stdlib_registrar_bridge!(register_stdlib_iter_bridge, lk_stdlib_register_module_iter);
stdlib_registrar_bridge!(register_stdlib_math_bridge, lk_stdlib_register_module_math);
stdlib_registrar_bridge!(register_stdlib_string_bridge, lk_stdlib_register_module_string);
stdlib_registrar_bridge!(register_stdlib_list_bridge, lk_stdlib_register_module_list);
stdlib_registrar_bridge!(register_stdlib_map_bridge, lk_stdlib_register_module_map);
stdlib_registrar_bridge!(register_stdlib_datetime_bridge, lk_stdlib_register_module_datetime);
stdlib_registrar_bridge!(register_stdlib_os_bridge, lk_stdlib_register_module_os);
stdlib_registrar_bridge!(register_stdlib_tcp_bridge, lk_stdlib_register_module_tcp);
stdlib_registrar_bridge!(register_stdlib_stream_bridge, lk_stdlib_register_module_stream);
stdlib_registrar_bridge!(register_stdlib_task_bridge, lk_stdlib_register_module_task);
stdlib_registrar_bridge!(register_stdlib_chan_bridge, lk_stdlib_register_module_chan);
stdlib_registrar_bridge!(register_stdlib_time_bridge, lk_stdlib_register_module_time);

fn register_stdlib_concurrency_globals_bridge(registry: &mut ModuleRegistry) -> Result<()> {
    #[cfg_attr(test, allow(unused_unsafe))]
    unsafe {
        lk_stdlib_register_concurrency_globals(registry);
    }
    Ok(())
}

static RUNTIME_STATE: OnceLock<Mutex<RuntimeState>> = OnceLock::new();

fn with_state<R>(f: impl FnOnce(&mut RuntimeState) -> R) -> R {
    let mutex = RUNTIME_STATE.get_or_init(|| Mutex::new(RuntimeState::default()));
    let mut guard = mutex.lock().expect("runtime state poisoned");
    f(&mut guard)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_begin_session() {
    with_state(|state| state.reset_session());
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_register_search_path(ptr: *const i8, len: i64) {
    let path = read_string(ptr, len);
    if path.is_empty() {
        return;
    }
    with_state(move |state| {
        if !state.pending_search_paths.iter().any(|existing| existing == &path) {
            state.pending_search_paths.push(path);
        }
        state.imports_applied = false;
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_register_imports(ptr: *const i8, len: i64) -> i32 {
    let json = read_string(ptr, len);
    if json.trim().is_empty() {
        with_state(|state| {
            state.pending_imports.clear();
            state.imports_applied = false;
        });
        return 0;
    }
    match deserialize_imports(&json) {
        Ok(imports) => {
            with_state(move |state| {
                state.pending_imports = imports;
                state.imports_applied = false;
            });
            0
        }
        Err(err) => {
            eprintln!("lk_rt_register_imports: failed to parse imports: {err}");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_register_package_modules(ptr: *const i8, len: i64) -> i32 {
    let json = read_string(ptr, len);
    if json.trim().is_empty() {
        with_state(|state| {
            state.pending_package_modules.clear();
            state.imports_applied = false;
        });
        return 0;
    }
    match serde_json::from_str::<BTreeMap<String, String>>(&json) {
        Ok(modules) => {
            with_state(move |state| {
                state.pending_package_modules = modules.into_iter().collect();
                state.imports_applied = false;
            });
            0
        }
        Err(err) => {
            eprintln!("lk_rt_register_package_modules: failed to parse modules: {err}");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_apply_imports() -> i32 {
    let result: Result<()> = with_state(|state| state.apply_pending());
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("lk_rt_apply_imports error: {err}");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_apply_native_imports() -> i32 {
    let result: Result<()> = with_state(|state| state.apply_pending_native_only());
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("lk_rt_apply_native_imports error: {err}");
            -1
        }
    }
}

macro_rules! require_stdlib_module {
    ($fn_name:ident, $registrar:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $fn_name() {
            with_state(|state| {
                push_unique_registrar(&mut state.pending_stdlib_registrars, $registrar);
                state.imports_applied = false;
            });
        }
    };
}

require_stdlib_module!(lk_rt_require_stdlib_io, register_stdlib_io_bridge);
require_stdlib_module!(lk_rt_require_stdlib_json, register_stdlib_json_bridge);
require_stdlib_module!(lk_rt_require_stdlib_yaml, register_stdlib_yaml_bridge);
require_stdlib_module!(lk_rt_require_stdlib_toml, register_stdlib_toml_bridge);
require_stdlib_module!(lk_rt_require_stdlib_iter, register_stdlib_iter_bridge);
require_stdlib_module!(lk_rt_require_stdlib_math, register_stdlib_math_bridge);
require_stdlib_module!(lk_rt_require_stdlib_string, register_stdlib_string_bridge);
require_stdlib_module!(lk_rt_require_stdlib_list, register_stdlib_list_bridge);
require_stdlib_module!(lk_rt_require_stdlib_map, register_stdlib_map_bridge);
require_stdlib_module!(lk_rt_require_stdlib_datetime, register_stdlib_datetime_bridge);
require_stdlib_module!(lk_rt_require_stdlib_os, register_stdlib_os_bridge);
require_stdlib_module!(lk_rt_require_stdlib_tcp, register_stdlib_tcp_bridge);
require_stdlib_module!(lk_rt_require_stdlib_stream, register_stdlib_stream_bridge);

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_require_stdlib_task() {
    require_stdlib_concurrency_module(register_stdlib_task_bridge);
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_require_stdlib_chan() {
    require_stdlib_concurrency_module(register_stdlib_chan_bridge);
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_require_stdlib_time() {
    require_stdlib_concurrency_module(register_stdlib_time_bridge);
}

fn require_stdlib_concurrency_module(registrar: StdlibRegistrar) {
    with_state(|state| {
        push_unique_registrar(
            &mut state.pending_stdlib_registrars,
            register_stdlib_concurrency_globals_bridge,
        );
        push_unique_registrar(&mut state.pending_stdlib_registrars, registrar);
        state.imports_applied = false;
    });
}

fn push_unique_registrar(registrars: &mut Vec<StdlibRegistrar>, registrar: StdlibRegistrar) {
    if !registrars
        .iter()
        .any(|existing| std::ptr::fn_addr_eq(*existing, registrar))
    {
        registrars.push(registrar);
    }
}

struct RuntimeState {
    ctx: VmContext,
    artifact_globals: FastHashMap<String, RuntimeVal>,
    heap: HeapStore,
    resolver: Arc<ModuleResolver>,
    pending_search_paths: Vec<String>,
    pending_imports: Vec<ImportStmt>,
    pending_package_modules: Vec<(String, String)>,
    pending_stdlib_registrars: Vec<StdlibRegistrar>,
    imports_applied: bool,
}

impl RuntimeState {
    fn new() -> Self {
        let (ctx, resolver) = match Self::build_native_context(&[], &[], &[]) {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("lk_rt: failed to initialise stdlib context: {err}");
                let resolver = Arc::new(ModuleResolver::new());
                let ctx = VmContext::new_without_core_vm_builtins().with_resolver(Arc::clone(&resolver));
                (ctx, resolver)
            }
        };
        Self {
            ctx,
            artifact_globals: fast_hash_map_new(),
            heap: HeapStore::new(),
            resolver,
            pending_search_paths: Vec::new(),
            pending_imports: Vec::new(),
            pending_package_modules: Vec::new(),
            pending_stdlib_registrars: Vec::new(),
            imports_applied: false,
        }
    }

    fn reset_session(&mut self) {
        self.pending_search_paths.clear();
        self.pending_imports.clear();
        self.pending_package_modules.clear();
        self.pending_stdlib_registrars.clear();
        self.imports_applied = false;
        self.artifact_globals.clear();
        self.heap = HeapStore::new();
        match Self::build_native_context(&[], &[], &[]) {
            Ok((ctx, resolver)) => {
                self.ctx = ctx;
                self.resolver = resolver;
            }
            Err(err) => {
                eprintln!("lk_rt: failed to reset stdlib context: {err}");
                let resolver = Arc::new(ModuleResolver::new());
                self.ctx = VmContext::new_without_core_vm_builtins().with_resolver(Arc::clone(&resolver));
                self.resolver = resolver;
            }
        }
    }

    fn apply_pending(&mut self) -> Result<()> {
        let (mut ctx, resolver) = Self::build_context(
            &self.pending_search_paths,
            &self.pending_package_modules,
            &self.pending_stdlib_registrars,
        )?;
        if !self.pending_imports.is_empty() {
            execute_imports(&self.pending_imports, resolver.as_ref(), &mut ctx)?;
        }
        self.ctx = ctx;
        self.resolver = resolver;
        self.heap = HeapStore::new();
        self.imports_applied = true;
        Ok(())
    }

    fn apply_pending_native_only(&mut self) -> Result<()> {
        let (ctx, resolver) = Self::build_native_context(
            &self.pending_search_paths,
            &self.pending_package_modules,
            &self.pending_stdlib_registrars,
        )?;
        self.ctx = ctx;
        self.resolver = resolver;
        self.heap = HeapStore::new();
        self.artifact_globals.clear();
        if !self.pending_imports.is_empty() {
            let imports = self.pending_imports.clone();
            let resolver = Arc::clone(&self.resolver);
            Self::apply_native_imports(&imports, resolver.as_ref(), self)?;
        }
        self.imports_applied = true;
        Ok(())
    }

    fn apply_native_imports(imports: &[ImportStmt], resolver: &ModuleResolver, state: &mut RuntimeState) -> Result<()> {
        for import in imports {
            match import {
                ImportStmt::Module { module } => {
                    let export = resolver.resolve_runtime_module(module)?;
                    let value = import_runtime_export(&export, &mut state.heap)?;
                    state.artifact_globals.insert(module.clone(), value);
                }
                ImportStmt::ModuleAlias { module, alias } => {
                    let export = resolver.resolve_runtime_module(module)?;
                    let value = import_runtime_export(&export, &mut state.heap)?;
                    state.artifact_globals.insert(alias.clone(), value);
                }
                ImportStmt::Items { items, source } => {
                    let module = Self::resolve_native_import_source(source, resolver)?;
                    for item in items {
                        let symbol_name = item.alias.as_ref().unwrap_or(&item.name);
                        let export = Self::runtime_export_field(&module, &item.name)?;
                        let value = import_runtime_export(&export, &mut state.heap)?;
                        state.artifact_globals.insert(symbol_name.clone(), value);
                    }
                }
                ImportStmt::Namespace { alias, source } => {
                    let export = Self::resolve_native_import_source(source, resolver)?;
                    let value = import_runtime_export(&export, &mut state.heap)?;
                    state.artifact_globals.insert(alias.clone(), value);
                }
                ImportStmt::File { path } => {
                    let module_name = std::path::Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("module")
                        .to_string();
                    let export = resolver.resolve_runtime_file(path)?;
                    let value = import_runtime_export(&export, &mut state.heap)?;
                    state.artifact_globals.insert(module_name, value);
                }
            }
        }
        Ok(())
    }

    fn resolve_native_import_source(source: &ImportSource, resolver: &ModuleResolver) -> Result<RuntimeExport32> {
        match source {
            ImportSource::Module(module) => resolver.resolve_runtime_module(module),
            ImportSource::File(path) => resolver.resolve_runtime_file(path),
        }
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

    fn build_context(
        search_paths: &[String],
        package_modules: &[(String, String)],
        stdlib_registrars: &[StdlibRegistrar],
    ) -> Result<(VmContext, Arc<ModuleResolver>)> {
        let mut registry = ModuleRegistry::new();
        #[cfg_attr(test, allow(unused_unsafe))]
        unsafe {
            lk_stdlib_register_core_globals(&mut registry);
        }
        register_artifact_stdlib_method_modules(&mut registry)?;
        for register in stdlib_registrars {
            register(&mut registry)?;
        }

        let mut resolver = ModuleResolver::with_registry(registry);
        for path in search_paths {
            resolver.add_search_path(PathBuf::from(path));
        }
        for (name, path) in package_modules {
            resolver.register_package_module(name.clone(), PathBuf::from(path));
        }

        let resolver_arc = Arc::new(resolver);
        let ctx = VmContext::new().with_resolver(Arc::clone(&resolver_arc));
        Ok((ctx, resolver_arc))
    }

    fn build_native_context(
        search_paths: &[String],
        package_modules: &[(String, String)],
        stdlib_registrars: &[StdlibRegistrar],
    ) -> Result<(VmContext, Arc<ModuleResolver>)> {
        let mut registry = ModuleRegistry::new();
        #[cfg_attr(test, allow(unused_unsafe))]
        unsafe {
            lk_stdlib_register_core_globals(&mut registry);
        }
        register_artifact_stdlib_method_modules(&mut registry)?;
        for register in stdlib_registrars {
            register(&mut registry)?;
        }

        let mut resolver = ModuleResolver::with_registry(registry);
        for path in search_paths {
            resolver.add_search_path(PathBuf::from(path));
        }
        for (name, path) in package_modules {
            resolver.register_package_module(name.clone(), PathBuf::from(path));
        }

        let resolver_arc = Arc::new(resolver);
        let ctx = VmContext::new_without_core_vm_builtins().with_resolver(Arc::clone(&resolver_arc));
        Ok((ctx, resolver_arc))
    }
}

fn register_artifact_stdlib_method_modules(registry: &mut ModuleRegistry) -> Result<()> {
    for register in [
        register_stdlib_iter_bridge,
        register_stdlib_string_bridge,
        register_stdlib_list_bridge,
        register_stdlib_map_bridge,
        register_stdlib_stream_bridge,
    ] {
        register(registry)?;
    }
    Ok(())
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

fn read_string(ptr: *const i8, len: i64) -> String {
    if len <= 0 {
        return String::new();
    }
    if ptr.is_null() {
        return String::new();
    }
    let len_usize = len as usize;
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len_usize) };
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}
