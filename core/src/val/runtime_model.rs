//! New runtime value model for the VM rewrite.
//!
//! The `LiteralVal` enum remains active while the compiler and executor are migrated.
//! New VM code should target these types first.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::util::fast_map::{FastHashMap, FastHashSet, fast_hash_map_from_iter, fast_hash_map_new, fast_hash_set_new};
use alloc::sync::Arc;

use crate::val::{ShortStr, Type};
use crate::vm::DeclaredType;

mod equality;
mod heap;

pub use equality::{runtime_value_equals_str, runtime_values_equal};

/// How far the runtime will walk into a value before giving up.
///
/// Comparing and rendering both recurse on the *shape* of a value, so a chain a
/// loop can build —
///
/// ```lk
/// let node: Any = [1];
/// for i in 0..200000 { node = [node]; }
/// ```
///
/// — put 200000 frames on the Rust stack and aborted the process with
/// `fatal runtime error: stack overflow`. A script must not be able to do that.
/// Past this depth those walks raise an ordinary catchable error instead, which
/// is what Python and Lua do with the same problem.
///
/// Generous for data — JSON nests single digits deep, a hand-written tree tens
/// — and far below the number of Rust frames the real stack would take.
pub const MAX_VALUE_DEPTH: u32 = 512;
pub use heap::{HeapRef, HeapStore};

/// A value, 16 bytes and `Copy`.
///
/// **No `PartialEq`, deliberately.** A derived one means two different things
/// for two of these variants: structural for a `ShortStr`, and *handle
/// identity* for an `Obj`. Every place that reached for `==` got identity
/// without noticing, and `ShortStr`'s seven-byte inline limit made half the
/// cases accidentally right — so the bug looked like "strings longer than
/// seven characters", which is not a thing any reader would suspect:
///
/// ```text
/// ["ab", "cd"].contains("ab")                 → true
/// ["abcdefghij", …].contains("abcdefghij")    → false
/// assert_eq("abcdefghij", "abcdefghij")       → failed (in the playground)
/// ```
///
/// Equality needs the heap, so it cannot be a `PartialEq` impl at all: it is
/// [`runtime_values_equal`], which takes the heap. Asking for it is a decision;
/// `==` was not.
///
/// [`RuntimeVal::same_value_or_handle`] is the escape hatch for the places that
/// genuinely mean "the same nil/bool/int, or literally the same object".
#[derive(Clone, Copy, Debug)]
pub enum RuntimeVal {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    ShortStr(ShortStr),
    Obj(HeapRef),
}

/// Equality for tests only.
///
/// Production code must not have this: `==` on a `RuntimeVal` can only compare
/// handles, and the whole point of removing the derive is that reaching for it
/// stops being possible by accident. A test, though, is written against known
/// values and says what it means — `assert_eq!(returned, RuntimeVal::Int(55))`
/// is about that integer, not about which handle it arrived on.
///
/// Gated on `cfg(test)`, so it exists while `lk-core`'s own tests compile and
/// nowhere else. A downstream crate that needs it enables the `testing`
/// feature.
#[cfg(any(test, feature = "testing"))]
impl PartialEq for RuntimeVal {
    fn eq(&self, other: &Self) -> bool {
        self.same_value_or_handle(other)
    }
}

impl Default for RuntimeVal {
    #[inline]
    fn default() -> Self {
        Self::Nil
    }
}

impl RuntimeVal {
    #[inline]
    pub const fn kind(&self) -> RuntimeValKind {
        match self {
            Self::Nil => RuntimeValKind::Nil,
            Self::Bool(_) => RuntimeValKind::Bool,
            Self::Int(_) => RuntimeValKind::Int,
            Self::Float(_) => RuntimeValKind::Float,
            Self::ShortStr(_) => RuntimeValKind::ShortStr,
            Self::Obj(_) => RuntimeValKind::Obj,
        }
    }

    /// The same scalar, or literally the same heap object.
    ///
    /// This is what the derived `PartialEq` used to provide silently. It is
    /// still the right question in a few places — deduplicating a constant
    /// pool, telling whether two registers hold the same object — and wrong in
    /// every place that means "equal". Having to name it is the point: the
    /// language's `==` is [`runtime_values_equal`], which needs the heap and
    /// answers by value.
    #[inline]
    pub fn same_value_or_handle(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Int(left), Self::Int(right)) => left == right,
            // Bit equality, so that two `NaN`s from the same source dedup and
            // `0.0`/`-0.0` stay distinct — this is identity, not arithmetic.
            (Self::Float(left), Self::Float(right)) => left.to_bits() == right.to_bits(),
            (Self::ShortStr(left), Self::ShortStr(right)) => left.as_str() == right.as_str(),
            (Self::Obj(left), Self::Obj(right)) => left == right,
            _ => false,
        }
    }

    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeValKind {
    Nil,
    Bool,
    Int,
    Float,
    ShortStr,
    Obj,
}

#[derive(Clone, Debug)]
pub enum HeapValue {
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    List(TypedList),
    Map(TypedMap),
    Set(RuntimeSet),
    Callable(CallableValue),
    Task(Arc<TaskValue>),
    Channel(Arc<ChannelValue>),
    Stream(Arc<StreamValue>),
    StreamCursor(Arc<StreamCursorValue>),
    Slice(Arc<SliceValue>),
    Resource(Arc<ResourceValue>),
    Object(RuntimeObject),
    UpvalCell(RuntimeVal),
    ErrorVal(ErrorVal),
}

impl HeapValue {
    #[inline]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "String",
            Self::Bytes(_) => "Bytes",
            Self::List(_) => "List",
            Self::Map(_) => "Map",
            Self::Set(_) => "Set",
            Self::Callable(_) => "Function",
            Self::Task(_) => "Task",
            Self::Channel(_) => "Channel",
            Self::Stream(_) => "Stream",
            Self::StreamCursor(_) => "StreamCursor",
            Self::Slice(_) => "Slice",
            Self::Resource(resource) => resource.kind,
            Self::Object(_) => "Object",
            Self::UpvalCell(_) => "UpvalCell",
            Self::ErrorVal(_) => "Error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSet {
    entries: FastHashSet<RuntimeMapKey>,
}

impl RuntimeSet {
    pub fn new() -> Self {
        Self {
            entries: fast_hash_set_new(),
        }
    }

