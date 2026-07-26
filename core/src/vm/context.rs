#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::util::fast_map::{FastHashMap, fast_hash_map_new};
use crate::vm::ModuleResolver;
use alloc::sync::Arc;

use anyhow::{Result, anyhow};

use crate::module::runtime_export_from_runtime_native;
use crate::typ::TypeChecker;
use crate::val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, ShortStr, Type, TypedMap};
use crate::vm::{
    Module, NativeArgs, NativeEntry, NativeFunction, NativeRuntime, RuntimeCallable, RuntimeExport,
    collect_runtime_export,
};

use crate::typ::{TraitDef, TraitImpl};

mod core_methods;
pub(crate) use core_methods::core_call_method_windowed;
use core_methods::{core_call_method_builtin, core_call_method_named_builtin, core_set_builtin};

/// Where a trait-impl method's body lives.
///
/// The distinction is the whole point of this table: a method declared in the
/// module being executed is addressed by index against that module, while a
/// method that arrived through an `import` must be called against the module
/// and heap it was compiled and run in.
#[derive(Debug, Clone)]
pub enum MethodImpl {
    /// A method declared by `module`, addressed by index into *its* function
    /// table.
    ///
    /// The module is carried rather than implied. A function index is only
    /// meaningful against the table it was compiled into, and this table is
    /// shared by every module running under one context. Resolving the index
    /// against whoever happens to be executing silently ran an unrelated
    /// function of the same index: an `impl` in the entry module, dispatched
    /// inside a function imported from another file, recursed into that file's
    /// function #N until the stack overflowed.
    ///
    /// Whether a cross-frame dispatch may proceed is decided from
    /// `module.type_info` on the cold path rather than carried here: this enum
    /// is cloned out of the table on *every* dynamic method call, so it stays
    /// two words (see `vm::exec::call_trait_method`).
    Local { module: Arc<Module>, function: u32 },
    /// A callable carrying its own module and runtime state. `Arc` because
    /// `RuntimeCallable` owns shared state and a cloned context must keep
    /// pointing at the same module, not a copy of it.
    Imported(Arc<RuntimeCallable>),
}

/// VM runtime context.
///
/// VM-visible globals live in `runtime_globals`; top-level locals and call
/// frames live in `RuntimeModuleState.stack`.
#[derive(Debug)]
pub struct VmContext {
    runtime_globals: FastHashMap<Arc<str>, RuntimeExport>,
    // Cache generation for invalidation
    generation: u64,
    resolver: Arc<ModuleResolver>,
    type_checker: Option<TypeChecker>,
    structs: FastHashMap<String, FastHashMap<String, Type>>,
    /// Runtime trait-method table: declaring module → type name → method name
    /// → implementation.
    ///
    /// This lives here, not in `TypeChecker`, because it is runtime data: an
    /// imported module's method must be called against *that* module's
    /// function table and heap, which the type system has no business knowing
    /// about. The type checker keeps only what it needs for checking
    /// (`method_sigs`).
    ///
    /// The outer key is what makes the table *correct* rather than merely fast.
    /// Keyed by type name alone, two modules that both declare `Point` shared
    /// one entry and the later registration silently won for both of them (see
    /// [`crate::vm::TypeScope`]). Scoping it also makes registration
    /// order-independent, which is what lets the transitive closure of loaded
    /// modules be registered wholesale without any of them clobbering another.
    ///
    /// Nested rather than tuple-keyed so a lookup borrows every part of the
    /// key: a flat map forced two `String` allocations on *every* dynamic
    /// method dispatch just to build a throwaway probe.
    methods: FastHashMap<crate::vm::TypeScope, FastHashMap<String, FastHashMap<String, MethodImpl>>>,
    /// Identity to stamp on the module compiled in this context, and therefore
    /// on every object it constructs. Set by the loader, which knows the path;
    /// the compiler does not (see [`crate::vm::TypeScope`]).
    type_scope: crate::vm::TypeScope,
    /// Which module declared each `impl Trait for <builtin>` — keyed by
    /// `(type name, trait name)`.
    ///
    /// A builtin type has no declaring module, so every module's impls for it
    /// share one scope (see [`crate::vm::TypeScope::builtin`]) and the later
    /// registration used to overwrite the earlier one *silently*: with two
    /// modules implementing `Doubler for Int`, `(5).dbl()` answered whichever
    /// was imported last, so moving a `use` line changed the result. Recording
    /// the owner turns the overlap into an error at registration.
    builtin_impl_owner: FastHashMap<(String, String), crate::vm::TypeScope>,
    call_stack: Vec<CallFrameInfo>,
    /// Per-context handle to the async (tokio) runtime. Replaces the former
    /// process-global runtime; clones (spawned tasks, shallow clones) share the
    /// same lazily-initialized reactor.
    async_runtime: crate::rt::AsyncRuntimeHandle,
}

impl Default for VmContext {
    fn default() -> Self {
        Self::new()
    }
}

/// 调用帧信息，用于错误报告。
#[derive(Debug, Clone)]
pub struct CallFrameInfo {
    pub function_name: Arc<str>,
    pub location: Option<Arc<str>>,
    pub depth: usize,
}

impl VmContext {
    /// 创建一个空上下文。
    pub fn new() -> Self {
        let mut ctx = Self::new_without_core_vm_builtins();
        ctx.type_checker = Some(TypeChecker::new());
        ctx.install_core_vm_builtins();
        ctx
    }

    /// Create an empty context without VM-only core builtins.
    ///
    /// Native compilation and low-level VM tests use this when they need only
    /// runtime-visible globals and resolver state.
    pub fn new_without_core_vm_builtins() -> Self {
        Self {
            runtime_globals: fast_hash_map_new(),
            generation: 0,
            resolver: Arc::new(ModuleResolver::default()),
            type_checker: None,
            structs: fast_hash_map_new(),
            methods: fast_hash_map_new(),
            type_scope: crate::vm::TypeScope::anonymous(),
            builtin_impl_owner: fast_hash_map_new(),
            call_stack: Vec::new(),
            async_runtime: crate::rt::AsyncRuntimeHandle::new(),
        }
    }

    /// 当前全局缓存版本。
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn shallow_clone_shared_runtime(&self) -> Self {
        let mut runtime_globals = fast_hash_map_new();
        for (name, value) in &self.runtime_globals {
            runtime_globals.insert(Arc::clone(name), value.shallow_clone_shared());
        }
        Self {
            runtime_globals,
            generation: self.generation,
            resolver: Arc::clone(&self.resolver),
            type_checker: self.type_checker.clone(),
            structs: self.structs.clone(),
            methods: self.methods.clone(),
            type_scope: self.type_scope.clone(),
            builtin_impl_owner: self.builtin_impl_owner.clone(),
            call_stack: self.call_stack.clone(),
            // Share the same async runtime so spawned tasks run on one reactor.
            async_runtime: self.async_runtime.clone(),
        }
    }

