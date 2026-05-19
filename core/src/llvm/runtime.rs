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
use arcstr::ArcStr;

use crate::{
    llvm::encoding,
    module::ModuleRegistry,
    op::BinOp,
    stmt::{ImportSource, ImportStmt, ModuleResolver, deserialize_imports, execute_imports},
    util::fast_map::{FastHashMap, fast_hash_map_new, fast_hash_map_with_capacity},
    val::{AotFunction, NativeArgs, Val, methods::find_method_for_val},
    vm::{BytecodeModule, Vm, VmContext, decode_module},
};

mod collections;
mod imports;
mod math;
#[cfg(test)]
mod tests;

pub use collections::*;
use imports::stdlib_registrars_from_imports;
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
    let function = Val::AotFunction(Box::new(AotFunction {
        ptr: fn_ptr as usize,
        arity: arity as u8,
    }));
    with_state(move |state| {
        state
            .pending_native_modules
            .entry(module)
            .or_insert_with(fast_hash_map_new)
            .insert(ArcStr::from(name), function);
        state.imports_applied = false;
    });
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_make_aot_function(fn_ptr: *const (), arity: i64) -> i64 {
    if fn_ptr.is_null() || arity < 0 || arity > u8::MAX as i64 {
        return encoding::NIL_VALUE;
    }
    with_state(|state| {
        state.encode_value(Val::AotFunction(Box::new(AotFunction {
            ptr: fn_ptr as usize,
            arity: arity as u8,
        })))
    })
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
    pending_native_modules: FastHashMap<String, FastHashMap<ArcStr, Val>>,
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
        native_modules: &FastHashMap<String, FastHashMap<ArcStr, Val>>,
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
        native_modules: &FastHashMap<String, FastHashMap<ArcStr, Val>>,
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
        native_modules: &FastHashMap<String, FastHashMap<ArcStr, Val>>,
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
        register_aot_stdlib_method_modules(&mut registry)?;
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
                value if value.as_str().is_some() => self.intern_string(value.as_str().unwrap()),
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
        let handle = self.handles.alloc(Val::from_str(value));
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

fn install_aot_core_vm_builtins(ctx: &mut VmContext) {
    ctx.globals_mut()
        .entry("__lk_call_method".to_string())
        .or_insert(Val::RustFunction(aot_core_call_method_builtin));
    ctx.globals_mut()
        .entry("__lk_call_method_named".to_string())
        .or_insert(Val::RustFunction(aot_core_call_method_named_builtin));
}

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

fn aot_core_call_method_builtin(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!(
            "__lk_call_method expects 3 arguments: receiver, method name, positional args list"
        ));
    }

    let receiver = args[0].clone();
    let method_name = args[1].as_str().ok_or_else(|| {
        anyhow!(
            "__lk_call_method expects method name as string, got {}",
            args[1].type_name()
        )
    })?;
    let method_key = Val::from_str(method_name);
    let positional_args: Vec<Val> = match &args[2] {
        Val::List(list) => list.iter().cloned().collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method expects positional arguments as list, got {}",
                other.type_name()
            ));
        }
    };

    if let Some(prop_val) = receiver.access(&method_key) {
        match prop_val {
            Val::Closure(_)
            | Val::RustFunction(_)
            | Val::RustFastFunction(_)
            | Val::RustFastFunctionNamed(_)
            | Val::RustFunctionNamed(_)
            | Val::AotFunction(_) => {
                return prop_val.call(&positional_args, ctx);
            }
            other if positional_args.is_empty() => return Ok(other),
            _ => {}
        }
    }

    if let Some(func) = find_method_for_val(&receiver, method_name) {
        let mut full_args = Vec::with_capacity(positional_args.len() + 1);
        full_args.push(receiver);
        full_args.extend(positional_args);
        return func.call(&full_args, ctx);
    }

    Err(anyhow!("{} has no method '{}'", args[0].type_name(), method_name))
}

fn aot_core_call_method_named_builtin(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 4 {
        return Err(anyhow!(
            "__lk_call_method_named expects 4 arguments: receiver, method name, positional args list, named args map"
        ));
    }

    let receiver = args[0].clone();
    let method_name = args[1].as_str().ok_or_else(|| {
        anyhow!(
            "__lk_call_method_named expects method name as string, got {}",
            args[1].type_name()
        )
    })?;
    let method_key = Val::from_str(method_name);
    let positional_args: Vec<Val> = match &args[2] {
        Val::List(list) => list.iter().cloned().collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method_named expects positional arguments as list, got {}",
                other.type_name()
            ));
        }
    };
    let named_pairs: Vec<(String, Val)> = match &args[3] {
        Val::Map(map) => map
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method_named expects named arguments as map, got {}",
                other.type_name()
            ));
        }
    };

    if let Some(prop_val) = receiver.access(&method_key) {
        match prop_val {
            Val::Closure(_) | Val::RustFastFunctionNamed(_) | Val::RustFunctionNamed(_) | Val::AotFunction(_) => {
                return prop_val.call_named(&positional_args, &named_pairs, ctx);
            }
            Val::RustFunction(_) | Val::RustFastFunction(_) if named_pairs.is_empty() => {
                return prop_val.call(&positional_args, ctx);
            }
            other if positional_args.is_empty() && named_pairs.is_empty() => return Ok(other),
            _ => {}
        }
    }

    if named_pairs.is_empty()
        && let Some(func) = find_method_for_val(&receiver, method_name)
    {
        let mut full_args = Vec::with_capacity(positional_args.len() + 1);
        full_args.push(receiver);
        full_args.extend(positional_args);
        return func.call(&full_args, ctx);
    }

    Err(anyhow!("{} has no method '{}'", args[0].type_name(), method_name))
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct HandleTable {
    values: Vec<Val>,
}