    pub fn from_entries(entries: FastHashSet<RuntimeMapKey>) -> Self {
        Self { entries }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub fn contains(&self, key: &RuntimeMapKey) -> bool {
        self.entries.contains(key)
    }

    #[inline]
    pub fn insert(&mut self, key: RuntimeMapKey) -> bool {
        self.entries.insert(key)
    }

    #[inline]
    pub fn remove(&mut self, key: &RuntimeMapKey) -> bool {
        self.entries.remove(key)
    }

    #[inline]
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn entries(&self) -> impl Iterator<Item = &RuntimeMapKey> {
        self.entries.iter()
    }
}

impl Default for RuntimeSet {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub enum CallableValue {
    Closure {
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
    },
    RuntimeNative {
        name: Arc<str>,
        arity: u16,
        function: crate::vm::NativeFunction,
    },
    Runtime(Arc<crate::vm::RuntimeCallable>),
}

#[derive(Clone, Debug)]
pub struct RuntimeObject {
    /// This object's type identity: the declaring module *and* the name. The
    /// name alone is only unique within one module, so dispatching on it made
    /// two modules' identically-named structs the same type (see
    /// [`crate::vm::TypeScope`]).
    ///
    /// Shared by `Arc` rather than stored inline — see [`DeclaredType`] for why
    /// this struct's width is worth caring about.
    pub ty: Arc<DeclaredType>,
    pub fields: FastHashMap<Arc<str>, RuntimeVal>,
    pub field_slots: Vec<Arc<str>>,
}

impl RuntimeObject {
    pub fn new(ty: Arc<DeclaredType>, fields: FastHashMap<Arc<str>, RuntimeVal>) -> Self {
        let mut field_slots = Vec::with_capacity(fields.len());
        for key in fields.keys() {
            field_slots.push(Arc::clone(key));
        }
        Self {
            ty,
            fields,
            field_slots,
        }
    }

    #[inline]
    pub fn type_name(&self) -> &Arc<str> {
        &self.ty.name
    }

    #[inline]
    pub fn type_scope(&self) -> &crate::vm::TypeScope {
        &self.ty.scope
    }

    pub fn field_slot(&self, key: &str) -> Option<usize> {
        self.field_slots.iter().position(|candidate| candidate.as_ref() == key)
    }

    pub fn get_field(&self, key: &str) -> Option<RuntimeVal> {
        self.fields.get(key).cloned()
    }

    pub fn get_field_slot(&self, slot: usize, key: &str) -> Option<RuntimeVal> {
        let slot_key = self.field_slots.get(slot)?;
        if slot_key.as_ref() == key {
            self.fields.get(slot_key).cloned()
        } else {
            None
        }
    }

