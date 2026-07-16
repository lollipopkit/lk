//! `lk-api` — L5 host-embedding API for LK.
//!
//! A minimal, safe surface for embedding the LK VM in a Rust host. Each [`Vm`]
//! is an **isolated instance**: it owns its own `VmContext` (heap, globals,
//! async runtime handle), so multiple VMs are fully independent with no shared
//! global state — this is exactly what the M0 "去全局状态" work enabled. Add a
//! fuel budget to sandbox execution (the instruction-budget knob of M2.6).

use std::sync::Arc;

use anyhow::Result;
use lk_core::module::ModuleRegistry;
use lk_core::stmt::ModuleResolver;
use lk_core::syntax::{ParseOptions, parse_program_source};
use lk_core::typ::TypeChecker;
use lk_core::vm::{NativeFunction, VmContext, execute_program_with_ctx_and_limits};

pub use lk_core::val::RuntimeVal;
pub use lk_core::vm::{NativeArgs, NativeRuntime};

/// Signature of a host-provided native function, callable from LK. Receives the
/// raw runtime ABI (positional/named [`NativeArgs`] and the [`NativeRuntime`]
/// for heap access) and returns a [`RuntimeVal`].
pub type HostFn = fn(NativeArgs<'_>, &mut NativeRuntime<'_>) -> Result<RuntimeVal>;

/// An isolated LK virtual machine instance.
pub struct Vm {
    /// Pending module/builtin registry; consumed into the context on first eval
    /// so host functions can be registered before execution.
    registry: Option<ModuleRegistry>,
    ctx: Option<VmContext>,
    fuel: Option<u64>,
    heap_limit: Option<usize>,
}

impl Vm {
    /// Create a VM with the full standard library registered.
    pub fn new() -> Self {
        let mut registry = ModuleRegistry::new();
        lk_stdlib::register_stdlib_globals(&mut registry);
        lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registration should not fail");
        Self {
            registry: Some(registry),
            ctx: None,
            fuel: None,
            heap_limit: None,
        }
    }

    /// Create a sandboxed VM: only the core builtins (`println`/`typeof`/
    /// …) and the explicitly allowed stdlib modules are registered. OS-capable
    /// modules (`fs`/`net`/`process`/…) are withheld unless named, so untrusted
    /// scripts cannot reach them — the module-whitelist knob of the sandbox model
    /// (plan M2.6). Combine with [`with_fuel`](Self::with_fuel) for a bounded,
    /// capability-restricted instance.
    pub fn sandboxed(allow_modules: &[&str]) -> Self {
        let mut registry = ModuleRegistry::new();
        lk_stdlib::register_stdlib_core_globals(&mut registry);
        let names: Vec<String> = allow_modules.iter().map(|m| (*m).to_string()).collect();
        lk_stdlib::register_stdlib_modules_named(&mut registry, &names)
            .expect("named stdlib registration should not fail");
        Self {
            registry: Some(registry),
            ctx: None,
            fuel: None,
            heap_limit: None,
        }
    }

    /// Bound execution to `budget` instructions (fuel). Beyond it the VM aborts
    /// with a step-limit error instead of running unbounded (sandbox, plan M2.6).
    pub fn with_fuel(mut self, budget: u64) -> Self {
        self.fuel = Some(budget);
        self
    }

    /// Cap the number of live heap objects — a coarse memory bound for the
    /// sandbox: allocation beyond `max_objects` aborts with a heap-limit error
    /// (plan M2.6). Zero-cost when unset.
    pub fn with_heap_limit(mut self, max_objects: usize) -> Self {
        self.heap_limit = Some(max_objects);
        self
    }

    /// Register a host native function callable from LK as `name` with the given
    /// `arity`. Must be called before the first [`eval`](Self::eval) (the context
    /// is finalized on first evaluation). Host extension point (plan M3.2).
    pub fn register_fn(&mut self, name: &str, arity: u16, f: HostFn) -> &mut Self {
        self.registry
            .as_mut()
            .expect("register_fn must be called before the first eval")
            .register_runtime_builtin(name, NativeFunction::Plain(f), arity);
        self
    }

    /// Finalize the pending registry into a context on first use.
    fn ctx_mut(&mut self) -> &mut VmContext {
        if self.ctx.is_none() {
            let registry = self.registry.take().expect("registry present before first eval");
            let resolver = Arc::new(ModuleResolver::with_registry(registry));
            self.ctx = Some(
                VmContext::new()
                    .with_resolver(resolver)
                    .with_type_checker(Some(TypeChecker::new_strict())),
            );
        }
        self.ctx.as_mut().expect("context finalized")
    }

    /// Parse and execute `source`, returning the compiled program's result.
    fn run(&mut self, source: &str) -> Result<lk_core::vm::ProgramResult> {
        let program = parse_program_source(source, ParseOptions::default())
            .map_err(|err| anyhow::anyhow!("parse error: {err}"))?;
        let fuel = self.fuel;
        let heap_limit = self.heap_limit;
        let ctx = self.ctx_mut();
        if fuel.is_some() || heap_limit.is_some() {
            execute_program_with_ctx_and_limits(&program, ctx, fuel, heap_limit)
        } else {
            program.execute_with_ctx(ctx)
        }
    }

    /// Parse and execute `source`, returning the display of the program's first
    /// return value (empty string when it is `nil`).
    pub fn eval(&mut self, source: &str) -> Result<String> {
        let result = self.run(source)?;
        if result.first_return_is_nil() {
            Ok(String::new())
        } else {
            Ok(result.display_first_return())
        }
    }

