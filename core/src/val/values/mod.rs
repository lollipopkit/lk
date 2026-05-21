use std::{
    cell::Cell,
    collections::HashMap,
    fmt::Debug,
    sync::{Arc, Mutex},
};

use arcstr::ArcStr;

use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};

// Using standard HashMap for maps and environments

use anyhow::{Result, anyhow};
use once_cell::sync::OnceCell;

use crate::stmt::{NamedParamDecl, Program, Stmt};

use crate::vm::{CaptureSpec, FrameInfo, Function, VmContext};

use crate::resolve::slots::{FunctionLayout, SlotResolver};

mod cache;
mod call;
mod clone;
mod convert;
mod intern;
mod map_key_cache;
mod native;
mod ops;
mod serde_impl;
mod strings;
mod types;

mod iter;

use cache::cached_list_contains;

pub use types::{FunctionNamedParamType, ShortStr, Type};

type DefaultSeedRegLayout = Vec<Option<Arc<[u16]>>>;

pub use iter::{IteratorState, IteratorValue, MutationGuardState, MutationGuardValue};
pub use native::{NativeArgs, RustFastFunction, RustFastFunctionNamed};

/// New VM-optimized function type that directly uses VmContext
pub type RustFunction = fn(args: &[Val], ctx: &mut VmContext) -> Result<Val>;

/// New VM-optimized function type that supports named arguments and uses VmContext
pub type RustFunctionNamed = fn(positional: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AotFunction {
    pub ptr: usize,
    pub arity: u8,
}

thread_local! {
    static VM_FORCE_FAST_PATH: Cell<bool> = const { Cell::new(false) };
}

#[inline]
pub(crate) fn vm_fast_path_forced() -> bool {
    VM_FORCE_FAST_PATH.with(|flag| flag.get())
}

pub(crate) struct VmFastPathGuard {
    prev: bool,
}

impl VmFastPathGuard {
    #[inline]
    pub(crate) fn enable() -> Self {
        let prev = VM_FORCE_FAST_PATH.with(|flag| {
            let prior = flag.get();
            flag.set(true);
            prior
        });
        Self { prev }
    }
}

impl Drop for VmFastPathGuard {
    fn drop(&mut self) {
        VM_FORCE_FAST_PATH.with(|flag| flag.set(self.prev));
    }
}

#[derive(Debug, Clone)]
pub struct VmCallEnv {
    _generation: u64,
    param_scope: FastHashMap<String, Val>,
    vm_ctx: VmContext,
}

#[derive(Debug, Clone)]
pub struct ClosureCapture {
    names: Arc<[String]>,
    values: CaptureValues,
}

#[derive(Debug, Clone)]
enum CaptureValues {
    Empty,
    One(Val),
    Many(Vec<Val>),
}

impl CaptureValues {
    fn from_vec(values: Vec<Val>) -> Self {
        match values.len() {
            0 => Self::Empty,
            1 => Self::One(values.into_iter().next().expect("one capture value")),
            _ => Self::Many(values),
        }
    }

    fn iter(&self) -> CaptureValuesIter<'_> {
        CaptureValuesIter { values: self, idx: 0 }
    }

    fn get(&self, idx: usize) -> Option<&Val> {
        match self {
            Self::Empty => None,
            Self::One(value) => (idx == 0).then_some(value),
            Self::Many(values) => values.get(idx),
        }
    }
}

struct CaptureValuesIter<'a> {
    values: &'a CaptureValues,
    idx: usize,
}

impl<'a> Iterator for CaptureValuesIter<'a> {
    type Item = &'a Val;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.values.get(self.idx);
        self.idx += 1;
        item
    }
}

impl ClosureCapture {
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            names: Arc::<[String]>::from(Vec::new()),
            values: CaptureValues::Empty,
        })
    }

    pub fn from_pairs(names: Vec<String>, values: Vec<Val>) -> Arc<Self> {
        Arc::new(Self {
            names: Arc::<[String]>::from(names),
            values: CaptureValues::from_vec(values),
        })
    }

    pub fn from_shared_names(names: Arc<[String]>, values: Vec<Val>) -> Arc<Self> {
        Arc::new(Self {
            names,
            values: CaptureValues::from_vec(values),
        })
    }

    pub fn from_shared_names_one(names: Arc<[String]>, value: Val) -> Arc<Self> {
        Arc::new(Self {
            names,
            values: CaptureValues::One(value),
        })
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Val)> {
        self.names
            .iter()
            .zip(self.values.iter())
            .map(|(name, value)| (name.as_str(), value))
    }

    pub fn value_at(&self, idx: usize) -> Option<&Val> {
        self.values.get(idx)
    }
}