    pub fn set_field(&mut self, key: Arc<str>, value: RuntimeVal) {
        if !self.fields.contains_key(key.as_ref()) {
            self.field_slots.push(key.clone());
        }
        self.fields.insert(key, value);
    }
}

/// A raised error: its message, and the values along the way.
///
/// No `PartialEq`: two errors are compared by their *message*, which
/// `same_message` says, and never by their traces — those are `RuntimeVal`s,
/// and comparing them without the heap would compare handles.
#[derive(Clone, Debug)]
pub struct ErrorVal {
    pub message: Arc<str>,
    pub trace: Vec<RuntimeVal>,
}

impl ErrorVal {
    #[inline]
    pub fn same_message(&self, other: &Self) -> bool {
        self.message == other.message
    }
}

/// The one shape a whole list of runtime values shares, if any.
///
/// Companion to [`TypedList::from_runtime_values`]; lives beside it so the
/// narrowing rule and the shapes it can produce cannot drift apart.
pub(crate) enum RuntimeListShape {
    Mixed,
    Int,
    Float,
    Bool,
    String,
}

pub(crate) fn runtime_value_list_shape(values: &[RuntimeVal], heap: &HeapStore) -> RuntimeListShape {
    if values.is_empty() {
        return RuntimeListShape::Mixed;
    }
    let mut shape: Option<RuntimeListShape> = None;
    for value in values {
        let next = match value {
            RuntimeVal::Int(_) => RuntimeListShape::Int,
            RuntimeVal::Float(_) => RuntimeListShape::Float,
            RuntimeVal::Bool(_) => RuntimeListShape::Bool,
            RuntimeVal::ShortStr(_) => RuntimeListShape::String,
            RuntimeVal::Obj(handle) if matches!(heap.get(*handle), Some(HeapValue::String(_))) => {
                RuntimeListShape::String
            }
            _ => return RuntimeListShape::Mixed,
        };
        match (&shape, next) {
            (None, next) => shape = Some(next),
            (Some(RuntimeListShape::Int), RuntimeListShape::Int)
            | (Some(RuntimeListShape::Float), RuntimeListShape::Float)
            | (Some(RuntimeListShape::Bool), RuntimeListShape::Bool)
            | (Some(RuntimeListShape::String), RuntimeListShape::String) => {}
            _ => return RuntimeListShape::Mixed,
        }
    }
    shape.unwrap_or(RuntimeListShape::Mixed)
}

#[derive(Clone, Debug)]
pub enum TypedList {
    Mixed(Vec<RuntimeVal>),
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<Arc<str>>),
}

impl TypedList {
    /// Build a list from runtime values, keeping the compact representation
    /// when they all share one shape.
    ///
    /// A list should look the same whether it came from a literal, a `push`
    /// loop, or a projection like `map` / `keys` / `values`. Producers that
    /// reach for `Mixed` directly are invisible in the *answer* but not in the
    /// cost: `Mixed` holds 16 bytes per element instead of 8, and every typed
    /// fast path downstream (arithmetic, `sort`, index reads) drops to the
    /// generic one. The scan is O(n) over a vector the caller just built.
    pub fn from_runtime_values(values: &[RuntimeVal], heap: &HeapStore) -> Self {
        match runtime_value_list_shape(values, heap) {
            RuntimeListShape::Mixed => Self::Mixed(values.to_vec()),
            RuntimeListShape::Int => Self::Int(
                values
                    .iter()
                    .map(|value| match value {
                        RuntimeVal::Int(value) => *value,
                        _ => unreachable!("shape scan only returns Int for int values"),
                    })
                    .collect(),
            ),
            RuntimeListShape::Float => Self::Float(
                values
                    .iter()
                    .map(|value| match value {
                        RuntimeVal::Float(value) => *value,
                        _ => unreachable!("shape scan only returns Float for float values"),
                    })
                    .collect(),
            ),
            RuntimeListShape::Bool => Self::Bool(
                values
                    .iter()
                    .map(|value| match value {
                        RuntimeVal::Bool(value) => *value,
                        _ => unreachable!("shape scan only returns Bool for bool values"),
                    })
                    .collect(),
            ),
            RuntimeListShape::String => Self::String(
                values
                    .iter()
                    .map(|value| match value {
                        RuntimeVal::ShortStr(value) => Arc::<str>::from(value.as_str()),
                        RuntimeVal::Obj(handle) => match heap.get(*handle) {
                            Some(HeapValue::String(value)) => Arc::clone(value),
                            _ => unreachable!("shape scan only returns String for string values"),
                        },
                        _ => unreachable!("shape scan only returns String for string values"),
                    })
                    .collect(),
            ),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::Mixed(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::String(values) => values.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append `value`, widening the representation only when it has to.
    ///
    /// `string_value` is the value's text when it is a string — a
    /// `TypedList::String` holds `Arc<str>`, which a `RuntimeVal` cannot carry
    /// past seven bytes, so the caller reads it out of the heap *before* taking
    /// the mutable borrow.
    ///
    /// This lived in the executor, so `vm::context`'s list methods — the path a
    /// native/host caller takes — could not reach it and copied the whole list
    /// through `from_runtime_values` instead. Same operation, two answers to
    /// "does pushing change this list".
    pub fn push(&mut self, value: RuntimeVal, string_value: Option<Arc<str>>) -> anyhow::Result<()> {
        let list = self;
        match list {
            TypedList::Mixed(values) if values.is_empty() => match (value, string_value) {
                (RuntimeVal::Int(value), _) => *list = TypedList::Int(vec![value]),
                (RuntimeVal::Float(value), _) => *list = TypedList::Float(vec![value]),
                (RuntimeVal::Bool(value), _) => *list = TypedList::Bool(vec![value]),
                (RuntimeVal::ShortStr(_) | RuntimeVal::Obj(_), Some(string_value)) => {
                    *list = TypedList::String(vec![string_value]);
                }
                (value, _) => values.push(value),
            },
            TypedList::Mixed(values) => values.push(value),
            TypedList::Int(values) => match value {
                RuntimeVal::Int(value) => values.push(value),
                value => {
                    let mut mixed = copy_numeric_list(values, RuntimeVal::Int);
                    mixed.push(value);
                    *list = TypedList::Mixed(mixed);
                }
            },
            TypedList::Float(values) => match value {
                RuntimeVal::Float(value) => values.push(value),
                value => {
                    let mut mixed = copy_numeric_list(values, RuntimeVal::Float);
                    mixed.push(value);
                    *list = TypedList::Mixed(mixed);
                }
            },
            TypedList::Bool(values) => match value {
                RuntimeVal::Bool(value) => values.push(value),
                value => {
                    let mut mixed = copy_numeric_list(values, RuntimeVal::Bool);
                    mixed.push(value);
                    *list = TypedList::Mixed(mixed);
                }
            },
            TypedList::String(values) => match string_value {
                Some(value) => values.push(value),
                None => {
                    anyhow::bail!("internal error: typed string list push must be materialized before mutable borrow")
                }
            },
        }
        Ok(())
    }

    /// Drop every element, keeping the representation.
    ///
    /// `Map` and `Set` have had this; a list did not, though the method table in
    /// `docs/stdlib.md` listed it — "one operation, one name, across every
    /// container" with one container missing.
    pub fn clear(&mut self) {
        match self {
            Self::Mixed(values) => values.clear(),
            Self::Int(values) => values.clear(),
            Self::Float(values) => values.clear(),
            Self::Bool(values) => values.clear(),
            Self::String(values) => values.clear(),
        }
    }

    /// Drop everything from `at` on, keeping the representation.
    ///
    /// What `pop` and `remove_at` need: a list is mutable in LK (`xs[0] = 9`
    /// and `push` both change it in place), so the methods that take an element
    /// *out* have to change it too. `pop` used to read the last element and
    /// leave it there.
    pub fn truncate(&mut self, at: usize) {
        match self {
            Self::Mixed(values) => values.truncate(at),
            Self::Int(values) => values.truncate(at),
            Self::Float(values) => values.truncate(at),
            Self::Bool(values) => values.truncate(at),
            Self::String(values) => values.truncate(at),
        }
    }

    /// Remove the element at `index`, keeping the representation and the order
    /// of the rest.
    pub fn remove_at(&mut self, index: usize) {
        match self {
            Self::Mixed(values) => {
                values.remove(index);
            }
            Self::Int(values) => {
                values.remove(index);
            }
            Self::Float(values) => {
                values.remove(index);
            }
            Self::Bool(values) => {
                values.remove(index);
            }
            Self::String(values) => {
                values.remove(index);
            }
        }
    }

    /// A copy of `[start, start + len)`, clamped to what is actually there.
    ///
    /// This is what materializing a [`SliceValue`] costs — the operation the
    /// window exists to avoid, so callers should be the ones that genuinely
    /// need every element at once (`to_list`, display).
    pub fn window(&self, start: usize, len: usize) -> Self {
        let start = start.min(self.len());
        let end = (start + len).min(self.len());
        fn copy<T: Clone>(values: &[T], start: usize, end: usize) -> Vec<T> {
            values[start..end].to_vec()
        }
        match self {
            Self::Mixed(values) => Self::Mixed(copy(values, start, end)),
            Self::Int(values) => Self::Int(copy(values, start, end)),
            Self::Float(values) => Self::Float(copy(values, start, end)),
            Self::Bool(values) => Self::Bool(copy(values, start, end)),
            Self::String(values) => Self::String(copy(values, start, end)),
        }
    }

    pub fn slice_from(&self, start: usize) -> Self {
        match self {
            Self::Mixed(values) => Self::Mixed(copy_slice_tail(values, start)),
            Self::Int(values) => Self::Int(copy_slice_tail(values, start)),
            Self::Float(values) => Self::Float(copy_slice_tail(values, start)),
            Self::Bool(values) => Self::Bool(copy_slice_tail(values, start)),
            Self::String(values) => Self::String(copy_slice_tail(values, start)),
        }
    }

    /// Remove and return the first `n` elements.
    pub fn drain_prefix(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        match self {
            Self::Mixed(values) => {
                let _ = values.drain(..n.min(values.len()));
            }
            Self::Int(values) => {
                let _ = values.drain(..n.min(values.len()));
            }
            Self::Float(values) => {
                let _ = values.drain(..n.min(values.len()));
            }
            Self::Bool(values) => {
                let _ = values.drain(..n.min(values.len()));
            }
            Self::String(values) => {
                let _ = values.drain(..n.min(values.len()));
            }
        }
    }

    /// Take the first `n` elements, returning them as a new list.
    pub fn take_prefix(&self, n: usize) -> Self {
        let n = n.min(self.len());
        match self {
            Self::Mixed(values) => Self::Mixed(values[..n].to_vec()),
            Self::Int(values) => Self::Int(values[..n].to_vec()),
            Self::Float(values) => Self::Float(values[..n].to_vec()),
            Self::Bool(values) => Self::Bool(values[..n].to_vec()),
            Self::String(values) => Self::String(values[..n].to_vec()),
        }
    }

    /// Every element as an owned `Vec<RuntimeVal>`, without allocating.
    ///
    /// `None` when an element cannot be produced without a heap — a string
    /// past `ShortStr`'s inline limit. Callers that can allocate should read
    /// elements through `Executor::typed_list_element_allocating` instead.
    ///
    /// This used to answer such an element with `ShortStr::new(..).unwrap()`,
    /// in the branch reached exactly when that returns `None`. The comment
    /// beside it admitted the hazard — "longer will fail here. In practice,
    /// iter/unique strings in examples are short" — and `xs[0..2]` over a list
    /// of long strings duly panicked.
    pub fn collect_owned(&self) -> Option<Vec<RuntimeVal>> {
        Some(match self {
            Self::Mixed(values) => values.clone(),
            Self::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
            Self::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
            Self::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
            Self::String(values) => {
                let mut out = Vec::with_capacity(values.len());
                for text in values {
                    // An element past the inline limit needs a heap
                    // allocation, and this method has no `&mut HeapStore`.
                    // Nothing sensible can be produced for it here, so the
                    // whole call declines rather than inventing a value.
                    out.push(RuntimeVal::ShortStr(ShortStr::new(text.as_ref())?));
                }
                out
            }
        })
    }
}

fn copy_slice_tail<T: Clone>(values: &[T], start: usize) -> Vec<T> {
    let tail = values.get(start..).unwrap_or(&[]);
    let mut out = Vec::with_capacity(tail.len());
    out.extend_from_slice(tail);
    out
}

impl PartialEq for TypedList {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Mixed(left), Self::Mixed(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right.iter())
                        .all(|(left, right)| left.same_value_or_handle(right))
            }
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            _ => typed_list_entries_equal_no_heap(self, other),
        }
    }
}

fn typed_list_entries_equal_no_heap(left: &TypedList, right: &TypedList) -> bool {
    left.len() == right.len() && (0..left.len()).all(|index| typed_list_item_equal_no_heap(left, index, right, index))
}

fn typed_list_item_equal_no_heap(left: &TypedList, left_index: usize, right: &TypedList, right_index: usize) -> bool {
    match (left, right) {
        (TypedList::Mixed(left), TypedList::Mixed(right)) => left[left_index].same_value_or_handle(&right[right_index]),
        (TypedList::Int(left), TypedList::Int(right)) => left[left_index] == right[right_index],
        (TypedList::Float(left), TypedList::Float(right)) => left[left_index] == right[right_index],
        (TypedList::Bool(left), TypedList::Bool(right)) => left[left_index] == right[right_index],
        (TypedList::String(left), TypedList::String(right)) => left[left_index] == right[right_index],
        (TypedList::Int(left), TypedList::Mixed(right)) => {
            right[right_index].same_value_or_handle(&RuntimeVal::Int(left[left_index]))
        }
        (TypedList::Mixed(left), TypedList::Int(right)) => {
            left[left_index].same_value_or_handle(&RuntimeVal::Int(right[right_index]))
        }
        (TypedList::Float(left), TypedList::Mixed(right)) => {
            right[right_index].same_value_or_handle(&RuntimeVal::Float(left[left_index]))
        }
        (TypedList::Mixed(left), TypedList::Float(right)) => {
            left[left_index].same_value_or_handle(&RuntimeVal::Float(right[right_index]))
        }
        (TypedList::Bool(left), TypedList::Mixed(right)) => {
            right[right_index].same_value_or_handle(&RuntimeVal::Bool(left[left_index]))
        }
        (TypedList::Mixed(left), TypedList::Bool(right)) => {
            left[left_index].same_value_or_handle(&RuntimeVal::Bool(right[right_index]))
        }
        (TypedList::String(left), TypedList::Mixed(right)) => ShortStr::new(&left[left_index])
            .map(RuntimeVal::ShortStr)
            .is_some_and(|value| right[right_index].same_value_or_handle(&value)),
        (TypedList::Mixed(left), TypedList::String(right)) => ShortStr::new(&right[right_index])
            .map(RuntimeVal::ShortStr)
            .is_some_and(|value| left[left_index].same_value_or_handle(&value)),
        _ => false,
    }
}

#[derive(Clone, Debug)]
pub enum TypedMap {
    Mixed(FastHashMap<RuntimeMapKey, RuntimeVal>),
    StringMixed(FastHashMap<Arc<str>, RuntimeVal>),
    StringInt(FastHashMap<Arc<str>, i64>),
    StringFloat(FastHashMap<Arc<str>, f64>),
    StringBool(FastHashMap<Arc<str>, bool>),
}

/// Build a string-keyed [`TypedMap`] from `(key, value)` pairs. Intended for
/// host embedders converting their own structured maps into VM values (see
/// `lk-api`'s `Value` → `RuntimeVal`); map iteration order is hash order.
pub fn typed_map_from_string_entries(entries: impl IntoIterator<Item = (Arc<str>, RuntimeVal)>) -> TypedMap {
    TypedMap::StringMixed(entries.into_iter().collect())
}

impl TypedMap {
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::Mixed(values) => values.len(),
            Self::StringMixed(values) => values.len(),
            Self::StringInt(values) => values.len(),
            Self::StringFloat(values) => values.len(),
            Self::StringBool(values) => values.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn get(&self, key: &RuntimeMapKey) -> Option<RuntimeVal> {
        match self {
            Self::Mixed(values) => values.get(key).cloned(),
            Self::StringMixed(values) => key.as_str().and_then(|key| values.get(key).cloned()),
            Self::StringInt(values) => key
                .as_str()
                .and_then(|key| values.get(key).copied().map(RuntimeVal::Int)),
            Self::StringFloat(values) => key
                .as_str()
                .and_then(|key| values.get(key).copied().map(RuntimeVal::Float)),
            Self::StringBool(values) => key
                .as_str()
                .and_then(|key| values.get(key).copied().map(RuntimeVal::Bool)),
        }
    }