    /// Parse and execute `source`, returning the program's first return value as
    /// a fully-detached host [`Value`]. Primitives come back typed; strings,
    /// lists, and maps are converted **structurally** (recursively) so the host
    /// can walk containers without touching the VM heap. This is the ergonomic
    /// typed counterpart to [`eval`](Self::eval) (plan M3.1).
    pub fn eval_value(&mut self, source: &str) -> Result<Value> {
        let result = self.run(source)?;
        Ok(value_from_runtime(result.first_return(), result.heap()))
    }
}

/// A host-owned, fully-detached LK value returned from [`Vm::eval_value`].
///
/// Primitives are typed; strings, lists, and maps are converted **structurally**
/// (recursively) so a host can walk them without touching the VM heap. Map keys
/// are stringified (LK map keys are typically strings or ints) and entries keep
/// the VM's iteration order. Heap kinds without a natural host representation
/// (sets, structs, callables, channels, …) arrive as their display string.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
}

impl Value {
    /// The integer, if this is an [`Value::Int`].
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    /// The float, if this is a [`Value::Float`] (or an [`Value::Int`] widened).
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(value) => Some(*value),
            Value::Int(value) => Some(*value as f64),
            _ => None,
        }
    }

    /// The boolean, if this is a [`Value::Bool`].
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The string slice, if this is a [`Value::Str`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(value) => Some(value),
            _ => None,
        }
    }

    /// The element slice, if this is a [`Value::List`].
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(values) => Some(values),
            _ => None,
        }
    }

    /// The entry slice, if this is a [`Value::Map`].
    pub fn as_map(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// Look up a key in a [`Value::Map`].
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Str(value)
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_string())
    }
}

/// Recursively convert a VM [`RuntimeVal`] plus its owning heap into a detached
/// host [`Value`]. Strings/lists/maps become structured `Value`s; other heap
/// kinds fall back to their display string.
fn value_from_runtime(value: &RuntimeVal, heap: &lk_core::val::HeapStore) -> Value {
    use lk_core::val::{HeapValue, TypedList};
    match value {
        RuntimeVal::Nil => Value::Nil,
        RuntimeVal::Bool(inner) => Value::Bool(*inner),
        RuntimeVal::Int(inner) => Value::Int(*inner),
        RuntimeVal::Float(inner) => Value::Float(*inner),
        RuntimeVal::ShortStr(inner) => Value::Str(inner.as_str().to_string()),
        RuntimeVal::Obj(handle) => match heap.get(*handle) {
            Some(HeapValue::String(text)) => Value::Str(text.to_string()),
            Some(HeapValue::List(list)) => {
                let items = match list {
                    TypedList::Mixed(items) => items.iter().map(|v| value_from_runtime(v, heap)).collect(),
                    TypedList::Int(items) => items.iter().map(|v| Value::Int(*v)).collect(),
                    TypedList::Float(items) => items.iter().map(|v| Value::Float(*v)).collect(),
                    TypedList::Bool(items) => items.iter().map(|v| Value::Bool(*v)).collect(),
                    TypedList::String(items) => items.iter().map(|v| Value::Str(v.to_string())).collect(),
                };
                Value::List(items)
            }
            Some(HeapValue::Map(map)) => Value::Map(
                map.entries_iter()
                    .into_iter()
                    .map(|(key, val)| (map_key_to_string(&key), value_from_runtime(&val, heap)))
                    .collect(),
            ),
            // Sets, structs, callables, channels, bytes, … have no structured
            // host form; hand back the VM's display string.
            _ => Value::Str(lk_core::vm::display_runtime_value(value, heap)),
        },
    }
}

fn map_key_to_string(key: &lk_core::val::RuntimeMapKey) -> String {
    use lk_core::val::RuntimeMapKey;
    match key {
        RuntimeMapKey::Nil => "nil".to_string(),
        RuntimeMapKey::Bool(value) => value.to_string(),
        RuntimeMapKey::Int(value) => value.to_string(),
        RuntimeMapKey::ShortStr(value) => value.as_str().to_string(),
        RuntimeMapKey::String(value) => value.to_string(),
        RuntimeMapKey::Obj(handle) => format!("<obj {}>", handle.index()),
    }
}

/// Scalar argument for [`HybridModule::call_discard`] (re-exported core type).
pub use lk_core::vm::ModuleFunctionArg as HybridArg;

/// Tier 1 hybrid bridge (`docs/llvm/tier1-hybrid.md`): a decoded module
/// artifact plus an isolated VM context, so a native binary can execute
/// individual VM-only functions of the *same* module it was compiled from.
/// The artifact goes through the verified decode path (`from_json_str` →
/// `into_module` → `verify_module`) and its imports are resolved against the
/// full stdlib once at load; each call then seeds fresh per-call state
/// (bridge-eligible functions touch no user globals, so this is invisible).
pub struct HybridModule {
    module: Arc<lk_core::vm::Module>,
    ctx: VmContext,
}

impl HybridModule {
    /// Decode a serialized `ModuleArtifact` (the `.lkm` JSON form) and prepare
    /// an isolated context with the full stdlib and the artifact's imports.
    pub fn from_artifact_json(json: &str) -> Result<Self> {
        let artifact = lk_core::vm::ModuleArtifact::from_json_str(json)?;
        let imports = artifact.imports.clone();
        let module = Arc::new(artifact.into_module()?);
        let mut registry = ModuleRegistry::new();
        lk_stdlib::register_stdlib_globals(&mut registry);
        lk_stdlib::register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut ctx = VmContext::new().with_resolver(Arc::clone(&resolver));
        lk_core::stmt::import::execute_imports(&imports, resolver.as_ref(), &mut ctx)?;
        Ok(Self { module, ctx })
    }

