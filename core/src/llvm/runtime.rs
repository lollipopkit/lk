//! Runtime helpers exposed to LLVM-generated code.
//!
//! The LLVM backend lowers high-level operations (string interning, list/map
//! construction, global access, etc.) into calls to `lkr_rt_*` functions. This
//! module provides those helpers and bridges them back to the VM runtime so we
//! can reuse existing semantics while sharing a common value encoding.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Result, anyhow};

use crate::{
    llvm::encoding,
    module::ModuleRegistry,
    op::BinOp,
    stmt::{ImportStmt, ModuleResolver, deserialize_imports, execute_imports},
    typ::TypeChecker,
    util::fast_map::{FastHashMap, fast_hash_map_new, fast_hash_map_with_capacity},
    val::Val,
    vm::{BytecodeModule, VmContext, decode_module},
};

#[cfg(not(test))]
unsafe extern "Rust" {
    fn lkr_stdlib_register_globals(registry: &mut ModuleRegistry);
    fn lkr_stdlib_register_modules(registry: &mut ModuleRegistry) -> Result<()>;
}

#[cfg(test)]
fn lkr_stdlib_register_globals(_registry: &mut ModuleRegistry) {}

#[cfg(test)]
fn lkr_stdlib_register_modules(_registry: &mut ModuleRegistry) -> Result<()> {
    Ok(())
}

static RUNTIME_STATE: OnceLock<Mutex<RuntimeState>> = OnceLock::new();