    pub fn get_str(&self, key: &str) -> Option<RuntimeVal> {
        match self {
            Self::Mixed(values) => {
                if let Some(value) =
                    ShortStr::new(key).and_then(|key| values.get(&RuntimeMapKey::ShortStr(key)).cloned())
                {
                    return Some(value);
                }
                values.get(&RuntimeMapKey::String(Arc::<str>::from(key))).cloned()
            }
            Self::StringMixed(values) => values.get(key).cloned(),
            Self::StringInt(values) => values.get(key).copied().map(RuntimeVal::Int),
            Self::StringFloat(values) => values.get(key).copied().map(RuntimeVal::Float),
            Self::StringBool(values) => values.get(key).copied().map(RuntimeVal::Bool),
        }
    }

    /// Iterate over (RuntimeMapKey, RuntimeVal) pairs.
    pub fn entries_iter(&self) -> Vec<(RuntimeMapKey, RuntimeVal)> {
        let mut out = Vec::with_capacity(self.len());
        match self {
            Self::Mixed(entries) => {
                for (k, v) in entries {
                    out.push((k.clone(), *v));
                }
            }
            Self::StringMixed(entries) => {
                for (k, v) in entries {
                    out.push((RuntimeMapKey::String(k.clone()), *v));
                }
            }
            Self::StringInt(entries) => {
                for (k, v) in entries {
                    out.push((RuntimeMapKey::String(k.clone()), RuntimeVal::Int(*v)));
                }
            }
            Self::StringFloat(entries) => {
                for (k, v) in entries {
                    out.push((RuntimeMapKey::String(k.clone()), RuntimeVal::Float(*v)));
                }
            }
            Self::StringBool(entries) => {
                for (k, v) in entries {
                    out.push((RuntimeMapKey::String(k.clone()), RuntimeVal::Bool(*v)));
                }
            }
        }
        out
    }

