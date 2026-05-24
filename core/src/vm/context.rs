use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, anyhow};

use crate::module::runtime_export_from_runtime_native;
use crate::stmt::ModuleResolver;
use crate::typ::TypeChecker;
use crate::util::fast_map::{FastHashMap, fast_hash_map_new};
use crate::val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, ShortStr, Type, TypedList, TypedMap};
use crate::vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeExport32, collect_runtime_export32};

use crate::typ::{TraitDef, TraitImpl};
use std::collections::HashMap;

mod core_methods;
use core_methods::{core_call_method_builtin32, core_call_method_named_builtin32};

/// VM runtime context.
///
/// VM-visible globals live in `runtime_globals`; top-level locals and call
/// frames live in `RuntimeModuleState32.stack`.
#[derive(Debug)]
pub struct VmContext {
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
        let mut ctx = Self::new_without_core_vm_builtins();
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
            call_stack: Vec::new(),
        }
    }

    /// 当前全局缓存版本。
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn shallow_clone_shared_runtime(&self) -> Self {
        Self {
            runtime_globals: self
                .runtime_globals
                .iter()
                .map(|(name, value)| (Arc::clone(name), value.shallow_clone_shared()))
                .collect(),
            generation: self.generation,
            resolver: Arc::clone(&self.resolver),
            type_checker: self.type_checker.clone(),
            structs: self.structs.clone(),
            call_stack: self.call_stack.clone(),
        }
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

    fn install_runtime_builtin(&mut self, name: &str, function: NativeFunction32, arity: u16) {
        if self.runtime_globals.contains_key(name) {
            return;
        }
        let value = runtime_export_from_runtime_native(function, arity);
        self.runtime_globals.insert(Arc::<str>::from(name), value);
    }
}

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
    let method_count = runtime_list_len(
        args.get(1).expect("arity checked"),
        runtime.heap(),
        "__lk_register_trait methods",
    )?;
    let mut methods = HashMap::with_capacity(method_count);
    for index in 0..method_count {
        let entry = runtime_list_item(
            args.get(1).expect("arity checked"),
            index,
            runtime,
            "__lk_register_trait methods",
        )?;
        let (method_name_value, type_value) = runtime_list_pair(&entry, runtime, "trait method entry")?;
        let method_name = runtime_string_arg(&method_name_value, runtime.heap(), "trait method name")?.to_string();
        let type_str = runtime_string_arg(&type_value, runtime.heap(), "trait method type")?;
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
    let method_count = runtime_list_len(
        args.get(2).expect("arity checked"),
        runtime.heap(),
        "__lk_register_trait_impl methods",
    )?;
    let mut method_map: HashMap<String, (RuntimeVal, Option<Type>)> = HashMap::with_capacity(method_count);
    for index in 0..method_count {
        let entry = runtime_list_item(
            args.get(2).expect("arity checked"),
            index,
            runtime,
            "__lk_register_trait_impl methods",
        )?;
        let inner_len = runtime_list_len(&entry, runtime.heap(), "trait impl entry")?;
        if inner_len != 3 {
            return Err(anyhow!(
                "trait impl entry must contain [name, closure, type], found {} items",
                inner_len
            ));
        }
        let method_name = runtime_string_arg(
            &runtime_list_item(&entry, 0, runtime, "trait impl entry")?,
            runtime.heap(),
            "trait impl method name",
        )?
        .to_string();
        let method_value = runtime_list_item(&entry, 1, runtime, "trait impl entry")?;
        ensure_runtime_callable(&method_value, runtime, "trait impl method")?;
        let signature_value = runtime_list_item(&entry, 2, runtime, "trait impl entry")?;
        let signature_ty = match &signature_value {
            RuntimeVal::Nil => None,
            value => {
                let type_str = runtime_string_arg(value, runtime.heap(), "trait impl method type")?;
                Some(
                    Type::parse(type_str.as_ref())
                        .ok_or_else(|| anyhow!("failed to parse method type '{}'", type_str))?,
                )
            }
        };
        method_map.insert(method_name, (method_value, signature_ty));
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

fn runtime_list_len(value: &RuntimeVal, heap: &HeapStore, helper: &str) -> anyhow::Result<usize> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{helper} expects list, got {:?}", value.kind()));
    };
    let list = heap
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
    let HeapValue::List(list) = list else {
        return Err(anyhow!("{helper} expects list, got {}", list.type_name()));
    };
    Ok(list.len())
}