    /// Find a named `fn` by its bytecode debug name (compile-time source name).
    pub fn find_function(&self, debug_name: &str) -> Option<u32> {
        self.module
            .functions
            .iter()
            .position(|function| function.debug_name.as_deref() == Some(debug_name))
            .map(|index| index as u32)
    }

    /// Call function `function_index` with scalar `args`, discarding the result
    /// (the v1 bridge only marks callees whose results are proven discarded).
    /// An uncaught VM error comes back as `Err` with the rendered message.
    pub fn call_discard(&mut self, function_index: u32, args: &[HybridArg]) -> Result<()> {
        lk_core::vm::call_module_function_with_ctx(&self.module, function_index, args, &mut self.ctx).map(|_| ())
    }

    /// Call function `function_index` and keep the per-call state alive so
    /// heap-backed results (returned *or raised*) stay readable — the v2
    /// bridge marshals them into native memory before dropping the outcome
    /// (`docs/llvm/tier1-hybrid.md` v2).
    pub fn call_keep_state(
        &mut self,
        function_index: u32,
        args: &[HybridArg],
    ) -> Result<lk_core::vm::ModuleFunctionCall> {
        lk_core::vm::call_module_function_with_ctx_keep_state(&self.module, function_index, args, &mut self.ctx)
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_returns_value() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval("return 6 * 7;").unwrap(), "42");
    }

    #[test]
    fn eval_value_returns_typed_primitives() {
        let mut vm = Vm::new();
        assert_eq!(vm.eval_value("return 6 * 7;").unwrap(), Value::Int(42));
        assert_eq!(vm.eval_value("return 1 < 2;").unwrap(), Value::Bool(true));
        assert_eq!(vm.eval_value("return 3.5;").unwrap(), Value::Float(3.5));
        assert_eq!(vm.eval_value("return nil;").unwrap(), Value::Nil);
        assert_eq!(vm.eval_value("return \"hi\";").unwrap(), Value::Str("hi".to_string()));
    }

    #[test]
    fn eval_value_converts_lists_structurally() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_value("return [1, 2, 3];").unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
        );
        // Mixed and nested containers recurse.
        let nested = vm.eval_value("return [1, \"two\", [3, 4]];").unwrap();
        assert_eq!(
            nested,
            Value::List(vec![
                Value::Int(1),
                Value::Str("two".to_string()),
                Value::List(vec![Value::Int(3), Value::Int(4)]),
            ])
        );
        assert_eq!(nested.as_list().unwrap().len(), 3);
        assert_eq!(nested.as_list().unwrap()[1].as_str(), Some("two"));
    }

    #[test]
    fn eval_value_converts_maps_structurally() {
        let mut vm = Vm::new();
        let value = vm.eval_value("return {\"name\": \"lk\", \"nums\": [1, 2]};").unwrap();
        assert_eq!(value.get("name").and_then(Value::as_str), Some("lk"));
        assert_eq!(
            value.get("nums").and_then(Value::as_list),
            Some(&[Value::Int(1), Value::Int(2)][..])
        );
        // Iteration order is the VM's map order; two known keys are present.
        assert_eq!(value.as_map().unwrap().len(), 2);
    }

    #[test]
    fn instances_are_isolated() {
        // Two independent VMs share no global state (M0 去全局状态).
        let mut a = Vm::new();
        let mut b = Vm::new();
        assert_eq!(a.eval("let x = 10; return x;").unwrap(), "10");
        assert_eq!(b.eval("let y = 20; return y;").unwrap(), "20");
    }

    fn host_add100(args: NativeArgs<'_>, _rt: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let n = args.get(0).and_then(RuntimeVal::as_int).unwrap_or(0);
        Ok(RuntimeVal::Int(n + 100))
    }

    #[test]
    fn register_host_fn() {
        let mut vm = Vm::new();
        vm.register_fn("host_add100", 1, host_add100);
        assert_eq!(vm.eval("return host_add100(5);").unwrap(), "105");
    }

    #[test]
    fn sandbox_allows_whitelisted_modules_only() {
        // `math` is allowed; `fs` is withheld.
        let mut vm = Vm::sandboxed(&["math"]);
        assert_eq!(vm.eval("use math; return math.max(3, 7);").unwrap(), "7");
        let denied = Vm::sandboxed(&["math"]).eval("use fs; return fs.exists(\"/\");");
        assert!(denied.is_err(), "fs must be unavailable in a math-only sandbox");
    }

    #[test]
    fn heap_limit_bounds_allocation() {
        let mut vm = Vm::new().with_heap_limit(1000);
        let err = vm
            .eval("let xs = []; for i in 1..=1000000 { xs.push([i]); } return 0;")
            .expect_err("heap-exhausted run should error");
        assert!(err.to_string().contains("heap object limit"), "unexpected error: {err}");
    }

    #[test]
    fn fuel_bounds_execution() {
        let mut vm = Vm::new().with_fuel(200);
        let err = vm
            .eval("let s = 0; for i in 1..=1000000 { s += i; } return s;")
            .expect_err("fuel-exhausted run should error");
        assert!(err.to_string().contains("step limit"), "unexpected error: {err}");
    }

    /// Compile `source` to the serialized module-artifact JSON a hybrid binary
    /// would embed (same pipeline as `lk compile bytecode`).
    fn artifact_json(source: &str) -> String {
        let program = parse_program_source(source, ParseOptions::default()).expect("parse");
        let mut registry = ModuleRegistry::new();
        lk_stdlib::register_stdlib_globals(&mut registry);
        lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registration");
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut ctx = VmContext::new().with_resolver(resolver);
        let module = lk_core::vm::compile_program_module_with_ctx(&program, &mut ctx).expect("compile");
        let artifact =
            lk_core::vm::ModuleArtifact::new(lk_core::stmt::import::collect_program_imports(&program), &module)
                .expect("artifact");
        artifact.to_json_string().expect("serialize artifact")
    }

    #[test]
    fn hybrid_module_calls_functions_by_index() {
        // The callee asserts its own inputs via `error(...)`: an Ok call proves
        // the computation actually ran (v1 bridge results are discarded).
        let json = artifact_json("fn check(a, b) { if a + b != 42 { error(\"bad sum\"); } }\nreturn 0;\n");
        let mut hybrid = HybridModule::from_artifact_json(&json).expect("load artifact");
        let index = hybrid.find_function("check").expect("check fn present");
        hybrid
            .call_discard(index, &[HybridArg::Int(2), HybridArg::Int(40)])
            .expect("42 passes");
        let err = hybrid
            .call_discard(index, &[HybridArg::Int(1), HybridArg::Int(1)])
            .expect_err("wrong sum raises");
        assert!(format!("{err:#}").contains("bad sum"), "unexpected error: {err:#}");
    }

    #[test]
    fn hybrid_module_resolves_stdlib_imports() {
        let json =
            artifact_json("use math;\nfn check(a, b) { if math.max(a, b) != 7 { error(\"bad max\"); } }\nreturn 0;\n");
        let mut hybrid = HybridModule::from_artifact_json(&json).expect("load artifact");
        let index = hybrid.find_function("check").expect("check fn present");
        hybrid
            .call_discard(index, &[HybridArg::Int(3), HybridArg::Int(7)])
            .expect("stdlib import works across the bridge");
    }

    #[test]
    fn hybrid_module_marshals_scalars() {
        let json = artifact_json(
            "fn check(s, f, b) { if s.len() != 30 { error(\"len\"); } if f < 1.4 { error(\"f\"); } if !b { error(\"b\"); } }\nreturn 0;\n",
        );
        let mut hybrid = HybridModule::from_artifact_json(&json).expect("load artifact");
        let index = hybrid.find_function("check").expect("check fn present");
        hybrid
            .call_discard(
                index,
                &[
                    HybridArg::Str("a".repeat(30)),
                    HybridArg::Float(1.5),
                    HybridArg::Bool(true),
                ],
            )
            .expect("scalar marshaling round-trips");
    }

    #[test]
    fn runaway_recursion_is_catchable_not_fatal() {
        // Unbounded LK recursion must surface as a try/catch-able error (the
        // call-depth cap) instead of overflowing the Rust stack and aborting
        // the process — segmented-stack growth carries it to the cap.
        let mut vm = Vm::new();
        let out = vm
            .eval("fn f(n) { return f(n + 1); }\nlet caught = false;\ntry { f(0); } catch e { caught = true; }\nassert(caught);\nreturn 1;")
            .expect("runaway recursion is caught");
        assert_eq!(out, "1");
    }

    #[test]
    fn hybrid_module_rejects_garbage_artifacts() {
        assert!(HybridModule::from_artifact_json("{}").is_err());
        assert!(HybridModule::from_artifact_json("not json").is_err());
    }
}

