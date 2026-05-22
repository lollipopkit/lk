use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, anyhow};

use crate::module::runtime_export_from_runtime_native;
use crate::stmt::ModuleResolver;
use crate::typ::TypeChecker;
use crate::util::fast_map::{FastHashMap, fast_hash_map_new};
use crate::val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, Type, TypedList, TypedMap, Val};
use crate::vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeExport32, collect_runtime_export32};

#[cfg(not(feature = "aot-minimal-runtime"))]
use crate::typ::{TraitDef, TraitImpl, TraitMethodValue};
#[cfg(not(feature = "aot-minimal-runtime"))]
use std::collections::HashMap;

mod core_methods;
#[cfg(not(feature = "aot-minimal-runtime"))]
use core_methods::{core_call_method_builtin32, core_call_method_named_builtin32};

mod legacy;
use legacy::LegacyValContext;

/// VM runtime context.
///
/// New VM-visible globals live in `runtime_globals`. The `legacy_*` Val tables
/// are retained only for legacy expression eval and LLVM AOT compatibility.
#[derive(Debug, Clone)]
pub struct VmContext {
    // Legacy Val symbol table; do not add new VM runtime behavior here.
    legacy: LegacyValContext,
    runtime_globals: FastHashMap<Arc<str>, RuntimeExport32>,
    // Cache generation for invalidation
    generation: u64,
    resolver: Arc<ModuleResolver>,
    type_checker: Option<TypeChecker>,
    structs: FastHashMap<String, FastHashMap<String, Type>>,
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
            legacy: LegacyValContext::new(),
            runtime_globals: fast_hash_map_new(),
            generation: 0,
            resolver: Arc::new(ModuleResolver::default()),
            type_checker: None,
            structs: fast_hash_map_new(),
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

    /// Legacy Val lookup path for old eval/AOT compatibility.
    #[inline]
    pub fn legacy_get(&self, name: &str) -> Option<&Val> {
        self.legacy.get(name)
    }

    /// Legacy Val define path for old eval/AOT compatibility.
    pub fn legacy_set<S: Into<String>>(&mut self, name: S, value: Val) -> Option<Val> {
        let name_str = name.into();
        self.runtime_globals.remove(name_str.as_str());
        let prev = self.legacy.set(name_str, value);
        self.bump_generation();
        prev
    }

    /// Legacy Val assign path for old eval/AOT compatibility.
    pub fn legacy_assign(&mut self, name: &str, value: Val) -> anyhow::Result<()> {
        self.runtime_globals.remove(name);
        self.legacy.assign(name, value)?;
        self.bump_generation();
        Ok(())
    }

    /// Legacy Val removal path for old eval/AOT compatibility.
    pub fn legacy_remove(&mut self, name: &str) -> Option<Val> {
        self.runtime_globals.remove(name);
        let prev = self.legacy.remove(name);
        if prev.is_some() {
            self.bump_generation();
        }
        prev
    }