    /// Per-context async (tokio) runtime handle.
    #[inline]
    pub fn async_runtime(&self) -> &crate::rt::AsyncRuntimeHandle {
        &self.async_runtime
    }

    /// Shut down this context's async runtime if it was initialized.
    #[inline]
    pub fn shutdown_async_runtime(&self) {
        self.async_runtime.shutdown();
    }

    #[inline]
    pub fn call_stack_depth(&self) -> usize {
        self.call_stack.len()
    }

    #[inline]
    pub fn truncate_call_stack(&mut self, depth: usize) {
        if depth < self.call_stack.len() {
            self.call_stack.truncate(depth);
        }
    }

    #[inline]
    pub fn restore_generation(&mut self, generation: u64) {
        self.generation = generation;
    }

    /// 构建函数，允许自定义组件。
    pub fn with_resolver(mut self, resolver: Arc<ModuleResolver>) -> Self {
        for (name, value) in resolver.runtime_builtin_iter() {
            if self.runtime_globals.contains_key(name.as_ref()) {
                continue;
            }
            self.runtime_globals
                .insert(Arc::clone(name), value.shallow_clone_shared());
        }
        self.resolver = resolver;
        self
    }

    /// 设置类型检查器。
    pub fn with_type_checker(mut self, type_checker: Option<TypeChecker>) -> Self {
        self.type_checker = type_checker;
        self
    }

    /// Identity to stamp on the module compiled here (see
    /// [`crate::vm::TypeScope`]). The loader sets this before compiling a file
    /// module; anything else keeps the anonymous scope.
    pub fn with_type_scope(mut self, type_scope: crate::vm::TypeScope) -> Self {
        self.type_scope = type_scope;
        self
    }

    #[inline]
    pub fn type_scope(&self) -> &crate::vm::TypeScope {
        &self.type_scope
    }

    #[inline]
    pub fn runtime_globals_iter(&self) -> impl Iterator<Item = (&Arc<str>, &RuntimeExport)> {
        self.runtime_globals.iter()
    }

    pub fn collect_runtime_globals_garbage(&self) -> Result<()> {
        for export in self.runtime_globals.values() {
            collect_runtime_export(export)?;
        }
        Ok(())
    }

    pub fn get_runtime_global(&self, name: &str) -> Option<&RuntimeExport> {
        self.runtime_globals.get(name)
    }

    pub fn define_runtime_global(&mut self, name: impl Into<Arc<str>>, value: RuntimeExport) {
        let name = name.into();
        self.runtime_globals.insert(name, value);
        self.bump_generation();
    }

    pub fn define_runtime_value(&mut self, name: impl Into<Arc<str>>, value: RuntimeVal, heap: HeapStore) {
        self.define_runtime_global(name, RuntimeExport::from_value(value, heap));
    }