/// C ABI surface (`ffi` feature). Opaque `Vm` pointer + eval returning an owned
/// C string; pair every `lk_vm_new`/`lk_vm_eval` with the matching free. A
/// header can be generated with cbindgen. Enables embedding from C/C++/Dart FFI
/// (plan M3.3).
#[cfg(feature = "ffi")]
pub mod ffi {
    use core::ffi::{CStr, c_char};

    use alloc::boxed::Box;
    use alloc::ffi::CString;

    extern crate alloc;

    use super::Vm;

    /// Create a new isolated VM. Free with [`lk_vm_free`].
    #[unsafe(no_mangle)]
    pub extern "C" fn lk_vm_new() -> *mut Vm {
        Box::into_raw(Box::new(Vm::new()))
    }

    /// Evaluate `src` (a NUL-terminated UTF-8 string) on `vm`, returning an owned
    /// C string with the first return value's display (free with [`lk_string_free`]),
    /// or NULL on error/invalid input.
    ///
    /// # Safety
    /// `vm` must come from [`lk_vm_new`] and not be freed; `src` must be a valid
    /// NUL-terminated string valid for the call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_vm_eval(vm: *mut Vm, src: *const c_char) -> *mut c_char {
        if vm.is_null() || src.is_null() {
            return core::ptr::null_mut();
        }
        let vm = unsafe { &mut *vm };
        let Ok(src) = (unsafe { CStr::from_ptr(src) }).to_str() else {
            return core::ptr::null_mut();
        };
        match vm.eval(src) {
            Ok(out) => CString::new(out)
                .map(CString::into_raw)
                .unwrap_or(core::ptr::null_mut()),
            Err(_) => core::ptr::null_mut(),
        }
    }

    /// Free a VM created by [`lk_vm_new`].
    ///
    /// # Safety
    /// `vm` must come from [`lk_vm_new`] and not be used afterwards.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_vm_free(vm: *mut Vm) {
        if !vm.is_null() {
            drop(unsafe { Box::from_raw(vm) });
        }
    }

    /// Free a string returned by [`lk_vm_eval`].
    ///
    /// # Safety
    /// `s` must come from [`lk_vm_eval`] and not be used afterwards.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_string_free(s: *mut c_char) {
        if !s.is_null() {
            drop(unsafe { CString::from_raw(s) });
        }
    }

    // ---- Tier 1 hybrid bridge (docs/llvm/tier1-hybrid.md) -------------------
    //
    // Process-singleton by design: a hybrid native binary embeds exactly one
    // module artifact, and threading a handle through every generated function
    // would churn the whole codegen ABI for nothing (the same reasoning that
    // keeps lkrt's per-process runtime state, M0.6/G4-G5). Registration is a
    // pointer store; the artifact decodes lazily on the first bridge call, so
    // a hybrid binary that never crosses the bridge pays nothing.

    use core::ffi::c_void;
    use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use super::{HybridArg, HybridModule};

    static HYBRID_ARTIFACT_JSON: AtomicPtr<c_char> = AtomicPtr::new(core::ptr::null_mut());
    static HYBRID: OnceLock<Mutex<HybridModule>> = OnceLock::new();

    /// Argument tags for [`LkHybridArg`]. Tag 2 (bool) reads the `i` union
    /// field as 0/1.
    pub const LK_HYBRID_ARG_I64: u8 = 0;
    pub const LK_HYBRID_ARG_F64: u8 = 1;
    pub const LK_HYBRID_ARG_BOOL: u8 = 2;
    pub const LK_HYBRID_ARG_STR: u8 = 3;

    /// Payload of one bridge argument (matches the `lk.h` union).
    #[repr(C)]
    pub union LkHybridArgValue {
        pub i: i64,
        pub f: f64,
        pub s: *const c_char,
    }

    /// One tagged scalar argument for [`lk_hybrid_call_v`].
    #[repr(C)]
    pub struct LkHybridArg {
        pub tag: u8,
        pub value: LkHybridArgValue,
    }

    /// Bridge failures abort the process: an uncaught VM error in a hybrid
    /// binary matches the VM's uncaught behavior (message + nonzero exit),
    /// and generated code stays branch-free around bridge calls.
    fn hybrid_die(message: core::fmt::Arguments<'_>) -> ! {
        eprintln!("lk hybrid bridge: {message}");
        std::process::exit(1)
    }

    /// Register the embedded module artifact JSON (NUL-terminated, 'static —
    /// hybrid wrappers embed it as a constant). Decoding is deferred to the
    /// first [`lk_hybrid_call_v`].
    ///
    /// # Safety
    /// `module_artifact_json` must be a NUL-terminated string that outlives
    /// every bridge call (hybrid wrappers pass a static constant).
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_hybrid_register(module_artifact_json: *const c_char) {
        HYBRID_ARTIFACT_JSON.store(module_artifact_json.cast_mut(), Ordering::Release);
    }

    /// The lazily-decoded process singleton (see the module header note).
    fn hybrid_module() -> &'static Mutex<HybridModule> {
        HYBRID.get_or_init(|| {
            let json_ptr = HYBRID_ARTIFACT_JSON.load(Ordering::Acquire);
            if json_ptr.is_null() {
                hybrid_die(format_args!("no module artifact registered (lk_hybrid_register)"));
            }
            let Ok(json) = (unsafe { CStr::from_ptr(json_ptr) }).to_str() else {
                hybrid_die(format_args!("embedded module artifact is not valid UTF-8"));
            };
            match HybridModule::from_artifact_json(json) {
                Ok(module) => Mutex::new(module),
                Err(err) => hybrid_die(format_args!("cannot load the embedded module artifact: {err:#}")),
            }
        })
    }

    /// Marshal the raw tagged argument array into [`HybridArg`]s.
    ///
    /// # Safety
    /// As documented on the callers: `args` must point to `argc` valid
    /// entries; string payloads must be valid NUL-terminated UTF-8.
    unsafe fn marshal_args(args: *const LkHybridArg, argc: usize) -> alloc::vec::Vec<HybridArg> {
        let raw_args = if argc == 0 {
            &[][..]
        } else {
            if args.is_null() {
                hybrid_die(format_args!("null args with argc {argc}"));
            }
            unsafe { core::slice::from_raw_parts(args, argc) }
        };
        let mut marshaled = alloc::vec::Vec::with_capacity(argc);
        for arg in raw_args {
            marshaled.push(match arg.tag {
                LK_HYBRID_ARG_I64 => HybridArg::Int(unsafe { arg.value.i }),
                LK_HYBRID_ARG_F64 => HybridArg::Float(unsafe { arg.value.f }),
                LK_HYBRID_ARG_BOOL => HybridArg::Bool(unsafe { arg.value.i } != 0),
                LK_HYBRID_ARG_STR => {
                    let ptr = unsafe { arg.value.s };
                    if ptr.is_null() {
                        hybrid_die(format_args!("null string argument"));
                    }
                    match (unsafe { CStr::from_ptr(ptr) }).to_str() {
                        Ok(s) => HybridArg::Str(s.to_string()),
                        Err(_) => hybrid_die(format_args!("string argument is not valid UTF-8")),
                    }
                }
                other => hybrid_die(format_args!("unknown hybrid arg tag {other}")),
            });
        }
        marshaled
    }

    /// Call VM-executed function `func_index` with `argc` tagged scalar
    /// arguments, discarding the result (v1 bridge: results are proven
    /// discarded before a callee is marked VM-executed). Never returns on
    /// error — see [`hybrid_die`].
    ///
    /// # Safety
    /// `args` must point to `argc` valid [`LkHybridArg`] values (may be null
    /// when `argc == 0`); string payloads must be valid NUL-terminated UTF-8.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_hybrid_call_v(func_index: u32, args: *const LkHybridArg, argc: usize) {
        let _ = unsafe { bridge_call(func_index, args, argc, false) };
    }

    /// Shared v/r bridge body: run the VM call, then either hand back the
    /// marshaled return (`want_result`; the void entry keeps v1's
    /// zero-marshal discard — a discarded container must not deep-copy, and
    /// a discarded *unmarshalable* return (struct, closure) must not die) or
    /// re-raise an uncaught VM error into the nearest native `try` frame (VM
    /// `try` semantics, v2 C6). Every Rust value — including the module
    /// Mutex guard and the per-call VM state — drops *before* the raise: the
    /// longjmp skips Rust drops, and a live guard would deadlock the next
    /// bridge call.
    ///
    /// # Safety
    /// As documented on [`lk_hybrid_call_v`].
    unsafe fn bridge_call(func_index: u32, args: *const LkHybridArg, argc: usize, want_result: bool) -> LkHybridDyn {
        use lk_core::vm::ModuleFunctionCall;

        let marshaled = unsafe { marshal_args(args, argc) };
        let mut module = hybrid_module()
            .lock()
            .unwrap_or_else(|_| hybrid_die(format_args!("bridge state poisoned")));
        let call = module.call_keep_state(func_index, &marshaled);
        drop(module);
        match call {
            Ok(ModuleFunctionCall::Return(outcome)) => {
                if want_result {
                    marshal_return(outcome.value, &outcome.state)
                } else {
                    LkHybridDyn {
                        tag: LK_HYBRID_DYN_NIL,
                        payload: 0,
                    }
                }
            }
            Ok(ModuleFunctionCall::Raise { value, rendered, state }) => {
                let raise = RT_RAISE_DYN.load(Ordering::Acquire);
                if raise == 0 {
                    // No runtime table (an old wrapper): the v1 behavior.
                    hybrid_die(format_args!("VM-executed function {func_index} failed: {rendered}"));
                }
                let payload = marshal_return(value, &state);
                drop(state);
                drop(rendered);
                drop(marshaled);
                // SAFETY: stored from a `RaiseDyn` in `lk_hybrid_register_rt`.
                let raise = unsafe { core::mem::transmute::<usize, RaiseDyn>(raise) };
                // Diverges: longjmp to the nearest native `try`, or lkrt's
                // uncaught path (message + abort).
                unsafe { raise(payload) };
                unreachable!("lkrt_rt_raise_dyn never returns");
            }
            Err(err) => hybrid_die(format_args!("VM-executed function {func_index} failed: {err:#}")),
        }
    }

    // ---- v2 return bridge --------------------------------------------------

    /// Mirror of lkrt's `LkDyn` (`{ i64, i64 }` by value) — the return type
    /// of [`lk_hybrid_call_r`]. lk-api must not link lkrt (the Tier 0 bundle
    /// has no lkrt and the hybrid link would collide two staticlibs), so the
    /// carrier and its tags are mirrored here; a conformance test in lk-cli
    /// (which dev-depends on both) pins them against lkrt's constants.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct LkHybridDyn {
        pub tag: i64,
        pub payload: i64,
    }

    /// `LkDyn` tag values (mirror of `lkrt::lkdyn::DYN_*`).
    pub const LK_HYBRID_DYN_NIL: i64 = 0;
    pub const LK_HYBRID_DYN_BOOL: i64 = 1;
    pub const LK_HYBRID_DYN_I64: i64 = 2;
    pub const LK_HYBRID_DYN_F64: i64 = 3;
    pub const LK_HYBRID_DYN_STR: i64 = 4;
    pub const LK_HYBRID_DYN_LIST: i64 = 5;
    pub const LK_HYBRID_DYN_MAP: i64 = 6;

    /// lkrt container constructors, injected by the hybrid wrapper's C
    /// constructor (`lk_hybrid_register_rt`): the wrapper is only compiled
    /// into hybrid binaries — which link lkrt — so lk-api reaches lkrt's
    /// arena builders through these pointers without ever linking it.
    type ListDynNew = unsafe extern "C" fn() -> *mut c_void;
    type ListDynPush = unsafe extern "C" fn(*mut c_void, LkHybridDyn);
    type MapStrDynNew = unsafe extern "C" fn() -> *mut c_void;
    type MapStrDynSet = unsafe extern "C" fn(*mut c_void, *const c_char, LkHybridDyn);
    type RaiseDyn = unsafe extern "C" fn(LkHybridDyn);

    static RT_LIST_DYN_NEW: AtomicUsize = AtomicUsize::new(0);
    static RT_LIST_DYN_PUSH: AtomicUsize = AtomicUsize::new(0);
    static RT_MAP_STR_DYN_NEW: AtomicUsize = AtomicUsize::new(0);
    static RT_MAP_STR_DYN_SET: AtomicUsize = AtomicUsize::new(0);
    static RT_RAISE_DYN: AtomicUsize = AtomicUsize::new(0);

    /// Register the lkrt runtime table (hybrid wrapper C constructor):
    /// container constructors for deep-converted returns, and the raise entry
    /// that re-raises an uncaught VM error into the nearest native `try`.
    /// Container returns die without it; raises fall back to the v1 exit.
    #[unsafe(no_mangle)]
    pub extern "C" fn lk_hybrid_register_rt(
        list_dyn_new: ListDynNew,
        list_dyn_push: ListDynPush,
        map_str_dyn_new: MapStrDynNew,
        map_str_dyn_set: MapStrDynSet,
        raise_dyn: RaiseDyn,
    ) {
        RT_LIST_DYN_NEW.store(list_dyn_new as usize, Ordering::Release);
        RT_LIST_DYN_PUSH.store(list_dyn_push as usize, Ordering::Release);
        RT_MAP_STR_DYN_NEW.store(map_str_dyn_new as usize, Ordering::Release);
        RT_MAP_STR_DYN_SET.store(map_str_dyn_set as usize, Ordering::Release);
        RT_RAISE_DYN.store(raise_dyn as usize, Ordering::Release);
    }

    struct HybridRt {
        list_dyn_new: ListDynNew,
        list_dyn_push: ListDynPush,
        map_str_dyn_new: MapStrDynNew,
        map_str_dyn_set: MapStrDynSet,
    }

    fn hybrid_rt() -> HybridRt {
        let list_dyn_new = RT_LIST_DYN_NEW.load(Ordering::Acquire);
        let list_dyn_push = RT_LIST_DYN_PUSH.load(Ordering::Acquire);
        let map_str_dyn_new = RT_MAP_STR_DYN_NEW.load(Ordering::Acquire);
        let map_str_dyn_set = RT_MAP_STR_DYN_SET.load(Ordering::Acquire);
        if list_dyn_new == 0 || list_dyn_push == 0 || map_str_dyn_new == 0 || map_str_dyn_set == 0 {
            hybrid_die(format_args!(
                "container return needs the lkrt constructor table (lk_hybrid_register_rt)"
            ));
        }
        // SAFETY: the values were stored from these exact fn-pointer types in
        // `lk_hybrid_register_rt`.
        unsafe {
            HybridRt {
                list_dyn_new: core::mem::transmute::<usize, ListDynNew>(list_dyn_new),
                list_dyn_push: core::mem::transmute::<usize, ListDynPush>(list_dyn_push),
                map_str_dyn_new: core::mem::transmute::<usize, MapStrDynNew>(map_str_dyn_new),
                map_str_dyn_set: core::mem::transmute::<usize, MapStrDynSet>(map_str_dyn_set),
            }
        }
    }

    /// Deep-conversion depth cap: a VM list can contain its own handle, and
    /// the marshal must fail loudly instead of recursing forever.
    const MARSHAL_DEPTH_CAP: usize = 128;

    fn leaked_c_string(text: &str) -> LkHybridDyn {
        let Ok(owned) = std::ffi::CString::new(text) else {
            // The native backend's strings are C strings end-to-end; an
            // embedded NUL is unrepresentable there, not just here.
            hybrid_die(format_args!("bridged string return contains an embedded NUL"));
        };
        LkHybridDyn {
            tag: LK_HYBRID_DYN_STR,
            payload: owned.into_raw() as i64,
        }
    }

    /// Marshal a VM result into an `LkDyn`-shaped return. Strings are copied
    /// into leaked `CString`s — native code treats every `LkDyn` string as
    /// arena-owned and never frees it, so the leak *is* the ownership model.
    /// Containers deep-convert through the injected lkrt constructor table:
    /// lists become `ListDyn` (whose bare-text display matches every VM list
    /// display except the *quoted* typed string list — that one dies until a
    /// typed-list Dyn tag exists), string-keyed maps become `MapStrDyn`
    /// replayed in the VM's iteration order (Fx-layout preserving, same
    /// discipline as chan's cross-thread deep copy).
    fn marshal_return(value: lk_core::val::RuntimeVal, state: &lk_core::vm::RuntimeModuleState) -> LkHybridDyn {
        marshal_value(value, state, 0)
    }

    fn marshal_value(
        value: lk_core::val::RuntimeVal,
        state: &lk_core::vm::RuntimeModuleState,
        depth: usize,
    ) -> LkHybridDyn {
        use lk_core::val::{HeapValue, RuntimeVal};

        if depth > MARSHAL_DEPTH_CAP {
            hybrid_die(format_args!(
                "bridged return exceeds the marshal depth cap (cyclic or absurdly nested container)"
            ));
        }
        match value {
            RuntimeVal::Nil => LkHybridDyn {
                tag: LK_HYBRID_DYN_NIL,
                payload: 0,
            },
            RuntimeVal::Bool(value) => LkHybridDyn {
                tag: LK_HYBRID_DYN_BOOL,
                payload: i64::from(value),
            },
            RuntimeVal::Int(value) => LkHybridDyn {
                tag: LK_HYBRID_DYN_I64,
                payload: value,
            },
            RuntimeVal::Float(value) => LkHybridDyn {
                tag: LK_HYBRID_DYN_F64,
                payload: value.to_bits() as i64,
            },
            RuntimeVal::ShortStr(value) => leaked_c_string(value.as_str()),
            RuntimeVal::Obj(handle) => match state.heap().get(handle) {
                Some(HeapValue::String(value)) => leaked_c_string(value.as_ref()),
                Some(HeapValue::List(list)) => marshal_list(list, state, depth),
                Some(HeapValue::Map(map)) => marshal_map(map, state, depth),
                Some(other) => hybrid_die(format_args!(
                    "bridged return kind not yet marshalable: {}",
                    other.type_name()
                )),
                None => hybrid_die(format_args!("bridged return handle is out of bounds")),
            },
        }
    }

    fn marshal_list(
        list: &lk_core::val::TypedList,
        state: &lk_core::vm::RuntimeModuleState,
        depth: usize,
    ) -> LkHybridDyn {
        // A typed string list displays *quoted* in the VM while `ListDyn`
        // displays bare (the Mixed-list quirk) — converting would silently
        // change program output, so it stays unmarshalable for now.
        if matches!(list, lk_core::val::TypedList::String(_)) {
            hybrid_die(format_args!(
                "bridged return kind not yet marshalable: List<Str> (quoted typed display)"
            ));
        }
        let rt = hybrid_rt();
        // SAFETY: the constructor table points at lkrt's no-mangle builders
        // (registered by the wrapper); handles stay arena-owned.
        unsafe {
            let handle = (rt.list_dyn_new)();
            for item in list.collect_owned() {
                let element = marshal_value(item, state, depth + 1);
                (rt.list_dyn_push)(handle, element);
            }
            LkHybridDyn {
                tag: LK_HYBRID_DYN_LIST,
                payload: handle as i64,
            }
        }
    }

    fn marshal_map(map: &lk_core::val::TypedMap, state: &lk_core::vm::RuntimeModuleState, depth: usize) -> LkHybridDyn {
        use lk_core::val::RuntimeMapKey;

        let rt = hybrid_rt();
        // SAFETY: as in `marshal_list`; keys are leaked C strings (arena).
        unsafe {
            let handle = (rt.map_str_dyn_new)();
            for (key, value) in map.entries_iter() {
                let key_text = match &key {
                    RuntimeMapKey::ShortStr(s) => s.as_str().to_string(),
                    RuntimeMapKey::String(s) => s.as_ref().to_string(),
                    other => hybrid_die(format_args!(
                        "bridged return kind not yet marshalable: map with non-string key {other:?}"
                    )),
                };
                let Ok(key_c) = std::ffi::CString::new(key_text) else {
                    hybrid_die(format_args!("bridged map key contains an embedded NUL"));
                };
                let element = marshal_value(value, state, depth + 1);
                (rt.map_str_dyn_set)(handle, key_c.into_raw(), element);
            }
            LkHybridDyn {
                tag: LK_HYBRID_DYN_MAP,
                payload: handle as i64,
            }
        }
    }

    /// Call VM-executed function `func_index` and return its result as an
    /// `LkDyn`-shaped value (v2 bridge: the lowering binds the destination
    /// register as `Dyn`). Never returns on error — see [`hybrid_die`].
    ///
    /// # Safety
    /// Same contract as [`lk_hybrid_call_v`].
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn lk_hybrid_call_r(func_index: u32, args: *const LkHybridArg, argc: usize) -> LkHybridDyn {
        unsafe { bridge_call(func_index, args, argc, true) }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The mirror must match lkrt's carrier bit-for-bit: the generated IR
        /// treats `lk_hybrid_call_r`'s return as an lkrt `LkDyn`.
        #[test]
        fn hybrid_dyn_mirrors_lkrt_carrier_and_tags() {
            assert_eq!(LK_HYBRID_DYN_NIL, lkrt::DYN_NIL);
            assert_eq!(LK_HYBRID_DYN_BOOL, lkrt::DYN_BOOL);
            assert_eq!(LK_HYBRID_DYN_I64, lkrt::DYN_I64);
            assert_eq!(LK_HYBRID_DYN_F64, lkrt::DYN_F64);
            assert_eq!(LK_HYBRID_DYN_STR, lkrt::DYN_STR);
            assert_eq!(LK_HYBRID_DYN_LIST, lkrt::DYN_LIST);
            assert_eq!(LK_HYBRID_DYN_MAP, lkrt::DYN_MAP);

            assert_eq!(core::mem::size_of::<LkHybridDyn>(), core::mem::size_of::<lkrt::LkDyn>());
            assert_eq!(
                core::mem::align_of::<LkHybridDyn>(),
                core::mem::align_of::<lkrt::LkDyn>()
            );
            let probe = LkHybridDyn { tag: 4, payload: 7 };
            // SAFETY: both are #[repr(C)] { i64, i64 } — that is the claim
            // under test; a layout drift fails the size/align asserts above.
            let mirrored: lkrt::LkDyn = unsafe { core::mem::transmute(probe) };
            assert_eq!(mirrored.tag, 4);
            assert_eq!(mirrored.payload, 7);
        }

        #[test]
        fn marshal_return_covers_the_scalar_tier() {
            use lk_core::val::RuntimeVal;
            use lk_core::vm::RuntimeModuleState;

            let mut state = RuntimeModuleState::default();
            assert_eq!(
                marshal_return(RuntimeVal::Nil, &state),
                LkHybridDyn {
                    tag: LK_HYBRID_DYN_NIL,
                    payload: 0
                }
            );
            assert_eq!(
                marshal_return(RuntimeVal::Bool(true), &state),
                LkHybridDyn {
                    tag: LK_HYBRID_DYN_BOOL,
                    payload: 1
                }
            );
            assert_eq!(
                marshal_return(RuntimeVal::Int(-42), &state),
                LkHybridDyn {
                    tag: LK_HYBRID_DYN_I64,
                    payload: -42
                }
            );
            assert_eq!(
                marshal_return(RuntimeVal::Float(2.5), &state),
                LkHybridDyn {
                    tag: LK_HYBRID_DYN_F64,
                    payload: 2.5f64.to_bits() as i64
                }
            );

            let short = RuntimeVal::ShortStr(lk_core::val::ShortStr::new("hey").expect("short"));
            let out = marshal_return(short, &state);
            assert_eq!(out.tag, LK_HYBRID_DYN_STR);
            let text = unsafe { CStr::from_ptr(out.payload as *const c_char) };
            assert_eq!(text.to_str().unwrap(), "hey");

            let long = "a-long-string-over-7-bytes";
            let handle = state
                .heap_mut()
                .alloc(lk_core::val::HeapValue::String(alloc::sync::Arc::from(long)));
            let out = marshal_return(RuntimeVal::Obj(handle), &state);
            assert_eq!(out.tag, LK_HYBRID_DYN_STR);
            let text = unsafe { CStr::from_ptr(out.payload as *const c_char) };
            assert_eq!(text.to_str().unwrap(), long);
        }
    }
}