    #[inline]
    pub fn clear(&mut self) {
        match self {
            Self::Mixed(values) => values.clear(),
            Self::StringMixed(values) => values.clear(),
            Self::StringInt(values) => values.clear(),
            Self::StringFloat(values) => values.clear(),
            Self::StringBool(values) => values.clear(),
        }
    }

    #[inline]
    pub fn set(&mut self, key: RuntimeMapKey, value: RuntimeVal) {
        match self {
            Self::Mixed(values) => {
                if values.is_empty()
                    && let Some(key_str) = key.as_str()
                {
                    let key = Arc::<str>::from(key_str);
                    *self = match value {
                        RuntimeVal::Int(value) => Self::StringInt(fast_hash_map_from_iter([(key, value)])),
                        RuntimeVal::Float(value) => Self::StringFloat(fast_hash_map_from_iter([(key, value)])),
                        RuntimeVal::Bool(value) => Self::StringBool(fast_hash_map_from_iter([(key, value)])),
                        value => Self::StringMixed(fast_hash_map_from_iter([(key, value)])),
                    };
                    return;
                }
                values.insert(key, value);
            }
            Self::StringMixed(values) => {
                if let Some(key_str) = key.as_str() {
                    if let Some(existing) = values.get_mut(key_str) {
                        *existing = value;
                    } else {
                        values.insert(Arc::<str>::from(key_str), value);
                    }
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
            Self::StringInt(values) => {
                if let Some(key_str) = key.as_str() {
                    match value {
                        RuntimeVal::Int(iv) => {
                            if let Some(existing) = values.get_mut(key_str) {
                                *existing = iv;
                            } else {
                                values.insert(Arc::<str>::from(key_str), iv);
                            }
                        }
                        value => {
                            let key = Arc::<str>::from(key_str);
                            let mut mixed = fast_hash_map_new();
                            for (k, v) in values.iter() {
                                mixed.insert(k.clone(), RuntimeVal::Int(*v));
                            }
                            mixed.insert(key, value);
                            *self = Self::StringMixed(mixed);
                        }
                    }
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
            Self::StringFloat(values) => {
                if let Some(key_str) = key.as_str() {
                    match value {
                        RuntimeVal::Float(fv) => {
                            if let Some(existing) = values.get_mut(key_str) {
                                *existing = fv;
                            } else {
                                values.insert(Arc::<str>::from(key_str), fv);
                            }
                        }
                        value => {
                            let key = Arc::<str>::from(key_str);
                            let mut mixed = fast_hash_map_new();
                            for (k, v) in values.iter() {
                                mixed.insert(k.clone(), RuntimeVal::Float(*v));
                            }
                            mixed.insert(key, value);
                            *self = Self::StringMixed(mixed);
                        }
                    }
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
            Self::StringBool(values) => {
                if let Some(key_str) = key.as_str() {
                    match value {
                        RuntimeVal::Bool(bv) => {
                            if let Some(existing) = values.get_mut(key_str) {
                                *existing = bv;
                            } else {
                                values.insert(Arc::<str>::from(key_str), bv);
                            }
                        }
                        value => {
                            let key = Arc::<str>::from(key_str);
                            let mut mixed = fast_hash_map_new();
                            for (k, v) in values.iter() {
                                mixed.insert(k.clone(), RuntimeVal::Bool(*v));
                            }
                            mixed.insert(key, value);
                            *self = Self::StringMixed(mixed);
                        }
                    }
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
        }
    }

    fn materialize_string_map_to_mixed(&mut self, key: RuntimeMapKey, value: RuntimeVal) {
        let mut mixed = match core::mem::replace(self, Self::Mixed(fast_hash_map_new())) {
            Self::Mixed(values) => values,
            Self::StringMixed(values) => {
                let mut mixed = fast_hash_map_new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), value);
                }
                mixed
            }
            Self::StringInt(values) => {
                let mut mixed = fast_hash_map_new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Int(value));
                }
                mixed
            }
            Self::StringFloat(values) => {
                let mut mixed = fast_hash_map_new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Float(value));
                }
                mixed
            }
            Self::StringBool(values) => {
                let mut mixed = fast_hash_map_new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Bool(value));
                }
                mixed
            }
        };
        mixed.insert(key, value);
        *self = Self::Mixed(mixed);
    }