pub struct ClosureValue {
    pub params: Arc<Vec<String>>,
    pub param_types: Arc<Vec<Option<Type>>>,
    pub named_params: Arc<Vec<NamedParamDecl>>,
    pub body: Arc<Stmt>,
    pub env: Arc<VmContext>,
    pub upvalues: Arc<Vec<Val>>,
    pub captures: Arc<ClosureCapture>,
    pub capture_specs: Arc<Vec<CaptureSpec>>,
    pub default_funcs: Arc<Vec<Option<Function>>>,
    pub code: Arc<OnceCell<Arc<Function>>>,
    pub call_env_pool: OnceCell<Mutex<Vec<VmCallEnv>>>,
    pub layout: OnceCell<FunctionLayout>,
    call_layout: OnceCell<CallLayoutInfo>,
    debug_name: Option<String>,
    debug_location: Option<String>,
    frame_info_cache: OnceCell<FrameInfo>,
    default_frame_infos: OnceCell<Vec<Option<FrameInfo>>>,
    named_param_index: OnceCell<FastHashMap<ArcStr, usize>>,
    named_param_kinds: OnceCell<Vec<NamedParamKind>>,
    default_seed_reg_layout: OnceCell<DefaultSeedRegLayout>,
}

pub struct ClosureInit {
    pub params: Arc<Vec<String>>,
    pub param_types: Arc<Vec<Option<Type>>>,
    pub named_params: Arc<Vec<NamedParamDecl>>,
    pub body: Arc<Stmt>,
    pub env: Arc<VmContext>,
    pub upvalues: Arc<Vec<Val>>,
    pub captures: Arc<ClosureCapture>,
    pub capture_specs: Arc<Vec<CaptureSpec>>,
    pub default_funcs: Arc<Vec<Option<Function>>>,
    pub code: Arc<OnceCell<Arc<Function>>>,
    pub debug_name: Option<String>,
    pub debug_location: Option<String>,
}

// Implement a non-recursive Debug for closures to avoid printing their captured
// environment, which can contain self-referential cycles via globals and lead
// to stack overflows when formatting with `{:?}`.
impl core::fmt::Debug for ClosureValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = self.debug_name.as_deref().unwrap_or("<closure>");
        let params = self.params.join(", ");
        let named: Vec<String> = self.named_params.iter().map(|p| p.name.clone()).collect();
        f.debug_struct("ClosureValue")
            .field("name", &name)
            .field("params", &params)
            .field("named_params", &named)
            .field("body", &"<body>")
            // Intentionally omit env/upvalues/captures to avoid recursive prints
            .finish()
    }
}

impl ClosureValue {
    #[inline]
    pub(crate) fn supports_vm_positional_fast_path(&self) -> bool {
        self.named_params.is_empty() && self.default_funcs.iter().all(|opt| opt.is_none())
    }

    #[inline]
    pub(crate) fn frame_captures(&self) -> (Option<Arc<ClosureCapture>>, Option<Arc<Vec<CaptureSpec>>>) {
        if self.captures.is_empty() && self.capture_specs.is_empty() {
            (None, None)
        } else {
            (Some(Arc::clone(&self.captures)), Some(Arc::clone(&self.capture_specs)))
        }
    }