    /// 手动递增版本号，用于强制失效缓存。
    #[inline]
    pub fn touch(&mut self) {
        self.bump_generation();
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// 调用栈管理：进入函数调用
    pub fn push_call_frame<N, L>(&mut self, name: N, location: Option<L>)
    where
        N: Into<Arc<str>>,
        L: Into<Arc<str>>,
    {
        self.call_stack.push(CallFrameInfo {
            function_name: name.into(),
            location: location.map(Into::into),
            depth: self.call_stack.len(),
        });
    }

    /// 调用栈管理：退出函数调用
    pub fn pop_call_frame(&mut self) -> Option<CallFrameInfo> {
        self.call_stack.pop()
    }

    /// 获取当前调用栈信息
    pub fn call_stack(&self) -> &[CallFrameInfo] {
        &self.call_stack
    }

    /// 获取当前函数名
    pub fn current_function(&self) -> Option<&str> {
        self.call_stack.last().map(|frame| frame.function_name.as_ref())
    }

    /// 返回当前调用栈的格式化字符串。深栈截断打印(头 20 帧 + 尾 10 帧):
    /// 递归打满调用深度上限时,完整 traceback 会有几十万行,淹没真正的错误。
    pub fn call_stack_report(&self) -> Option<String> {
        const HEAD_FRAMES: usize = 20;
        const TAIL_FRAMES: usize = 10;
        if self.call_stack.is_empty() {
            return None;
        }
        fn push_frame(msg: &mut String, frame: &CallFrameInfo) {
            msg.push_str("  [");
            msg.push_str(&frame.depth.to_string());
            msg.push_str("] ");
            msg.push_str(frame.function_name.as_ref());
            if let Some(location) = frame.location.as_ref() {
                msg.push_str(" at ");
                msg.push_str(location.as_ref());
            }
            msg.push('\n');
        }
        let mut msg = String::from("Call stack:\n");
        let total = self.call_stack.len();
        if total <= HEAD_FRAMES + TAIL_FRAMES {
            for frame in self.call_stack.iter().rev() {
                push_frame(&mut msg, frame);
            }
        } else {
            for frame in self.call_stack.iter().rev().take(HEAD_FRAMES) {
                push_frame(&mut msg, frame);
            }
            msg.push_str("  … ");
            msg.push_str(&(total - HEAD_FRAMES - TAIL_FRAMES).to_string());
            msg.push_str(" frames elided …\n");
            for frame in self.call_stack.iter().take(TAIL_FRAMES).rev() {
                push_frame(&mut msg, frame);
            }
        }
        Some(msg)
    }

    /// 生成增强的错误信息，包含调用栈上下文
    pub fn format_error_with_context(&self, error_message: &str) -> String {
        if let Some(report) = self.call_stack_report() {
            let mut msg = error_message.to_string();
            msg.push_str("\n\n");
            msg.push_str(&report);
            msg
        } else {
            error_message.to_string()
        }
    }

    /// 获取模块解析器的引用
    pub fn resolver(&self) -> &Arc<ModuleResolver> {
        &self.resolver
    }

    /// 获取类型检查器的引用
    pub fn type_checker(&self) -> &Option<TypeChecker> {
        &self.type_checker
    }

    /// 获取结构体定义的引用
    pub fn structs(&self) -> &FastHashMap<String, FastHashMap<String, Type>> {
        &self.structs
    }

    /// 获取类型检查器的可变引用
    pub fn get_type_checker_mut(&mut self) -> Option<&mut TypeChecker> {
        self.type_checker.as_mut()
    }

    /// 注册结构体模式
    pub fn register_struct_schema(&mut self, name: String, fields: FastHashMap<String, Type>) {
        self.structs.insert(name, fields);
    }

    fn install_core_vm_builtins(&mut self) {
        self.install_runtime_builtin(
            "__lk_call_method",
            NativeFunction::FullState(core_call_method_builtin),
            3,
        );
        self.install_runtime_builtin(
            "__lk_call_method_named",
            NativeFunction::FullState(core_call_method_named_builtin),
            4,
        );
        self.install_runtime_builtin("__lk_make_struct", NativeFunction::Plain(core_make_struct_builtin), 2);
        self.install_runtime_builtin("typeof", NativeFunction::Plain(core_typeof_builtin), 1);
        self.install_runtime_builtin("Set", NativeFunction::Plain(core_set_builtin), NativeEntry::VARIADIC);
        self.install_runtime_builtin("__lk_set_field", NativeFunction::Plain(core_set_field_builtin), 3);
        self.install_runtime_builtin("__lk_merge_fields", NativeFunction::Plain(core_merge_fields_builtin), 2);
        // CPU control. Meaningless under the interpreter for the same reason
        // as MMIO: there is no core to mask interrupts on, and a barrier
        // orders accesses the VM never makes.
        self.install_runtime_builtin("cpu_barrier", NativeFunction::Plain(core_cpu_unavailable_builtin), 0);
        self.install_runtime_builtin(
            "cpu_compiler_barrier",
            NativeFunction::Plain(core_cpu_unavailable_builtin),
            0,
        );
        self.install_runtime_builtin("cpu_irq_save", NativeFunction::Plain(core_cpu_unavailable_builtin), 0);
        self.install_runtime_builtin(
            "cpu_irq_restore",
            NativeFunction::Plain(core_cpu_unavailable_builtin),
            1,
        );
        self.install_runtime_builtin(
            "cpu_wait_for_interrupt",
            NativeFunction::Plain(core_cpu_unavailable_builtin),
            0,
        );
        // Volatile MMIO access. The bytecode VM has no address space, so
        // these exist only to *fail loudly* there — a driver reading a
        // register under the interpreter must not get a plausible zero.
        // The AOT path lowers them to real loads and stores instead of
        // calling these.
        self.install_runtime_builtin(
            "volatile_read_u8",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            1,
        );
        self.install_runtime_builtin(
            "volatile_write_u8",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            2,
        );
        self.install_runtime_builtin(
            "volatile_read_u16",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            1,
        );
        self.install_runtime_builtin(
            "volatile_write_u16",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            2,
        );
        self.install_runtime_builtin(
            "volatile_read_u32",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            1,
        );
        self.install_runtime_builtin(
            "volatile_write_u32",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            2,
        );
        self.install_runtime_builtin(
            "volatile_read_u64",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            1,
        );
        self.install_runtime_builtin(
            "volatile_write_u64",
            NativeFunction::Plain(core_volatile_unavailable_builtin),
            2,
        );
        self.install_runtime_builtin("__lk_bit_and", NativeFunction::Plain(core_bit_and_builtin), 2);
        self.install_runtime_builtin("__lk_bit_or", NativeFunction::Plain(core_bit_or_builtin), 2);
        self.install_runtime_builtin("__lk_bit_not", NativeFunction::Plain(core_bit_not_builtin), 1);
    }

    /// Looks up a trait-impl method for the type `type_name` **as declared by
    /// `scope`**.
    ///
    /// The scope is not optional and there is deliberately no name-only
    /// fallback: falling back would re-admit exactly the cross-module
    /// collision this key exists to prevent, and would do it silently.
    pub fn trait_method(&self, scope: &crate::vm::TypeScope, type_name: &str, method: &str) -> Option<&MethodImpl> {
        self.methods.get(scope)?.get(type_name)?.get(method)
    }

    /// Records `decl` as the owner of `impl <trait> for <builtin type>`, or fails
    /// if a *different* module already owns it.
    ///
    /// Only builtin targets need this: a declared type is scoped to its own
    /// module, so two modules' `Point` impls never meet. `Int`/`List`/… have no
    /// declaring module, so the pair is global and admits exactly one impl.
    /// Re-registering the same module (REPL, hybrid bridge, transitive imports)
    /// is fine — it is the *same* owner.
    fn claim_builtin_impl(
        &mut self,
        scope: &crate::vm::TypeScope,
        declaring: &crate::vm::TypeScope,
        type_name: &str,
        trait_name: &str,
    ) -> Result<()> {
        if !scope.is_builtin() {
            return Ok(());
        }
        let key = (type_name.to_string(), trait_name.to_string());
        match self.builtin_impl_owner.get(&key) {
            Some(owner) if owner != declaring => Err(anyhow!(
                "conflicting `impl {trait_name} for {type_name}`: already implemented by {}, now by {}. \
                 A builtin type has no declaring module, so one trait can be implemented for it only once \
                 — otherwise which one runs depends on import order",
                owner.as_str(),
                declaring.as_str()
            )),
            _ => {
                self.builtin_impl_owner.insert(key, declaring.clone());
                Ok(())
            }
        }
    }

    /// Records an imported module's trait impls, bound to *that* module's
    /// function table and heap.
    ///
    /// Without this an `impl` in an imported file was simply invisible: the
    /// importer executed the file in a throwaway `VmContext` and kept only its
    /// exported values, so `use { make } from "./shape.lk"; make(4).area()`
    /// failed with "Object has no method 'area'".
    pub fn register_imported_types(&mut self, export: &RuntimeExport) -> Result<()> {
        let module = export.shared_module();
        if module.type_info.is_empty() {
            return Ok(());
        }
        for decl in &module.type_info.impls {
            let scope = impl_target_scope(&decl.type_name, &module.type_scope);
            self.claim_builtin_impl(&scope, &module.type_scope, &decl.type_name, &decl.trait_name)?;
            let by_method = self
                .methods
                .entry(scope)
                .or_default()
                .entry(decl.type_name.clone())
                .or_default();
            for method in &decl.methods {
                by_method.insert(
                    method.name.clone(),
                    MethodImpl::Imported(Arc::new(RuntimeCallable::with_shared_captures(
                        Arc::clone(&module),
                        method.function,
                        Arc::new(Vec::new()),
                        export.shared_state(),
                    ))),
                );
            }
        }
        Ok(())
    }

    /// Populates the runtime method table from the module's compiled
    /// declarations.
    ///
    /// This replaces the previous scheme, where the compiler emitted
    /// `__lk_register_trait_impl` calls that the entry function executed to
    /// build the same table from string literals and closures. Reading
    /// [`crate::vm::TypeInfo`] instead means the table is available before any
    /// user code runs, needs no bytecode, and holds no heap handles — the
    /// registry was never a GC root, so storing closures there was only safe
    /// while they happened to still be live in a register.
    pub fn register_module_types(&mut self, module: &Arc<Module>) -> anyhow::Result<()> {
        let type_info = &module.type_info;
        if type_info.is_empty() {
            return Ok(());
        }
        // Coherence first, before *any* registry mutation: `register_trait_impl`
        // below writes into the checker, so claiming ownership from the dispatch
        // loop afterwards left a rejected module half-registered.
        for decl in &type_info.impls {
            let scope = impl_target_scope(&decl.type_name, &module.type_scope);
            self.claim_builtin_impl(&scope, &module.type_scope, &decl.type_name, &decl.trait_name)?;
        }
        // Checker registration only happens when there *is* a checker; the
        // dispatch table below is unconditional. Returning early without one
        // used to skip both, so a context built without a type checker could
        // execute a module whose own `impl` methods were undispatchable.
        if let Some(checker) = self.type_checker.as_mut() {
            for decl in &type_info.traits {
                let methods = decl
                    .methods
                    .iter()
                    // A signature this build cannot parse degrades to the top
                    // type rather than vanishing: dropping the entry would
                    // hide the method from `validate_trait_impl`, so an `impl`
                    // that never defines it would validate clean. Same choice
                    // as the impl loop below, which keeps the method with a
                    // `None` type.
                    .map(|(name, ty)| (name.clone(), Type::parse(ty).unwrap_or(Type::Any)))
                    .collect();
                checker.registry_mut().register_trait(TraitDef {
                    name: decl.name.clone(),
                    methods,
                });
            }
            for decl in &type_info.impls {
                let target_type = Type::parse(&decl.type_name)
                    .ok_or_else(|| anyhow!("failed to parse impl target type '{}'", decl.type_name))?;
                let methods = decl
                    .methods
                    .iter()
                    .map(|method| (method.name.clone(), (method.function, Type::parse(&method.ty))))
                    .collect();
                let impl_def = TraitImpl {
                    trait_name: decl.trait_name.clone(),
                    target_type,
                    methods,
                };
                checker.registry().validate_trait_impl(&impl_def)?;
                checker.registry_mut().register_trait_impl(impl_def);
            }
        }
        // The dispatch table itself, each entry bound to the module that
        // declared it (see `MethodImpl::Local`) and filed under that module's
        // scope, so an identically-named type elsewhere keeps its own entry.
        for decl in &type_info.impls {
            let scope = impl_target_scope(&decl.type_name, &module.type_scope);
            let by_method = self
                .methods
                .entry(scope)
                .or_default()
                .entry(decl.type_name.clone())
                .or_default();
            for method in &decl.methods {
                by_method.insert(
                    method.name.clone(),
                    MethodImpl::Local {
                        module: Arc::clone(module),
                        function: method.function,
                    },
                );
            }
        }
        Ok(())
    }

    fn install_runtime_builtin(&mut self, name: &str, function: NativeFunction, arity: u16) {
        if self.runtime_globals.contains_key(name) {
            return;
        }
        let value = runtime_export_from_runtime_native(name, function, arity);
        self.runtime_globals.insert(Arc::<str>::from(name), value);
    }
}

/// Which scope an `impl` for `target_type` is filed under.
///
/// A user-declared type (`Type::Named`) belongs to the module that declared it;
/// anything else is a builtin, shared by every module (see
/// [`crate::vm::TypeScope::builtin`]). An unparseable target is treated as
/// declared — the conservative side, since filing it under the builtin scope
/// would let it collide with every other module's.
fn impl_target_scope(target_type: &str, declaring: &crate::vm::TypeScope) -> crate::vm::TypeScope {
    match Type::parse(target_type) {
        // A user *generic* (`Wrapper<Int>` → `Type::Generic`) is as module-local
        // as a plain `Named`: two modules may each declare their own `Wrapper`.
        // Lumping it in with the builtins made them share one coherence key and
        // conflict with each other.
        Some(Type::Named(_)) | Some(Type::Generic { .. }) | None => declaring.clone(),
        Some(_) => crate::vm::TypeScope::builtin(),
    }
}

/// The scope to dispatch `receiver`'s methods in: its own, if it is a declared
/// type; the builtin scope otherwise. Only a heap `Object` carries a declared
/// type — every other receiver is an `Int`, a `List`, a string, and so on.
pub fn receiver_type_scope(receiver: &RuntimeVal, heap: &HeapStore) -> crate::vm::TypeScope {
    if let RuntimeVal::Obj(handle) = receiver
        && let Some(HeapValue::Object(object)) = heap.get(*handle)
    {
        return object.type_scope().clone();
    }
    crate::vm::TypeScope::builtin()
}

fn core_make_struct_builtin(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> anyhow::Result<RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!(
            "__lk_make_struct expects 2 arguments: struct name and fields map"
        ));
    }

    let type_name = runtime_string_arg(args.get(0).expect("arity checked"), runtime.heap(), "__lk_make_struct")?;
    // The type belongs to the module running this construction: a struct
    // literal can only name a type declared in its own compilation unit (an
    // imported struct is neither constructible nor nameable), so "who is
    // executing" and "who declared it" are the same module here.
    let type_scope = runtime
        .module()
        .map(|module| module.type_scope.clone())
        .unwrap_or_default();

    let fields = match args.get(1).expect("arity checked") {
        RuntimeVal::Nil => fast_hash_map_new(),
        RuntimeVal::Obj(handle) => {
            let value = runtime
                .heap()
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            let HeapValue::Map(map) = value else {
                return Err(anyhow!(
                    "__lk_make_struct expects fields as map, got {}",
                    value.type_name()
                ));
            };
            runtime_object_fields_from_map(map)?
        }
        other => {
            return Err(anyhow!(
                "__lk_make_struct expects fields as map, got {:?}",
                other.kind()
            ));
        }
    };

    let ty = Arc::new(crate::vm::DeclaredType::new(type_scope, type_name));
    Ok(RuntimeVal::Obj(
        runtime
            .heap_mut()
            .alloc(HeapValue::Object(RuntimeObject::new(ty, fields))),
    ))
}