    /// 构建函数，允许自定义组件。
    pub fn with_resolver(mut self, resolver: Arc<ModuleResolver>) -> Self {
        for (name, value) in resolver.runtime_builtin_iter() {
            if self.runtime_globals.contains_key(name.as_ref()) {
                continue;
            }
            self.runtime_globals.insert(Arc::clone(name), value.clone());
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
    pub fn runtime_globals_iter(&self) -> impl Iterator<Item = (&Arc<str>, &RuntimeExport32)> {
        self.runtime_globals.iter()
    }

    pub fn collect_runtime_globals_garbage(&self) -> Result<()> {
        for export in self.runtime_globals.values() {
            collect_runtime_export32(export)?;
        }
        Ok(())
    }

    pub fn get_runtime_global(&self, name: &str) -> Option<&RuntimeExport32> {
        self.runtime_globals.get(name)
    }

    pub fn define_runtime_global(&mut self, name: impl Into<Arc<str>>, value: RuntimeExport32) {
        let name = name.into();
        let name_str = name.as_ref();
        self.legacy.remove_global(name_str);
        self.runtime_globals.insert(name, value);
        self.bump_generation();
    }

    pub fn define_runtime_value(&mut self, name: impl Into<Arc<str>>, value: RuntimeVal, heap: HeapStore) {
        self.define_runtime_global(name, RuntimeExport32::from_value(value, heap));
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

    /// Enter a legacy Val lexical scope.
    pub fn push_scope(&mut self) {
        self.legacy.push_scope();
    }

    /// Exit a legacy Val lexical scope.
    pub fn pop_scope(&mut self) {
        if self.legacy.pop_scope() {
            self.bump_generation();
        }
    }

    /// Legacy Val lookup path for old eval/AOT compatibility.
    pub fn legacy_get_value(&self, name: &str) -> Option<Val> {
        self.legacy_get(name).cloned()
    }

    /// 获取类型检查器的可变引用
    pub fn get_type_checker_mut(&mut self) -> Option<&mut TypeChecker> {
        self.type_checker.as_mut()
    }

    /// 注册结构体模式
    pub fn register_struct_schema(&mut self, name: String, fields: FastHashMap<String, Type>) {
        self.structs.insert(name, fields);
    }

    /// Legacy Val define path for old eval/AOT compatibility.
    pub fn legacy_define<S: Into<String>>(&mut self, name: S, value: Val) -> Option<Val> {
        self.legacy_set(name, value)
    }

    /// Legacy Val const define path for old eval/AOT compatibility.
    pub fn legacy_define_const<S: Into<String>>(&mut self, name: S, value: Val) {
        self.legacy.define_const(name.into(), value);
        self.bump_generation();
    }

    /// 创建当前上下文的快照
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Check whether legacy Val lexical scopes are active.
    pub fn is_local_name(&self, _name: &str) -> bool {
        self.legacy.has_local_scope()
    }

    /// Bind a parameter into the current lexical scope.
    ///
    /// The old context-owned slot cache has been removed; executable frame
    /// windows now live in `RuntimeModuleState32.stack`.
    pub fn bind_param_at_slot(&mut self, name: String, _slot: u16, value: Val) {
        self.legacy.bind_param_at_slot(name, value);
        self.bump_generation();
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
            NativeFunction32::FullState(core_call_method_builtin32),
            3,
        );
        self.install_runtime_builtin(
            "__lk_call_method_named",
            NativeFunction32::FullState(core_call_method_named_builtin32),
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
        if self.runtime_globals.contains_key(name) {
            return;
        }
        let value = runtime_export_from_runtime_native(function, arity);
        self.runtime_globals.insert(Arc::<str>::from(name), value);
    }
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
        TypedList::OwnedRuntime(values) => values.copy_values_into(runtime.heap_mut()),
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
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            let HeapValue::Map(map) = &value else {
                return Err(anyhow!(
                    "__lk_make_struct expects fields as map, got {}",
                    value.type_name()
                ));
            };
            runtime_object_fields_from_map(map, runtime.heap_mut())?
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
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            match &value {
                HeapValue::Object(object) => object
                    .fields
                    .iter()
                    .map(|(key, value)| (RuntimeMapKey::String(key.clone()), value.clone()))
                    .collect::<BTreeMap<_, _>>(),
                HeapValue::Map(map) => map
                    .entries_into_heap(runtime.heap_mut())?
                    .into_iter()
                    .collect::<BTreeMap<_, _>>(),
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
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
            let HeapValue::Map(overlay) = &value else {
                return Err(anyhow!(
                    "__lk_merge_fields overlay must be Map, got {}",
                    value.type_name()
                ));
            };
            for (key, value) in overlay.entries_into_heap(runtime.heap_mut())? {
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
fn runtime_object_fields_from_map(
    map: &TypedMap,
    heap: &mut HeapStore,
) -> anyhow::Result<BTreeMap<Arc<str>, RuntimeVal>> {
    let mut fields = BTreeMap::new();
    for (key, value) in map.entries_into_heap(heap)? {
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
    use crate::vm::{Module32, RuntimeModuleState32};

    #[test]
    fn test_bind_param_at_slot_binds_only_locals() {
        let mut ctx = VmContext::new();
        ctx.bind_param_at_slot("p".to_string(), 1, Val::Int(42));
        assert_eq!(ctx.legacy_get("p"), Some(&Val::Int(42)));
    }

    #[test]
    fn test_set_assign_remove_updates_locals_without_context_slots() {
        let mut ctx = VmContext::new();
        ctx.push_scope();

        ctx.legacy_set("x".to_string(), Val::Int(7));
        assert_eq!(ctx.legacy_get("x"), Some(&Val::Int(7)));

        ctx.legacy_assign("x", Val::Int(9)).expect("assign x");
        assert_eq!(ctx.legacy_get("x"), Some(&Val::Int(9)));

        let prev = ctx.legacy_remove("x");
        assert_eq!(prev, Some(Val::Int(9)));
        assert_eq!(ctx.legacy_get("x"), None);
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
            RuntimeExport32 {
                value: RuntimeVal::Obj(exported),
                state: Arc::new(std::sync::Mutex::new(RuntimeModuleState32::new(
                    heap,
                    vec![RuntimeVal::Obj(global)],
                ))),
                module: Arc::new(Module32::default()),
            },
        );

        ctx.collect_runtime_globals_garbage().expect("collect globals");
        let export = ctx.get_runtime_global("module").expect("runtime export");
        let state = export.state.lock().expect("runtime export state");

        assert!(state.heap.get(exported).is_some());
        assert!(state.heap.get(global).is_some());
        assert!(state.heap.get(dead).is_none());
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
