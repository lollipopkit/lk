use std::sync::Arc;

use anyhow::anyhow;

use crate::stmt::{ImportContext, ModuleResolver};
use crate::typ::TypeChecker;
use crate::util::fast_map::{FastHashMap, FastHashSet, fast_hash_map_new, fast_hash_set_new};
use crate::val::{Type, Val};

#[cfg(not(feature = "aot-minimal-runtime"))]
use crate::typ::{TraitDef, TraitImpl};
#[cfg(not(feature = "aot-minimal-runtime"))]
use crate::val::{NativeArgs, ObjectValue};
#[cfg(not(feature = "aot-minimal-runtime"))]
use std::collections::HashMap;

mod core_methods;
use core_methods::{call_method_positional, method_name_arc};
#[cfg(not(feature = "aot-minimal-runtime"))]
use core_methods::{core_call_method_builtin_fast, core_call_method_named_builtin_fast};

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
    // Names of global constants (immutable)
    const_globals: FastHashSet<String>,
    // Simple stack of local scopes; top-most is current
    locals: Vec<FastHashMap<String, Val>>,
    // Cache generation for invalidation
    generation: u64,
    import_ctx: ImportContext,
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
            const_globals: fast_hash_set_new(),
            locals: Vec::new(),
            generation: 0,
            import_ctx: ImportContext::new(),
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
            if !self.globals.contains_key(name) {
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

    /// 设置导入上下文。
    pub fn with_import_context(mut self, import_ctx: ImportContext) -> Self {
        self.import_ctx = import_ctx;
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

    /// 获取导入上下文的引用
    pub fn import_context(&self) -> &ImportContext {
        &self.import_ctx
    }

    /// 获取导入上下文的可变引用
    pub fn import_context_mut(&mut self) -> &mut ImportContext {
        &mut self.import_ctx
    }

    pub(crate) fn call_method_zero(&mut self, receiver: Val, method: &Val) -> anyhow::Result<Val> {
        let method_arc = method_name_arc("__lk_call_method", method)?;
        call_method_positional(receiver, method_arc, &[], self)
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
        if !self.globals.contains_key("__lk_register_trait") {
            self.globals.insert(
                "__lk_register_trait".to_string(),
                Val::RustFastFunction(core_register_trait_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_register_trait_impl") {
            self.globals.insert(
                "__lk_register_trait_impl".to_string(),
                Val::RustFastFunction(core_register_trait_impl_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_call_method") {
            self.globals.insert(
                "__lk_call_method".to_string(),
                Val::RustFastFunction(core_call_method_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_call_method_named") {
            self.globals.insert(
                "__lk_call_method_named".to_string(),
                Val::RustFastFunction(core_call_method_named_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_make_struct") {
            self.globals.insert(
                "__lk_make_struct".to_string(),
                Val::RustFastFunction(core_make_struct_builtin_fast),
            );
        }
        if !self.globals.contains_key("typeof") {
            self.globals
                .insert("typeof".to_string(), Val::RustFastFunction(core_typeof_builtin_fast));
        }
        if !self.globals.contains_key("__lk_set_field") {
            self.globals.insert(
                "__lk_set_field".to_string(),
                Val::RustFastFunction(core_set_field_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_merge_fields") {
            self.globals.insert(
                "__lk_merge_fields".to_string(),
                Val::RustFastFunction(core_merge_fields_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_bit_and") {
            self.globals.insert(
                "__lk_bit_and".to_string(),
                Val::RustFastFunction(core_bit_and_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_bit_or") {
            self.globals.insert(
                "__lk_bit_or".to_string(),
                Val::RustFastFunction(core_bit_or_builtin_fast),
            );
        }
        if !self.globals.contains_key("__lk_bit_not") {
            self.globals.insert(
                "__lk_bit_not".to_string(),
                Val::RustFastFunction(core_bit_not_builtin_fast),
            );
        }
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

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_register_trait_builtin_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> anyhow::Result<Val> {
    core_register_trait_builtin(args.as_slice(), ctx)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_register_trait_builtin(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!(
            "__lk_register_trait expects 2 arguments: name and methods list"
        ));
    }

    let name = match args[0].as_str() {
        Some(s) => s.to_string(),
        None => {
            return Err(anyhow!(
                "__lk_register_trait expects trait name as string, got {}",
                args[0].type_name()
            ));
        }
    };

    let method_entries = match &args[1] {
        Val::List(list) => list.as_ref(),
        other => {
            return Err(anyhow!(
                "__lk_register_trait expects methods as list, got {}",
                other.type_name()
            ));
        }
    };

    let mut methods = HashMap::with_capacity(method_entries.len());
    for entry in method_entries.iter() {
        let inner = match entry {
            Val::List(values) => values.as_ref(),
            other => {
                return Err(anyhow!("trait methods must be lists, got {}", other.type_name()));
            }
        };

        if inner.len() != 2 {
            return Err(anyhow!(
                "trait method entry must contain [name, type], found {} items",
                inner.len()
            ));
        }

        let method_name = match inner[0].as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(anyhow!(
                    "trait method name must be string, got {}",
                    inner[0].type_name()
                ));
            }
        };

        let type_str = match inner[1].as_str() {
            Some(s) => s,
            None => {
                return Err(anyhow!(
                    "trait method type must be string, got {}",
                    inner[1].type_name()
                ));
            }
        };

        let ty = Type::parse(type_str).ok_or_else(|| anyhow!("failed to parse trait method type '{}'", type_str))?;
        methods.insert(method_name, ty);
    }

    let type_checker = ctx
        .get_type_checker_mut()
        .ok_or_else(|| anyhow!("type checker not available for trait registration"))?;
    type_checker.registry_mut().register_trait(TraitDef { name, methods });
    Ok(Val::Nil)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_register_trait_impl_builtin_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> anyhow::Result<Val> {
    core_register_trait_impl_builtin(args.as_slice(), ctx)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_register_trait_impl_builtin(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!(
            "__lk_register_trait_impl expects 3 arguments: trait_name, target_type, methods"
        ));
    }

    let trait_name = match args[0].as_str() {
        Some(s) => s.to_string(),
        None => {
            return Err(anyhow!(
                "__lk_register_trait_impl expects trait name string, got {}",
                args[0].type_name()
            ));
        }
    };

    let target_type_str = match args[1].as_str() {
        Some(s) => s,
        None => {
            return Err(anyhow!(
                "__lk_register_trait_impl expects target type string, got {}",
                args[1].type_name()
            ));
        }
    };
    let target_type =
        Type::parse(target_type_str).ok_or_else(|| anyhow!("failed to parse target type '{}'", target_type_str))?;

    let methods_list = match &args[2] {
        Val::List(list) => list.as_ref(),
        other => {
            return Err(anyhow!(
                "__lk_register_trait_impl expects methods list, got {}",
                other.type_name()
            ));
        }
    };

    let mut method_map: HashMap<String, (Val, Option<Type>)> = HashMap::with_capacity(methods_list.len());
    for entry in methods_list.iter() {
        let inner = match entry {
            Val::List(values) => values.as_ref(),
            other => {
                return Err(anyhow!("trait impl methods must be lists, got {}", other.type_name()));
            }
        };

        if inner.len() != 3 {
            return Err(anyhow!(
                "trait impl entry must contain [name, closure, type], found {} items",
                inner.len()
            ));
        }

        let method_name = match inner[0].as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(anyhow!(
                    "trait impl method name must be string, got {}",
                    inner[0].type_name()
                ));
            }
        };

        let closure_val = inner[1].clone();
        let signature_ty = match &inner[2] {
            val if val.as_str().is_some() => {
                let s = val.as_str().unwrap();
                let parsed = Type::parse(s).ok_or_else(|| anyhow!("failed to parse method type '{}'", s))?;
                Some(parsed)
            }
            Val::Nil => None,
            other => {
                return Err(anyhow!(
                    "trait impl method type must be string or nil, got {}",
                    other.type_name()
                ));
            }
        };

        method_map.insert(method_name, (closure_val, signature_ty));
    }

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
    Ok(Val::Nil)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_make_struct_builtin_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> anyhow::Result<Val> {
    core_make_struct_builtin(args.as_slice(), ctx)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_make_struct_builtin(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!(
            "__lk_make_struct expects 2 arguments: struct name and fields map"
        ));
    }

    let type_name = match &args[0] {
        Val::Str(s) => s.clone(),
        Val::ShortStr(s) => Val::intern_str(s.as_str()),
        other => {
            return Err(anyhow!(
                "__lk_make_struct expects struct name as string, got {}",
                other.type_name()
            ));
        }
    };

    let fields_map = match &args[1] {
        Val::Map(map) => map,
        Val::Nil => {
            return Ok(Val::Object(Arc::new(ObjectValue {
                type_name,
                fields: Arc::new(HashMap::new()),
            })));
        }
        other => {
            return Err(anyhow!(
                "__lk_make_struct expects fields as map, got {}",
                other.type_name()
            ));
        }
    };

    let mut fields = HashMap::with_capacity(fields_map.len());
    for (k, v) in fields_map.iter() {
        fields.insert(k.to_string(), v.clone());
    }

    Ok(Val::Object(Arc::new(ObjectValue {
        type_name,
        fields: Arc::new(fields),
    })))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_typeof_builtin_fast(args: NativeArgs<'_>, _ctx: &mut VmContext) -> anyhow::Result<Val> {
    let value = args
        .get(0)
        .ok_or_else(|| anyhow!("typeof(value) expects exactly one argument"))?;
    let name = match value {
        Val::Int(_) => "Int",
        Val::Float(_) => "Float",
        Val::Bool(_) => "Bool",
        Val::ShortStr(_) | Val::Str(_) => "String",
        Val::List(_) => "List",
        Val::Map(_) => "Map",
        Val::Object(object) => object.type_name.as_str(),
        Val::Closure(_)
        | Val::RustFunction(_)
        | Val::RustFastFunction(_)
        | Val::RustFastFunctionNamed(_)
        | Val::RustFunctionNamed(_)
        | Val::AotFunction(_) => "Function",
        Val::Task(_) => "Task",
        Val::Channel(_) => "Channel",
        Val::Stream(_) => "Stream",
        Val::Iterator(_) => "Iterator",
        Val::MutationGuard(guard) => guard.guard_type(),
        Val::StreamCursor(_) => "StreamCursor",
        Val::Nil => "Nil",
    };
    Ok(Val::from_str(name))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_set_field_builtin_fast(args: NativeArgs<'_>, _ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!("__lk_set_field(base, key, value) expects exactly 3 arguments"));
    }
    let base = args.get(0).expect("arity checked");
    let key = args
        .get(1)
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("__lk_set_field key must be a string"))?;
    let value = args.get(2).expect("arity checked").clone();
    match base {
        Val::Map(map) => {
            let mut updated = (**map).clone();
            Val::map_insert_arcstr(&mut updated, Val::intern_str(key), value);
            Ok(Val::Map(Arc::new(updated)))
        }
        Val::Object(object) => {
            let mut fields = (*object.fields).clone();
            fields.insert(key.to_string(), value);
            Ok(Val::Object(Arc::new(ObjectValue {
                type_name: object.type_name.clone(),
                fields: Arc::new(fields),
            })))
        }
        other => Err(anyhow!(
            "__lk_set_field target must be Map or Object, got {}",
            other.type_name()
        )),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_merge_fields_builtin_fast(args: NativeArgs<'_>, _ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_merge_fields(base, overlay) expects exactly 2 arguments"));
    }
    let mut fields = match args.get(0).expect("arity checked") {
        Val::Object(object) => object
            .fields
            .iter()
            .map(|(key, value)| (Val::intern_str(key), value.clone()))
            .collect(),
        Val::Map(map) => (**map).clone(),
        Val::Nil => fast_hash_map_new(),
        other => {
            return Err(anyhow!(
                "__lk_merge_fields base must be Object, Map, or Nil, got {}",
                other.type_name()
            ));
        }
    };

    match args.get(1).expect("arity checked") {
        Val::Map(overlay) => {
            for (key, value) in overlay.iter() {
                fields.insert(key.clone(), value.clone());
            }
            Ok(Val::Map(Arc::new(fields)))
        }
        other => Err(anyhow!(
            "__lk_merge_fields overlay must be Map, got {}",
            other.type_name()
        )),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn bit_arg(value: &Val, func: &str) -> anyhow::Result<i64> {
    match value {
        Val::Int(i) => Ok(*i),
        other => Err(anyhow!("{func} expects Int arguments, got {}", other.type_name())),
    }
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_bit_and_builtin_fast(args: NativeArgs<'_>, _ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_bit_and(left, right) expects exactly 2 arguments"));
    }
    Ok(Val::Int(
        bit_arg(args.get(0).expect("arity checked"), "__lk_bit_and")?
            & bit_arg(args.get(1).expect("arity checked"), "__lk_bit_and")?,
    ))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_bit_or_builtin_fast(args: NativeArgs<'_>, _ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 2 {
        return Err(anyhow!("__lk_bit_or(left, right) expects exactly 2 arguments"));
    }
    Ok(Val::Int(
        bit_arg(args.get(0).expect("arity checked"), "__lk_bit_or")?
            | bit_arg(args.get(1).expect("arity checked"), "__lk_bit_or")?,
    ))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn core_bit_not_builtin_fast(args: NativeArgs<'_>, _ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 1 {
        return Err(anyhow!("__lk_bit_not(value) expects exactly 1 argument"));
    }
    Ok(Val::Int(!bit_arg(args.get(0).expect("arity checked"), "__lk_bit_not")?))
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
}
