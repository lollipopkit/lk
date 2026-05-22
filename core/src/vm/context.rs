use std::{collections::BTreeMap, sync::Arc};

use anyhow::anyhow;

use crate::stmt::ModuleResolver;
use crate::typ::TypeChecker;
use crate::util::fast_map::{FastHashMap, FastHashSet, fast_hash_map_new, fast_hash_set_new};
use crate::val::{
    CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, Type, TypedList, TypedMap, Val,
};
use crate::vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeExport32};

#[cfg(not(feature = "aot-minimal-runtime"))]
use crate::typ::{TraitDef, TraitImpl, TraitMethodValue};
#[cfg(not(feature = "aot-minimal-runtime"))]
use std::collections::HashMap;

mod core_methods;
#[cfg(not(feature = "aot-minimal-runtime"))]
use core_methods::{core_call_method_builtin32, core_call_method_named_builtin32};

/// VM 运行期全局上下文。
///
/// - 保存当前可见的全局符号表；
/// - 维护一个 `generation` 用于失效指令级缓存；
/// - 维护调用栈信息用于错误报告；
/// - 提供必要的读写接口。
#[derive(Debug, Clone)]
pub struct VmContext {
    // Global symbol table
    globals: FastHashMap<String, Val>,
    runtime_globals: FastHashMap<Arc<str>, RuntimeExport32>,
    // Names of global constants (immutable)
    const_globals: FastHashSet<String>,
    // Simple stack of local scopes; top-most is current
    locals: Vec<FastHashMap<String, Val>>,
    // Cache generation for invalidation
    generation: u64,
    resolver: Arc<ModuleResolver>,
    type_checker: Option<TypeChecker>,
    structs: FastHashMap<String, FastHashMap<String, Type>>,
    // Slot-based fast-path cache for VM execution. See docs/vm/slot-cache.md for design notes.
    slot_values: Vec<Val>,
    slot_scopes: Vec<FastHashMap<String, u16>>,
    call_stack: Vec<CallFrameInfo>,
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
        #[cfg(not(feature = "aot-minimal-runtime"))]
        {
            let mut ctx = Self::new_without_core_vm_builtins();
            ctx.install_core_vm_builtins();
            ctx
        }
        #[cfg(feature = "aot-minimal-runtime")]
        {
            Self::new_without_core_vm_builtins()
        }
    }

    /// Create an empty context without VM-only core builtins.
    ///
    /// LLVM AOT executables use this to avoid linking method-dispatch and
    /// trait-registration fallback paths when imports are replayed natively.
    pub fn new_without_core_vm_builtins() -> Self {
        Self {
            globals: fast_hash_map_new(),
            runtime_globals: fast_hash_map_new(),
            const_globals: fast_hash_set_new(),
            locals: Vec::new(),
            generation: 0,
            resolver: Arc::new(ModuleResolver::default()),
            type_checker: None,
            structs: fast_hash_map_new(),
            slot_values: Vec::new(),
            slot_scopes: vec![fast_hash_map_new()],
            call_stack: Vec::new(),
        }
    }

    /// 当前全局缓存版本。
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
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

    /// 从上下文中读取符号（先局部后全局）。
    #[inline]
    pub fn get(&self, name: &str) -> Option<&Val> {
        // Search local scopes from innermost to outermost
        for scope in self.locals.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v);
            }
        }
        // Fallback to globals
        self.globals.get(name)
    }

    /// 定义或覆盖一个变量。
    /// 当存在局部作用域时，在当前局部作用域中设置；否则设置为全局变量。
    pub fn set<S: Into<String>>(&mut self, name: S, value: Val) -> Option<Val> {
        let name_str = name.into();
        self.runtime_globals.remove(name_str.as_str());
        if self.locals.last().is_some() {
            // Sync to frame slot when mapping exists
            self.try_update_slot(&name_str, &value);
            let prev = if let Some(scope) = self.locals.last_mut() {
                scope.insert(name_str, value)
            } else {
                None
            };
            self.bump_generation();
            prev
        } else {
            self.const_globals.remove(name_str.as_str());
            // In case a slot mapping exists (should not for globals), best-effort sync
            self.try_update_slot(&name_str, &value);
            let prev = self.globals.insert(name_str, value);
            self.bump_generation();
            prev
        }
    }

    /// 对已存在的变量赋值（优先局部作用域）。
    pub fn assign(&mut self, name: &str, value: Val) -> anyhow::Result<()> {
        self.runtime_globals.remove(name);
        // Try local scopes first (from innermost to outermost)
        for scope in self.locals.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = value.clone();
                // Sync to frame slot when mapping exists
                self.try_update_slot(name, &value);
                self.bump_generation();
                return Ok(());
            }
        }
        // Then globals with const check
        if self.const_globals.contains(name) {
            return Err(anyhow!("Cannot assign to const variable '{}'", name));
        }
        if let Some(slot) = self.globals.get_mut(name) {
            *slot = value.clone();
            // Best-effort sync into any mapped slot (shouldn't normally exist for globals)
            self.try_update_slot(name, &value);
            self.bump_generation();
            Ok(())
        } else {
            Err(anyhow!("Undefined variable: {}", name))
        }
    }

    /// 删除一个变量：优先删除当前局部作用域中的变量，否则删除全局变量。
    pub fn remove(&mut self, name: &str) -> Option<Val> {
        // Try remove from innermost local scope
        if let Some(scope) = self.locals.last_mut()
            && let Some(prev) = scope.remove(name)
        {
            // Clear corresponding frame slot to Nil if mapped
            self.try_update_slot(name, &Val::Nil);
            self.bump_generation();
            return Some(prev);
        }
        // Otherwise remove from globals
        self.const_globals.remove(name);
        self.runtime_globals.remove(name);
        let prev = self.globals.remove(name);
        if prev.is_some() {
            // Best-effort clear mapped slot (shouldn't exist for globals)
            self.try_update_slot(name, &Val::Nil);
            self.bump_generation();
        }
        prev
    }

    /// 构建函数，允许自定义组件。
    pub fn with_resolver(mut self, resolver: Arc<ModuleResolver>) -> Self {
        for (name, val) in resolver.builtin_iter() {
            if self.globals.contains_key(name) || self.runtime_globals.contains_key(name.as_str()) {
                continue;
            }
            if let Some(value) = runtime_export_from_builtin_val(val) {
                self.runtime_globals.insert(Arc::<str>::from(name.as_str()), value);
            } else {
                self.globals.insert(name.clone(), val.clone());
            }
        }
        self.resolver = resolver;
        self
    }

    /// 设置类型检查器。
    pub fn with_type_checker(mut self, type_checker: Option<TypeChecker>) -> Self {
        self.type_checker = type_checker;
        self
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Val)> {
        self.globals.iter()
    }

    #[inline]
    pub fn globals(&self) -> &FastHashMap<String, Val> {
        &self.globals
    }

    #[inline]
    pub fn runtime_globals_iter(&self) -> impl Iterator<Item = (&Arc<str>, &RuntimeExport32)> {
        self.runtime_globals.iter()
    }

    pub fn define_runtime_global(&mut self, name: impl Into<Arc<str>>, value: RuntimeExport32) {
        let name = name.into();
        let name_str = name.as_ref();
        self.globals.remove(name_str);
        self.const_globals.remove(name_str);
        self.runtime_globals.insert(name, value);
        self.bump_generation();
    }

    pub fn define_runtime_value(&mut self, name: impl Into<Arc<str>>, value: RuntimeVal, heap: HeapStore) {
        self.define_runtime_global(name, RuntimeExport32::from_value(value, heap));
    }

    #[inline]
    pub fn globals_mut(&mut self) -> &mut FastHashMap<String, Val> {
        &mut self.globals
    }

    /// 手动递增版本号，用于强制失效缓存。
    #[inline]
    pub fn touch(&mut self) {
        self.bump_generation();
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    #[inline]
    fn ensure_slot_scope_depth(&mut self, depth: usize) -> &mut FastHashMap<String, u16> {
        while self.slot_scopes.len() <= depth {
            self.slot_scopes.push(fast_hash_map_new());
        }
        self.slot_scopes
            .get_mut(depth)
            .expect("slot scope guaranteed by ensure")
    }

    fn ensure_slot_capacity(&mut self, slot: usize) {
        let needed = slot + 1;
        if self.slot_values.len() < needed {
            self.slot_values.resize_with(needed, || Val::Nil);
        }
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

    /// 返回当前调用栈的格式化字符串。
    pub fn call_stack_report(&self) -> Option<String> {
        if self.call_stack.is_empty() {
            None
        } else {
            let mut msg = String::from("Call stack:\n");
            for frame in self.call_stack.iter().rev() {
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
            Some(msg)
        }
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

    /// 导出全局符号（用于模块导入系统）
    pub fn export_symbols(&self) -> FastHashMap<String, Val> {
        self.globals.clone()
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

    /// 作用域管理：进入新的词法作用域
    pub fn push_scope(&mut self) {
        // Push a new (empty) local scope
        self.locals.push(fast_hash_map_new());
        // Maintain slot scope stack for VM fast-path cache
        self.slot_scopes.push(fast_hash_map_new());
    }

    /// 作用域管理：退出当前词法作用域
    pub fn pop_scope(&mut self) {
        // Pop the current local scope if present
        if self.locals.pop().is_some() {
            self.bump_generation();
        }
        if self.slot_scopes.len() > 1 {
            let _ = self.slot_scopes.pop();
        }
    }

    /// 获取变量值（与get方法相同，为了兼容性）
    pub fn get_value(&self, name: &str) -> Option<Val> {
        self.get(name).cloned()
    }

    /// 获取类型检查器的可变引用
    pub fn get_type_checker_mut(&mut self) -> Option<&mut TypeChecker> {
        self.type_checker.as_mut()
    }

    /// 注册结构体模式
    pub fn register_struct_schema(&mut self, name: String, fields: FastHashMap<String, Type>) {
        self.structs.insert(name, fields);
    }

    /// 定义一个变量（与 set 方法相同，为了兼容性）
    pub fn define<S: Into<String>>(&mut self, name: S, value: Val) -> Option<Val> {
        self.set(name, value)
    }

    /// 定义一个常量变量（不能被重新赋值）
    pub fn define_const<S: Into<String>>(&mut self, name: S, value: Val) {
        let name_str = name.into();
        self.globals.insert(name_str.clone(), value);
        self.const_globals.insert(name_str);
        // Constants are globals; do not mirror into frame slots by default.
        self.bump_generation();
    }

    /// 创建当前上下文的快照
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// 检查名称是否为本地名称
    /// 在当前实现中，所有名称都是全局的，所以此方法返回 false
    pub fn is_local_name(&self, _name: &str) -> bool {
        // If any local scope exists, names may be local; exact lookup happens in get/assign
        !self.locals.is_empty()
    }

    /// 在指定槽位绑定参数值（用于函数调用优化）
    pub fn bind_param_at_slot(&mut self, name: String, slot: u16, value: Val) {
        if self.locals.is_empty() {
            self.locals.push(fast_hash_map_new());
        }
        let last_scope_idx = self.slot_scopes.len().saturating_sub(1);
        self.ensure_slot_scope_depth(last_scope_idx);
        self.ensure_slot_capacity(slot as usize);
        self.slot_values[slot as usize] = value.clone();
        if let Some(scope) = self.slot_scopes.last_mut() {
            scope.insert(name.clone(), slot);
        }
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name, value);
        }
        self.bump_generation();
    }

    /// 预加载按深度分组的槽映射（用于函数调用优化）
    /// `mappings` 形如 (name, slot, depth)。depth=0 为函数级作用域。
    pub fn preload_slot_mappings_per_depth(&mut self, mappings: &[(String, u16, u16)]) {
        if mappings.is_empty() {
            return;
        }
        let mut max_depth: usize = 0;
        let mut max_slot: usize = 0;
        for (_, slot, depth) in mappings.iter() {
            max_depth = max_depth.max(*depth as usize);
            max_slot = max_slot.max(*slot as usize);
        }
        self.ensure_slot_capacity(max_slot);
        for (name, slot, depth) in mappings.iter() {
            let scope = self.ensure_slot_scope_depth(*depth as usize);
            scope.entry(name.clone()).or_insert(*slot);
        }
    }

    #[cfg(not(feature = "aot-minimal-runtime"))]
    fn install_core_vm_builtins(&mut self) {
        self.install_runtime_builtin(
            "__lk_register_trait",
            NativeFunction32::Plain(core_register_trait_builtin32),
            2,
        );
        self.install_runtime_builtin(
            "__lk_register_trait_impl",
            NativeFunction32::Plain(core_register_trait_impl_builtin32),
            3,
        );
        self.install_runtime_builtin(
            "__lk_call_method",
            NativeFunction32::Plain(core_call_method_builtin32),
            3,
        );
        self.install_runtime_builtin(
            "__lk_call_method_named",
            NativeFunction32::Plain(core_call_method_named_builtin32),
            4,
        );
        self.install_runtime_builtin(
            "__lk_make_struct",
            NativeFunction32::Plain(core_make_struct_builtin32),
            2,
        );
        self.install_runtime_builtin("typeof", NativeFunction32::Plain(core_typeof_builtin32), 1);
        self.install_runtime_builtin("__lk_set_field", NativeFunction32::Plain(core_set_field_builtin32), 3);
        self.install_runtime_builtin(
            "__lk_merge_fields",
            NativeFunction32::Plain(core_merge_fields_builtin32),
            2,
        );
        self.install_runtime_builtin("__lk_bit_and", NativeFunction32::Plain(core_bit_and_builtin32), 2);
        self.install_runtime_builtin("__lk_bit_or", NativeFunction32::Plain(core_bit_or_builtin32), 2);
        self.install_runtime_builtin("__lk_bit_not", NativeFunction32::Plain(core_bit_not_builtin32), 1);
    }

    #[cfg(not(feature = "aot-minimal-runtime"))]
    fn install_runtime_builtin(&mut self, name: &str, function: NativeFunction32, arity: u16) {
        if self.globals.contains_key(name) || self.runtime_globals.contains_key(name) {
            return;
        }
        let value = runtime_export_from_runtime_native(function, arity);
        self.runtime_globals.insert(Arc::<str>::from(name), value);
    }

    // -------- Internal helpers --------
    fn try_update_slot(&mut self, name: &str, value: &Val) {
        for scope in self.slot_scopes.iter().rev() {
            if let Some(&slot) = scope.get(name) {
                let idx = slot as usize;
                self.ensure_slot_capacity(idx);
                if let Some(entry) = self.slot_values.get_mut(idx) {
                    *entry = value.clone();
                }
                break;
            }
        }
    }
}

fn runtime_export_from_builtin_val(value: &Val) -> Option<RuntimeExport32> {
    let Val::Obj(object) = value else {
        return None;
    };
    let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
        return None;
    };
    Some(runtime_export_from_runtime_native(function.clone(), *arity))
}

fn runtime_export_from_runtime_native(function: NativeFunction32, arity: u16) -> RuntimeExport32 {
    let mut heap = HeapStore::new();
    let value = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function })));
    RuntimeExport32::from_value(value, heap)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_register_trait_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!(
            "__lk_register_trait expects 2 arguments: name and methods list"
        ));
    }
    let name = runtime_string_arg(
        args.get(0).expect("arity checked"),
        runtime.heap(),
        "__lk_register_trait",
    )?
    .to_string();
    let method_entries = runtime_list_values(
        args.get(1).expect("arity checked"),
        runtime,
        "__lk_register_trait methods",
    )?;
    let mut methods = HashMap::with_capacity(method_entries.len());
    for entry in method_entries {
        let inner = runtime_list_values(&entry, runtime, "trait method entry")?;
        if inner.len() != 2 {
            return Err(anyhow!(
                "trait method entry must contain [name, type], found {} items",
                inner.len()
            ));
        }
        let method_name = runtime_string_arg(&inner[0], runtime.heap(), "trait method name")?.to_string();
        let type_str = runtime_string_arg(&inner[1], runtime.heap(), "trait method type")?;
        let ty = Type::parse(type_str.as_ref())
            .ok_or_else(|| anyhow!("failed to parse trait method type '{}'", type_str))?;
        methods.insert(method_name, ty);
    }
    let ctx = runtime
        .ctx_mut()
        .ok_or_else(|| anyhow!("__lk_register_trait requires VmContext"))?;
    let type_checker = ctx
        .get_type_checker_mut()
        .ok_or_else(|| anyhow!("type checker not available for trait registration"))?;
    type_checker.registry_mut().register_trait(TraitDef { name, methods });
    Ok(RuntimeVal::Nil)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_register_trait_impl_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 3 {
        return Err(anyhow!(
            "__lk_register_trait_impl expects 3 arguments: trait_name, target_type, methods"
        ));
    }
    let trait_name = runtime_string_arg(
        args.get(0).expect("arity checked"),
        runtime.heap(),
        "__lk_register_trait_impl",
    )?
    .to_string();
    let target_type_str = runtime_string_arg(
        args.get(1).expect("arity checked"),
        runtime.heap(),
        "__lk_register_trait_impl",
    )?;
    let target_type = Type::parse(target_type_str.as_ref())
        .ok_or_else(|| anyhow!("failed to parse target type '{}'", target_type_str))?;
    let method_entries = runtime_list_values(
        args.get(2).expect("arity checked"),
        runtime,
        "__lk_register_trait_impl methods",
    )?;
    let mut method_map: HashMap<String, (TraitMethodValue, Option<Type>)> =
        HashMap::with_capacity(method_entries.len());
    for entry in method_entries {
        let inner = runtime_list_values(&entry, runtime, "trait impl entry")?;
        if inner.len() != 3 {
            return Err(anyhow!(
                "trait impl entry must contain [name, closure, type], found {} items",
                inner.len()
            ));
        }
        let method_name = runtime_string_arg(&inner[0], runtime.heap(), "trait impl method name")?.to_string();
        ensure_runtime_callable(&inner[1], runtime, "trait impl method")?;
        let signature_ty = match &inner[2] {
            RuntimeVal::Nil => None,
            value => {
                let type_str = runtime_string_arg(value, runtime.heap(), "trait impl method type")?;
                Some(
                    Type::parse(type_str.as_ref())
                        .ok_or_else(|| anyhow!("failed to parse method type '{}'", type_str))?,
                )
            }
        };
        method_map.insert(method_name, (TraitMethodValue::Runtime(inner[1].clone()), signature_ty));
    }
    let ctx = runtime
        .ctx_mut()
        .ok_or_else(|| anyhow!("__lk_register_trait_impl requires VmContext"))?;
    let type_checker = ctx
        .get_type_checker_mut()
        .ok_or_else(|| anyhow!("type checker not available for trait implementation"))?;
    let impl_def = TraitImpl {
        trait_name,
        target_type,
        methods: method_map,
    };
    type_checker.registry().validate_trait_impl(&impl_def)?;
    type_checker.registry_mut().register_trait_impl(impl_def);
    Ok(RuntimeVal::Nil)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn runtime_list_values(
    value: &RuntimeVal,
    runtime: &mut NativeRuntime32<'_>,
    helper: &str,
) -> anyhow::Result<Vec<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{helper} expects list, got {:?}", value.kind()));
    };
    let list = runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::List(list) = list else {
        return Err(anyhow!("{helper} expects list, got {}", list.type_name()));
    };
    let list = list.clone();
    Ok(match list {
        TypedList::Mixed(values) => values,
        TypedList::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
        TypedList::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
        TypedList::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
        TypedList::String(values) => values
            .iter()
            .map(|value| runtime_string_value(value, runtime.heap_mut()))
            .collect(),
    })
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn ensure_runtime_callable(value: &RuntimeVal, runtime: &NativeRuntime32<'_>, helper: &str) -> anyhow::Result<()> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{helper} must be callable, got {:?}", value.kind()));
    };
    match runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::Callable(_) => Ok(()),
        other => Err(anyhow!("{helper} must be callable, got {}", other.type_name())),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_make_struct_builtin32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> anyhow::Result<RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!(
            "__lk_make_struct expects 2 arguments: struct name and fields map"
        ));
    }

    let type_name = runtime_string_arg(args.get(0).expect("arity checked"), runtime.heap(), "__lk_make_struct")?;

    let fields = match args.get(1).expect("arity checked") {
        RuntimeVal::Nil => BTreeMap::new(),
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

    Ok(RuntimeVal::Obj(
        runtime
            .heap_mut()
            .alloc(HeapValue::Object(RuntimeObject { type_name, fields })),
    ))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_typeof_builtin32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> anyhow::Result<RuntimeVal> {
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

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_set_field_builtin32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> anyhow::Result<RuntimeVal> {
    if args.len() != 3 {
        return Err(anyhow!("__lk_set_field(base, key, value) expects exactly 3 arguments"));
    }
    let base = args.get(0).expect("arity checked").clone();
    let key = runtime_string_arg(args.get(1).expect("arity checked"), runtime.heap(), "__lk_set_field")?;
    let field_value = args.get(2).expect("arity checked").clone();
    match base {
        RuntimeVal::Obj(handle) => {
            let heap_value = runtime
                .heap()
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                .clone();
            match heap_value {
                HeapValue::Map(mut map) => {
                    map.set(RuntimeMapKey::String(key), field_value);
                    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Map(map))))
                }
                HeapValue::Object(mut object) => {
                    object.fields.insert(key, field_value);
                    Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::Object(object))))
                }
                other => Err(anyhow!(
                    "__lk_set_field target must be Map or Object, got {}",
                    other.type_name()
                )),
            }
        }
        other => Err(anyhow!(
            "__lk_set_field target must be Map or Object, got {:?}",
            other.kind()
        )),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_merge_fields_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_merge_fields(base, overlay) expects exactly 2 arguments"));
    }

    let mut fields = match args.get(0).expect("arity checked") {
        RuntimeVal::Obj(handle) => {
            let value = runtime
                .heap()
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            match value {
                HeapValue::Object(object) => object
                    .fields
                    .iter()
                    .map(|(key, value)| (RuntimeMapKey::String(key.clone()), value.clone()))
                    .collect::<BTreeMap<_, _>>(),
                HeapValue::Map(map) => map.entries().into_iter().collect::<BTreeMap<_, _>>(),
                other => {
                    return Err(anyhow!(
                        "__lk_merge_fields base must be Object, Map, or Nil, got {}",
                        other.type_name()
                    ));
                }
            }
        }
        RuntimeVal::Nil => BTreeMap::new(),
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
            for (key, value) in overlay.entries() {
                fields.insert(key, value);
            }
            Ok(RuntimeVal::Obj(
                runtime
                    .heap_mut()
                    .alloc(HeapValue::Map(TypedMap::from_runtime_entries(fields))),
            ))
        }
        other => Err(anyhow!("__lk_merge_fields overlay must be Map, got {:?}", other.kind())),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
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