fn runtime_list_item(
    value: &RuntimeVal,
    index: usize,
    runtime: &mut NativeRuntime32<'_>,
    helper: &str,
) -> anyhow::Result<RuntimeVal> {
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{helper} expects list, got {:?}", value.kind()));
    };
    let item = match runtime
        .heap()
        .get(*handle)
        .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
    {
        HeapValue::List(TypedList::Mixed(values)) => values.get(index).cloned(),
        HeapValue::List(TypedList::Int(values)) => values.get(index).copied().map(RuntimeVal::Int),
        HeapValue::List(TypedList::Float(values)) => values.get(index).copied().map(RuntimeVal::Float),
        HeapValue::List(TypedList::Bool(values)) => values.get(index).copied().map(RuntimeVal::Bool),
        HeapValue::List(TypedList::String(values)) => {
            let value = values.get(index).cloned();
            return value
                .map(|value| runtime_string_value(&value, runtime.heap_mut()))
                .ok_or_else(|| anyhow!("{helper} index {index} out of bounds"));
        }
        other => return Err(anyhow!("{helper} expects list, got {}", other.type_name())),
    };
    item.ok_or_else(|| anyhow!("{helper} index {index} out of bounds"))
}

fn runtime_list_pair(
    value: &RuntimeVal,
    runtime: &mut NativeRuntime32<'_>,
    helper: &str,
) -> anyhow::Result<(RuntimeVal, RuntimeVal)> {
    let len = runtime_list_len(value, runtime.heap(), helper)?;
    if len != 2 {
        return Err(anyhow!("{helper} must contain [name, type], found {len} items"));
    }
    Ok((
        runtime_list_item(value, 0, runtime, helper)?,
        runtime_list_item(value, 1, runtime, helper)?,
    ))
}

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
            .alloc(HeapValue::Object(RuntimeObject::new(type_name, fields))),
    ))
}

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

fn core_set_field_builtin32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> anyhow::Result<RuntimeVal> {
    if args.len() != 3 {
        return Err(anyhow!("__lk_set_field(base, key, value) expects exactly 3 arguments"));
    }
    let base = args.get(0).expect("arity checked").clone();
    let key = runtime_string_arg(args.get(1).expect("arity checked"), runtime.heap(), "__lk_set_field")?;
    let field_value = args.get(2).expect("arity checked").clone();
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

fn core_merge_fields_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
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
    let mut fields = object
        .fields
        .iter()
        .filter(|(field_key, _)| field_key.as_ref() != key.as_ref())
        .map(|(field_key, field_value)| (Arc::clone(field_key), field_value.clone()))
        .collect::<BTreeMap<_, _>>();
    fields.insert(Arc::clone(&key), value);

    let mut field_slots = object.field_slots.clone();
    if !field_slots.iter().any(|field_key| field_key.as_ref() == key.as_ref()) {
        field_slots.push(key);
    }

    RuntimeObject {
        type_name: Arc::clone(&object.type_name),
        fields,
        field_slots,
    }
}

