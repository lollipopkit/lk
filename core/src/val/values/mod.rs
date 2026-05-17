use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fmt::{self, Debug},
    sync::{Arc, Mutex},
};

use arcstr::ArcStr;

use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};

// Using standard HashMap for maps and environments

use anyhow::{Result, anyhow};
use once_cell::sync::OnceCell;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::stmt::{NamedParamDecl, Program, Stmt};

use crate::vm::{CaptureSpec, Compiler, FrameInfo, Function, Vm, VmContext, with_current_vm};

use crate::resolve::slots::{FunctionLayout, SlotResolver};

mod cache;
mod convert;
mod intern;
mod ops;
mod types;

mod iter;

use cache::cached_list_contains;
use intern::intern;

pub use types::{FunctionNamedParamType, Type};

type DefaultSeedRegLayout = Vec<Option<Arc<[u16]>>>;

pub use iter::{IteratorState, IteratorValue, MutationGuardState, MutationGuardValue};

/// New VM-optimized function type that directly uses VmContext
pub type RustFunction = fn(args: &[Val], ctx: &mut VmContext) -> Result<Val>;

/// New VM-optimized function type that supports named arguments and uses VmContext
pub type RustFunctionNamed = fn(positional: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AotFunction {
    pub ptr: usize,
    pub arity: u8,
}

/// 内联短字符串：0–7 字节 UTF-8，完全存储在 Val 内（零堆分配）。
/// 实现了 Copy，克隆无需原子操作。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShortStr {
    len: u8,
    data: [u8; 7],
}