#[cfg(not(feature = "aot-minimal-runtime"))]
fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(short) = crate::val::ShortStr::new(value) {
        RuntimeVal::ShortStr(short)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn runtime_object_fields_from_map(map: &TypedMap) -> anyhow::Result<BTreeMap<Arc<str>, RuntimeVal>> {
    let mut fields = BTreeMap::new();
    for (key, value) in map.entries() {
        let Some(key) = key.as_arc_str() else {
            return Err(anyhow!("__lk_make_struct field keys must be strings"));
        };
        fields.insert(key, value);
    }
    Ok(fields)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn bit_arg32(value: &crate::val::RuntimeVal, func: &str) -> anyhow::Result<i64> {
    match value {
        crate::val::RuntimeVal::Int(i) => Ok(*i),
        other => Err(anyhow!("{func} expects Int arguments, got {:?}", other.kind())),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_bit_and_builtin32(
    args: NativeArgs32<'_>,
    _runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_bit_and(left, right) expects exactly 2 arguments"));
    }
    Ok(crate::val::RuntimeVal::Int(
        bit_arg32(args.get(0).expect("arity checked"), "__lk_bit_and")?
            & bit_arg32(args.get(1).expect("arity checked"), "__lk_bit_and")?,
    ))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_bit_or_builtin32(
    args: NativeArgs32<'_>,
    _runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_bit_or(left, right) expects exactly 2 arguments"));
    }
    Ok(crate::val::RuntimeVal::Int(
        bit_arg32(args.get(0).expect("arity checked"), "__lk_bit_or")?
            | bit_arg32(args.get(1).expect("arity checked"), "__lk_bit_or")?,
    ))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_bit_not_builtin32(
    args: NativeArgs32<'_>,
    _runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<crate::val::RuntimeVal> {
    if args.len() != 1 {
        return Err(anyhow!("__lk_bit_not(value) expects exactly 1 argument"));
    }
    Ok(crate::val::RuntimeVal::Int(!bit_arg32(
        args.get(0).expect("arity checked"),
        "__lk_bit_not",
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bind_param_at_slot_syncs_frame_and_locals() {
        let mut ctx = VmContext::new();
        // Bind a parameter into slot 1
        ctx.bind_param_at_slot("p".to_string(), 1, Val::Int(42));

        // Visible via locals API
        assert_eq!(ctx.get("p"), Some(&Val::Int(42)));

        assert!(ctx.slot_values.len() >= 2);
        assert_eq!(ctx.slot_values[1], Val::Int(42));

        let top_scope = ctx.slot_scopes.last().expect("slot scope present");
        assert_eq!(top_scope.get("p").copied(), Some(1));
    }

    #[test]
    fn test_set_assign_remove_syncs_slot() {
        let mut ctx = VmContext::new();
        ctx.push_scope();

        // Preload name -> slot mapping at function depth 0
        ctx.preload_slot_mappings_per_depth(&[("x".to_string(), 2, 0)]);

        // set should write both locals map and frame slot
        ctx.set("x".to_string(), Val::Int(7));
        assert_eq!(ctx.get("x"), Some(&Val::Int(7)));
        assert!(ctx.slot_values.len() >= 3);
        assert_eq!(ctx.slot_values[2], Val::Int(7));

        // assign should update the same slot
        ctx.assign("x", Val::Int(9)).expect("assign x");
        assert_eq!(ctx.slot_values[2], Val::Int(9));

        // remove should clear slot to Nil and remove from locals scope
        let prev = ctx.remove("x");
        assert_eq!(prev, Some(Val::Int(9)));
        assert_eq!(ctx.slot_values[2], Val::Nil);
        assert_eq!(ctx.get("x"), None);
    }

    #[cfg(not(feature = "aot-minimal-runtime"))]
    #[test]
    fn core_vm_builtins_use_runtime_native32() {
        let ctx = VmContext::new();
        for name in [
            "__lk_register_trait",
            "__lk_register_trait_impl",
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
            let state = value.state.lock().expect("runtime builtin state");
            let RuntimeVal::Obj(handle) = value.value else {
                panic!("{name} should be runtime heap callable");
            };
            assert!(matches!(
                state.heap.get(handle),
                Some(HeapValue::Callable(crate::val::CallableValue::RuntimeNative32 { .. }))
            ));
        }
    }
}