fn set_string_field_on_map(map: &TypedMap, key: Arc<str>, value: RuntimeVal) -> TypedMap {
    match (map, value) {
        (TypedMap::Mixed(entries), value) => {
            let runtime_key = RuntimeMapKey::String(key);
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| **entry_key != runtime_key)
                .map(|(entry_key, entry_value)| (entry_key.clone(), entry_value.clone()))
                .collect::<BTreeMap<_, _>>();
            out.insert(runtime_key, value);
            TypedMap::Mixed(out)
        }
        (TypedMap::StringMixed(entries), value) => {
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| entry_key.as_ref() != key.as_ref())
                .map(|(entry_key, entry_value)| (Arc::clone(entry_key), entry_value.clone()))
                .collect::<BTreeMap<_, _>>();
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
        (TypedMap::StringInt(entries), RuntimeVal::Int(value)) => {
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| entry_key.as_ref() != key.as_ref())
                .map(|(entry_key, entry_value)| (Arc::clone(entry_key), *entry_value))
                .collect::<BTreeMap<_, _>>();
            out.insert(key, value);
            TypedMap::StringInt(out)
        }
        (TypedMap::StringFloat(entries), RuntimeVal::Float(value)) => {
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| entry_key.as_ref() != key.as_ref())
                .map(|(entry_key, entry_value)| (Arc::clone(entry_key), *entry_value))
                .collect::<BTreeMap<_, _>>();
            out.insert(key, value);
            TypedMap::StringFloat(out)
        }
        (TypedMap::StringBool(entries), RuntimeVal::Bool(value)) => {
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| entry_key.as_ref() != key.as_ref())
                .map(|(entry_key, entry_value)| (Arc::clone(entry_key), *entry_value))
                .collect::<BTreeMap<_, _>>();
            out.insert(key, value);
            TypedMap::StringBool(out)
        }
        (TypedMap::StringInt(entries), value) => {
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| entry_key.as_ref() != key.as_ref())
                .map(|(entry_key, entry_value)| (Arc::clone(entry_key), RuntimeVal::Int(*entry_value)))
                .collect::<BTreeMap<_, _>>();
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
        (TypedMap::StringFloat(entries), value) => {
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| entry_key.as_ref() != key.as_ref())
                .map(|(entry_key, entry_value)| (Arc::clone(entry_key), RuntimeVal::Float(*entry_value)))
                .collect::<BTreeMap<_, _>>();
            out.insert(key, value);
            TypedMap::StringMixed(out)
        }
        (TypedMap::StringBool(entries), value) => {
            let mut out = entries
                .iter()
                .filter(|(entry_key, _)| entry_key.as_ref() != key.as_ref())
                .map(|(entry_key, entry_value)| (Arc::clone(entry_key), RuntimeVal::Bool(*entry_value)))
                .collect::<BTreeMap<_, _>>();
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
            let mut out = TypedMap::StringMixed(
                object
                    .fields
                    .iter()
                    .filter(|(key, _)| !typed_map_contains_str(overlay, key.as_ref()))
                    .map(|(key, value)| (Arc::clone(key), value.clone()))
                    .collect(),
            );
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
        TypedMap::Mixed(entries) => TypedMap::Mixed(entries.clone()),
        TypedMap::StringMixed(entries) => TypedMap::StringMixed(entries.clone()),
        TypedMap::StringInt(entries) => TypedMap::StringInt(entries.clone()),
        TypedMap::StringFloat(entries) => TypedMap::StringFloat(entries.clone()),
        TypedMap::StringBool(entries) => TypedMap::StringBool(entries.clone()),
    }
}