fn core_typeof_builtin(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> anyhow::Result<RuntimeVal> {
    let value = args
        .get(0)
        .ok_or_else(|| anyhow!("typeof(value) expects exactly one argument"))?;
    let name = match value {
        RuntimeVal::Int(_) => "Int",
        RuntimeVal::Float(_) => "Float",
        RuntimeVal::Bool(_) => "Bool",
        RuntimeVal::ShortStr(_) => "String",
        RuntimeVal::Nil => "Nil",
        RuntimeVal::Obj(handle) => runtime
            .heap()
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            .type_name(),
    };
    Ok(runtime_string_value(name, runtime.heap_mut()))
}

fn core_set_field_builtin(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> anyhow::Result<RuntimeVal> {
    if args.len() != 3 {
        return Err(anyhow!("__lk_set_field(base, key, value) expects exactly 3 arguments"));
    }
    let base = *args.get(0).expect("arity checked");
    let key = runtime_string_arg(args.get(1).expect("arity checked"), runtime.heap(), "__lk_set_field")?;
    let field_value = *args.get(2).expect("arity checked");
    match base {
        RuntimeVal::Obj(handle) => {
            let updated = match runtime
                .heap()
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::Map(map) => HeapValue::Map(set_string_field_on_map(map, key, field_value)),
                HeapValue::Object(object) => HeapValue::Object(set_string_field_on_object(object, key, field_value)),
                other => Err(anyhow!(
                    "__lk_set_field target must be Map or Object, got {}",
                    other.type_name()
                ))?,
            };
            Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(updated)))
        }
        other => Err(anyhow!(
            "__lk_set_field target must be Map or Object, got {:?}",
            other.kind()
        )),
    }
}

