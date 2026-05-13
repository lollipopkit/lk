//! Runtime helpers exposed to LLVM-generated code.
//!
//! The LLVM backend lowers high-level operations (string interning, list/map
//! construction, global access, etc.) into calls to `lk_rt_*` functions. This
//! module provides those helpers and bridges them back to the VM runtime so we
//! can reuse existing semantics while sharing a common value encoding.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Result, anyhow};

use crate::{
    llvm::encoding,
    module::ModuleRegistry,
    op::BinOp,
    stmt::{ImportSource, ImportStmt, ModuleResolver, deserialize_imports, execute_imports},
    util::fast_map::{FastHashMap, fast_hash_map_new, fast_hash_map_with_capacity},
    val::{AotFunction, Val},
    vm::{BytecodeModule, Vm, VmContext, decode_module},
};

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
pub extern "C" fn lk_rt_register_bundled_module(
    path_ptr: *const i8,
    path_len: i64,
    data_ptr: *const u8,
    data_len: i64,
) -> i32 {
    if data_ptr.is_null() || data_len <= 0 {
        return -1;
    }
    let len = data_len.max(0) as usize;
    if len == 0 {
        return -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data_ptr, len).to_vec() };
    let module = match decode_module(&bytes) {
        Ok(module) => module,
        Err(err) => {
            eprintln!("lk_rt_register_bundled_module: failed to decode module: {err}");
            return -1;
        }
    };
    let path = read_string(path_ptr, path_len);
    with_state(move |state| {
        if let Some(existing) = state.pending_bundled.iter_mut().find(|m| m.path == path) {
            existing.module = module.clone();
        } else {
            state.pending_bundled.push(DecodedBundledModule { path, module });
        }
        state.imports_applied = false;
    });
    0
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
pub extern "C" fn lk_rt_register_native_module_function(
    module_ptr: *const i8,
    module_len: i64,
    name_ptr: *const i8,
    name_len: i64,
    fn_ptr: *const (),
    arity: i64,
) -> i32 {
    if fn_ptr.is_null() || arity < 0 || arity > u8::MAX as i64 {
        return -1;
    }
    let module = read_string(module_ptr, module_len);
    let name = read_string(name_ptr, name_len);
    if module.is_empty() || name.is_empty() {
        return -1;
    }
    let function = Val::AotFunction(AotFunction {
        ptr: fn_ptr as usize,
        arity: arity as u8,
    });
    with_state(move |state| {
        state
            .pending_native_modules
            .entry(module)
            .or_insert_with(fast_hash_map_new)
            .insert(Arc::from(name), function);
        state.imports_applied = false;
    });
    0
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

struct DecodedBundledModule {
    path: String,
    module: BytecodeModule,
}

struct RuntimeState {
    ctx: VmContext,
    handles: HandleTable,
    interned_strings: FastHashMap<String, i64>,
    resolver: Arc<ModuleResolver>,
    pending_search_paths: Vec<String>,
    pending_imports: Vec<ImportStmt>,
    pending_bundled: Vec<DecodedBundledModule>,
    pending_package_modules: Vec<(String, String)>,
    pending_native_modules: FastHashMap<String, FastHashMap<Arc<str>, Val>>,
    pending_stdlib_registrars: Vec<StdlibRegistrar>,
    imports_applied: bool,
}

impl RuntimeState {
    fn new() -> Self {
        let (ctx, resolver) = match Self::build_native_context(&[], &[]) {
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
            handles: HandleTable::default(),
            interned_strings: fast_hash_map_new(),
            resolver,
            pending_search_paths: Vec::new(),
            pending_imports: Vec::new(),
            pending_bundled: Vec::new(),
            pending_package_modules: Vec::new(),
            pending_native_modules: fast_hash_map_new(),
            pending_stdlib_registrars: Vec::new(),
            imports_applied: false,
        }
    }

    fn reset_session(&mut self) {
        self.pending_search_paths.clear();
        self.pending_imports.clear();
        self.pending_bundled.clear();
        self.pending_package_modules.clear();
        self.pending_native_modules.clear();
        self.pending_stdlib_registrars.clear();
        self.imports_applied = false;
        self.handles = HandleTable::default();
        self.interned_strings.clear();
        match Self::build_native_context(&[], &[]) {
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
            &self.pending_bundled,
            &self.pending_package_modules,
            &self.pending_stdlib_registrars,
        )?;
        if !self.pending_imports.is_empty() {
            execute_imports(&self.pending_imports, resolver.as_ref(), &mut ctx)?;
        }
        self.ctx = ctx;
        self.resolver = resolver;
        self.handles = HandleTable::default();
        self.interned_strings.clear();
        self.imports_applied = true;
        Ok(())
    }

    fn apply_pending_native_only(&mut self) -> Result<()> {
        let (mut ctx, resolver) =
            Self::build_native_context(&self.pending_search_paths, &self.pending_stdlib_registrars)?;
        if !self.pending_imports.is_empty() {
            let native_modules = self.pending_native_modules.clone();
            Self::apply_native_imports(&self.pending_imports, &native_modules, resolver.as_ref(), &mut ctx)?;
        }
        self.ctx = ctx;
        self.resolver = resolver;
        self.handles = HandleTable::default();
        self.interned_strings.clear();
        self.imports_applied = true;
        Ok(())
    }

    fn apply_native_imports(
        imports: &[ImportStmt],
        native_modules: &FastHashMap<String, FastHashMap<Arc<str>, Val>>,
        resolver: &ModuleResolver,
        ctx: &mut VmContext,
    ) -> Result<()> {
        for import in imports {
            match import {
                ImportStmt::Module { module } => {
                    let value = Self::resolve_native_import_module(module, native_modules, resolver)?;
                    ctx.define(module.clone(), value);
                }
                ImportStmt::ModuleAlias { module, alias } => {
                    let value = Self::resolve_native_import_module(module, native_modules, resolver)?;
                    ctx.define(alias.clone(), value);
                }
                ImportStmt::Items { items, source } => {
                    let value = Self::resolve_native_import_source(source, native_modules, resolver)?;
                    let Val::Map(exports) = value else {
                        return Err(anyhow!("import source is not a module map"));
                    };
                    for item in items {
                        let export_value = exports
                            .get(item.name.as_str())
                            .ok_or_else(|| anyhow!("Export '{}' not found in module", item.name))?;
                        let symbol_name = item.alias.as_ref().unwrap_or(&item.name);
                        ctx.define(symbol_name.clone(), export_value.clone());
                    }
                }
                ImportStmt::Namespace { alias, source } => {
                    let value = Self::resolve_native_import_source(source, native_modules, resolver)?;
                    ctx.define(alias.clone(), value);
                }
                ImportStmt::File { path } => {
                    let module_name = std::path::Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("module")
                        .to_string();
                    let value = Self::resolve_native_import_module(&module_name, native_modules, resolver)?;
                    ctx.define(module_name, value);
                }
            }
        }
        Ok(())
    }

    fn resolve_native_import_source(
        source: &ImportSource,
        native_modules: &FastHashMap<String, FastHashMap<Arc<str>, Val>>,
        resolver: &ModuleResolver,
    ) -> Result<Val> {
        match source {
            ImportSource::Module(module) => Self::resolve_native_import_module(module, native_modules, resolver),
            ImportSource::File(path) => {
                let module_name = std::path::Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("module");
                Self::resolve_native_import_module(module_name, native_modules, resolver)
            }
        }
    }

    fn resolve_native_import_module(
        module: &str,
        native_modules: &FastHashMap<String, FastHashMap<Arc<str>, Val>>,
        resolver: &ModuleResolver,
    ) -> Result<Val> {
        if let Some(exports) = native_modules.get(module) {
            return Ok(Val::Map(Arc::new(exports.clone())));
        }
        resolver.resolve_registered_module(module)
    }

    fn build_context(
        search_paths: &[String],
        bundled: &[DecodedBundledModule],
        package_modules: &[(String, String)],
        stdlib_registrars: &[StdlibRegistrar],
    ) -> Result<(VmContext, Arc<ModuleResolver>)> {
        let mut registry = ModuleRegistry::new();
        #[cfg_attr(test, allow(unused_unsafe))]
        unsafe {
            lk_stdlib_register_core_globals(&mut registry);
        }
        for register in stdlib_registrars {
            register(&mut registry)?;
        }

        let mut resolver = ModuleResolver::with_registry(registry);
        for path in search_paths {
            resolver.add_search_path(PathBuf::from(path));
        }
        for module in bundled {
            Self::register_embedded_recursive(&resolver, &module.path, &module.module);
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
        stdlib_registrars: &[StdlibRegistrar],
    ) -> Result<(VmContext, Arc<ModuleResolver>)> {
        let mut registry = ModuleRegistry::new();
        #[cfg_attr(test, allow(unused_unsafe))]
        unsafe {
            lk_stdlib_register_core_globals(&mut registry);
        }
        for register in stdlib_registrars {
            register(&mut registry)?;
        }

        let mut resolver = ModuleResolver::with_registry(registry);
        for path in search_paths {
            resolver.add_search_path(PathBuf::from(path));
        }

        let resolver_arc = Arc::new(resolver);
        let ctx = VmContext::new_without_core_vm_builtins().with_resolver(Arc::clone(&resolver_arc));
        Ok((ctx, resolver_arc))
    }

    fn register_embedded_recursive(resolver: &ModuleResolver, path: &str, module: &BytecodeModule) {
        resolver.register_embedded_module(PathBuf::from(path), module.clone());
        for child in &module.bundled_modules {
            Self::register_embedded_recursive(resolver, &child.path, &child.module);
        }
    }

    fn encode_value(&mut self, value: Val) -> i64 {
        if let Ok(immediate) = encoding::encode_immediate(&value) {
            immediate
        } else {
            match value {
                Val::Str(s) => self.intern_string(s.as_ref()),
                other => self.handles.alloc(other),
            }
        }
    }

    fn decode_value(&self, raw: i64) -> Val {
        if let Some(val) = self.handles.get(raw) {
            val
        } else {
            encoding::decode_immediate(raw)
        }
    }

    fn decode_values(&self, ptr: *const i64, len: usize) -> Vec<Val> {
        if len == 0 || ptr.is_null() {
            return Vec::new();
        }
        let values = unsafe { std::slice::from_raw_parts(ptr, len) };
        values.iter().map(|&raw| self.decode_value(raw)).collect()
    }

    fn intern_string(&mut self, value: &str) -> i64 {
        if let Some(&handle) = self.interned_strings.get(value) {
            return handle;
        }
        let handle = self.handles.alloc(Val::Str(Arc::<str>::from(value)));
        self.interned_strings.insert(value.to_owned(), handle);
        handle
    }

    fn load_global(&mut self, name: &str) -> Val {
        self.ctx
            .get(name)
            .cloned()
            .or_else(|| self.ctx.resolver().get_builtin(name).cloned())
            .unwrap_or(Val::Nil)
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

struct HandleTable {
    next_handle: i64,
    values: FastHashMap<i64, Arc<Val>>,
}

impl HandleTable {
    fn alloc(&mut self, value: Val) -> i64 {
        let arc = Arc::new(value);
        loop {
            let handle = self.next_handle;
            self.next_handle = self.next_handle.wrapping_sub(1);
            if encoding::is_reserved_sentinel(handle) || self.values.contains_key(&handle) {
                continue;
            }
            self.values.insert(handle, arc.clone());
            return handle;
        }
    }

    fn get(&self, handle: i64) -> Option<Val> {
        self.values.get(&handle).map(|value| (**value).clone())
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self {
            next_handle: i64::MAX,
            values: fast_hash_map_new(),
        }
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

fn stdlib_module_names_from_imports(imports: &[ImportStmt]) -> Vec<String> {
    let mut names = Vec::new();
    for import in imports {
        match import {
            ImportStmt::Module { module } | ImportStmt::ModuleAlias { module, .. } => {
                push_unique(&mut names, module);
            }
            ImportStmt::Items {
                source: ImportSource::Module(module),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::Module(module),
                ..
            } => {
                push_unique(&mut names, module);
            }
            ImportStmt::File { .. }
            | ImportStmt::Items {
                source: ImportSource::File(_),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::File(_),
                ..
            } => {}
        }
    }
    names
}

#[cfg(test)]
fn imports_need_concurrency_globals(imports: &[ImportStmt]) -> bool {
    stdlib_module_names_from_imports(imports)
        .iter()
        .any(|name| matches!(name.as_str(), "task" | "chan" | "time"))
}

fn stdlib_registrars_from_imports(imports: &[ImportStmt]) -> Vec<StdlibRegistrar> {
    let mut registrars = Vec::new();
    for name in stdlib_module_names_from_imports(imports) {
        match name.as_str() {
            "io" => push_unique_registrar(&mut registrars, register_stdlib_io_bridge),
            "json" => push_unique_registrar(&mut registrars, register_stdlib_json_bridge),
            "yaml" => push_unique_registrar(&mut registrars, register_stdlib_yaml_bridge),
            "toml" => push_unique_registrar(&mut registrars, register_stdlib_toml_bridge),
            "iter" => push_unique_registrar(&mut registrars, register_stdlib_iter_bridge),
            "math" => push_unique_registrar(&mut registrars, register_stdlib_math_bridge),
            "string" => push_unique_registrar(&mut registrars, register_stdlib_string_bridge),
            "list" => push_unique_registrar(&mut registrars, register_stdlib_list_bridge),
            "map" => push_unique_registrar(&mut registrars, register_stdlib_map_bridge),
            "datetime" => push_unique_registrar(&mut registrars, register_stdlib_datetime_bridge),
            "os" => push_unique_registrar(&mut registrars, register_stdlib_os_bridge),
            "tcp" => push_unique_registrar(&mut registrars, register_stdlib_tcp_bridge),
            "stream" => push_unique_registrar(&mut registrars, register_stdlib_stream_bridge),
            "task" => {
                push_unique_registrar(&mut registrars, register_stdlib_concurrency_globals_bridge);
                push_unique_registrar(&mut registrars, register_stdlib_task_bridge);
            }
            "chan" => {
                push_unique_registrar(&mut registrars, register_stdlib_concurrency_globals_bridge);
                push_unique_registrar(&mut registrars, register_stdlib_chan_bridge);
            }
            "time" => {
                push_unique_registrar(&mut registrars, register_stdlib_concurrency_globals_bridge);
                push_unique_registrar(&mut registrars, register_stdlib_time_bridge);
            }
            _ => {}
        }
    }
    registrars
}

fn push_unique(names: &mut Vec<String>, candidate: &str) {
    if !names.iter().any(|name| name == candidate) {
        names.push(candidate.to_string());
    }
}

fn bool_to_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn encode_map_key(value: &Val) -> Result<Arc<str>> {
    match value {
        Val::Str(s) => Ok(s.clone()),
        Val::Int(i) => Ok(Arc::from(i.to_string())),
        Val::Float(f) => Ok(Arc::from(f.to_string())),
        Val::Bool(b) => Ok(Arc::from(bool_to_str(*b))),
        other => Err(anyhow!("map key must be primitive, got {}", other.type_name())),
    }
}

fn map_to_iterable(map: &FastHashMap<Arc<str>, Val>) -> Val {
    let mut keys: Vec<&str> = map.keys().map(|k| k.as_ref()).collect();
    keys.sort();
    let mut pairs = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(value) = map.get(key) {
            let pair = Val::List(vec![Val::Str(key.to_string().into()), value.clone()].into());
            pairs.push(pair);
        }
    }
    Val::List(Arc::new(pairs))
}

fn list_slice(list: &Arc<Vec<Val>>, start: i64) -> Val {
    if start <= 0 {
        return Val::List(list.clone());
    }
    let idx = start as usize;
    if idx >= list.len() {
        Val::List(Arc::new(Vec::new()))
    } else {
        Val::List(list[idx..].to_vec().into())
    }
}

fn index_value(base: &Val, idx: &Val) -> Val {
    match (base, idx) {
        (Val::List(list), Val::Int(i)) => {
            if *i < 0 {
                Val::Nil
            } else {
                list.get(*i as usize).cloned().unwrap_or(Val::Nil)
            }
        }
        (Val::Str(s), Val::Int(i)) => {
            if *i < 0 {
                Val::Nil
            } else if s.is_ascii() {
                let i = *i as usize;
                let bytes = s.as_bytes();
                if i < bytes.len() {
                    let ch = bytes[i] as char;
                    Val::Str(ch.to_string().into())
                } else {
                    Val::Nil
                }
            } else {
                s.chars()
                    .nth(*i as usize)
                    .map(|ch| Val::Str(ch.to_string().into()))
                    .unwrap_or(Val::Nil)
            }
        }
        _ => Val::Nil,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_intern_string(ptr: *const i8, len: i64) -> i64 {
    let text = read_string(ptr, len);
    with_state(|state| state.intern_string(text.as_str()))
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_to_string(value: i64) -> i64 {
    with_state(|state| {
        let rendered = state.decode_value(value).to_string();
        state.intern_string(rendered.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_load_global(name: i64) -> i64 {
    with_state(|state| {
        let key_val = state.decode_value(name);
        let name_str = key_val
            .as_str()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| key_val.to_string());
        let value = state.load_global(name_str.as_str());
        state.encode_value(value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_define_global(name: i64, value: i64) {
    with_state(|state| {
        let key_val = state.decode_value(name);
        let name_str = key_val
            .as_str()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| key_val.to_string());
        let val = state.decode_value(value);
        state.ctx.set(name_str, val);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_build_list(ptr: *const i64, len: i64) -> i64 {
    let len_usize = len.max(0) as usize;
    with_state(|state| {
        let elements = state.decode_values(ptr, len_usize);
        let list = Val::List(Arc::new(elements));
        state.encode_value(list)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_build_map(ptr: *const i64, len: i64) -> i64 {
    let len_usize = len.max(0) as usize;
    with_state(|state| {
        if len_usize == 0 {
            return state.encode_value(Val::Map(Arc::new(fast_hash_map_new())));
        }
        if ptr.is_null() {
            return encoding::NIL_VALUE;
        }
        let raw = unsafe { std::slice::from_raw_parts(ptr, len_usize * 2) };
        let mut map = fast_hash_map_with_capacity(len_usize);
        for i in 0..len_usize {
            let key = state.decode_value(raw[2 * i]);
            let val = state.decode_value(raw[2 * i + 1]);
            match encode_map_key(&key) {
                Ok(k) => {
                    map.insert(k, val);
                }
                Err(err) => {
                    eprintln!("lk_rt_build_map: {err}");
                    return encoding::NIL_VALUE;
                }
            }
        }
        state.encode_value(Val::Map(Arc::new(map)))
    })
}

#[cold]
#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_run_bytecode(data_ptr: *const u8, data_len: i64) -> i32 {
    if data_ptr.is_null() || data_len <= 0 {
        eprintln!("lk_rt_run_bytecode: empty bytecode payload");
        return 1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data_ptr, data_len as usize) };
    let result: Result<Val> = (|| {
        let module = decode_module(bytes)?;
        let imports = module
            .meta
            .as_ref()
            .and_then(|meta| meta.tags.get("imports"))
            .map(|raw| deserialize_imports(raw))
            .transpose()?;
        let bundled: Vec<DecodedBundledModule> = module
            .bundled_modules
            .iter()
            .map(|child| DecodedBundledModule {
                path: child.path.clone(),
                module: child.module.clone(),
            })
            .collect();
        if let Some(imports) = imports.as_ref() {
            let stdlib_registrars = stdlib_registrars_from_imports(imports);
            let (mut ctx, resolver) = RuntimeState::build_context(&[], &bundled, &[], &stdlib_registrars)?;
            execute_imports(imports, resolver.as_ref(), &mut ctx)?;
            let mut vm = Vm::new();
            return vm.exec(&module.entry, &mut ctx);
        }
        let (mut ctx, _resolver) = RuntimeState::build_context(&[], &bundled, &[], &[])?;
        let mut vm = Vm::new();
        vm.exec(&module.entry, &mut ctx)
    })();

    match result {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("lk_rt_run_bytecode error: {err}");
            1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_call(func: i64, args_ptr: *const i64, argc: i64, retc: i64) -> i64 {
    let argc_usize = argc.max(0) as usize;
    let aot_function = with_state(|state| match state.decode_value(func) {
        Val::AotFunction(function) => Some(function),
        _ => None,
    });
    if let Some(function) = aot_function {
        return match call_aot_function_raw(function, args_ptr, argc_usize) {
            Ok(raw) => {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    raw
                }
            }
            Err(err) => {
                eprintln!("lk_rt_call error: {err}");
                encoding::NIL_VALUE
            }
        };
    }
    with_state(|state| {
        let callee = state.decode_value(func);
        let args = state.decode_values(args_ptr, argc_usize);
        let result: Result<i64> = callee.call(&args, &mut state.ctx).map(|val| {
            if retc <= 0 {
                encoding::NIL_VALUE
            } else {
                state.encode_value(val)
            }
        });
        match result {
            Ok(val) => {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    val
                }
            }
            Err(err) => {
                eprintln!("lk_rt_call error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_call_method(receiver: i64, method: i64, args_ptr: *const i64, argc: i64, retc: i64) -> i64 {
    let argc_usize = argc.max(0) as usize;
    let aot_function = with_state(|state| {
        let receiver_val = state.decode_value(receiver);
        let method_val = state.decode_value(method);
        let method_name = method_val
            .as_str()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| method_val.to_string());
        match receiver_val
            .access(&Val::Str(Arc::from(method_name.as_str())))
            .unwrap_or(Val::Nil)
        {
            Val::AotFunction(function) => Some(function),
            _ => None,
        }
    });
    if let Some(function) = aot_function {
        return match call_aot_function_raw(function, args_ptr, argc_usize) {
            Ok(raw) => {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    raw
                }
            }
            Err(err) => {
                eprintln!("lk_rt_call_method error: {err}");
                encoding::NIL_VALUE
            }
        };
    }
    with_state(|state| {
        let receiver_val = state.decode_value(receiver);
        let method_val = state.decode_value(method);
        let method_name = method_val
            .as_str()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| method_val.to_string());
        let callee = receiver_val
            .access(&Val::Str(Arc::from(method_name.as_str())))
            .unwrap_or(Val::Nil);
        let args = state.decode_values(args_ptr, argc_usize);
        let result: Result<i64> = callee.call(&args, &mut state.ctx).map(|val| {
            if retc <= 0 {
                encoding::NIL_VALUE
            } else {
                state.encode_value(val)
            }
        });
        match result {
            Ok(val) => {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    val
                }
            }
            Err(err) => {
                eprintln!("lk_rt_call_method error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_call_native(func: i64, args_ptr: *const i64, argc: i64, retc: i64) -> i64 {
    let argc_usize = argc.max(0) as usize;
    let aot_function = with_state(|state| match state.decode_value(func) {
        Val::AotFunction(function) => Some(function),
        _ => None,
    });
    if let Some(function) = aot_function {
        return match call_aot_function_raw(function, args_ptr, argc_usize) {
            Ok(raw) => {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    raw
                }
            }
            Err(err) => {
                eprintln!("lk_rt_call_native error: {err}");
                encoding::NIL_VALUE
            }
        };
    }

    with_state(|state| {
        let callee = state.decode_value(func);
        let args = state.decode_values(args_ptr, argc_usize);
        let result = match callee {
            Val::RustFunction(function) => function(&args, &mut state.ctx),
            Val::RustFunctionNamed(function) => function(&args, &[], &mut state.ctx),
            other => Err(anyhow!("{} is not a native function", other.type_name())),
        };
        match result {
            Ok(val) => {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    state.encode_value(val)
                }
            }
            Err(err) => {
                eprintln!("lk_rt_call_native error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

fn call_aot_function_raw(function: AotFunction, args_ptr: *const i64, argc: usize) -> Result<i64> {
    if argc != function.arity as usize {
        return Err(anyhow!(
            "AOT function expects {} arguments, got {}",
            function.arity,
            argc
        ));
    }
    if argc > 0 && args_ptr.is_null() {
        return Err(anyhow!("AOT function arguments pointer is null"));
    }
    let raw = unsafe {
        match argc {
            0 => {
                let f: extern "C" fn() -> i64 = std::mem::transmute(function.ptr);
                f()
            }
            1 => {
                let args = std::slice::from_raw_parts(args_ptr, 1);
                let f: extern "C" fn(i64) -> i64 = std::mem::transmute(function.ptr);
                f(args[0])
            }
            2 => {
                let args = std::slice::from_raw_parts(args_ptr, 2);
                let f: extern "C" fn(i64, i64) -> i64 = std::mem::transmute(function.ptr);
                f(args[0], args[1])
            }
            3 => {
                let args = std::slice::from_raw_parts(args_ptr, 3);
                let f: extern "C" fn(i64, i64, i64) -> i64 = std::mem::transmute(function.ptr);
                f(args[0], args[1], args[2])
            }
            4 => {
                let args = std::slice::from_raw_parts(args_ptr, 4);
                let f: extern "C" fn(i64, i64, i64, i64) -> i64 = std::mem::transmute(function.ptr);
                f(args[0], args[1], args[2], args[3])
            }
            _ => {
                return Err(anyhow!(
                    "AOT function arity {} exceeds runtime call bridge limit",
                    function.arity
                ));
            }
        }
    };
    Ok(raw)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_add(lhs: i64, rhs: i64) -> i64 {
    with_state(|state| {
        let left = state.decode_value(lhs);
        let right = state.decode_value(rhs);
        match BinOp::Add.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("lk_rt_add error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_access(base: i64, key: i64) -> i64 {
    with_state(|state| {
        let base_val = state.decode_value(base);
        let key_val = state.decode_value(key);
        let result = base_val.access(&key_val).unwrap_or(Val::Nil);
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_index(base: i64, idx: i64) -> i64 {
    with_state(|state| {
        let base_val = state.decode_value(base);
        let idx_val = state.decode_value(idx);
        let result = index_value(&base_val, &idx_val);
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_in(needle: i64, haystack: i64) -> i64 {
    with_state(|state| {
        let l = state.decode_value(needle);
        let r = state.decode_value(haystack);
        match BinOp::In.cmp(&l, &r) {
            Ok(result) => state.encode_value(Val::Bool(result)),
            Err(err) => {
                eprintln!("lk_rt_in error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_len(value: i64) -> i64 {
    with_state(|state| {
        let val = state.decode_value(value);
        let len = match val {
            Val::List(ref l) => l.len() as i64,
            Val::Str(ref s) => s.len() as i64,
            Val::Map(ref m) => m.len() as i64,
            _ => 0,
        };
        state.encode_value(Val::Int(len))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_list_slice(list: i64, start: i64) -> i64 {
    with_state(|state| {
        let list_val = state.decode_value(list);
        let start_val = state.decode_value(start);
        let result = match (list_val, start_val) {
            (Val::List(l), Val::Int(i)) => list_slice(&l, i),
            _ => Val::Nil,
        };
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_to_iter(value: i64) -> i64 {
    with_state(|state| {
        let val = state.decode_value(value);
        let iter = match val {
            Val::List(_) | Val::Str(_) => val,
            Val::Map(ref map) => map_to_iterable(map),
            other => match other {
                Val::Nil => Val::List(Arc::new(Vec::new())),
                Val::Bool(_) | Val::Int(_) | Val::Float(_) | Val::Object(_) | Val::Task(_) | Val::Channel(_) => {
                    Val::List(Arc::new(Vec::new()))
                }
                _ => Val::List(Arc::new(Vec::new())),
            },
        };
        state.encode_value(iter)
    })
}

trait ValStrExt {
    fn as_str(&self) -> Option<&str>;
}

impl ValStrExt for Val {
    fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s.as_ref()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::val::Val;
    use crate::{
        stmt::{Program, stmt_parser::StmtParser},
        token::Tokenizer,
        vm::{ModuleFlags, compile_program, encode_module},
    };
    use once_cell::sync::Lazy;
    use std::{
        ffi::CString,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    static RUNTIME_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn reset_runtime_state() {
        if let Some(mutex) = RUNTIME_STATE.get() {
            let mut guard = mutex.lock().unwrap();
            *guard = RuntimeState::default();
        }
    }

    fn decode_for_tests(value: i64) -> Val {
        with_state(|state| state.decode_value(value))
    }

    #[test]
    fn intern_string_roundtrips() {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        reset_runtime_state();
        let text = b"hello";
        let handle = lk_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
        let handle_again = lk_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
        assert_eq!(handle, handle_again);
        assert!(matches!(decode_for_tests(handle), Val::Str(s) if s.as_ref() == "hello"));
    }

    #[test]
    fn build_list_and_len() {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        reset_runtime_state();
        let values = [encoding::BOOL_TRUE_VALUE, 42, encoding::NIL_VALUE];
        let list_handle = lk_rt_build_list(values.as_ptr(), values.len() as i64);
        let len = lk_rt_len(list_handle);
        assert_eq!(len, 3);
        match decode_for_tests(list_handle) {
            Val::List(list) => {
                assert_eq!(list.len(), 3);
                assert!(matches!(list[0], Val::Bool(true)));
                assert!(matches!(list[1], Val::Int(42)));
                assert!(matches!(list[2], Val::Nil));
            }
            other => panic!("unexpected value: {other:?}"),
        }
    }

    #[test]
    fn define_and_load_global() {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        reset_runtime_state();
        let name_bytes = b"g";
        let name_handle = lk_rt_intern_string(name_bytes.as_ptr().cast(), 1);
        lk_rt_define_global(name_handle, encoding::BOOL_TRUE_VALUE);
        let loaded = lk_rt_load_global(name_handle);
        assert_eq!(loaded, encoding::BOOL_TRUE_VALUE);
    }

    fn add_one(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let value = args.first().cloned().unwrap_or(Val::Int(0));
        match value {
            Val::Int(i) => Ok(Val::Int(i + 1)),
            _ => Ok(Val::Nil),
        }
    }

    #[test]
    fn call_rust_function() {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        reset_runtime_state();
        let func_handle = with_state(|state| state.encode_value(Val::RustFunction(add_one)));
        let arg = 41i64;
        let result = lk_rt_call(func_handle, &arg, 1, 1);
        assert_eq!(result, 42);
    }

    #[test]
    fn stdlib_module_names_are_collected_from_module_imports() {
        let imports = vec![
            ImportStmt::Module {
                module: "math".to_string(),
            },
            ImportStmt::Items {
                items: Vec::new(),
                source: ImportSource::Module("json".to_string()),
            },
            ImportStmt::Namespace {
                alias: "m".to_string(),
                source: ImportSource::File("local.lk".to_string()),
            },
            ImportStmt::ModuleAlias {
                module: "math".to_string(),
                alias: "m".to_string(),
            },
        ];

        assert_eq!(
            stdlib_module_names_from_imports(&imports),
            vec!["math".to_string(), "json".to_string()]
        );
    }

    #[test]
    fn concurrency_globals_are_registered_only_for_concurrency_imports() {
        assert!(!imports_need_concurrency_globals(&[ImportStmt::Module {
            module: "math".to_string(),
        }]));
        assert!(imports_need_concurrency_globals(&[ImportStmt::Module {
            module: "chan".to_string(),
        }]));
    }

    fn compile_module_from_path(path: &Path) -> BytecodeModule {
        let src = std::fs::read_to_string(path).expect("module source readable");
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(&src).expect("tokenize module");
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program: Program = parser.parse_program_with_enhanced_errors(&src).expect("parse module");
        let func = compile_program(&program);
        let mut module = BytecodeModule::new(func);
        module.flags.insert(ModuleFlags::CONST_FOLDED);
        module
    }

    #[test]
    fn apply_imports_registers_bundled_module() {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        reset_runtime_state();
        lk_rt_begin_session();

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf();
        let examples_dir = workspace.join("examples");
        let search_path = CString::new("examples").unwrap();
        lk_rt_register_search_path(search_path.as_ptr(), search_path.as_bytes().len() as i64);

        let fib_path = examples_dir.join("fib.lk");
        let fib_module = compile_module_from_path(&fib_path);
        let fib_bytes = encode_module(&fib_module).expect("encode module");
        let fib_path_rel = CString::new("examples/fib.lk").unwrap();
        let _ = lk_rt_register_bundled_module(
            fib_path_rel.as_ptr(),
            fib_path_rel.as_bytes().len() as i64,
            fib_bytes.as_ptr(),
            fib_bytes.len() as i64,
        );

        let imports_json = CString::new("[{\"File\":{\"path\":\"examples/fib\"}}]").unwrap();
        let _ = lk_rt_register_imports(imports_json.as_ptr(), imports_json.as_bytes().len() as i64);
        let apply = lk_rt_apply_imports();
        assert_eq!(apply, 0, "apply imports succeeds");

        let fib_name = CString::new("fib").unwrap();
        let fib_handle = lk_rt_intern_string(fib_name.as_ptr(), fib_name.as_bytes().len() as i64);
        let fib_value = lk_rt_load_global(fib_handle);
        assert_ne!(fib_value, encoding::NIL_VALUE, "fib module is loaded");
    }
}