fn copy_typed_map_without_overlay_keys(map: &TypedMap, overlay: &TypedMap) -> TypedMap {
    match map {
        TypedMap::Mixed(entries) => TypedMap::Mixed(
            entries
                .iter()
                .filter(|(key, _)| !typed_map_contains(overlay, key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        TypedMap::StringMixed(entries) => TypedMap::StringMixed(
            entries
                .iter()
                .filter(|(key, _)| !typed_map_contains_str(overlay, key.as_ref()))
                .map(|(key, value)| (Arc::clone(key), value.clone()))
                .collect(),
        ),
        TypedMap::StringInt(entries) => TypedMap::StringInt(
            entries
                .iter()
                .filter(|(key, _)| !typed_map_contains_str(overlay, key.as_ref()))
                .map(|(key, value)| (Arc::clone(key), *value))
                .collect(),
        ),
        TypedMap::StringFloat(entries) => TypedMap::StringFloat(
            entries
                .iter()
                .filter(|(key, _)| !typed_map_contains_str(overlay, key.as_ref()))
                .map(|(key, value)| (Arc::clone(key), *value))
                .collect(),
        ),
        TypedMap::StringBool(entries) => TypedMap::StringBool(
            entries
                .iter()
                .filter(|(key, _)| !typed_map_contains_str(overlay, key.as_ref()))
                .map(|(key, value)| (Arc::clone(key), *value))
                .collect(),
        ),
    }
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

fn runtime_object_fields_from_map(map: &TypedMap) -> anyhow::Result<BTreeMap<Arc<str>, RuntimeVal>> {
    let mut fields = BTreeMap::new();
    match map {
        TypedMap::Mixed(entries) => {
            for (key, value) in entries {
                let Some(key) = key.as_arc_str() else {
                    return Err(anyhow!("__lk_make_struct field keys must be strings"));
                };
                fields.insert(key, value.clone());
            }
        }
        TypedMap::StringMixed(entries) => {
            fields.extend(entries.iter().map(|(key, value)| (key.clone(), value.clone())));
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
                out.set(key.clone(), value.clone());
            }
        }
        TypedMap::StringMixed(entries) => {
            for (key, value) in entries {
                out.set(RuntimeMapKey::String(key.clone()), value.clone());
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

fn bit_arg32(value: &crate::val::RuntimeVal, func: &str) -> anyhow::Result<i64> {
    match value {
        crate::val::RuntimeVal::Int(i) => Ok(*i),
        other => Err(anyhow!("{func} expects Int arguments, got {:?}", other.kind())),
    }
}

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
    fn collect_runtime_globals_garbage_keeps_export_values_and_globals() {
        let mut ctx = VmContext::new_without_core_vm_builtins();
        let mut heap = HeapStore::new();
        let exported = heap.alloc(HeapValue::String(Arc::<str>::from("exported")));
        let global = heap.alloc(HeapValue::String(Arc::<str>::from("global")));
        let dead = heap.alloc(HeapValue::String(Arc::<str>::from("dead")));
        ctx.define_runtime_global(
            "module",
            RuntimeExport32::new(
                RuntimeVal::Obj(exported),
                Arc::new(std::sync::Mutex::new(RuntimeModuleState32::new(
                    heap,
                    vec![RuntimeVal::Obj(global)],
                ))),
                Arc::new(Module32::default()),
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
            let state = value.state_lock().expect("runtime builtin state");
            let RuntimeVal::Obj(handle) = value.value() else {
                panic!("{name} should be runtime heap callable");
            };
            assert!(matches!(
                state.heap.get(*handle),
                Some(HeapValue::Callable(crate::val::CallableValue::RuntimeNative32 { .. }))
            ));
        }
    }

    #[test]
    fn core_make_struct_reads_typed_map_backing_directly() {
        let mut state = RuntimeModuleState32::default();
        let fields = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([(
            Arc::<str>::from("answer"),
            42,
        )])))));
        let name = RuntimeVal::ShortStr(crate::val::ShortStr::new("Point").expect("short"));
        let args = [name, fields];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = core_make_struct_builtin32(NativeArgs32::new(&args), &mut runtime).expect("make struct");

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
        let mut state = RuntimeModuleState32::default();
        let base = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([(
            Arc::<str>::from("a"),
            1,
        )])))));
        let overlay = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([(
            Arc::<str>::from("b"),
            2,
        )])))));
        let args = [base, overlay];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = core_merge_fields_builtin32(NativeArgs32::new(&args), &mut runtime).expect("merge fields");

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
        let mut state = RuntimeModuleState32::default();
        let base = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([
            (Arc::<str>::from("answer"), 1),
            (Arc::<str>::from("keep"), 2),
        ])))));
        let key = RuntimeVal::ShortStr(crate::val::ShortStr::new("answer").expect("short"));
        let args = [base, key, RuntimeVal::Int(42)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = core_set_field_builtin32(NativeArgs32::new(&args), &mut runtime).expect("set field");

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
        let mut state = RuntimeModuleState32::default();
        let base = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([
            (Arc::<str>::from("answer"), 1),
            (Arc::<str>::from("keep"), 2),
        ])))));
        let key = RuntimeVal::ShortStr(crate::val::ShortStr::new("answer").expect("short"));
        let args = [base, key, RuntimeVal::Bool(true)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = core_set_field_builtin32(NativeArgs32::new(&args), &mut runtime).expect("set field");

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
        let mut state = RuntimeModuleState32::default();
        let base = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([
            (Arc::<str>::from("answer"), 1),
            (Arc::<str>::from("keep"), 2),
        ])))));
        let overlay = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringInt(BTreeMap::from([(
            Arc::<str>::from("answer"),
            42,
        )])))));
        let args = [base, overlay];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = core_merge_fields_builtin32(NativeArgs32::new(&args), &mut runtime).expect("merge fields");

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
        let mut state = RuntimeModuleState32::default();
        let overlay = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(TypedMap::StringBool(BTreeMap::from([(
            Arc::<str>::from("ok"),
            true,
        )])))));
        let args = [RuntimeVal::Nil, overlay];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);

        let result = core_merge_fields_builtin32(NativeArgs32::new(&args), &mut runtime).expect("merge fields");

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