fn core_merge_fields_builtin(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> anyhow::Result<RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_merge_fields(base, overlay) expects exactly 2 arguments"));
    }

    let base = match args.get(0).expect("arity checked") {
        RuntimeVal::Obj(handle) => {
            let value = runtime
                .heap()
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            match value {
                HeapValue::Object(object) => Some(FieldMergeBase::Object(object)),
                HeapValue::Map(map) => Some(FieldMergeBase::Map(map)),
                other => {
                    return Err(anyhow!(
                        "__lk_merge_fields base must be Object, Map, or Nil, got {}",
                        other.type_name()
                    ));
                }
            }
        }
        RuntimeVal::Nil => None,
        other => {
            return Err(anyhow!(
                "__lk_merge_fields base must be Object, Map, or Nil, got {:?}",
                other.kind()
            ));
        }
    };

    match args.get(1).expect("arity checked") {
        RuntimeVal::Obj(handle) => {
            let value = runtime
                .heap()
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            let HeapValue::Map(overlay) = value else {
                return Err(anyhow!(
                    "__lk_merge_fields overlay must be Map, got {}",
                    value.type_name()
                ));
            };
            let fields = match base {
                Some(base) => merge_field_maps(base, overlay),
                None => copy_typed_map(overlay),
            };
            Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(fields))))
        }
        other => Err(anyhow!("__lk_merge_fields overlay must be Map, got {:?}", other.kind())),
    }
}

fn set_string_field_on_object(object: &RuntimeObject, key: Arc<str>, value: RuntimeVal) -> RuntimeObject {
    let mut fields = fast_hash_map_new();
    for (field_key, field_value) in &object.fields {
        if field_key.as_ref() != key.as_ref() {
            fields.insert(Arc::clone(field_key), *field_value);
        }
    }
    fields.insert(Arc::clone(&key), value);

    let mut field_slots = object.field_slots.clone();
    if !field_slots.iter().any(|field_key| field_key.as_ref() == key.as_ref()) {
        field_slots.push(key);
    }

    RuntimeObject {
        // Setting a field produces the same object with one value replaced —
        // same type, so the identity is shared, not rebuilt.
        ty: Arc::clone(&object.ty),
        fields,
        field_slots,
    }
}

fn set_string_field_on_map(map: &TypedMap, key: Arc<str>, value: RuntimeVal) -> TypedMap {
    match (map, value) {
        (TypedMap::Mixed(entries), value) => {
            let runtime_key = RuntimeMapKey::String(key);
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if *entry_key != runtime_key {
                    out.insert(entry_key.clone(), *entry_value);
                }
            }
            out.insert(runtime_key, value);
            TypedMap::Mixed(out)
        }
        (TypedMap::StringMixed(entries), value) => {
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), *entry_value);
                }
            }
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
        (TypedMap::StringInt(entries), RuntimeVal::Int(value)) => {
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), *entry_value);
                }
            }
            out.insert(key, value);
            TypedMap::StringInt(out)
        }
        (TypedMap::StringFloat(entries), RuntimeVal::Float(value)) => {
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), *entry_value);
                }
            }
            out.insert(key, value);
            TypedMap::StringFloat(out)
        }
        (TypedMap::StringBool(entries), RuntimeVal::Bool(value)) => {
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), *entry_value);
                }
            }
            out.insert(key, value);
            TypedMap::StringBool(out)
        }
        (TypedMap::StringInt(entries), value) => {
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), RuntimeVal::Int(*entry_value));
                }
            }
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
        (TypedMap::StringFloat(entries), value) => {
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), RuntimeVal::Float(*entry_value));
                }
            }
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
        (TypedMap::StringBool(entries), value) => {
            let mut out = fast_hash_map_new();
            for (entry_key, entry_value) in entries {
                if entry_key.as_ref() != key.as_ref() {
                    out.insert(Arc::clone(entry_key), RuntimeVal::Bool(*entry_value));
                }
            }
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
    }
}

enum FieldMergeBase<'a> {
    Object(&'a RuntimeObject),
    Map(&'a TypedMap),
}

fn merge_field_maps(base: FieldMergeBase<'_>, overlay: &TypedMap) -> TypedMap {
    match base {
        FieldMergeBase::Object(object) => {
            let mut entries = fast_hash_map_new();
            for (key, value) in &object.fields {
                if !typed_map_contains_str(overlay, key.as_ref()) {
                    entries.insert(Arc::clone(key), *value);
                }
            }
            let mut out = TypedMap::StringMixed(entries);
            extend_typed_map(&mut out, overlay);
            out
        }
        FieldMergeBase::Map(map) => {
            let mut out = copy_typed_map_without_overlay_keys(map, overlay);
            extend_typed_map(&mut out, overlay);
            out
        }
    }
}

fn copy_typed_map(map: &TypedMap) -> TypedMap {
    match map {
        TypedMap::Mixed(entries) => {
            let mut out = fast_hash_map_new();
            for (key, value) in entries {
                out.insert(key.clone(), *value);
            }
            TypedMap::Mixed(out)
        }
        TypedMap::StringMixed(entries) => {
            let mut out = fast_hash_map_new();
            for (key, value) in entries {
                out.insert(Arc::clone(key), *value);
            }
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(entries) => TypedMap::StringInt(copy_string_map_entries(entries)),
        TypedMap::StringFloat(entries) => TypedMap::StringFloat(copy_string_map_entries(entries)),
        TypedMap::StringBool(entries) => TypedMap::StringBool(copy_string_map_entries(entries)),
    }
}

fn copy_typed_map_without_overlay_keys(map: &TypedMap, overlay: &TypedMap) -> TypedMap {
    match map {
        TypedMap::Mixed(entries) => {
            let mut out = fast_hash_map_new();
            for (key, value) in entries {
                if !typed_map_contains(overlay, key) {
                    out.insert(key.clone(), *value);
                }
            }
            TypedMap::Mixed(out)
        }
        TypedMap::StringMixed(entries) => {
            let mut out = fast_hash_map_new();
            for (key, value) in entries {
                if !typed_map_contains_str(overlay, key.as_ref()) {
                    out.insert(Arc::clone(key), *value);
                }
            }
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(entries) => {
            TypedMap::StringInt(copy_string_map_entries_without_overlay_keys(entries, overlay))
        }
        TypedMap::StringFloat(entries) => {
            TypedMap::StringFloat(copy_string_map_entries_without_overlay_keys(entries, overlay))
        }
        TypedMap::StringBool(entries) => {
            TypedMap::StringBool(copy_string_map_entries_without_overlay_keys(entries, overlay))
        }
    }
}

fn copy_string_map_entries<T: Copy>(entries: &FastHashMap<Arc<str>, T>) -> FastHashMap<Arc<str>, T> {
    let mut out = fast_hash_map_new();
    for (key, value) in entries {
        out.insert(Arc::clone(key), *value);
    }
    out
}

fn copy_string_map_entries_without_overlay_keys<T: Copy>(
    entries: &FastHashMap<Arc<str>, T>,
    overlay: &TypedMap,
) -> FastHashMap<Arc<str>, T> {
    let mut out = fast_hash_map_new();
    for (key, value) in entries {
        if !typed_map_contains_str(overlay, key.as_ref()) {
            out.insert(Arc::clone(key), *value);
        }
    }
    out
}

fn typed_map_contains(map: &TypedMap, key: &RuntimeMapKey) -> bool {
    match map {
        TypedMap::Mixed(entries) => entries.contains_key(key),
        TypedMap::StringMixed(entries) => key.as_str().is_some_and(|key| entries.contains_key(key)),
        TypedMap::StringInt(entries) => key.as_str().is_some_and(|key| entries.contains_key(key)),
        TypedMap::StringFloat(entries) => key.as_str().is_some_and(|key| entries.contains_key(key)),
        TypedMap::StringBool(entries) => key.as_str().is_some_and(|key| entries.contains_key(key)),
    }
}

fn typed_map_contains_str(map: &TypedMap, key: &str) -> bool {
    match map {
        TypedMap::Mixed(entries) => {
            ShortStr::new(key).is_some_and(|key| entries.contains_key(&RuntimeMapKey::ShortStr(key)))
                || entries.contains_key(&RuntimeMapKey::String(Arc::<str>::from(key)))
        }
        TypedMap::StringMixed(entries) => entries.contains_key(key),
        TypedMap::StringInt(entries) => entries.contains_key(key),
        TypedMap::StringFloat(entries) => entries.contains_key(key),
        TypedMap::StringBool(entries) => entries.contains_key(key),
    }
}

fn runtime_string_arg(value: &RuntimeVal, heap: &HeapStore, func: &str) -> anyhow::Result<Arc<str>> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(Arc::<str>::from(value.as_str())),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(value.clone()),
            other => Err(anyhow!("{func} expects string argument, got {}", other.type_name())),
        },
        other => Err(anyhow!("{func} expects string argument, got {:?}", other.kind())),
    }
}

fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(short) = crate::val::ShortStr::new(value) {
        RuntimeVal::ShortStr(short)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}

fn runtime_object_fields_from_map(map: &TypedMap) -> anyhow::Result<FastHashMap<Arc<str>, RuntimeVal>> {
    let mut fields = fast_hash_map_new();
    match map {
        TypedMap::Mixed(entries) => {
            for (key, value) in entries {
                let Some(key) = key.as_arc_str() else {
                    return Err(anyhow!("__lk_make_struct field keys must be strings"));
                };
                fields.insert(key, *value);
            }
        }
        TypedMap::StringMixed(entries) => {
            fields.extend(entries.iter().map(|(key, value)| (key.clone(), *value)));
        }
        TypedMap::StringInt(entries) => {
            fields.extend(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), RuntimeVal::Int(*value))),
            );
        }
        TypedMap::StringFloat(entries) => {
            fields.extend(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), RuntimeVal::Float(*value))),
            );
        }
        TypedMap::StringBool(entries) => {
            fields.extend(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), RuntimeVal::Bool(*value))),
            );
        }
    }
    Ok(fields)
}

fn extend_typed_map(out: &mut TypedMap, map: &TypedMap) {
    match map {
        TypedMap::Mixed(entries) => {
            for (key, value) in entries {
                out.set(key.clone(), *value);
            }
        }
        TypedMap::StringMixed(entries) => {
            for (key, value) in entries {
                out.set(RuntimeMapKey::String(key.clone()), *value);
            }
        }
        TypedMap::StringInt(entries) => {
            for (key, value) in entries {
                out.set(RuntimeMapKey::String(key.clone()), RuntimeVal::Int(*value));
            }
        }
        TypedMap::StringFloat(entries) => {
            for (key, value) in entries {
                out.set(RuntimeMapKey::String(key.clone()), RuntimeVal::Float(*value));
            }
        }
        TypedMap::StringBool(entries) => {
            for (key, value) in entries {
                out.set(RuntimeMapKey::String(key.clone()), RuntimeVal::Bool(*value));
            }
        }
    }
}

fn bit_arg(value: &crate::val::RuntimeVal, func: &str) -> anyhow::Result<i64> {
    match value {
        crate::val::RuntimeVal::Int(i) => Ok(*i),
        other => Err(anyhow!("{func} expects Int arguments, got {:?}", other.kind())),
    }
}

fn core_bit_and_builtin(
    args: NativeArgs<'_>,
    _runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_bit_and(left, right) expects exactly 2 arguments"));
    }
    Ok(crate::val::RuntimeVal::Int(
        bit_arg(args.get(0).expect("arity checked"), "__lk_bit_and")?
            & bit_arg(args.get(1).expect("arity checked"), "__lk_bit_and")?,
    ))
}

fn core_bit_or_builtin(
    args: NativeArgs<'_>,
    _runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_bit_or(left, right) expects exactly 2 arguments"));
    }
    Ok(crate::val::RuntimeVal::Int(
        bit_arg(args.get(0).expect("arity checked"), "__lk_bit_or")?
            | bit_arg(args.get(1).expect("arity checked"), "__lk_bit_or")?,
    ))
}

fn core_bit_not_builtin(
    args: NativeArgs<'_>,
    _runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    if args.len() != 1 {
        return Err(anyhow!("__lk_bit_not(value) expects exactly 1 argument"));
    }
    Ok(crate::val::RuntimeVal::Int(!bit_arg(
        args.get(0).expect("arity checked"),
        "__lk_bit_not",
    )?))
}

/// The VM side of the `cpu_*` intrinsics.
///
/// Raises for the same reason the volatile ones do: under the interpreter there
/// is no core whose interrupts could be masked, and a barrier would order
/// accesses that are not happening. Silently succeeding would let a driver's
/// critical section "work" on the VM and then race on hardware.
fn core_cpu_unavailable_builtin(
    _args: NativeArgs<'_>,
    _runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    Err(anyhow!(
        "CPU control (barriers, interrupt masking, wait-for-interrupt) requires native execution; \
         the bytecode VM has no core to apply it to. Compile with the AOT backend to run this."
    ))
}