impl ShortStr {
    /// 从 str 创建。若 s.len() > 7 返回 None。
    #[inline]
    pub fn new(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() > 7 {
            return None;
        }
        let mut data = [0u8; 7];
        data[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            len: bytes.len() as u8,
            data,
        })
    }

    #[inline]
    pub fn from_char(ch: char) -> Self {
        let mut data = [0u8; 7];
        let encoded = ch.encode_utf8(&mut data);
        Self {
            len: encoded.len() as u8,
            data,
        }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: data 在构造时已验证为合法 UTF-8
        std::str::from_utf8(&self.data[..self.len as usize]).expect("ShortStr contains valid UTF-8")
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Debug for ShortStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl fmt::Display for ShortStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ShortStr> for ArcStr {
    fn from(s: ShortStr) -> ArcStr {
        ArcStr::from(s.as_str())
    }
}

impl serde::Serialize for ShortStr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ShortStr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ShortStr::new(&s).ok_or_else(|| serde::de::Error::custom("string too long for ShortStr"))
    }
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
                param_types: Vec::new(),
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
        self.frame_info_cache
            .get_or_init(|| {
                FrameInfo::new(
                    self.debug_name.as_deref().unwrap_or("<closure>"),
                    self.debug_location.as_deref(),
                )
            })
            .clone()
    }

    pub(crate) fn default_frame_info(&self, idx: usize) -> Option<FrameInfo> {
        let cache = self.default_frame_infos.get_or_init(|| {
            let base_info = self.frame_info();
            let base_name = base_info.name.clone();
            let base_location = base_info.location.clone();
            self.named_params
                .iter()
                .enumerate()
                .map(|(param_idx, decl)| {
                    if self.default_funcs.get(param_idx).and_then(|opt| opt.as_ref()).is_some() {
                        let default_name = format!("{}::<default:{}>", base_name.as_ref(), decl.name);
                        Some(FrameInfo::new(default_name, base_location.clone()))
                    } else {
                        None
                    }
                })
                .collect()
        });
        cache.get(idx).and_then(|info| info.clone())
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

    pub(crate) fn build_named_slots(&self, named: &[(String, Val)]) -> Result<Vec<Option<Val>>> {
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
            named_slots[idx] = Some(value.clone());
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

#[derive(Debug, Default, Clone)]
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
    /// 从 &str 构造 Val，≤7 字节走 ShortStr（零分配），更长走 Str(ArcStr)。
    #[inline]
    pub fn from_str(s: &str) -> Val {
        if let Some(short) = ShortStr::new(s) {
            Val::ShortStr(short)
        } else {
            Val::Str(intern(s))
        }
    }

    #[inline]
    pub fn str_intern(s: &str) -> Val {
        Val::Str(intern(s))
    }

    #[inline]
    pub fn intern_str(s: &str) -> ArcStr {
        intern(s)
    }

    /// 若 Val 是字符串变体，返回 &str；否则返回 None。
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::ShortStr(s) => Some(s.as_str()),
            Val::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

impl Type {
    pub fn validate(&self, val: &Val) -> Result<()> {
        match (self, val) {
            // Primitive types
            (Type::Int, Val::Int(_)) => Ok(()),
            (Type::Float, Val::Float(_)) => Ok(()),
            (Type::Float, Val::Int(_)) => Ok(()),
            (Type::String, Val::ShortStr(_)) | (Type::String, Val::Str(_)) => Ok(()),
            (Type::Bool, Val::Bool(_)) => Ok(()),
            (Type::Nil, Val::Nil) => Ok(()),
            (Type::Boxed(inner), value) => {
                if matches!(**inner, Type::Any) {
                    Ok(())
                } else {
                    inner.validate(value)
                }
            }

            // Any type accepts everything
            (Type::Any, _) => Ok(()),

            // Generic container types
            (Type::List(elem_type), Val::List(list)) => {
                // Validate all elements match the expected type
                for item in list.iter() {
                    elem_type.validate(item)?;
                }
                Ok(())
            }
            (Type::Map(key_type, val_type), Val::Map(map)) => {
                // Validate all keys and values match expected types
                for (k, v) in map.iter() {
                    let key_val = Val::from_str(k.as_str());
                    key_type.validate(&key_val)?;
                    val_type.validate(v)?;
                }
                Ok(())
            }
            // Tuple types validate against lists of identical arity
            (Type::Tuple(elems), Val::List(list)) => {
                if list.len() != elems.len() {
                    return Err(anyhow!(
                        "Tuple length mismatch: expected {}, got {}",
                        elems.len(),
                        list.len()
                    ));
                }
                for (i, (et, v)) in elems.iter().zip(list.iter()).enumerate() {
                    et.validate(v).map_err(|e| anyhow!("Tuple element {}: {}", i, e))?;
                }
                Ok(())
            }

            // Function types
            (Type::Function { .. }, Val::Closure(_)) => Ok(()),
            (Type::Function { .. }, Val::RustFunction(_)) => Ok(()),
            (Type::Function { .. }, Val::RustFunctionNamed(_)) => Ok(()),
            (Type::Function { .. }, Val::AotFunction(_)) => Ok(()),

            // Concurrency types
            (Type::Task(inner_type), Val::Task(task)) => {
                if let Some(v) = &task.value {
                    inner_type.validate(v)?;
                }
                Ok(())
            }
            // Stream<T> represented as Generic { name: "Stream", params: [T] }
            (Type::Generic { name, params }, Val::Stream(stream)) if name == "Stream" && params.len() == 1 => {
                let expected_inner = &params[0];
                if expected_inner == &stream.inner_type || expected_inner.is_assignable_to(&stream.inner_type) {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "Stream type mismatch: expected Stream<{:?}>, got Stream<{:?}>",
                        expected_inner,
                        stream.inner_type
                    ))
                }
            }
            (Type::Channel(inner_type), Val::Channel(channel)) => {
                if inner_type.as_ref() == &channel.inner_type {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "Channel type mismatch: expected {:?}, got {:?}",
                        inner_type,
                        channel.inner_type
                    ))
                }
            }

            // Union types - value must match at least one type in the union
            (Type::Union(types), val) => {
                for typ in types {
                    if typ.validate(val).is_ok() {
                        return Ok(());
                    }
                }
                Err(anyhow!(
                    "Union type mismatch: value {:?} doesn't match any of {:?}",
                    val.type_name(),
                    types
                ))
            }

            // Optional types - value must be Nil or match the inner type
            (Type::Optional(_inner_type), Val::Nil) => Ok(()),
            (Type::Optional(inner_type), val) => inner_type.validate(val),

            // Type variables and named types are handled by the type checker
            (Type::Variable(_), _) => Ok(()), // Always valid during inference
            (Type::Named(_), _) => Ok(()),    // Validated by type registry

            // Generic types are validated by the type system
            (Type::Generic { .. }, _) => Ok(()),

            // Iterator / mutation guard currently surface as opaque runtime types.
            (expected, actual @ Val::Iterator(_)) | (expected, actual @ Val::MutationGuard(_)) => Err(anyhow!(
                "Type mismatch: expected {:?}, got {:?}",
                expected,
                actual.type_name()
            )),
            // Type mismatch
            (expected, actual) => Err(anyhow!(
                "Type mismatch: expected {:?}, got {:?}",
                expected,
                actual.type_name()
            )),
        }
    }
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

    fn bind_positional_params(call_env: &mut VmContext, params: &[String], args: &[Val], layout_info: &CallLayoutInfo) {
        for (idx, (param, arg_val)) in params.iter().zip(args.iter()).enumerate() {
            if let Some(slot) = layout_info.param_slots.get(idx).copied().flatten() {
                call_env.bind_param_at_slot(param.clone(), slot, arg_val.clone());
            } else if let Some(&slot) = layout_info.param_slot_by_name.get(param.as_str()) {
                call_env.bind_param_at_slot(param.clone(), slot, arg_val.clone());
            } else {
                call_env.define(param.clone(), arg_val.clone());
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

    /// Call this value as a function with the given arguments
    #[inline]
    fn call_with_mode(&self, args: &[Val], ctx: &mut VmContext, force_vm: bool) -> Result<Val> {
        let _ = force_vm;
        let _ = vm_fast_path_forced();
        match self {
            #[cfg(feature = "aot-minimal-runtime")]
            Val::Closure(_) => Err(anyhow!("AOT minimal runtime cannot call VM closures")),
            #[cfg(not(feature = "aot-minimal-runtime"))]
            Val::Closure(closure_arc) => {
                let closure = closure_arc.as_ref();
                let params = closure.params.as_ref();
                if args.len() != params.len() {
                    return Err(anyhow!(
                        "Function expects {} arguments, got {}",
                        params.len(),
                        args.len()
                    ));
                }
                let scope_capacity = params.len() + closure.named_params.len();
                let mut named_slots: Vec<Option<Val>> = vec![None; closure.named_params.len()];
                closure.with_call_env(ctx, scope_capacity, |call_env, layout_info| {
                    let frame_info = closure.frame_info();
                    call_env.push_call_frame(frame_info.name.clone(), frame_info.location.clone());
                    let result = Self::call_named_vm_fast(closure, args, &mut named_slots, call_env, layout_info);
                    call_env.pop_call_frame();
                    result
                })
            }
            Val::RustFunction(func) => func(args, ctx),
            Val::RustFunctionNamed(func) => func(args, &[], ctx),
            _ => Err(anyhow!("{} is not a function", self.type_name())),
        }
    }

    #[inline]
    pub fn call(&self, args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        self.call_with_mode(args, ctx, false)
    }

    #[inline]
    pub fn call_vm(&self, args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        self.call_with_mode(args, ctx, true)
    }

    /// Call a function value with positional and named arguments.
    /// Named arguments are supported for user-defined closures and opt-in native functions.
    fn call_named_with_mode(
        &self,
        pos: &[Val],
        named: &[(String, Val)],
        ctx: &mut VmContext,
        force_vm: bool,
    ) -> Result<Val> {
        let _ = force_vm;
        let _ = vm_fast_path_forced();
        match self {
            #[cfg(feature = "aot-minimal-runtime")]
            Val::Closure(_) => Err(anyhow!("AOT minimal runtime cannot call VM closures")),
            #[cfg(not(feature = "aot-minimal-runtime"))]
            Val::Closure(closure_arc) => {
                let closure = closure_arc.as_ref();
                let params = closure.params.as_ref();
                if pos.len() != params.len() {
                    return Err(anyhow!(
                        "Function expects {} positional arguments, got {}",
                        params.len(),
                        pos.len()
                    ));
                }
                let mut named_slots = closure.build_named_slots(named)?;
                let scope_capacity = params.len() + closure.named_params.len();
                closure.with_call_env(ctx, scope_capacity, |call_env, layout_info| {
                    let frame_info = closure.frame_info();
                    call_env.push_call_frame(frame_info.name.clone(), frame_info.location.clone());
                    let result = Self::call_named_vm_fast(closure, pos, &mut named_slots, call_env, layout_info);
                    call_env.pop_call_frame();
                    result
                })
            }
            Val::RustFunction(_func) => {
                if named.is_empty() {
                    self.call_with_mode(pos, ctx, force_vm)
                } else {
                    Err(anyhow!("Named arguments are not supported for native functions"))
                }
            }
            Val::RustFunctionNamed(func) => func(pos, named, ctx),
            _ => Err(anyhow!("{} is not a function", self.type_name())),
        }
    }

    fn call_named_vm_fast(
        closure: &ClosureValue,
        pos: &[Val],
        named_slots: &mut [Option<Val>],
        call_env: &mut VmContext,
        layout_info: &CallLayoutInfo,
    ) -> Result<Val> {
        let frame_info = closure.frame_info();
        let params = closure.params.as_ref();
        let named_params = closure.named_params.as_ref();
        let named_kinds = closure.named_param_kinds();
        let fun = closure.code.get_or_init(|| {
            let c = Compiler::new();
            Arc::new(c.compile_function_with_captures(
                params,
                named_params,
                closure.body.as_ref(),
                closure.capture_specs.as_ref(),
            ))
        });
        Self::bind_positional_params(call_env, params, pos, layout_info);
        let named_regs = &fun.named_param_regs;
        debug_assert_eq!(
            named_regs.len(),
            named_params.len(),
            "named param register layout mismatch ({} regs vs {} params)",
            named_regs.len(),
            named_params.len()
        );
        let mut named_seed_pairs: Vec<(usize, Val)> = Vec::with_capacity(named_params.len());
        let mut named_seed: Vec<(u16, Val)> = Vec::with_capacity(named_params.len());
        for (idx, decl) in named_params.iter().enumerate() {
            let kind = named_kinds.get(idx).copied().unwrap_or(NamedParamKind::Required);
            let value = if let Some(val) = named_slots.get_mut(idx).and_then(|slot| slot.take()) {
                val
            } else {
                match kind {
                    NamedParamKind::DefaultThunk => {
                        let default_fun = closure
                            .default_funcs
                            .get(idx)
                            .and_then(|opt| opt.as_ref())
                            .expect("default function must exist for DefaultThunk kind");
                        let default_frame = closure
                            .default_frame_info(idx)
                            .expect("default frame info should exist");
                        let layout = closure
                            .default_seed_regs(idx)
                            .expect("default seed layout should exist for default thunk");
                        let mut default_named_seed: Vec<(u16, Val)> = Vec::with_capacity(named_seed_pairs.len());
                        for (seed_idx, seed_val) in named_seed_pairs.iter() {
                            let reg = layout
                                .get(*seed_idx)
                                .copied()
                                .expect("default seed layout must cover parent indices");
                            default_named_seed.push((reg, seed_val.clone()));
                        }
                        Self::exec_function_with_bindings(
                            default_fun,
                            call_env,
                            pos,
                            default_named_seed.as_slice(),
                            &closure.captures,
                            &closure.capture_specs,
                            Some(default_frame.clone()),
                        )?
                    }
                    NamedParamKind::OptionalNil => Val::Nil,
                    NamedParamKind::Required => {
                        return Err(anyhow!("Missing required named argument: {}", decl.name));
                    }
                }
            };
            Self::bind_named_param_value(call_env, decl, value.clone(), layout_info);
            named_seed_pairs.push((idx, value.clone()));
            named_seed.push((named_regs[idx], value));
        }
        call_env.preload_slot_mappings_per_depth(&layout_info.locals);
        Self::exec_function_with_bindings(
            fun.as_ref(),
            call_env,
            pos,
            named_seed.as_slice(),
            &closure.captures,
            &closure.capture_specs,
            Some(frame_info.clone()),
        )
    }

    fn exec_function_with_bindings(
        fun: &Function,
        env: &mut VmContext,
        pos: &[Val],
        named_seed: &[(u16, Val)],
        captures: &Arc<ClosureCapture>,
        capture_specs: &Arc<Vec<CaptureSpec>>,
        frame_info: Option<FrameInfo>,
    ) -> Result<Val> {
        if let Some(res) = with_current_vm(|vm| {
            vm.exec_with_bindings(
                fun,
                env,
                Some(pos),
                named_seed,
                Some(Arc::clone(captures)),
                Some(Arc::clone(capture_specs)),
                frame_info.clone(),
            )
        }) {
            res
        } else {
            thread_local! {
                static VM_POOL_NAMED_CALL: RefCell<Option<Vm>> = const { RefCell::new(None) };
            }
            let mut vm = VM_POOL_NAMED_CALL
                .with(|cell| cell.borrow_mut().take())
                .unwrap_or_default();
            let res = vm.exec_with_bindings(
                fun,
                env,
                Some(pos),
                named_seed,
                Some(Arc::clone(captures)),
                Some(Arc::clone(capture_specs)),
                frame_info,
            );
            VM_POOL_NAMED_CALL.with(|cell| {
                let _ = cell.borrow_mut().replace(vm);
            });
            res
        }
    }

    pub fn call_named(&self, pos: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val> {
        self.call_named_with_mode(pos, named, ctx, false)
    }

    pub fn call_named_vm(&self, pos: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val> {
        self.call_named_with_mode(pos, named, ctx, true)
    }
    #[inline]
    pub(crate) fn access(&self, field: &Val) -> Option<Val> {
        match (self, field) {
            // Map: field lookup by key only (do not shadow keys with synthetic fields)
            (Val::Map(m), key) if key.as_str().is_some() => m.get(key.as_str().unwrap()).cloned(),
            // String indexing and metadata
            (lhs, Val::Int(i)) if lhs.as_str().is_some() => {
                let s_str = lhs.as_str().unwrap();
                if *i < 0 {
                    return None;
                }
                let idx = *i as usize;
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
                if *i < 0 {
                    return None;
                }
                l.get(*i as usize).cloned()
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
                Some(Val::List(vec![Val::from_str(key.as_str()), value.clone()].into()))
            }
            (Val::Object(object), key) if key.as_str().is_some() => object.fields.get(key.as_str().unwrap()).cloned(),
            (Val::Task(task), key) if key.as_str() == Some("value") => match &task.value {
                Some(v) => Some(v.clone()),
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

    /// Fast string concatenation — hot path for `s = s + "x"` loops.
    #[inline]
    pub(crate) fn concat_strings(a: &str, b: &str) -> Val {
        if a.is_empty() {
            return Val::from_str(b);
        }
        if b.is_empty() {
            return Val::from_str(a);
        }
        let total = a.len() + b.len();
        let mut s = String::with_capacity(total);
        s.push_str(a);
        s.push_str(b);
        Val::from_str(&s)
    }

    #[inline]
    pub(crate) fn to_str_value(value: &Val) -> Val {
        match value {
            Val::ShortStr(s) => Val::ShortStr(*s),
            Val::Str(s) => Val::Str(s.clone()),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Val::from_str(buf.format(*i))
            }
            Val::Float(f) => {
                let mut buf = ryu::Buffer::new();
                Val::from_str(buf.format(*f))
            }
            Val::Bool(true) => Val::ShortStr(ShortStr::new("true").unwrap()),
            Val::Bool(false) => Val::ShortStr(ShortStr::new("false").unwrap()),
            Val::Nil => Val::ShortStr(ShortStr::new("nil").unwrap()),
            other => Val::from_str(&other.to_string()),
        }
    }

    #[inline]
    pub(crate) fn ascii_char_value(byte: u8) -> Val {
        debug_assert!(byte.is_ascii());
        Val::ShortStr(ShortStr::from_char(byte as char))
    }

    #[inline]
    pub(crate) fn concat_str_add_rhs(prefix: &str, rhs: &Val) -> Option<Val> {
        match rhs {
            Val::ShortStr(s) => Some(Self::concat_strings(prefix, s.as_str())),
            Val::Str(s) => Some(Self::concat_strings(prefix, s.as_str())),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Some(Self::concat_strings(prefix, buf.format(*i)))
            }
            Val::Float(f) => {
                let mut buf = ryu::Buffer::new();
                Some(Self::concat_strings(prefix, buf.format(*f)))
            }
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn concat_add_lhs_str(lhs: &Val, suffix: &str) -> Option<Val> {
        match lhs {
            Val::ShortStr(s) => Some(Self::concat_strings(s.as_str(), suffix)),
            Val::Str(s) => Some(Self::concat_strings(s.as_str(), suffix)),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Some(Self::concat_strings(buf.format(*i), suffix))
            }
            Val::Float(f) => {
                let mut buf = ryu::Buffer::new();
                Some(Self::concat_strings(buf.format(*f), suffix))
            }
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn clone_list_slice(slice: &[Val]) -> Arc<Vec<Val>> {
        if slice.is_empty() {
            return Arc::new(Vec::new());
        }
        Arc::new(slice.to_vec())
    }

    #[inline]
    pub(crate) fn concat_lists(left: &[Val], right: &[Val]) -> Arc<Vec<Val>> {
        if left.is_empty() {
            return Self::clone_list_slice(right);
        }
        if right.is_empty() {
            return Self::clone_list_slice(left);
        }
        let mut vec = Vec::with_capacity(left.len() + right.len());
        vec.extend_from_slice(left);
        vec.extend_from_slice(right);
        Arc::new(vec)
    }

    #[inline(always)]
    pub fn append_to_list(list: &[Val], value: &Val) -> Arc<Vec<Val>> {
        let mut vec = Vec::with_capacity(list.len() + 1);
        vec.extend_from_slice(list);
        vec.push(value.clone());
        Arc::new(vec)
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        // Unify string comparisons across ShortStr and Str variants
        match (self.as_str(), other.as_str()) {
            (Some(a), Some(b)) => return a == b,
            _ => {}
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

impl Serialize for Val {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Val::ShortStr(s) => serializer.serialize_str(s.as_str()),
            Val::Str(s) => serializer.serialize_str(s.as_ref()),
            Val::Int(i) => serializer.serialize_i64(*i),
            Val::Float(f) => serializer.serialize_f64(*f),
            Val::Bool(b) => serializer.serialize_bool(*b),
            Val::Map(m) => (**m).serialize(serializer),
            Val::List(l) => (**l).serialize(serializer),
            Val::Closure(_) | Val::RustFunction(_) | Val::RustFunctionNamed(_) | Val::AotFunction(_) => {
                // Functions can't be serialized, use placeholder
                serializer.serialize_str("<function>")
            }
            Val::Iterator(_) => serializer.serialize_str("<iterator>"),
            Val::MutationGuard(_) => serializer.serialize_str("<mutation-guard>"),
            Val::Task(task) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "task")?;
                map.serialize_entry("value", &task.value)?;
                map.end()
            }
            Val::Channel(channel) => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "channel")?;
                map.serialize_entry("capacity", &channel.capacity)?;
                map.serialize_entry("inner_type", &format!("{:?}", channel.inner_type))?;
                map.end()
            }
            Val::Stream(stream) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "stream")?;
                map.serialize_entry("inner_type", &format!("{:?}", stream.inner_type))?;
                map.end()
            }
            Val::StreamCursor(cur) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "stream_cursor")?;
                map.serialize_entry("stream_id", &cur.stream_id)?;
                map.end()
            }
            Val::Object(object) => {
                let mut map = serializer.serialize_map(Some(object.fields.len() + 1))?;
                map.serialize_entry("__type", object.type_name.as_str())?;
                for (k, v) in object.fields.iter() {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
            Val::Nil => serializer.serialize_unit(),
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
            Val::RustFunction(_) | Val::RustFunctionNamed(_) | Val::AotFunction(_) => {
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
            Val::Closure(_) | Val::RustFunction(_) | Val::RustFunctionNamed(_) | Val::AotFunction(_) => {
                Type::Function {
                    params: vec![],
                    named_params: Vec::new(),
                    return_type: Box::new(Type::Any),
                }
            }
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