    pub fn new(init: ClosureInit) -> Self {
        let ClosureInit {
            params,
            param_types,
            named_params,
            body,
            env,
            upvalues,
            captures,
            capture_specs,
            default_funcs,
            code,
            debug_name,
            debug_location,
        } = init;
        Self {
            params,
            param_types,
            named_params,
            body,
            env,
            upvalues,
            captures,
            capture_specs,
            default_funcs,
            code,
            call_env_pool: OnceCell::new(),
            layout: OnceCell::new(),
            call_layout: OnceCell::new(),
            debug_name,
            debug_location,
            frame_info_cache: OnceCell::new(),
            default_frame_infos: OnceCell::new(),
            named_param_index: OnceCell::new(),
            named_param_kinds: OnceCell::new(),
            default_seed_reg_layout: OnceCell::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NamedParamKind {
    Required,
    OptionalNil,
    DefaultThunk,
}

impl ClosureValue {
    fn acquire_call_env(&self, caller_ctx: &mut VmContext, scope_capacity: usize) -> Result<VmCallEnv> {
        let pool = self.call_env_pool.get_or_init(|| Mutex::new(Vec::new()));
        let mut wrapper = {
            let mut guard = pool.lock().map_err(|_| anyhow!("Call environment pool poisoned"))?;
            guard.pop().unwrap_or_else(|| VmCallEnv {
                _generation: caller_ctx.generation(),
                param_scope: fast_hash_map_with_capacity(0),
                vm_ctx: caller_ctx.clone(),
            })
        };
        if wrapper.param_scope.capacity() < scope_capacity {
            wrapper
                .param_scope
                .reserve(scope_capacity - wrapper.param_scope.capacity());
        }
        Ok(wrapper)
    }

    fn release_call_env(&self, wrapper: VmCallEnv) -> Result<()> {
        let pool = self.call_env_pool.get_or_init(|| Mutex::new(Vec::new()));
        let mut guard = pool.lock().map_err(|_| anyhow!("Call environment pool poisoned"))?;
        guard.push(wrapper);
        Ok(())
    }

    fn with_call_env<F, R>(&self, caller_ctx: &mut VmContext, scope_capacity: usize, mut f: F) -> Result<R>
    where
        F: FnMut(&mut VmContext, &CallLayoutInfo) -> Result<R>,
    {
        let mut wrapper = self.acquire_call_env(caller_ctx, scope_capacity)?;
        let layout = self.layout.get_or_init(|| {
            let mut resolver = SlotResolver::new();
            let func_stmt = Stmt::Function {
                name: "__anon".to_string(),
                params: (*self.params).to_vec(),
                param_types: (*self.param_types).to_vec(),
                named_params: (*self.named_params).to_vec(),
                return_type: None,
                body: Box::new((*self.body).clone()),
            };
            let prog = Program::new(vec![Box::new(func_stmt)]).unwrap_or_else(|_| Program { statements: Vec::new() });
            resolver
                .resolve_program_slots(&prog)
                .root
                .children
                .first()
                .cloned()
                .unwrap_or(FunctionLayout {
                    decls: Vec::new(),
                    total_locals: (self.params.len() + self.named_params.len()) as u16,
                    uses: Vec::new(),
                    children: Vec::new(),
                })
        });
        let layout_info = self
            .call_layout
            .get_or_init(|| CallLayoutInfo::from_layout(layout, self.params.as_ref(), self.named_params.as_ref()));
        let result = f(&mut wrapper.vm_ctx, layout_info);
        let release_res = self.release_call_env(wrapper);
        match (result, release_res) {
            (Ok(val), Ok(())) => Ok(val),
            (Ok(_), Err(err)) => Err(err),
            (Err(err), Ok(())) => Err(err),
            (Err(err), Err(release_err)) => Err(err.context(release_err)),
        }
    }

    #[inline]
    pub fn debug_name(&self) -> Option<&str> {
        self.debug_name.as_deref()
    }

    #[inline]
    pub fn debug_location(&self) -> Option<&str> {
        self.debug_location.as_deref()
    }

    #[inline]
    pub fn frame_display_name(&self) -> String {
        self.debug_name.clone().unwrap_or_else(|| "<closure>".to_string())
    }

    #[inline]
    pub(crate) fn frame_info(&self) -> FrameInfo {
        self.frame_info_ref().clone()
    }

    #[inline]
    pub(crate) fn frame_info_ref(&self) -> &FrameInfo {
        self.frame_info_cache.get_or_init(|| {
            FrameInfo::new(
                self.debug_name.as_deref().unwrap_or("<closure>"),
                self.debug_location.as_deref(),
            )
        })
    }

    pub(crate) fn default_frame_info_ref(&self, idx: usize) -> Option<&FrameInfo> {
        let cache = self.default_frame_infos.get_or_init(|| {
            let base_info = self.frame_info_ref();
            let base_name = Arc::clone(&base_info.name);
            let base_location = base_info.location.as_ref().map(Arc::clone);
            self.named_params
                .iter()
                .enumerate()
                .map(|(param_idx, decl)| {
                    if self.default_funcs.get(param_idx).and_then(|opt| opt.as_ref()).is_some() {
                        let default_name = format!("{}::<default:{}>", base_name.as_ref(), decl.name);
                        Some(FrameInfo::new(default_name, base_location.as_ref().map(Arc::clone)))
                    } else {
                        None
                    }
                })
                .collect()
        });
        cache.get(idx).and_then(|info| info.as_ref())
    }

    pub(crate) fn named_param_index(&self) -> &FastHashMap<ArcStr, usize> {
        self.named_param_index.get_or_init(|| {
            let mut map = fast_hash_map_with_capacity(self.named_params.len());
            for (idx, decl) in self.named_params.iter().enumerate() {
                map.insert(ArcStr::from(decl.name.as_str()), idx);
            }
            map
        })
    }

    pub(crate) fn build_named_slots_with_metrics(
        &self,
        named: &[(String, Val)],
        collect_metrics: bool,
    ) -> Result<Vec<Option<Val>>> {
        let named_params = self.named_params.as_ref();
        if named_params.is_empty() {
            return if named.is_empty() {
                Ok(Vec::new())
            } else {
                Err(anyhow!("Named arguments are not supported for this function"))
            };
        }
        let mut named_slots: Vec<Option<Val>> = vec![None; named_params.len()];
        let index_by_name = self.named_param_index();
        for (name, value) in named.iter() {
            let idx = index_by_name
                .get(name.as_str())
                .copied()
                .ok_or_else(|| anyhow!("Unknown named argument: {}", name))?;
            if named_slots[idx].is_some() {
                return Err(anyhow!("Duplicate named argument: {}", name));
            }
            named_slots[idx] = Some(crate::vm::copy_call_arg_value_for_register_with_metrics(
                value,
                collect_metrics,
            ));
        }
        Ok(named_slots)
    }

    pub(crate) fn named_param_kinds(&self) -> &[NamedParamKind] {
        self.named_param_kinds
            .get_or_init(|| {
                let mut kinds = Vec::with_capacity(self.named_params.len());
                for (idx, decl) in self.named_params.iter().enumerate() {
                    if self.default_funcs.get(idx).and_then(|opt| opt.as_ref()).is_some() {
                        kinds.push(NamedParamKind::DefaultThunk);
                    } else if matches!(decl.type_annotation, Some(Type::Optional(_))) {
                        kinds.push(NamedParamKind::OptionalNil);
                    } else {
                        kinds.push(NamedParamKind::Required);
                    }
                }
                kinds
            })
            .as_slice()
    }

    pub(crate) fn default_seed_regs(&self, idx: usize) -> Option<&[u16]> {
        let layouts = self.default_seed_reg_layout.get_or_init(|| {
            self.default_funcs
                .iter()
                .map(|maybe_fun| {
                    maybe_fun
                        .as_ref()
                        .map(|fun| Arc::from(fun.named_param_regs.clone().into_boxed_slice()) as Arc<[u16]>)
                })
                .collect()
        });
        layouts
            .get(idx)
            .and_then(|entry| entry.as_ref().map(|arc| arc.as_ref()))
    }
}

#[derive(Debug, Default, Clone)]
struct CallLayoutInfo {
    _total_locals: usize,
    param_slots: Vec<Option<u16>>,
    param_slot_by_name: FastHashMap<String, u16>,
    locals: Vec<(String, u16, u16)>,
}

impl CallLayoutInfo {
    fn from_layout(layout: &FunctionLayout, params: &[String], named_params: &[NamedParamDecl]) -> Self {
        let total_locals = layout.total_locals as usize;
        let mut param_slot_by_name = fast_hash_map_with_capacity(params.len().saturating_add(named_params.len()));
        let mut locals: Vec<(String, u16, u16)> = Vec::with_capacity(
            layout
                .decls
                .len()
                .saturating_sub(params.len().saturating_add(named_params.len())),
        );
        for decl in &layout.decls {
            if decl.is_param {
                param_slot_by_name.insert(decl.name.clone(), decl.index);
            } else {
                locals.push((decl.name.clone(), decl.index, decl.block_depth));
            }
        }
        let param_slots = params
            .iter()
            .map(|name| param_slot_by_name.get(name.as_str()).copied())
            .collect();
        Self {
            _total_locals: total_locals,
            param_slots,
            param_slot_by_name,
            locals,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskValue {
    pub id: u64,
    pub value: Option<Val>,
}

#[derive(Debug, Clone)]
pub struct ChannelValue {
    pub id: u64,
    pub capacity: Option<i64>,
    pub inner_type: Type,
}

#[derive(Debug, Clone)]
pub struct StreamValue {
    pub id: u64,
    pub inner_type: Type,
}

#[derive(Debug, Clone)]
pub struct StreamCursorValue {
    pub id: u64,
    pub stream_id: u64,
}

#[derive(Debug, Clone)]
pub struct ObjectValue {
    pub type_name: ArcStr,
    pub fields: Arc<HashMap<String, Val>>,
}

#[derive(Debug, Default)]
pub enum Val {
    /// 内联短字符串（≤7 字节），零堆分配，实现 Copy
    ShortStr(ShortStr),
    /// 堆字符串，thin Arc 指针（8B），任意长度
    Str(ArcStr),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Map 类型，key 使用 ArcStr（thin 8B 指针，deref 到 &str）
    Map(Arc<FastHashMap<ArcStr, Val>>),
    /// List 类型，Arc<Vec<Val>> 共享不可变存储
    List(Arc<Vec<Val>>),
    /// 闭包
    Closure(Arc<ClosureValue>),
    /// Rust 函数指针
    RustFunction(RustFunction),
    /// Native fastcall function pointer
    RustFastFunction(RustFastFunction),
    /// Native fastcall function pointer with named argument support
    RustFastFunctionNamed(RustFastFunctionNamed),
    /// Rust 具名参数函数
    RustFunctionNamed(RustFunctionNamed),
    /// LLVM AOT 编译函数，Box 包装消除 padding（8B thin 指针）
    AotFunction(Box<AotFunction>),
    /// Task
    Task(Arc<TaskValue>),
    /// Channel
    Channel(Arc<ChannelValue>),
    /// Stream
    Stream(Arc<StreamValue>),
    /// Iterator
    Iterator(Arc<IteratorValue>),
    /// Mutation guard
    MutationGuard(Arc<MutationGuardValue>),
    /// Stream cursor
    StreamCursor(Arc<StreamCursorValue>),
    /// 具名类型的运行时对象
    Object(Arc<ObjectValue>),
    #[default]
    Nil,
}

impl Val {
    #[inline]
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::ShortStr(_) | Val::Str(_) => "String",
            Val::Int(_) => "Int",
            Val::Float(_) => "Float",
            Val::Bool(_) => "Bool",
            Val::Map(_) => "Map",
            Val::List(_) => "List",
            Val::Closure(_) => "Function",
            Val::RustFunction(_) => "Function",
            Val::RustFastFunction(_) => "Function",
            Val::RustFastFunctionNamed(_) => "Function",
            Val::RustFunctionNamed(_) => "Function",
            Val::AotFunction(_) => "Function",
            Val::Task(_) => "Task",
            Val::Channel(_) => "Channel",
            Val::Stream(_) => "Stream",
            Val::Iterator(_) => "Iterator",
            Val::MutationGuard(guard) => guard.guard_type(),
            Val::StreamCursor(_) => "StreamCursor",
            Val::Object(_) => "Object",
            Val::Nil => "Nil",
        }
    }

    #[inline]
    pub(crate) fn list_contains(list: &Arc<Vec<Val>>, needle: &Val) -> bool {
        if let Some(result) = cached_list_contains(list, needle) {
            result
        } else {
            (**list).contains(needle)
        }
    }

    #[inline]
    pub(crate) fn list_contains_all(list: &Arc<Vec<Val>>, subset: &Arc<Vec<Val>>) -> bool {
        subset.iter().all(|item| Val::list_contains(list, item))
    }

    /// Construct a runtime object of a named custom type
    #[inline]
    pub fn object<T: AsRef<str>>(type_name: T, fields: HashMap<String, Val>) -> Val {
        Val::Object(Arc::new(ObjectValue {
            type_name: ArcStr::from(type_name.as_ref()),
            fields: Arc::new(fields),
        }))
    }

    fn bind_positional_params(
        call_env: &mut VmContext,
        params: &[String],
        args: &[Val],
        layout_info: &CallLayoutInfo,
        collect_metrics: bool,
    ) {
        for (idx, (param, arg_val)) in params.iter().zip(args.iter()).enumerate() {
            let value = crate::vm::copy_call_arg_value_for_register_with_metrics(arg_val, collect_metrics);
            if let Some(slot) = layout_info.param_slots.get(idx).copied().flatten() {
                call_env.bind_param_at_slot(param.clone(), slot, value);
            } else if let Some(&slot) = layout_info.param_slot_by_name.get(param.as_str()) {
                call_env.bind_param_at_slot(param.clone(), slot, value);
            } else {
                call_env.define(param.clone(), value);
            }
        }
    }

    fn bind_named_param_value(
        call_env: &mut VmContext,
        decl: &NamedParamDecl,
        value: Val,
        layout_info: &CallLayoutInfo,
    ) {
        if let Some(&idx) = layout_info.param_slot_by_name.get(decl.name.as_str()) {
            call_env.bind_param_at_slot(decl.name.clone(), idx, value);
        } else {
            call_env.define(decl.name.clone(), value);
        }
    }

    #[inline]
    pub(crate) fn access(&self, field: &Val) -> Option<Val> {
        self.access_impl(field, None)
    }

    #[inline]
    pub(crate) fn access_with_metrics(&self, field: &Val, collect_metrics: bool) -> Option<Val> {
        self.access_impl(field, Some(collect_metrics))
    }

    #[inline]
    fn access_copy_value(value: &Val, collect_metrics: Option<bool>) -> Val {
        match collect_metrics {
            Some(collect_metrics) => crate::vm::copy_container_value_for_register_with_metrics(value, collect_metrics),
            None => value.clone(),
        }
    }

    #[inline]
    fn access_copy_slice(slice: &[Val], collect_metrics: Option<bool>) -> Arc<Vec<Val>> {
        if slice.is_empty() {
            return Arc::new(Vec::new());
        }
        match collect_metrics {
            Some(collect_metrics) => {
                let mut out = Vec::with_capacity(slice.len());
                for value in slice {
                    out.push(crate::vm::copy_container_value_for_register_with_metrics(
                        value,
                        collect_metrics,
                    ));
                }
                Arc::new(out)
            }
            None => Arc::new(slice.to_vec()),
        }
    }

    #[inline]
    fn access_impl(&self, field: &Val, collect_metrics: Option<bool>) -> Option<Val> {
        match (self, field) {
            // Map: field lookup by key only (do not shadow keys with synthetic fields)
            (Val::Map(m), key) if key.as_str().is_some() => {
                Self::map_get_str(m, key.as_str().unwrap()).map(|value| Self::access_copy_value(value, collect_metrics))
            }
            // String indexing and metadata
            (lhs, Val::Int(i)) if lhs.as_str().is_some() => {
                let s_str = lhs.as_str().unwrap();
                let len = if s_str.is_ascii() {
                    s_str.len()
                } else {
                    s_str.chars().count()
                };
                let idx = if *i < 0 {
                    len.checked_sub(i.unsigned_abs() as usize)?
                } else {
                    *i as usize
                };
                if s_str.is_ascii() {
                    let bs = s_str.as_bytes();
                    if idx < bs.len() {
                        Some(Val::ascii_char_value(bs[idx]))
                    } else {
                        None
                    }
                } else {
                    let ch = s_str.chars().nth(idx)?;
                    Some(Val::from_str(&ch.to_string()))
                }
            }
            (Val::List(l), Val::Int(i)) => {
                let idx = if *i < 0 {
                    l.len().checked_sub(i.unsigned_abs() as usize)?
                } else {
                    *i as usize
                };
                l.get(idx).map(|value| Self::access_copy_value(value, collect_metrics))
            }
            (Val::List(l), Val::List(key)) => {
                let (start, end) = range_key_bounds(key, l.len())?;
                Some(Val::List(Self::access_copy_slice(&l[start..end], collect_metrics)))
            }
            (lhs, Val::List(key)) if lhs.as_str().is_some() => {
                let text = lhs.as_str().unwrap();
                let len = if text.is_ascii() {
                    text.len()
                } else {
                    text.chars().count()
                };
                let (start, end) = range_key_bounds(key, len)?;
                if text.is_ascii() {
                    Some(Val::from_str(&text[start..end]))
                } else {
                    Some(Val::from_str(
                        &text
                            .chars()
                            .skip(start)
                            .take(end.saturating_sub(start))
                            .collect::<String>(),
                    ))
                }
            }
            (Val::List(l), key) if key.as_str() == Some("len") => Some(Val::Int(l.len() as i64)),
            (lhs, key) if lhs.as_str().is_some() && key.as_str() == Some("len") => {
                Some(Val::Int(lhs.as_str().unwrap().len() as i64))
            }
            // Map index -> [key, value]
            (Val::Map(m), Val::Int(i)) => {
                if *i < 0 {
                    return None;
                }
                let mut entries: Vec<_> = m.iter().collect();
                entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
                let idx = *i as usize;
                if idx >= entries.len() {
                    return None;
                }
                let (key, value) = entries[idx];
                Some(Val::List(
                    vec![
                        Val::from_str(key.as_str()),
                        Self::access_copy_value(value, collect_metrics),
                    ]
                    .into(),
                ))
            }
            (Val::Object(object), key) if key.as_str().is_some() => object
                .fields
                .get(key.as_str().unwrap())
                .map(|value| Self::access_copy_value(value, collect_metrics)),
            (Val::Task(task), key) if key.as_str() == Some("value") => match &task.value {
                Some(v) => Some(Self::access_copy_value(v, collect_metrics)),
                None => Some(Val::Nil),
            },
            (Val::Channel(channel), key) => match key.as_str() {
                Some("capacity") => Some(Val::Int(channel.capacity.unwrap_or(0))),
                Some("type") => Some(Val::from_str(&format!("{:?}", channel.inner_type))),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn clone_list_slice_with_metrics(slice: &[Val], collect_metrics: bool) -> Arc<Vec<Val>> {
        if slice.is_empty() {
            return Arc::new(Vec::new());
        }
        if !collect_metrics {
            return Arc::new(slice.to_vec());
        }
        let mut vec = Vec::with_capacity(slice.len());
        for value in slice {
            vec.push(crate::vm::copy_container_value_for_register_with_metrics(
                value,
                collect_metrics,
            ));
        }
        Arc::new(vec)
    }

    #[inline]
    pub(crate) fn concat_lists_with_metrics(left: &[Val], right: &[Val], collect_metrics: bool) -> Arc<Vec<Val>> {
        if left.is_empty() {
            return Self::clone_list_slice_with_metrics(right, collect_metrics);
        }
        if right.is_empty() {
            return Self::clone_list_slice_with_metrics(left, collect_metrics);
        }
        if !collect_metrics {
            let mut vec = Vec::with_capacity(left.len() + right.len());
            vec.extend_from_slice(left);
            vec.extend_from_slice(right);
            return Arc::new(vec);
        }
        let mut vec = Vec::with_capacity(left.len() + right.len());
        for value in left.iter().chain(right.iter()) {
            vec.push(crate::vm::copy_container_value_for_register_with_metrics(
                value,
                collect_metrics,
            ));
        }
        Arc::new(vec)
    }

    #[inline(always)]
    pub fn append_to_list(list: &[Val], value: &Val) -> Arc<Vec<Val>> {
        Self::append_to_list_with_metrics(list, value, crate::vm::vm_runtime_metrics_enabled())
    }

    #[inline(always)]
    pub fn append_to_list_with_metrics(list: &[Val], value: &Val, collect_metrics: bool) -> Arc<Vec<Val>> {
        if !collect_metrics {
            let mut vec = Vec::with_capacity(list.len() + 1);
            vec.extend_from_slice(list);
            vec.push(value.clone());
            return Arc::new(vec);
        }
        let mut vec = Vec::with_capacity(list.len() + 1);
        for item in list {
            vec.push(crate::vm::copy_container_value_for_register_with_metrics(
                item,
                collect_metrics,
            ));
        }
        vec.push(crate::vm::copy_container_value_for_register_with_metrics(
            value,
            collect_metrics,
        ));
        Arc::new(vec)
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        // Unify string comparisons across ShortStr and Str variants
        if let (Some(a), Some(b)) = (self.as_str(), other.as_str()) {
            return a == b;
        }
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => a == b,
            (Val::Float(a), Val::Float(b)) => a == b,
            (Val::Bool(a), Val::Bool(b)) => a == b,
            (Val::Map(a), Val::Map(b)) => a == b,
            (Val::List(a), Val::List(b)) => a == b,
            (Val::Closure(a), Val::Closure(b)) => {
                a.params == b.params && Arc::ptr_eq(&a.body, &b.body) && Arc::ptr_eq(&a.env, &b.env)
            }
            (Val::RustFunction(a), Val::RustFunction(b)) => std::ptr::fn_addr_eq(*a, *b),
            (Val::RustFastFunction(a), Val::RustFastFunction(b)) => std::ptr::fn_addr_eq(*a, *b),
            (Val::RustFastFunctionNamed(a), Val::RustFastFunctionNamed(b)) => std::ptr::fn_addr_eq(*a, *b),
            (Val::RustFunctionNamed(a), Val::RustFunctionNamed(b)) => std::ptr::fn_addr_eq(*a, *b),
            (Val::AotFunction(a), Val::AotFunction(b)) => a == b,
            (Val::Task(a), Val::Task(b)) => {
                let (a, b) = (a.as_ref(), b.as_ref());
                a.id == b.id && a.value == b.value
            }
            (Val::Channel(a), Val::Channel(b)) => {
                let (a, b) = (a.as_ref(), b.as_ref());
                a.id == b.id && a.capacity == b.capacity && a.inner_type == b.inner_type
            }
            (Val::Stream(a), Val::Stream(b)) => {
                let (a, b) = (a.as_ref(), b.as_ref());
                a.id == b.id && a.inner_type == b.inner_type
            }
            (Val::Iterator(a), Val::Iterator(b)) => Arc::ptr_eq(a, b),
            (Val::MutationGuard(a), Val::MutationGuard(b)) => Arc::ptr_eq(a, b),
            (Val::StreamCursor(a), Val::StreamCursor(b)) => a.id == b.id && a.stream_id == b.stream_id,
            (Val::Object(a), Val::Object(b)) => a.type_name == b.type_name && a.fields == b.fields,
            (Val::Nil, Val::Nil) => true,
            _ => false,
        }
    }
}

impl PartialOrd for Val {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => a.partial_cmp(b),
            (Val::Float(a), Val::Float(b)) => a.partial_cmp(b),
            (Val::Int(a), Val::Float(b)) => (*a as f64).partial_cmp(b),
            (Val::Float(a), Val::Int(b)) => a.partial_cmp(&(*b as f64)),
            _ => match (self.as_str(), other.as_str()) {
                (Some(a), Some(b)) => a.partial_cmp(b),
                _ => None,
            },
        }
    }
}

impl core::fmt::Display for Val {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Val::Int(i) => write!(f, "{i}"),
            Val::Float(fl) => write!(f, "{fl}"),
            Val::Bool(b) => write!(f, "{b}"),
            Val::ShortStr(s) => f.write_str(s.as_str()),
            Val::Str(s) => write!(f, "{s}"),
            Val::Map(m) => {
                // Avoid serialization errors by using debug fallback
                match serde_json::to_string(&**m) {
                    Ok(s) => write!(f, "{}", s),
                    Err(_) => write!(f, "{:?}", m),
                }
            }
            Val::List(l) => match serde_json::to_string(&**l) {
                Ok(s) => write!(f, "{}", s),
                Err(_) => write!(f, "{:?}", l),
            },
            Val::Closure(closure) => {
                write!(f, "fn({})", closure.params.join(", "))
            }
            Val::RustFunction(_)
            | Val::RustFastFunction(_)
            | Val::RustFastFunctionNamed(_)
            | Val::RustFunctionNamed(_)
            | Val::AotFunction(_) => {
                write!(f, "<native function>")
            }
            Val::Task(task) => match &task.value {
                Some(v) => write!(f, "Task(id={}, value={})", task.id, v),
                None => write!(f, "Task(id={}, pending)", task.id),
            },
            Val::Channel(channel) => {
                write!(
                    f,
                    "Channel(id={}, capacity={}, type={:?})",
                    channel.id,
                    channel.capacity.unwrap_or(0),
                    channel.inner_type
                )
            }
            Val::Stream(stream) => {
                write!(f, "Stream(id={}, type={:?})", stream.id, stream.inner_type)
            }
            Val::Iterator(iter) => {
                if let Some(origin) = iter.origin() {
                    write!(f, "<iterator:{}>", origin)
                } else {
                    write!(f, "<iterator>")
                }
            }
            Val::MutationGuard(guard) => write!(f, "<{}>", guard.guard_type()),
            Val::StreamCursor(cur) => {
                write!(f, "StreamCursor(id={}, stream={})", cur.id, cur.stream_id)
            }
            Val::Object(object) => {
                write!(f, "Object(type={}, fields={:?})", object.type_name, object.fields)
            }
            Val::Nil => write!(f, "nil"),
        }
    }
}

#[inline]
fn range_key_bounds(key: &[Val], len: usize) -> Option<(usize, usize)> {
    let Val::Int(first) = key.first()? else {
        return None;
    };
    let mut previous = *first;
    for item in key.iter().skip(1) {
        let Val::Int(current) = item else {
            return None;
        };
        if *current != previous + 1 {
            return None;
        }
        previous = *current;
    }

    let start = normalize_slice_bound(*first, len);
    let end = normalize_slice_bound(previous + 1, len);
    Some((start.min(end), end))
}

#[inline]
fn normalize_slice_bound(index: i64, len: usize) -> usize {
    if index < 0 {
        len.saturating_sub(index.unsigned_abs() as usize)
    } else {
        (index as usize).min(len)
    }
}

impl Val {
    /// Derive a static type hint suitable for method/trait dispatch.
    #[inline]
    pub fn dispatch_type(&self) -> Type {
        match self {
            Val::Int(_) => Type::Int,
            Val::Float(_) => Type::Float,
            Val::Bool(_) => Type::Bool,
            Val::ShortStr(_) | Val::Str(_) => Type::String,
            Val::List(_) => Type::List(Box::new(Type::Any)),
            Val::Map(_) => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
            Val::Object(object) => Type::Named(object.type_name.as_str().to_string()),
            Val::Task(_) => Type::Task(Box::new(Type::Any)),
            Val::Channel(channel) => Type::Channel(Box::new(channel.inner_type.clone())),
            Val::Stream(stream) => Type::Generic {
                name: "Stream".to_string(),
                params: vec![stream.inner_type.clone()],
            },
            Val::Iterator(_) => Type::Named("Iterator".to_string()),
            Val::MutationGuard(guard) => Type::Named(guard.guard_type().to_string()),
            Val::StreamCursor(_) => Type::Named("StreamCursor".to_string()),
            Val::Closure(_)
            | Val::RustFunction(_)
            | Val::RustFastFunction(_)
            | Val::RustFastFunctionNamed(_)
            | Val::RustFunctionNamed(_)
            | Val::AotFunction(_) => Type::Function {
                params: vec![],
                named_params: Vec::new(),
                return_type: Box::new(Type::Any),
            },
            Val::Nil => Type::Nil,
        }
    }

    /// Format the value into a String, preferring a user-defined Display-like
    /// trait method when available in the provided environment. This enables
    /// automatically using `impl Display for Type { fn display(self) -> String }`
    /// or a legacy `show(self) -> String` method if present via the trait/impl
    /// registry. Falls back to the built-in Display for Val.
    pub fn display_string(&self, ctx: Option<&VmContext>) -> String {
        // Fast path for primitives that don't need trait lookup
        match self {
            Val::ShortStr(s) => return s.as_str().to_string(),
            Val::Str(s) => return s.to_string(),
            Val::Int(i) => return i.to_string(),
            Val::Float(f) => return f.to_string(),
            Val::Bool(b) => return b.to_string(),
            Val::Nil => return "nil".to_string(),
            _ => {}
        }

        if let Some(ctx_ref) = ctx
            && let Some(tc) = ctx_ref.type_checker()
        {
            let method_val = tc
                .registry()
                .get_method(&self.dispatch_type(), "to_string")
                .or_else(|| tc.registry().get_method(&self.dispatch_type(), "show"));

            if let Some(fun_val) = method_val {
                // Create a temporary mutable context for method calls
                let mut temp_ctx = ctx_ref.clone();
                let call_res = fun_val.call(std::slice::from_ref(self), &mut temp_ctx);
                if let Ok(v) = call_res {
                    // If the method returned a string, use it directly; otherwise use default formatting of returned value
                    return match v.as_str() {
                        Some(s) => s.to_string(),
                        None => format!("{}", v),
                    };
                }
            }
        }

        // Fallback to default Display implementation for Val
        format!("{}", self)
    }
}
