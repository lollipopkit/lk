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
    util::fast_map::{FastHashMap, fast_hash_map_new},
    val::{HeapStore, HeapValue, RuntimeVal, ShortStr},
    vm::VmContext,
};

#[cfg(test)]
mod imports;
mod math;
#[cfg(test)]
mod tests;

#[cfg(test)]
use imports::{imports_need_concurrency_globals, stdlib_module_names_from_imports};
pub use math::*;

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
    _data_ptr: *const u8,
    _data_len: i64,
) -> i32 {
    let path = read_string(path_ptr, path_len);
    eprintln!(
        "lk_rt_register_bundled_module: bundled LKB imports are disabled during the Instr32 VM migration: {path}"
    );
    -1
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
    let module = read_string(module_ptr, module_len);
    let name = read_string(name_ptr, name_len);
    let _ = (fn_ptr, arity);
    eprintln!(
        "lk_rt_register_native_module_function: AOT native module replay is disabled during the Instr32 VM migration: {module}.{name}"
    );
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_make_aot_function(fn_ptr: *const (), arity: i64) -> i64 {
    let _ = (fn_ptr, arity);
    eprintln!("lk_rt_make_aot_function: AOT function handles are disabled during the Instr32 VM migration");
    encoding::NIL_VALUE
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
    aot_globals: FastHashMap<String, RuntimeVal>,
    handles: HandleTable,
    heap: HeapStore,
    interned_strings: FastHashMap<String, i64>,
    resolver: Arc<ModuleResolver>,
    pending_search_paths: Vec<String>,
    pending_imports: Vec<ImportStmt>,
    pending_package_modules: Vec<(String, String)>,
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
            aot_globals: fast_hash_map_new(),
            handles: HandleTable::default(),
            heap: HeapStore::new(),
            interned_strings: fast_hash_map_new(),
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
        self.aot_globals.clear();
        self.handles = HandleTable::default();
        self.heap = HeapStore::new();
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
            &self.pending_package_modules,
            &self.pending_stdlib_registrars,
        )?;
        if !self.pending_imports.is_empty() {
            execute_imports(&self.pending_imports, resolver.as_ref(), &mut ctx)?;
        }
        self.ctx = ctx;
        self.resolver = resolver;
        self.handles = HandleTable::default();
        self.heap = HeapStore::new();
        self.interned_strings.clear();
        self.imports_applied = true;
        Ok(())
    }

    fn apply_pending_native_only(&mut self) -> Result<()> {
        let (ctx, resolver) = Self::build_native_context(&self.pending_search_paths, &self.pending_stdlib_registrars)?;
        if !self.pending_imports.is_empty() {
            let imports = self.pending_imports.clone();
            Self::apply_native_imports(&imports, resolver.as_ref(), self)?;
        }
        self.ctx = ctx;
        self.resolver = resolver;
        self.handles = HandleTable::default();
        self.heap = HeapStore::new();
        self.interned_strings.clear();
        self.imports_applied = true;
        Ok(())
    }

    fn apply_native_imports(imports: &[ImportStmt], resolver: &ModuleResolver, state: &mut RuntimeState) -> Result<()> {
        for import in imports {
            match import {
                ImportStmt::Module { module } => {
                    let value = Self::resolve_native_import_module(module, resolver)?;
                    state.aot_globals.insert(module.clone(), value);
                }
                ImportStmt::ModuleAlias { module, alias } => {
                    let value = Self::resolve_native_import_module(module, resolver)?;
                    state.aot_globals.insert(alias.clone(), value);
                }
                ImportStmt::Items { items, source } => {
                    let _ = (source, items, resolver);
                    return Err(anyhow!(
                        "AOT native item imports are disabled during the Instr32 VM migration"
                    ));
                }
                ImportStmt::Namespace { alias, source } => {
                    let value = Self::resolve_native_import_source(source, resolver)?;
                    state.aot_globals.insert(alias.clone(), value);
                }
                ImportStmt::File { path } => {
                    let module_name = std::path::Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("module")
                        .to_string();
                    let value = Self::resolve_native_import_module(&module_name, resolver)?;
                    state.aot_globals.insert(module_name, value);
                }
            }
        }
        Ok(())
    }

    fn resolve_native_import_source(source: &ImportSource, resolver: &ModuleResolver) -> Result<RuntimeVal> {
        match source {
            ImportSource::Module(module) => Self::resolve_native_import_module(module, resolver),
            ImportSource::File(path) => {
                let module_name = std::path::Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("module");
                Self::resolve_native_import_module(module_name, resolver)
            }
        }
    }

    fn resolve_native_import_module(module: &str, _resolver: &ModuleResolver) -> Result<RuntimeVal> {
        Err(anyhow!(
            "AOT native import replay is disabled during the Instr32 VM migration: module '{}'",
            module
        ))
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
        register_aot_stdlib_method_modules(&mut registry)?;
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
        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver_arc));
        install_aot_core_vm_builtins(&mut ctx);
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
        register_aot_stdlib_method_modules(&mut registry)?;
        for register in stdlib_registrars {
            register(&mut registry)?;
        }

        let mut resolver = ModuleResolver::with_registry(registry);
        for path in search_paths {
            resolver.add_search_path(PathBuf::from(path));
        }

        let resolver_arc = Arc::new(resolver);
        let mut ctx = VmContext::new_without_core_vm_builtins().with_resolver(Arc::clone(&resolver_arc));
        install_aot_core_vm_builtins(&mut ctx);
        Ok((ctx, resolver_arc))
    }

    fn encode_value(&mut self, value: RuntimeVal) -> i64 {
        if let Ok(immediate) = encoding::encode_immediate(&value) {
            immediate
        } else {
            if let Some(text) = self.runtime_string(&value) {
                let owned = text.to_owned();
                self.intern_string(owned.as_str())
            } else {
                self.handles.alloc(value)
            }
        }
    }

    fn decode_value(&self, raw: i64) -> RuntimeVal {
        if let Some(val) = self.handles.get(raw) {
            val
        } else {
            encoding::decode_immediate(raw)
        }
    }

    fn intern_string(&mut self, value: &str) -> i64 {
        if let Some(&handle) = self.interned_strings.get(value) {
            return handle;
        }
        let string_value = self.runtime_string_value(value.to_owned());
        let handle = self.handles.alloc(string_value);
        self.interned_strings.insert(value.to_owned(), handle);
        handle
    }

    fn load_global(&mut self, name: &str) -> RuntimeVal {
        self.aot_globals.get(name).cloned().unwrap_or(RuntimeVal::Nil)
    }

    fn runtime_string_value(&mut self, value: String) -> RuntimeVal {
        if let Some(short) = ShortStr::new(value.as_str()) {
            RuntimeVal::ShortStr(short)
        } else {
            RuntimeVal::Obj(self.heap.alloc(HeapValue::String(Arc::<str>::from(value))))
        }
    }

    fn runtime_string<'a>(&'a self, value: &'a RuntimeVal) -> Option<&'a str> {
        match value {
            RuntimeVal::ShortStr(value) => Some(value.as_str()),
            RuntimeVal::Obj(handle) => match self.heap.get(*handle) {
                Some(HeapValue::String(value)) => Some(value.as_ref()),
                _ => None,
            },
            _ => None,
        }
    }

    fn runtime_value_to_string(&self, value: &RuntimeVal) -> String {
        match value {
            RuntimeVal::Nil => "nil".to_string(),
            RuntimeVal::Bool(value) => value.to_string(),
            RuntimeVal::Int(value) => value.to_string(),
            RuntimeVal::Float(value) => value.to_string(),
            RuntimeVal::ShortStr(value) => value.as_str().to_string(),
            RuntimeVal::Obj(handle) => match self.heap.get(*handle) {
                Some(HeapValue::String(value)) => value.to_string(),
                Some(value) => format!("<{}>", value.type_name()),
                None => "<dangling>".to_string(),
            },
        }
    }
}