fn with_state<R>(f: impl FnOnce(&mut RuntimeState) -> R) -> R {
    let mutex = RUNTIME_STATE.get_or_init(|| Mutex::new(RuntimeState::default()));
    let mut guard = mutex.lock().expect("runtime state poisoned");
    f(&mut guard)
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_begin_session() {
    with_state(|state| state.reset_session());
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_register_search_path(ptr: *const i8, len: i64) {
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
pub extern "C" fn lkr_rt_register_bundled_module(
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
            eprintln!("lkr_rt_register_bundled_module: failed to decode module: {err}");
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
pub extern "C" fn lkr_rt_register_imports(ptr: *const i8, len: i64) -> i32 {
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
            eprintln!("lkr_rt_register_imports: failed to parse imports: {err}");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_apply_imports() -> i32 {
    let result: Result<()> = with_state(|state| state.apply_pending());
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("lkr_rt_apply_imports error: {err}");
            -1
        }
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
    imports_applied: bool,
}

impl RuntimeState {
    fn new() -> Self {
        let (ctx, resolver) = match Self::build_context(&[], &[]) {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("lkr_rt: failed to initialise stdlib context: {err}");
                let resolver = Arc::new(ModuleResolver::new());
                let ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
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
            imports_applied: false,
        }
    }

    fn reset_session(&mut self) {
        self.pending_search_paths.clear();
        self.pending_imports.clear();
        self.pending_bundled.clear();
        self.imports_applied = false;
        self.handles = HandleTable::default();
        self.interned_strings.clear();
        match Self::build_context(&[], &[]) {
            Ok((ctx, resolver)) => {
                self.ctx = ctx;
                self.resolver = resolver;
            }
            Err(err) => {
                eprintln!("lkr_rt: failed to reset stdlib context: {err}");
                let resolver = Arc::new(ModuleResolver::new());
                self.ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
                self.resolver = resolver;
            }
        }
    }

    fn apply_pending(&mut self) -> Result<()> {
        let (mut ctx, resolver) = Self::build_context(&self.pending_search_paths, &self.pending_bundled)?;
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

    fn build_context(
        search_paths: &[String],
        bundled: &[DecodedBundledModule],
    ) -> Result<(VmContext, Arc<ModuleResolver>)> {
        let mut registry = ModuleRegistry::new();
        #[cfg_attr(test, allow(unused_unsafe))]
        unsafe {
            lkr_stdlib_register_globals(&mut registry);
            lkr_stdlib_register_modules(&mut registry)?;
        }

        let mut resolver = ModuleResolver::with_registry(registry);
        for path in search_paths {
            resolver.add_search_path(PathBuf::from(path));
        }
        for module in bundled {
            Self::register_embedded_recursive(&resolver, &module.path, &module.module);
        }

        let resolver_arc = Arc::new(resolver);
        let ctx = VmContext::new()
            .with_resolver(Arc::clone(&resolver_arc))
            .with_type_checker(Some(TypeChecker::new_strict()));
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
    Val::List(pairs.into())
}

fn list_slice(list: &Arc<[Val]>, start: i64) -> Val {
    if start <= 0 {
        return Val::List(list.clone());
    }
    let idx = start as usize;
    if idx >= list.len() {
        Val::List(Vec::<Val>::new().into())
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
pub extern "C" fn lkr_rt_intern_string(ptr: *const i8, len: i64) -> i64 {
    let text = read_string(ptr, len);
    with_state(|state| state.intern_string(text.as_str()))
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_to_string(value: i64) -> i64 {
    with_state(|state| {
        let rendered = state.decode_value(value).to_string();
        state.intern_string(rendered.as_str())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_load_global(name: i64) -> i64 {
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
pub extern "C" fn lkr_rt_define_global(name: i64, value: i64) {
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
pub extern "C" fn lkr_rt_build_list(ptr: *const i64, len: i64) -> i64 {
    let len_usize = len.max(0) as usize;
    with_state(|state| {
        let elements = state.decode_values(ptr, len_usize);
        let list = Val::List(elements.into());
        state.encode_value(list)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_build_map(ptr: *const i64, len: i64) -> i64 {
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
                    eprintln!("lkr_rt_build_map: {err}");
                    return encoding::NIL_VALUE;
                }
            }
        }
        state.encode_value(Val::Map(Arc::new(map)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_call(func: i64, args_ptr: *const i64, argc: i64, retc: i64) -> i64 {
    with_state(|state| {
        let callee = state.decode_value(func);
        let argc_usize = argc.max(0) as usize;
        let args = state.decode_values(args_ptr, argc_usize);
        let result = callee.call(&args, &mut state.ctx);
        match result {
            Ok(val) => {
                if retc <= 0 {
                    encoding::NIL_VALUE
                } else {
                    state.encode_value(val)
                }
            }
            Err(err) => {
                eprintln!("lkr_rt_call error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_add(lhs: i64, rhs: i64) -> i64 {
    with_state(|state| {
        let left = state.decode_value(lhs);
        let right = state.decode_value(rhs);
        match BinOp::Add.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("lkr_rt_add error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_access(base: i64, key: i64) -> i64 {
    with_state(|state| {
        let base_val = state.decode_value(base);
        let key_val = state.decode_value(key);
        let result = base_val.access(&key_val).unwrap_or(Val::Nil);
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_index(base: i64, idx: i64) -> i64 {
    with_state(|state| {
        let base_val = state.decode_value(base);
        let idx_val = state.decode_value(idx);
        let result = index_value(&base_val, &idx_val);
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_in(needle: i64, haystack: i64) -> i64 {
    with_state(|state| {
        let l = state.decode_value(needle);
        let r = state.decode_value(haystack);
        match BinOp::In.cmp(&l, &r) {
            Ok(result) => state.encode_value(Val::Bool(result)),
            Err(err) => {
                eprintln!("lkr_rt_in error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lkr_rt_len(value: i64) -> i64 {
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
pub extern "C" fn lkr_rt_list_slice(list: i64, start: i64) -> i64 {
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
pub extern "C" fn lkr_rt_to_iter(value: i64) -> i64 {
    with_state(|state| {
        let val = state.decode_value(value);
        let iter = match val {
            Val::List(_) | Val::Str(_) => val,
            Val::Map(ref map) => map_to_iterable(map),
            other => match other {
                Val::Nil => Val::List(Vec::<Val>::new().into()),
                Val::Bool(_) | Val::Int(_) | Val::Float(_) | Val::Object(_) | Val::Task(_) | Val::Channel(_) => {
                    Val::List(Vec::<Val>::new().into())
                }
                _ => Val::List(Vec::<Val>::new().into()),
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
        let handle = lkr_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
        let handle_again = lkr_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
        assert_eq!(handle, handle_again);
        assert!(matches!(decode_for_tests(handle), Val::Str(s) if s.as_ref() == "hello"));
    }

    #[test]
    fn build_list_and_len() {
        let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
        reset_runtime_state();
        let values = [encoding::BOOL_TRUE_VALUE, 42, encoding::NIL_VALUE];
        let list_handle = lkr_rt_build_list(values.as_ptr(), values.len() as i64);
        let len = lkr_rt_len(list_handle);
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
        let name_handle = lkr_rt_intern_string(name_bytes.as_ptr().cast(), 1);
        lkr_rt_define_global(name_handle, encoding::BOOL_TRUE_VALUE);
        let loaded = lkr_rt_load_global(name_handle);
        assert_eq!(loaded, encoding::BOOL_TRUE_VALUE);
    }

    fn add_one(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        let value = args.get(0).cloned().unwrap_or(Val::Int(0));
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
        let result = lkr_rt_call(func_handle, &arg, 1, 1);
        assert_eq!(result, 42);
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
        lkr_rt_begin_session();

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .to_path_buf();
        let examples_dir = workspace.join("examples");
        let search_path = CString::new("examples").unwrap();
        lkr_rt_register_search_path(search_path.as_ptr(), search_path.as_bytes().len() as i64);

        let fib_path = examples_dir.join("fib.lkr");
        let fib_module = compile_module_from_path(&fib_path);
        let fib_bytes = encode_module(&fib_module).expect("encode module");
        let fib_path_rel = CString::new("examples/fib.lkr").unwrap();
        let _ = lkr_rt_register_bundled_module(
            fib_path_rel.as_ptr(),
            fib_path_rel.as_bytes().len() as i64,
            fib_bytes.as_ptr(),
            fib_bytes.len() as i64,
        );

        let imports_json = CString::new("[{\"File\":{\"path\":\"examples/fib\"}}]").unwrap();
        let _ = lkr_rt_register_imports(imports_json.as_ptr(), imports_json.as_bytes().len() as i64);
        let apply = lkr_rt_apply_imports();
        assert_eq!(apply, 0, "apply imports succeeds");

        let fib_name = CString::new("fib").unwrap();
        let fib_handle = lkr_rt_intern_string(fib_name.as_ptr(), fib_name.as_bytes().len() as i64);
        let fib_value = lkr_rt_load_global(fib_handle);
        assert_ne!(fib_value, encoding::NIL_VALUE, "fib module is loaded");
    }
}