impl HandleTable {
    fn alloc(&mut self, value: Val) -> i64 {
        let index = self.values.len();
        self.values.push(value);
        i64::MAX - index as i64
    }

    fn get(&self, handle: i64) -> Option<Val> {
        let index = usize::try_from(i64::MAX.checked_sub(handle)?).ok()?;
        self.values.get(index).cloned()
    }

    fn get_ref(&self, handle: i64) -> Option<&Val> {
        let index = usize::try_from(i64::MAX.checked_sub(handle)?).ok()?;
        self.values.get(index)
    }

    fn get_mut(&mut self, handle: i64) -> Option<&mut Val> {
        let index = usize::try_from(i64::MAX.checked_sub(handle)?).ok()?;
        self.values.get_mut(index)
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

fn bool_to_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn encode_map_key(value: &Val) -> Result<ArcStr> {
    match value {
        Val::ShortStr(s) => Ok(Val::intern_str(s.as_str())),
        Val::Str(s) => Ok(s.clone()),
        Val::Int(i) => Ok(Val::intern_str(i.to_string().as_str())),
        Val::Float(f) => Ok(Val::intern_str(f.to_string().as_str())),
        Val::Bool(b) => Ok(Val::intern_str(bool_to_str(*b))),
        other => Err(anyhow!("map key must be primitive, got {}", other.type_name())),
    }
}

fn map_to_iterable(map: &FastHashMap<ArcStr, Val>) -> Val {
    let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
    keys.sort();
    let mut pairs = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(value) = map.get(key) {
            let pair = Val::List(vec![Val::from_str(key), value.clone()].into());
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
        (base, Val::Int(i)) if base.as_str().is_some() => {
            let s = base.as_str().unwrap();
            if *i < 0 {
                Val::Nil
            } else if s.is_ascii() {
                let i = *i as usize;
                let bytes = s.as_bytes();
                if i < bytes.len() {
                    let ch = bytes[i] as char;
                    Val::from_str(&ch.to_string())
                } else {
                    Val::Nil
                }
            } else {
                s.chars()
                    .nth(*i as usize)
                    .map(|ch| Val::from_str(&ch.to_string()))
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
        Val::AotFunction(function) => Some(*function),
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
            .access(&Val::from_str(method_name.as_str()))
            .unwrap_or(Val::Nil)
        {
            Val::AotFunction(function) => Some(*function),
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
        let method_key = Val::from_str(method_name.as_str());
        let args = state.decode_values(args_ptr, argc_usize);
        if let Some(callee) = receiver_val.access(&method_key) {
            let result: Result<i64> = callee.call(&args, &mut state.ctx).map(|val| {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    state.encode_value(val)
                }
            });
            return match result {
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
            };
        }
        let result: Result<i64> = if let Some(function) = find_method_for_val(&receiver_val, method_name.as_str()) {
            let mut full_args = Vec::with_capacity(args.len() + 1);
            full_args.push(receiver_val);
            full_args.extend(args);
            function.call(&full_args, &mut state.ctx).map(|val| {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    state.encode_value(val)
                }
            })
        } else {
            let callee = receiver_val
                .access(&Val::from_str(method_name.as_str()))
                .unwrap_or(Val::Nil);
            callee.call(&args, &mut state.ctx).map(|val| {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    state.encode_value(val)
                }
            })
        };
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
        Val::AotFunction(function) => Some(*function),
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
            Val::RustFastFunction(function) => function(NativeArgs::new(&args), &mut state.ctx),
            Val::RustFastFunctionNamed(function) => function(NativeArgs::new(&args), &[], &mut state.ctx),
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
            5 => {
                let args = std::slice::from_raw_parts(args_ptr, 5);
                let f: extern "C" fn(i64, i64, i64, i64, i64) -> i64 = std::mem::transmute(function.ptr);
                f(args[0], args[1], args[2], args[3], args[4])
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
pub extern "C" fn lk_rt_float(value: f64) -> i64 {
    with_state(|state| state.encode_value(Val::Float(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_floor(value: i64) -> i64 {
    with_state(|state| {
        let out = match state.decode_value(value) {
            Val::Float(f) => f.floor() as i64,
            Val::Int(i) => i,
            _ => 0,
        };
        state.encode_value(Val::Int(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_starts_with(value: i64, prefix: i64) -> i64 {
    with_state(|state| {
        let value = state.decode_value(value);
        let prefix = state.decode_value(prefix);
        let out = match (value.as_str(), prefix.as_str()) {
            (Some(value), Some(prefix)) => value.starts_with(prefix),
            _ => false,
        };
        state.encode_value(Val::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_contains(value: i64, needle: i64) -> i64 {
    with_state(|state| {
        let value = state.decode_value(value);
        let needle = state.decode_value(needle);
        let out = match (value.as_str(), needle.as_str()) {
            (Some(value), Some(needle)) => value.contains(needle),
            _ => false,
        };
        state.encode_value(Val::Bool(out))
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
        match op.cmp(&left, &right) {
            Ok(value) => state.encode_value(Val::Bool(value)),
            Err(err) => {
                eprintln!("lk_rt_cmp error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}