fn install_aot_core_vm_builtins(_ctx: &mut VmContext) {}

fn register_aot_stdlib_method_modules(registry: &mut ModuleRegistry) -> Result<()> {
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

#[derive(Default)]
struct HandleTable {
    values: Vec<RuntimeVal>,
}

impl HandleTable {
    fn alloc(&mut self, value: RuntimeVal) -> i64 {
        let index = self.values.len();
        self.values.push(value);
        i64::MAX - index as i64
    }

    fn get(&self, handle: i64) -> Option<RuntimeVal> {
        let index = usize::try_from(i64::MAX.checked_sub(handle)?).ok()?;
        self.values.get(index).cloned()
    }

    fn get_ref(&self, handle: i64) -> Option<&RuntimeVal> {
        let index = usize::try_from(i64::MAX.checked_sub(handle)?).ok()?;
        self.values.get(index)
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

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_intern_string(ptr: *const i8, len: i64) -> i64 {
    let text = read_string(ptr, len);
    with_state(|state| state.intern_string(text.as_str()))
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_to_string(value: i64) -> i64 {
    with_state(|state| {
        let decoded = state.decode_value(value);
        let rendered = state.runtime_value_to_string(&decoded);
        state.intern_string(rendered.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_load_global(name: i64) -> i64 {
    with_state(|state| {
        let key_val = state.decode_value(name);
        let name_str = state
            .runtime_string(&key_val)
            .map(|s| s.to_owned())
            .unwrap_or_else(|| state.runtime_value_to_string(&key_val));
        let value = state.load_global(name_str.as_str());
        state.encode_value(value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_define_global(name: i64, value: i64) {
    with_state(|state| {
        let key_val = state.decode_value(name);
        let name_str = state
            .runtime_string(&key_val)
            .map(|s| s.to_owned())
            .unwrap_or_else(|| state.runtime_value_to_string(&key_val));
        let val = state.decode_value(value);
        state.aot_globals.insert(name_str, val);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_call(func: i64, args_ptr: *const i64, argc: i64, retc: i64) -> i64 {
    let _ = (func, args_ptr, argc, retc);
    eprintln!("lk_rt_call: AOT runtime call bridge is disabled during the Instr32 VM migration");
    encoding::NIL_VALUE
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_call_method(receiver: i64, method: i64, args_ptr: *const i64, argc: i64, retc: i64) -> i64 {
    let _ = (receiver, method, args_ptr, argc, retc);
    eprintln!("lk_rt_call_method: AOT method calls are disabled during the Instr32 VM migration");
    encoding::NIL_VALUE
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_call_native(func: i64, args_ptr: *const i64, argc: i64, retc: i64) -> i64 {
    let _ = (func, args_ptr, argc, retc);
    eprintln!("lk_rt_call_native: AOT native call bridge is disabled during the Instr32 VM migration");
    encoding::NIL_VALUE
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_float(value: f64) -> i64 {
    with_state(|state| state.encode_value(RuntimeVal::Float(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_floor(value: i64) -> i64 {
    with_state(|state| {
        let out = match state.decode_value(value) {
            RuntimeVal::Float(f) => f.floor() as i64,
            RuntimeVal::Int(i) => i,
            _ => 0,
        };
        state.encode_value(RuntimeVal::Int(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_floor_div_imm(value: i64, divisor: i64) -> i64 {
    with_state(|state| {
        if divisor == 0 {
            return encoding::NIL_VALUE;
        }
        let out = match state.decode_value(value) {
            RuntimeVal::Int(lhs) => {
                let q = lhs / divisor;
                let r = lhs % divisor;
                if r != 0 && ((r < 0) != (divisor < 0)) { q - 1 } else { q }
            }
            RuntimeVal::Float(lhs) => (lhs / divisor as f64).floor() as i64,
            _ => 0,
        };
        state.encode_value(RuntimeVal::Int(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_starts_with(value: i64, prefix: i64) -> i64 {
    with_state(|state| {
        let value = state.decode_value(value);
        let prefix = state.decode_value(prefix);
        let out = match (state.runtime_string(&value), state.runtime_string(&prefix)) {
            (Some(value), Some(prefix)) => value.starts_with(prefix),
            _ => false,
        };
        state.encode_value(RuntimeVal::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_contains(value: i64, needle: i64) -> i64 {
    with_state(|state| {
        let value = state.decode_value(value);
        let needle = state.decode_value(needle);
        let out = match (state.runtime_string(&value), state.runtime_string(&needle)) {
            (Some(value), Some(needle)) => value.contains(needle),
            _ => false,
        };
        state.encode_value(RuntimeVal::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_cmp(lhs: i64, rhs: i64, code: i64) -> i64 {
    with_state(|state| {
        let left = state.decode_value(lhs);
        let right = state.decode_value(rhs);
        let op = match code {
            0 => BinOp::Eq,
            1 => BinOp::Ne,
            2 => BinOp::Lt,
            3 => BinOp::Le,
            4 => BinOp::Gt,
            5 => BinOp::Ge,
            _ => {
                eprintln!("lk_rt_cmp error: unknown compare code {code}");
                return encoding::NIL_VALUE;
            }
        };
        match runtime_cmp(&left, &right, &op, state) {
            Ok(value) => state.encode_value(RuntimeVal::Bool(value)),
            Err(err) => {
                eprintln!("lk_rt_cmp error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

fn runtime_cmp(left: &RuntimeVal, right: &RuntimeVal, op: &BinOp, state: &RuntimeState) -> Result<bool> {
    let ordering = match (left, right) {
        (RuntimeVal::Int(a), RuntimeVal::Int(b)) => a.partial_cmp(b),
        (RuntimeVal::Float(a), RuntimeVal::Float(b)) => a.partial_cmp(b),
        (RuntimeVal::Int(a), RuntimeVal::Float(b)) => (*a as f64).partial_cmp(b),
        (RuntimeVal::Float(a), RuntimeVal::Int(b)) => a.partial_cmp(&(*b as f64)),
        _ => match (state.runtime_string(left), state.runtime_string(right)) {
            (Some(a), Some(b)) => a.partial_cmp(b),
            _ => None,
        },
    };
    match op {
        BinOp::Eq => Ok(runtime_eq(left, right, state)),
        BinOp::Ne => Ok(!runtime_eq(left, right, state)),
        BinOp::Lt => Ok(ordering == Some(std::cmp::Ordering::Less)),
        BinOp::Le => Ok(matches!(
            ordering,
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        )),
        BinOp::Gt => Ok(ordering == Some(std::cmp::Ordering::Greater)),
        BinOp::Ge => Ok(matches!(
            ordering,
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        )),
        _ => Err(anyhow!("unsupported compare op {:?}", op)),
    }
}

fn runtime_eq(left: &RuntimeVal, right: &RuntimeVal, state: &RuntimeState) -> bool {
    match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => true,
        (RuntimeVal::Bool(a), RuntimeVal::Bool(b)) => a == b,
        (RuntimeVal::Int(a), RuntimeVal::Int(b)) => a == b,
        (RuntimeVal::Float(a), RuntimeVal::Float(b)) => a == b,
        (RuntimeVal::Int(a), RuntimeVal::Float(b)) => *a as f64 == *b,
        (RuntimeVal::Float(a), RuntimeVal::Int(b)) => *a == *b as f64,
        _ => match (state.runtime_string(left), state.runtime_string(right)) {
            (Some(a), Some(b)) => a == b,
            _ => left == right,
        },
    }
}