    /// Remove a key from the map, returning the removed value if present.
    /// For typed string maps, if the key type doesn't match (e.g., integer key on StringInt map),
    /// returns None without modification.
    pub fn remove(&mut self, key: &RuntimeMapKey) -> Option<RuntimeVal> {
        match self {
            Self::Mixed(entries) => entries.remove(key),
            Self::StringMixed(entries) => {
                let key_str = key.as_str()?;
                entries.remove(key_str)
            }
            Self::StringInt(entries) => {
                let key_str = key.as_str()?;
                entries.remove(key_str).map(RuntimeVal::Int)
            }
            Self::StringFloat(entries) => {
                let key_str = key.as_str()?;
                entries.remove(key_str).map(RuntimeVal::Float)
            }
            Self::StringBool(entries) => {
                let key_str = key.as_str()?;
                entries.remove(key_str).map(RuntimeVal::Bool)
            }
        }
    }
}

/// Test-support for lkrt's map-order conformance: the key iteration order of
/// a string→int map built exactly like a literal (stage-1 `RuntimeMapKey`
/// insertion in the given order, then [`typed_map_from_entries`]). The native
/// runtime mirrors this construction; the lkrt test compares against this
/// function so any drift (hasher, table layout, key shape) fails loudly.
pub fn typed_map_iteration_keys<'a>(entries: impl Iterator<Item = (&'a str, i64)>) -> Vec<String> {
    let mut stage1 = fast_hash_map_new();
    for (key, value) in entries {
        let key = match ShortStr::new(key) {
            Some(short) => RuntimeMapKey::ShortStr(short),
            None => RuntimeMapKey::String(Arc::from(key)),
        };
        stage1.insert(key, RuntimeVal::Int(value));
    }
    match typed_map_from_entries(stage1) {
        TypedMap::StringInt(map) => map.keys().map(|k| k.to_string()).collect(),
        other => unreachable!("string→int literal always shapes to StringInt, got {other:?}"),
    }
}

pub(crate) fn typed_map_from_entries(entries: FastHashMap<RuntimeMapKey, RuntimeVal>) -> TypedMap {
    if entries.is_empty() {
        return TypedMap::Mixed(entries);
    }

    #[derive(Clone, Copy)]
    enum StringMapShape {
        Mixed,
        Int,
        Float,
        Bool,
    }

    let mut shape: Option<StringMapShape> = None;
    for (key, value) in &entries {
        if key.as_arc_str().is_none() {
            return TypedMap::Mixed(entries);
        }
        let value_shape = match value {
            RuntimeVal::Int(_) => StringMapShape::Int,
            RuntimeVal::Float(_) => StringMapShape::Float,
            RuntimeVal::Bool(_) => StringMapShape::Bool,
            _ => StringMapShape::Mixed,
        };
        shape = match (shape, value_shape) {
            (None, shape) => Some(shape),
            (Some(StringMapShape::Int), StringMapShape::Int) => Some(StringMapShape::Int),
            (Some(StringMapShape::Float), StringMapShape::Float) => Some(StringMapShape::Float),
            (Some(StringMapShape::Bool), StringMapShape::Bool) => Some(StringMapShape::Bool),
            (Some(StringMapShape::Mixed), StringMapShape::Mixed) => Some(StringMapShape::Mixed),
            _ => {
                return TypedMap::StringMixed(string_mixed_entries_from_runtime_entries(entries));
            }
        };
    }

    match shape.expect("non-empty map has a shape") {
        StringMapShape::Mixed => TypedMap::StringMixed(string_mixed_entries_from_runtime_entries(entries)),
        StringMapShape::Int => TypedMap::StringInt(string_int_entries_from_runtime_entries(entries)),
        StringMapShape::Float => TypedMap::StringFloat(string_float_entries_from_runtime_entries(entries)),
        StringMapShape::Bool => TypedMap::StringBool(string_bool_entries_from_runtime_entries(entries)),
    }
}