/// The VM side of `volatile_read_*` / `volatile_write_*`.
///
/// Always raises. A raw pointer under the bytecode VM is an address with
/// nothing behind it: there is no honest value to return, and returning a
/// plausible one would turn a "this cannot run here" into a wrong answer that
/// looks right. The AOT backend never calls this — it lowers these builtins to
/// machine loads and stores.
fn core_volatile_unavailable_builtin(
    _args: NativeArgs<'_>,
    _runtime: &mut NativeRuntime<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    Err(anyhow!(
        "volatile memory access requires native execution: the bytecode VM has no address space \
         behind a raw pointer. Compile with the AOT backend to run this."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::fast_map::fast_hash_map_from_iter;
    use crate::vm::{Module, RuntimeModuleState};

    fn module_with_impl(type_name: &str, method: &str, function: u32) -> Arc<Module> {
        scoped_module_with_impl(crate::vm::TypeScope::anonymous(), type_name, method, function)
    }

    fn scoped_module_with_impl(
        type_scope: crate::vm::TypeScope,
        type_name: &str,
        method: &str,
        function: u32,
    ) -> Arc<Module> {
        Arc::new(Module {
            type_scope,
            type_info: crate::vm::TypeInfo {
                traits: vec![crate::vm::TraitDecl {
                    name: "Area".to_string(),
                    methods: vec![(method.to_string(), "Function".to_string())],
                }],
                impls: vec![crate::vm::ImplDecl {
                    trait_name: "Area".to_string(),
                    type_name: type_name.to_string(),
                    methods: vec![crate::vm::ImplMethod {
                        name: method.to_string(),
                        function,
                        ty: "Function".to_string(),
                        writes_globals: false,
                        reads_globals: Vec::new(),
                    }],
                }],
            },
            ..Module::default()
        })
    }

    #[test]
    fn dispatch_entry_records_the_module_that_declared_the_impl() {
        // A function index means nothing without the table it indexes, so the
        // entry has to name the module it indexes into. Re-registering the
        // *same* scope replaces the entry (the REPL and the hybrid bridge both
        // do it); it must still point at the module it came from.
        let scope = crate::vm::TypeScope::from_path("a.lk");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let first = scoped_module_with_impl(scope.clone(), "Sq", "area", 3);
        ctx.register_module_types(&first).expect("register first module");

        let Some(MethodImpl::Local { module, function, .. }) = ctx.trait_method(&scope, "Sq", "area") else {
            panic!("a locally declared impl registers as `Local`");
        };
        assert_eq!(*function, 3);
        assert!(Arc::ptr_eq(module, &first));

        let second = scoped_module_with_impl(scope.clone(), "Sq", "area", 9);
        ctx.register_module_types(&second).expect("register second module");
        let Some(MethodImpl::Local { module, function, .. }) = ctx.trait_method(&scope, "Sq", "area") else {
            panic!("still `Local`");
        };
        assert_eq!(*function, 9);
        assert!(Arc::ptr_eq(module, &second), "the entry follows its declaring module");
    }

    #[test]
    fn same_type_name_in_two_modules_keeps_two_entries() {
        // `struct Point` in `a.lk` and in `b.lk` are different types. Keyed by
        // the bare name they shared one slot and the later registration won for
        // both, so `a`'s value ran `b`'s method body (see `vm::TypeScope`).
        let a = crate::vm::TypeScope::from_path("a.lk");
        let b = crate::vm::TypeScope::from_path("b.lk");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let from_a = scoped_module_with_impl(a.clone(), "Point", "tag", 3);
        let from_b = scoped_module_with_impl(b.clone(), "Point", "tag", 9);
        ctx.register_module_types(&from_a).expect("register a");
        ctx.register_module_types(&from_b).expect("register b");

        let Some(MethodImpl::Local { module, function, .. }) = ctx.trait_method(&a, "Point", "tag") else {
            panic!("a's impl survives b's registration");
        };
        assert_eq!(*function, 3, "a's value must not reach b's body");
        assert!(Arc::ptr_eq(module, &from_a));

        let Some(MethodImpl::Local { function, .. }) = ctx.trait_method(&b, "Point", "tag") else {
            panic!("b's impl is registered too");
        };
        assert_eq!(*function, 9);
    }

    #[test]
    fn an_impl_on_a_builtin_type_is_not_scoped_to_its_module() {
        // `impl Doubler for Int` has no declaring module to be scoped to — the
        // receiver is a bare `5` — so it is filed under the shared builtin
        // scope and found from anywhere.
        let declaring = crate::vm::TypeScope::from_path("a.lk");
        let mut ctx = VmContext::new_without_core_vm_builtins();
        ctx.register_module_types(&scoped_module_with_impl(declaring.clone(), "Int", "dbl", 2))
            .expect("register");
        assert!(
            ctx.trait_method(&declaring, "Int", "dbl").is_none(),
            "a builtin target does not belong to the declaring module's scope"
        );
        assert!(matches!(
            ctx.trait_method(&crate::vm::TypeScope::builtin(), "Int", "dbl"),
            Some(MethodImpl::Local { function: 2, .. })
        ));
    }

    #[test]
    fn registering_the_same_module_twice_does_not_grow_the_type_registry() {
        // The hybrid bridge and the REPL both reuse one context for the life of
        // the process; re-registration must be idempotent or the impl list
        // grows once per call.
        let mut ctx = VmContext::new();
        let module = module_with_impl("Sq", "area", 0);
        for _ in 0..64 {
            ctx.register_module_types(&module).expect("register");
        }
        let checker = ctx.type_checker.as_ref().expect("checker present");
        let target = crate::val::Type::Named("Sq".to_string());
        assert!(checker.registry().implements_trait(&target, "Area"));
        assert_eq!(
            checker.registry().trait_impl_count(&target),
            1,
            "re-registering an impl must replace it, not stack another copy"
        );
    }

    #[test]
    fn dispatch_table_is_populated_without_a_type_checker() {
        // `new_without_core_vm_builtins` (goroutine fallback, low-level tests)
        // has no checker. Returning early on that used to skip the dispatch
        // table too, silently making every trait method unreachable.
        let mut ctx = VmContext::new_without_core_vm_builtins();
        assert!(ctx.type_checker.is_none());
        ctx.register_module_types(&module_with_impl("Sq", "area", 1))
            .expect("register without a checker");
        assert!(matches!(
            ctx.trait_method(&crate::vm::TypeScope::anonymous(), "Sq", "area"),
            Some(MethodImpl::Local { function: 1, .. })
        ));
    }

    #[test]
    fn deep_call_stack_report_is_truncated() {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        for i in 0..100 {
            ctx.push_call_frame(format!("f{i}"), None::<String>);
        }
        let report = ctx.call_stack_report().expect("frames present");
        assert!(report.contains("70 frames elided"), "unexpected report:\n{report}");
        assert!(report.lines().count() < 40, "report must stay short:\n{report}");
        // Innermost and outermost frames both survive the truncation.
        assert!(report.contains("f99"));
        assert!(report.contains("f0"));
    }

    #[test]
    fn shallow_call_stack_report_is_complete() {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        for i in 0..5 {
            ctx.push_call_frame(format!("g{i}"), None::<String>);
        }
        let report = ctx.call_stack_report().expect("frames present");
        assert!(!report.contains("elided"));
        assert_eq!(report.lines().count(), 6, "header + 5 frames:\n{report}");
    }

    #[test]
    fn collect_runtime_globals_garbage_keeps_export_values_and_globals() {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let mut heap = HeapStore::new();
        let exported = heap.alloc(HeapValue::String(Arc::<str>::from("exported")));
        let global = heap.alloc(HeapValue::String(Arc::<str>::from("global")));
        let dead = heap.alloc(HeapValue::String(Arc::<str>::from("dead")));
        ctx.define_runtime_global(
            "module",
            RuntimeExport::new(
                RuntimeVal::Obj(exported),
                Arc::new(crate::compat::sync::Mutex::new(RuntimeModuleState::new(
                    heap,
                    vec![RuntimeVal::Obj(global)],
                ))),
                Arc::new(Module::default()),
            ),
        );

        ctx.collect_runtime_globals_garbage().expect("collect globals");
        let export = ctx.get_runtime_global("module").expect("runtime export");
        let state = export.state_lock().expect("runtime export state");

        assert!(state.heap.get(exported).is_some());
        assert!(state.heap.get(global).is_some());
        assert!(state.heap.get(dead).is_none());
    }

    #[test]
    fn core_vm_builtins_use_runtime_native() {
        let ctx = VmContext::new();
        for name in [
            "__lk_call_method",
            "__lk_call_method_named",
            "__lk_make_struct",
            "typeof",
            "__lk_set_field",
            "__lk_merge_fields",
            "__lk_bit_and",
            "__lk_bit_or",
            "__lk_bit_not",
        ] {
            let value = ctx
                .runtime_globals
                .get(name)
                .unwrap_or_else(|| panic!("{name} builtin present"));
            let state = value.state_lock().expect("runtime builtin state");
            let RuntimeVal::Obj(handle) = value.value() else {
                panic!("{name} should be runtime heap callable");
            };
            assert!(matches!(
                state.heap.get(*handle),
                Some(HeapValue::Callable(crate::val::CallableValue::RuntimeNative { .. }))
            ));
        }
    }

    #[test]
    fn core_make_struct_reads_typed_map_backing_directly() {
        let mut state = RuntimeModuleState::default();
        let fields = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([(
                    Arc::<str>::from("answer"),
                    42,
                )])))),
        );
        let name = RuntimeVal::ShortStr(crate::val::ShortStr::new("Point").expect("short"));
        let args = [name, fields];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = core_make_struct_builtin(NativeArgs::new(&args), &mut runtime).expect("make struct");

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected object");
        };
        let Some(HeapValue::Object(object)) = runtime.heap().get(handle) else {
            panic!("expected heap object");
        };
        assert_eq!(object.fields.get("answer"), Some(&RuntimeVal::Int(42)));
        assert_eq!(runtime.heap().len(), 2);
    }

    #[test]
    fn core_merge_fields_reads_typed_map_backing_directly() {
        let mut state = RuntimeModuleState::default();
        let base = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([(
                    Arc::<str>::from("a"),
                    1,
                )])))),
        );
        let overlay = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([(
                    Arc::<str>::from("b"),
                    2,
                )])))),
        );
        let args = [base, overlay];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = core_merge_fields_builtin(NativeArgs::new(&args), &mut runtime).expect("merge fields");

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected map");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("expected heap map");
        };
        assert!(matches!(map, TypedMap::StringInt(_)));
        assert_eq!(map.get_str("a"), Some(RuntimeVal::Int(1)));
        assert_eq!(map.get_str("b"), Some(RuntimeVal::Int(2)));
        assert_eq!(runtime.heap().len(), 3);
    }

    #[test]
    fn core_set_field_preserves_typed_string_int_map_without_copying_overwritten_entry() {
        let mut state = RuntimeModuleState::default();
        let base = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([
                    (Arc::<str>::from("answer"), 1),
                    (Arc::<str>::from("keep"), 2),
                ])))),
        );
        let key = RuntimeVal::ShortStr(crate::val::ShortStr::new("answer").expect("short"));
        let args = [base, key, RuntimeVal::Int(42)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = core_set_field_builtin(NativeArgs::new(&args), &mut runtime).expect("set field");

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected map");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("expected heap map");
        };
        let TypedMap::StringInt(entries) = map else {
            panic!("expected string-int backing");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("answer"), Some(&42));
        assert_eq!(entries.get("keep"), Some(&2));
    }

    #[test]
    fn core_set_field_pollutes_typed_map_without_copying_overwritten_entry() {
        let mut state = RuntimeModuleState::default();
        let base = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([
                    (Arc::<str>::from("answer"), 1),
                    (Arc::<str>::from("keep"), 2),
                ])))),
        );
        let key = RuntimeVal::ShortStr(crate::val::ShortStr::new("answer").expect("short"));
        let args = [base, key, RuntimeVal::Bool(true)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = core_set_field_builtin(NativeArgs::new(&args), &mut runtime).expect("set field");

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected map");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("expected heap map");
        };
        let TypedMap::StringMixed(entries) = map else {
            panic!("expected string-mixed backing");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("answer"), Some(&RuntimeVal::Bool(true)));
        assert_eq!(entries.get("keep"), Some(&RuntimeVal::Int(2)));
    }

    #[test]
    fn core_merge_fields_filters_base_keys_overwritten_by_overlay() {
        let mut state = RuntimeModuleState::default();
        let base = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([
                    (Arc::<str>::from("answer"), 1),
                    (Arc::<str>::from("keep"), 2),
                ])))),
        );
        let overlay = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([(
                    Arc::<str>::from("answer"),
                    42,
                )])))),
        );
        let args = [base, overlay];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = core_merge_fields_builtin(NativeArgs::new(&args), &mut runtime).expect("merge fields");

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected map");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("expected heap map");
        };
        let TypedMap::StringInt(entries) = map else {
            panic!("expected string-int backing");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("answer"), Some(&42));
        assert_eq!(entries.get("keep"), Some(&2));
    }

    #[test]
    fn core_merge_fields_nil_base_preserves_overlay_typed_backing() {
        let mut state = RuntimeModuleState::default();
        let overlay = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::Map(TypedMap::StringBool(fast_hash_map_from_iter([(
                    Arc::<str>::from("ok"),
                    true,
                )])))),
        );
        let args = [RuntimeVal::Nil, overlay];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = core_merge_fields_builtin(NativeArgs::new(&args), &mut runtime).expect("merge fields");

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected map");
        };
        let Some(HeapValue::Map(map)) = runtime.heap().get(handle) else {
            panic!("expected heap map");
        };
        assert!(matches!(map, TypedMap::StringBool(_)));
        assert_eq!(map.get_str("ok"), Some(RuntimeVal::Bool(true)));
    }
}