fn string_mixed_entries_from_runtime_entries(
    entries: FastHashMap<RuntimeMapKey, RuntimeVal>,
) -> FastHashMap<Arc<str>, RuntimeVal> {
    let mut out = fast_hash_map_new();
    for (key, value) in entries {
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_int_entries_from_runtime_entries(
    entries: FastHashMap<RuntimeMapKey, RuntimeVal>,
) -> FastHashMap<Arc<str>, i64> {
    let mut out = fast_hash_map_new();
    for (key, value) in entries {
        let RuntimeVal::Int(value) = value else {
            unreachable!("validated int map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_float_entries_from_runtime_entries(
    entries: FastHashMap<RuntimeMapKey, RuntimeVal>,
) -> FastHashMap<Arc<str>, f64> {
    let mut out = fast_hash_map_new();
    for (key, value) in entries {
        let RuntimeVal::Float(value) = value else {
            unreachable!("validated float map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_bool_entries_from_runtime_entries(
    entries: FastHashMap<RuntimeMapKey, RuntimeVal>,
) -> FastHashMap<Arc<str>, bool> {
    let mut out = fast_hash_map_new();
    for (key, value) in entries {
        let RuntimeVal::Bool(value) = value else {
            unreachable!("validated bool map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

impl PartialEq for TypedMap {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Mixed(left), Self::Mixed(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .all(|(key, value)| right.get(key).is_some_and(|other| value.same_value_or_handle(other)))
            }
            (Self::StringMixed(left), Self::StringMixed(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .all(|(key, value)| right.get(key).is_some_and(|other| value.same_value_or_handle(other)))
            }
            (Self::StringInt(left), Self::StringInt(right)) => left == right,
            (Self::StringFloat(left), Self::StringFloat(right)) => left == right,
            (Self::StringBool(left), Self::StringBool(right)) => left == right,
            _ => typed_map_entries_equal(self, other),
        }
    }
}

fn typed_map_entries_equal(left: &TypedMap, right: &TypedMap) -> bool {
    left.len() == right.len()
        && typed_map_entries_all(left, |key, value| {
            typed_map_entry_value(right, &key).is_some_and(|right| right.same_value_or_handle(&value))
        })
}

fn typed_map_entries_all(map: &TypedMap, mut visit: impl FnMut(RuntimeMapKey, RuntimeVal) -> bool) -> bool {
    match map {
        TypedMap::Mixed(entries) => entries.iter().all(|(key, value)| visit(key.clone(), *value)),
        TypedMap::StringMixed(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), *value)),
        TypedMap::StringInt(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Int(*value))),
        TypedMap::StringFloat(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Float(*value))),
        TypedMap::StringBool(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Bool(*value))),
    }
}

fn typed_map_entry_value(map: &TypedMap, key: &RuntimeMapKey) -> Option<RuntimeVal> {
    match map {
        TypedMap::Mixed(entries) => entries.get(key).cloned(),
        TypedMap::StringMixed(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).cloned()
        }
        TypedMap::StringInt(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).copied().map(RuntimeVal::Int)
        }
        TypedMap::StringFloat(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).copied().map(RuntimeVal::Float)
        }
        TypedMap::StringBool(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).copied().map(RuntimeVal::Bool)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimeMapKey {
    Nil,
    Bool(bool),
    Int(i64),
    ShortStr(ShortStr),
    String(Arc<str>),
    Obj(HeapRef),
}

impl RuntimeMapKey {
    /// The key a value is used under — the only conversion.
    ///
    /// There were two, and they disagreed about the case that matters. The
    /// executor's (`m[k] = v`) rejected a list; the container methods' accepted
    /// one as `Obj(handle)`, comparing by *handle*. So a set quietly kept
    /// members it could never find again:
    ///
    /// ```text
    /// let s = Set([]);
    /// s.add([1, 2]); s.has([1, 2])   → false
    /// s.add([1, 2]); s.len()         → 2
    /// println(s)                     → Set([<object:80>,<object:82>])
    /// ```
    ///
    /// A `Set` is a map's key set, so it answers the question the same way: a
    /// value whose identity is its handle is not a key. Keying on a mutable
    /// container by *value* is not the alternative — mutating the key would
    /// lose the entry — which is why maps rejected it in the first place.
    pub fn from_value(value: &RuntimeVal, heap: &HeapStore) -> anyhow::Result<Self> {
        match value {
            RuntimeVal::Nil => Ok(Self::Nil),
            RuntimeVal::Bool(value) => Ok(Self::Bool(*value)),
            RuntimeVal::Int(value) => Ok(Self::Int(*value)),
            // `0.0` and `-0.0` are equal but hash differently, and `NaN` is not
            // equal to itself: neither can index anything.
            RuntimeVal::Float(_) => Err(anyhow::anyhow!("Float cannot be a map key or set member")),
            RuntimeVal::ShortStr(value) => Ok(Self::ShortStr(*value)),
            RuntimeVal::Obj(handle) => match heap.get(*handle) {
                Some(HeapValue::String(value)) => Ok(Self::String(Arc::clone(value))),
                Some(other) => Err(anyhow::anyhow!(
                    "{} cannot be a map key or set member: only nil, Bool, Int and String can",
                    other.type_name()
                )),
                None => Err(anyhow::anyhow!("heap object {} out of bounds", handle.index())),
            },
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::ShortStr(value) => Some(value.as_str()),
            Self::String(value) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn as_arc_str(&self) -> Option<Arc<str>> {
        match self {
            Self::ShortStr(value) => Some(Arc::<str>::from(value.as_str())),
            Self::String(value) => Some(value.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_entries_materialize_to_typed_string_maps() {
        let mut entries = fast_hash_map_new();
        entries.insert(RuntimeMapKey::String(Arc::<str>::from("answer")), RuntimeVal::Int(42));

        assert!(matches!(
            typed_map_from_entries(entries),
            TypedMap::StringInt(values) if values.get("answer") == Some(&42)
        ));

        let mut entries = fast_hash_map_new();
        entries.insert(
            RuntimeMapKey::ShortStr(ShortStr::new("ok").expect("short")),
            RuntimeVal::Bool(true),
        );
        assert!(matches!(
            typed_map_from_entries(entries),
            TypedMap::StringBool(values) if values.get("ok") == Some(&true)
        ));

        let mut entries = fast_hash_map_new();
        entries.insert(RuntimeMapKey::Int(1), RuntimeVal::Int(42));
        assert!(matches!(typed_map_from_entries(entries), TypedMap::Mixed(_)));
    }

    #[test]
    fn typed_list_equality_compares_backing_without_runtime_value_vector() {
        let short = ShortStr::new("short").expect("short");
        let typed_int = TypedList::Int(vec![1, 2]);
        let mixed_int = TypedList::Mixed(vec![RuntimeVal::Int(1), RuntimeVal::Int(2)]);
        let typed_short_string = TypedList::String(vec![Arc::<str>::from("short")]);
        let mixed_short_string = TypedList::Mixed(vec![RuntimeVal::ShortStr(short)]);
        let typed_long_string = TypedList::String(vec![Arc::<str>::from("longer-than-short")]);
        let mixed_long_string = TypedList::Mixed(vec![RuntimeVal::Obj(HeapRef::new(7))]);

        assert_eq!(typed_int, mixed_int);
        assert_eq!(typed_short_string, mixed_short_string);
        assert_ne!(typed_long_string, mixed_long_string);
    }

    #[test]
    fn typed_map_get_and_set_preserve_specialized_backing_until_polluted() {
        let mut map = TypedMap::StringInt(fast_hash_map_from_iter([(Arc::<str>::from("answer"), 41)]));

        assert_eq!(
            map.get(&RuntimeMapKey::ShortStr(ShortStr::new("answer").expect("short"))),
            Some(RuntimeVal::Int(41))
        );

        map.set(RuntimeMapKey::String(Arc::<str>::from("answer")), RuntimeVal::Int(42));
        assert!(matches!(map, TypedMap::StringInt(_)));
        assert_eq!(map.get_str("answer"), Some(RuntimeVal::Int(42)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("answer"))),
            Some(RuntimeVal::Int(42))
        );

        map.set(
            RuntimeMapKey::String(Arc::<str>::from("answer")),
            RuntimeVal::Bool(true),
        );
        assert!(matches!(map, TypedMap::StringMixed(_)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("answer"))),
            Some(RuntimeVal::Bool(true))
        );
    }

    #[test]
    fn empty_mixed_map_set_with_string_key_specializes_backing() {
        let mut map = TypedMap::Mixed(fast_hash_map_new());

        map.set(
            RuntimeMapKey::ShortStr(ShortStr::new("answer").expect("short")),
            RuntimeVal::Int(42),
        );

        assert!(matches!(map, TypedMap::StringInt(_)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("answer"))),
            Some(RuntimeVal::Int(42))
        );
    }

    #[test]
    fn typed_map_set_materializes_to_mixed_for_non_string_key() {
        let mut map = TypedMap::StringBool(fast_hash_map_from_iter([(Arc::<str>::from("ok"), true)]));

        map.set(RuntimeMapKey::Int(7), RuntimeVal::Bool(false));

        assert!(matches!(map, TypedMap::Mixed(_)));
        assert_eq!(map.get_str("ok"), Some(RuntimeVal::Bool(true)));
        assert_eq!(map.get(&RuntimeMapKey::Int(7)), Some(RuntimeVal::Bool(false)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("ok"))),
            Some(RuntimeVal::Bool(true))
        );
    }

    #[test]
    fn typed_map_equality_compares_entries_without_materializing_vector() {
        let typed = TypedMap::StringInt(fast_hash_map_from_iter([(Arc::<str>::from("answer"), 42)]));
        let string_mixed = TypedMap::StringMixed(fast_hash_map_from_iter([(
            Arc::<str>::from("answer"),
            RuntimeVal::Int(42),
        )]));
        let exact_mixed = TypedMap::Mixed(fast_hash_map_from_iter([(
            RuntimeMapKey::String(Arc::<str>::from("answer")),
            RuntimeVal::Int(42),
        )]));
        let short_key_mixed = TypedMap::Mixed(fast_hash_map_from_iter([(
            RuntimeMapKey::ShortStr(ShortStr::new("answer").expect("short")),
            RuntimeVal::Int(42),
        )]));

        assert_eq!(typed, string_mixed);
        assert_eq!(typed, exact_mixed);
        assert_ne!(typed, short_key_mixed);
    }
}

// ---------------------------------------------------------------------------
// Runtime resource-handle values (moved from `super::values`, M0.1 decoupling).
// These embed `RuntimeVal`/`RuntimePayload`, so they belong with the runtime
// model rather than the front-end literal/type model. Re-exported at
// `crate::val` via `pub use runtime_model::*`, so external paths are unchanged.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TaskValue {
    pub id: u64,
    pub value: Option<crate::rt::RuntimePayload>,
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
    pub roots: Vec<RuntimeVal>,
}

#[derive(Debug, Clone)]
pub struct StreamCursorValue {
    pub id: u64,
    pub stream_id: u64,
    pub roots: Vec<RuntimeVal>,
}

/// A window over a list: `source[start .. start + len]`, without copying it.
///
/// There used to be a `SliceKind` beside this, distinguishing a list window
/// from a *byte* window over a string. The byte one went with the `slice`
/// module: string positions are characters now, and code that wants bytes says
/// `s.bytes()`. One variant is not a choice, so the field is gone too.
#[derive(Debug, Clone)]
pub struct SliceValue {
    pub source: RuntimeVal,
    pub start: usize,
    /// How long the window was when it was taken. Read [`SliceValue::live_len`]
    /// instead — the source can shrink underneath it.
    pub len: usize,
}

impl SliceValue {
    /// How long the window is *now*, clamped to what the source still holds.
    ///
    /// A window does not copy, so `xs.pop()` can leave it pointing past the end
    /// — and every reader used to answer that differently. For a window of 3
    /// over a list that lost its last element:
    ///
    /// ```text
    /// s.len()       → 3          s.to_list()   → [1,2,nil]
    /// println(s)    → [1,2]      s.last()      → nil
    /// s == [1,2]    → false      s.get(2)      → nil
    /// ```
    ///
    /// Six answers to one question. Clamping is the one the rest of the
    /// language already gives — reading past the end is `nil`, not an error —
    /// and it makes `len()` agree with what the window will actually hand out.
    pub fn live_len(&self, heap: &HeapStore) -> usize {
        let RuntimeVal::Obj(handle) = self.source else {
            return 0;
        };
        let Some(HeapValue::List(list)) = heap.get(handle) else {
            return 0;
        };
        self.len.min(list.len().saturating_sub(self.start))
    }
}

#[derive(Clone)]
pub struct ResourceValue {
    pub kind: &'static str,
    // `ResourceValue` wraps OS resources (files/sockets). The guard goes through
    // the compat `Mutex` so the type resolves under no_std too (std's `Mutex`
    // is absent there); its `.lock() -> Result` shape keeps the stdlib
    // `.lock().map_err(..)` call sites unchanged.
    pub handle: Arc<crate::compat::sync::Mutex<ResourceHandle>>,
}

impl core::fmt::Debug for ResourceValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ResourceValue")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for ResourceHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match self {
            #[cfg(feature = "std")]
            Self::File(_) => "File",
            Self::Stdin => "Stdin",
            Self::Stdout => "Stdout",
            Self::Stderr => "Stderr",
            #[cfg(feature = "std")]
            Self::TcpStream(_) => "TcpStream",
            #[cfg(feature = "std")]
            Self::TcpListener(_) => "TcpListener",
            #[cfg(feature = "std")]
            Self::UdpSocket(_) => "UdpSocket",
            Self::Closed => "Closed",
        };
        f.write_str(name)
    }
}

pub enum ResourceHandle {
    #[cfg(feature = "std")]
    File(std::fs::File),
    Stdin,
    Stdout,
    Stderr,
    #[cfg(feature = "std")]
    TcpStream(std::net::TcpStream),
    #[cfg(feature = "std")]
    TcpListener(std::net::TcpListener),
    #[cfg(feature = "std")]
    UdpSocket(std::net::UdpSocket),
    Closed,
}

#[cfg(test)]
mod layout {
    /// `RuntimeObject` is the widest `HeapValue` variant, so its size is the
    /// size of *every* heap cell — lists, maps and strings included.
    ///
    /// This is a real budget, not a style rule. Adding the declaring module to
    /// an object's identity as a second `Arc<str>` field pushed `HeapValue`
    /// from 72 to 88 bytes and cost ~1.3% geometric mean on the workload suite,
    /// on programs that declare no structs at all. Folding both halves behind
    /// one `Arc<DeclaredType>` brought it to 64.
    #[test]
    fn heap_cells_stay_narrow() {
        assert_eq!(
            core::mem::size_of::<super::RuntimeObject>(),
            core::mem::size_of::<super::HeapValue>(),
            "RuntimeObject still sets the heap cell size; re-read the budget below before widening it"
        );
        assert!(
            core::mem::size_of::<super::HeapValue>() <= 64,
            "HeapValue grew to {} bytes — every heap cell pays for this",
            core::mem::size_of::<super::HeapValue>()
        );
    }
}

fn copy_numeric_list<T: Copy>(values: &[T], wrap: impl Fn(T) -> RuntimeVal) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len() + 1);
    mixed.extend(values.iter().copied().map(wrap));
    mixed
}
