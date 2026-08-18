//! New runtime value model for the VM rewrite.
//!
//! The `LiteralVal` enum remains active while the compiler and executor are migrated.
//! New VM code should target these types first.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::util::fast_map::{FastHashSet, fast_hash_set_new};
use crate::util::value_map::{ValueMap, value_map_from_iter, value_map_new};
use alloc::sync::Arc;

use crate::val::DeclaredType;
use crate::val::{ShortStr, Type};

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

    /// The **language type** name of this value: the struct's own name for an
    /// instance, `List` / `Map` / `Set` / `Bytes` / `String` for the other
    /// handles, the scalar's own name otherwise.
    ///
    /// Takes the heap because that is what makes the question answerable — a
    /// handle's type lives there. That is the point of the signature: an error
    /// message that has the heap cannot accidentally print `Object`, and one
    /// that does not have it cannot call this at all.
    ///
    /// The return borrows the heap for the same reason. It was `&'static str`,
    /// and a struct's name is not static — so this function, the one written to
    /// stop messages saying `Object`, said `Object` for every struct instance:
    /// `p.nonexistent()` reported "Object has no method 'nonexistent'" while the
    /// dispatch two lines away already had `Point` in hand. A rule's own carrier
    /// had exactly the hole the rule exists to close.
    pub fn type_name_in<'heap>(&self, heap: &'heap HeapStore) -> &'heap str {
        match self {
            Self::Obj(handle) => match heap.get(*handle) {
                Some(HeapValue::Object(object)) => object.type_name(),
                Some(other) => other.type_name(),
                None => "Object",
            },
            other => other.kind().scalar_type_name(),
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

/// What a [`RuntimeVal`] is, as a program would say it.
///
/// The variants are named after the *representation* — `ShortStr` is a string
/// that fits inline, `Obj` is a handle — and that is a distinction no program
/// can see. It reached users anyway: some forty error messages are written
/// `bail!("… got {:?}", value.kind())`, so `-x` on a string answered
///
/// ```text
/// Neg expected Int or Float, got ShortStr
/// ```
///
/// naming a type the language does not have. `Debug` is written by hand for
/// that reason: it is what those messages print, so it prints `String` and
/// `Object`. [`RuntimeValKind::repr_name`] is still there for anyone debugging
/// the representation itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RuntimeValKind {
    Nil,
    Bool,
    Int,
    Float,
    ShortStr,
    Obj,
}

impl RuntimeValKind {
    /// The type name a program would use — **for a scalar**. A handle answers
    /// `Object`, which is not a type the language has.
    ///
    /// Named for that limit on purpose. It used to be `type_name`, and 40-odd
    /// error messages reached for it and printed `Object` where they meant
    /// `List`, `Map`, `Set`, or `String`-that-did-not-fit-in-seven-bytes. The
    /// doc said "a caller that has one should reach for `HeapValue::type_name`
    /// instead" and nothing did, because the wrong function had the right name.
    ///
    /// [`RuntimeVal::type_name_in`] is the one to use: it takes the heap, so
    /// forgetting it is a compile error rather than a wrong string.
    pub const fn scalar_type_name(self) -> &'static str {
        match self {
            Self::Nil => "Nil",
            Self::Bool => "Bool",
            Self::Int => "Int",
            Self::Float => "Float",
            Self::ShortStr => "String",
            // A handle; which kind of object needs the heap, so a caller that
            // has one should reach for `HeapValue::type_name` instead.
            Self::Obj => "Object",
        }
    }

    /// The variant's own name — the representation, not the language's type.
    pub const fn repr_name(self) -> &'static str {
        match self {
            Self::Nil => "Nil",
            Self::Bool => "Bool",
            Self::Int => "Int",
            Self::Float => "Float",
            Self::ShortStr => "ShortStr",
            Self::Obj => "Obj",
        }
    }
}

impl core::fmt::Debug for RuntimeValKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.scalar_type_name())
    }
}

impl core::fmt::Display for RuntimeValKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.scalar_type_name())
    }
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
    /// The type name a program would use, including a struct instance's
    /// *declared* name.
    ///
    /// This is the function [`RuntimeValKind::scalar_type_name`] tells callers
    /// to reach for instead of saying `Object` — and it said `Object` itself,
    /// for every struct instance, at all thirty-odd `bail!` sites that took the
    /// advice. Third time the same rule has been fixed one layer further down
    /// (`scalar_type_name`, then [`RuntimeVal::type_name_in`], now here), so:
    /// **the language's name for a struct instance is what the `struct` was
    /// called**, and the variant's own spelling lives in
    /// [`Self::representation_name`] under a name that says so.
    #[inline]
    pub fn type_name(&self) -> &str {
        match self {
            Self::Object(object) => object.type_name(),
            other => other.representation_name(),
        }
    }

    /// The variant's own spelling — the representation, not the language's
    /// type. `Object` for every struct instance, whatever it was declared as.
    ///
    /// Only [`Self::type_name`] and code genuinely talking about the
    /// representation (a heap dump, a GC statistic) should want this.
    #[inline]
    pub fn representation_name(&self) -> &'static str {
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
    /// [`crate::val::TypeScope`]).
    ///
    /// Shared by `Arc` rather than stored inline — see [`DeclaredType`] for why
    /// this struct's width is worth caring about.
    pub ty: Arc<DeclaredType>,
    /// Insertion-ordered, so it is also the field *slot* table: slot `i` is the
    /// `i`th key. There used to be a parallel `Vec<Arc<str>>` for that, kept in
    /// step by hand, because the carrier was a hash map and had no `i`th
    /// anything. It cost 24 bytes on **every heap cell** — `RuntimeObject` is
    /// the widest `HeapValue` variant, so its width is every list's and every
    /// string's too — which is the budget `layout::heap_cells_stay_narrow`
    /// guards.
    pub fields: ValueMap<Arc<str>, RuntimeVal>,
}

impl RuntimeObject {
    pub fn new(ty: Arc<DeclaredType>, fields: ValueMap<Arc<str>, RuntimeVal>) -> Self {
        Self { ty, fields }
    }

    #[inline]
    pub fn type_name(&self) -> &Arc<str> {
        &self.ty.name
    }

    #[inline]
    pub fn type_scope(&self) -> &crate::val::TypeScope {
        &self.ty.scope
    }

    pub fn field_slot(&self, key: &str) -> Option<usize> {
        self.fields.get_index_of(key)
    }

    pub fn get_field(&self, key: &str) -> Option<RuntimeVal> {
        self.fields.get(key).cloned()
    }

    pub fn get_field_slot(&self, slot: usize, key: &str) -> Option<RuntimeVal> {
        let (slot_key, value) = self.fields.get_index(slot)?;
        (slot_key.as_ref() == key).then_some(*value)
    }

    pub fn set_field(&mut self, key: Arc<str>, value: RuntimeVal) {
        // A new key lands at the end, an existing one keeps its slot — which is
        // `IndexMap::insert`'s own behaviour, and used to need a second write to
        // the slot table beside it.
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
                // An `Int` into a `Float` list is the checker's numeric
                // promotion, and it has already been *accepted*:
                // `let xs = [1.5]; xs.push(9)` type-checks because `Int` is
                // assignable to `Float`. Widening to `Mixed` here stores the
                // `Int` unchanged, so `typeof(xs[1])` answered `Int` — the
                // list stopped being the `List<Float>` the checker had just
                // promised, and the native carrier (which does hold `9.0`)
                // read back a different type for the same program.
                //
                // Materializing the promotion is what the acceptance meant.
                // The reverse is not symmetric and is left alone: `Float` into
                // an `Int` list is a narrowing the checker rejects, so it is
                // reachable only through an erased type, where widening is the
                // dynamic behaviour.
                RuntimeVal::Int(value) => values.push(value as f64),
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
    Mixed(ValueMap<RuntimeMapKey, RuntimeVal>),
    StringMixed(ValueMap<Arc<str>, RuntimeVal>),
    StringInt(ValueMap<Arc<str>, i64>),
    StringFloat(ValueMap<Arc<str>, f64>),
    StringBool(ValueMap<Arc<str>, bool>),
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
                        RuntimeVal::Int(value) => Self::StringInt(value_map_from_iter([(key, value)])),
                        RuntimeVal::Float(value) => Self::StringFloat(value_map_from_iter([(key, value)])),
                        RuntimeVal::Bool(value) => Self::StringBool(value_map_from_iter([(key, value)])),
                        value => Self::StringMixed(value_map_from_iter([(key, value)])),
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
                            let mut mixed = value_map_new();
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
                            let mut mixed = value_map_new();
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
                            let mut mixed = value_map_new();
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
        let mut mixed = match core::mem::replace(self, Self::Mixed(value_map_new())) {
            Self::Mixed(values) => values,
            Self::StringMixed(values) => {
                let mut mixed = value_map_new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), value);
                }
                mixed
            }
            Self::StringInt(values) => {
                let mut mixed = value_map_new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Int(value));
                }
                mixed
            }
            Self::StringFloat(values) => {
                let mut mixed = value_map_new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Float(value));
                }
                mixed
            }
            Self::StringBool(values) => {
                let mut mixed = value_map_new();
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
    ///
    /// `shift_remove`, not `swap_remove`: the survivors keep their order, which
    /// is the guarantee the carrier exists for. It costs a memmove of the tail,
    /// and a delete that silently reordered the rest would cost the guarantee.
    pub fn remove(&mut self, key: &RuntimeMapKey) -> Option<RuntimeVal> {
        match self {
            Self::Mixed(entries) => entries.shift_remove(key),
            Self::StringMixed(entries) => {
                let key_str = key.as_str()?;
                entries.shift_remove(key_str)
            }
            Self::StringInt(entries) => {
                let key_str = key.as_str()?;
                entries.shift_remove(key_str).map(RuntimeVal::Int)
            }
            Self::StringFloat(entries) => {
                let key_str = key.as_str()?;
                entries.shift_remove(key_str).map(RuntimeVal::Float)
            }
            Self::StringBool(entries) => {
                let key_str = key.as_str()?;
                entries.shift_remove(key_str).map(RuntimeVal::Bool)
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
    let mut stage1 = value_map_new();
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

/// Test-support for lkrt's set-order conformance: the member iteration order
/// of a `Set` built by inserting the given strings in order.
///
/// A set has no second stage — `RuntimeSet` *is* the `FastHashSet`, so the
/// order is a function of the key hashes and this one insertion sequence. That
/// is only mirrorable if the native side keys its set by the same shape, which
/// is why `lkrt` has exactly one `RtKey`.
pub fn set_iteration_order(members: impl Iterator<Item = MirrorMember>) -> Vec<MirrorMember> {
    let mut set = fast_hash_set_new();
    for member in members {
        set.insert(match &member {
            MirrorMember::Int(v) => RuntimeMapKey::Int(*v),
            MirrorMember::Str(v) => match ShortStr::new(v) {
                Some(short) => RuntimeMapKey::ShortStr(short),
                None => RuntimeMapKey::String(Arc::from(v.as_str())),
            },
        });
    }
    set.iter()
        .map(|key| match key {
            RuntimeMapKey::Int(v) => MirrorMember::Int(*v),
            other => MirrorMember::Str(other.as_str().expect("string key").to_string()),
        })
        .collect()
}

/// The member kinds [`set_iteration_order`] round-trips. Deliberately not
/// `RuntimeMapKey` itself: the point of the test is that lkrt does *not* get to
/// see the VM's key type, only the values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorMember {
    Int(i64),
    Str(String),
}

/// The same, for an **int**-keyed literal — where the shaping is different in
/// the way that matters: a non-string key makes [`typed_map_from_entries`]
/// return `Mixed`, which *is* the stage-1 table. There is no stage 2, so a
/// native carrier has to be built by replaying the same insertion sequence
/// rather than by iterating stage 1 into a second table.
pub fn typed_map_iteration_int_keys(entries: impl Iterator<Item = (i64, i64)>) -> Vec<i64> {
    let mut stage1 = value_map_new();
    for (key, value) in entries {
        stage1.insert(RuntimeMapKey::Int(key), RuntimeVal::Int(value));
    }
    match typed_map_from_entries(stage1) {
        TypedMap::Mixed(map) => map
            .keys()
            .map(|k| match k {
                RuntimeMapKey::Int(i) => *i,
                other => unreachable!("int literal keys stay Int, got {other:?}"),
            })
            .collect(),
        other => unreachable!("an int-keyed literal always shapes to Mixed, got {other:?}"),
    }
}

pub(crate) fn typed_map_from_entries(entries: ValueMap<RuntimeMapKey, RuntimeVal>) -> TypedMap {
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
    entries: ValueMap<RuntimeMapKey, RuntimeVal>,
) -> ValueMap<Arc<str>, RuntimeVal> {
    let mut out = value_map_new();
    for (key, value) in entries {
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_int_entries_from_runtime_entries(entries: ValueMap<RuntimeMapKey, RuntimeVal>) -> ValueMap<Arc<str>, i64> {
    let mut out = value_map_new();
    for (key, value) in entries {
        let RuntimeVal::Int(value) = value else {
            unreachable!("validated int map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_float_entries_from_runtime_entries(entries: ValueMap<RuntimeMapKey, RuntimeVal>) -> ValueMap<Arc<str>, f64> {
    let mut out = value_map_new();
    for (key, value) in entries {
        let RuntimeVal::Float(value) = value else {
            unreachable!("validated float map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_bool_entries_from_runtime_entries(entries: ValueMap<RuntimeMapKey, RuntimeVal>) -> ValueMap<Arc<str>, bool> {
    let mut out = value_map_new();
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

/// What a map or set indexes by.
///
/// Every variant is **self-contained**: no heap handle. That is not an accident
/// of the current variants but the rule — a value whose identity is a handle
/// cannot be a key, because mutating it would lose the entry (see
/// [`RuntimeMapKey::from_value`]). There was an `Obj(HeapRef)` variant, and once
/// the two "value → key" conversions were unified nothing could produce one; it
/// left a GC edge to walk, an artifact variant to encode, and two cross-heap
/// translations that chased a handle no key held.
///
/// The rule is worth keeping because of what it buys: a key crosses heaps as
/// itself, a set has no outgoing GC edges at all, and neither needs the heap to
/// be copied.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimeMapKey {
    Nil,
    Bool(bool),
    Int(i64),
    ShortStr(ShortStr),
    String(Arc<str>),
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

    /// The order a `Set` displays its members in: nil, then Bool, then Int by
    /// value, then String by content.
    ///
    /// A stable order is the point — a set's hash iteration order is not
    /// portable, so displaying one has to impose something. The display code
    /// imposed it on the *rendered text* instead of the members, which made
    /// `Set([1, 2, 10, 20, 3])` print `Set([1,10,2,20,3])` and
    /// `Set([-1, -2, 5])` print `Set([-1,-2,5])`: an order that is neither
    /// insertion, nor value, nor anything a reader can use.
    ///
    /// Not `derive(Ord)` either, and that is the reason this is a function
    /// rather than one: the derive compares *variants*, so a string of 8 bytes
    /// (`String`) would sort after every string of 7 (`ShortStr`) —
    /// `Set(["ab", "aaaaaaaaaa"])` would come out `"ab"` first. A string's
    /// representation is not part of its value anywhere else in the language,
    /// and it is not here.
    pub fn display_order(&self, other: &Self) -> core::cmp::Ordering {
        fn kind(key: &RuntimeMapKey) -> u8 {
            match key {
                RuntimeMapKey::Nil => 0,
                RuntimeMapKey::Bool(_) => 1,
                RuntimeMapKey::Int(_) => 2,
                RuntimeMapKey::ShortStr(_) | RuntimeMapKey::String(_) => 3,
            }
        }
        kind(self).cmp(&kind(other)).then_with(|| match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Int(a), Self::Int(b)) => a.cmp(b),
            _ => match (self.as_str(), other.as_str()) {
                (Some(a), Some(b)) => a.cmp(b),
                _ => core::cmp::Ordering::Equal,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checker's numeric promotion survives into the representation.
    ///
    /// `xs.push(9)` on a `List<Float>` type-checks — `Int` is assignable to
    /// `Float` — so the list is still a `List<Float>` afterwards. Widening to
    /// `Mixed` and storing the `Int` unchanged made `typeof(xs[1])` answer
    /// `Int`, which the native carrier (holding `9.0`) contradicts.
    ///
    /// The reverse stays a widening: `Float` into an `Int` list is a narrowing
    /// the checker rejects, so it arrives only through an erased type, where
    /// the dynamic answer is the right one.
    #[test]
    fn an_int_pushed_into_a_float_list_becomes_a_float() {
        let mut list = TypedList::Float(vec![1.5]);
        list.push(RuntimeVal::Int(9), None).expect("push");
        assert!(
            matches!(&list, TypedList::Float(values) if values == &[1.5, 9.0]),
            "an accepted promotion must be materialized, not widened away: {list:?}"
        );

        let mut narrowing = TypedList::Int(vec![1]);
        narrowing.push(RuntimeVal::Float(1.5), None).expect("push");
        assert!(
            matches!(&narrowing, TypedList::Mixed(_)),
            "a narrowing arrives only through an erased type and stays dynamic: {narrowing:?}"
        );
    }

    #[test]
    fn runtime_entries_materialize_to_typed_string_maps() {
        let mut entries = value_map_new();
        entries.insert(RuntimeMapKey::String(Arc::<str>::from("answer")), RuntimeVal::Int(42));

        assert!(matches!(
            typed_map_from_entries(entries),
            TypedMap::StringInt(values) if values.get("answer") == Some(&42)
        ));

        let mut entries = value_map_new();
        entries.insert(
            RuntimeMapKey::ShortStr(ShortStr::new("ok").expect("short")),
            RuntimeVal::Bool(true),
        );
        assert!(matches!(
            typed_map_from_entries(entries),
            TypedMap::StringBool(values) if values.get("ok") == Some(&true)
        ));

        let mut entries = value_map_new();
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
        let mut map = TypedMap::StringInt(value_map_from_iter([(Arc::<str>::from("answer"), 41)]));

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
        let mut map = TypedMap::Mixed(value_map_new());

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
        let mut map = TypedMap::StringBool(value_map_from_iter([(Arc::<str>::from("ok"), true)]));

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
        let typed = TypedMap::StringInt(value_map_from_iter([(Arc::<str>::from("answer"), 42)]));
        let string_mixed =
            TypedMap::StringMixed(value_map_from_iter([(Arc::<str>::from("answer"), RuntimeVal::Int(42))]));
        let exact_mixed = TypedMap::Mixed(value_map_from_iter([(
            RuntimeMapKey::String(Arc::<str>::from("answer")),
            RuntimeVal::Int(42),
        )]));
        let short_key_mixed = TypedMap::Mixed(value_map_from_iter([(
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
    /// Every heap cell is one `HeapValue`, so this size is what a list, a
    /// string and a map each pay — including programs that declare no structs.
    ///
    /// This is a real budget, not a style rule. Adding the declaring module to
    /// an object's identity as a second `Arc<str>` field pushed `HeapValue`
    /// from 72 to 88 bytes and cost ~1.3% geometric mean on the workload suite.
    /// Folding both halves behind one `Arc<DeclaredType>` brought it to 64.
    ///
    /// It is 72 again, and this time deliberately. Insertion-ordered value maps
    /// (`util::value_map`) carry an entry vector beside the index table, which
    /// is 8 bytes wider than a bare hash table, and `TypedMap`'s own
    /// discriminant no longer fits in a niche on top of it. What it buys is in
    /// that module's docs; the 24 bytes it *would* have cost were paid back by
    /// deleting `RuntimeObject::field_slots`, which an ordered map makes
    /// redundant. Measured on the workload suite: ~2% geometric mean, against a
    /// 10% gate.
    ///
    /// The way back to 64, if it is ever wanted, is to flatten `TypedMap`'s
    /// five variants into `HeapValue` so the two discriminants become one.
    #[test]
    fn heap_cells_stay_narrow() {
        assert!(
            core::mem::size_of::<super::HeapValue>() <= 72,
            "HeapValue grew to {} bytes ({} for RuntimeObject, {} for TypedMap) — every heap cell pays for this",
            core::mem::size_of::<super::HeapValue>(),
            core::mem::size_of::<super::RuntimeObject>(),
            core::mem::size_of::<super::TypedMap>()
        );
    }
}

fn copy_numeric_list<T: Copy>(values: &[T], wrap: impl Fn(T) -> RuntimeVal) -> Vec<RuntimeVal> {
    let mut mixed = Vec::with_capacity(values.len() + 1);
    mixed.extend(values.iter().copied().map(wrap));
    mixed
}

/// Ascending order over floats that is *total*, which `partial_cmp` is not.
///
/// `sort_by` may panic — "user-provided comparison function does not correctly
/// implement a total order" — and `partial_cmp(..).unwrap_or(Equal)` earns it: a
/// NaN reads equal to every value while those values stay ordered among
/// themselves, so the relation is not transitive. `xs.sort()` on a float list
/// holding a NaN therefore aborted the interpreter with a Rust panic, which `try`
/// cannot catch. Whether it fired depended on the data: 601 elements went
/// through, 60 did not, which is the worst kind of reachable.
///
/// So NaN is *ordered* rather than equal-to-everything: all NaNs compare equal to
/// each other and greater than every number, and a sorted list reads as ascending
/// values with the not-a-numbers gathered at the end.
///
/// `-0.0` and `0.0` stay equal here, where `f64::total_cmp` would separate them —
/// `==` in the language says they are equal, and `sort` disagreeing with `==`
/// about two values would be a second rule to remember for no gain.
///
/// `lkrt`'s `list_sort!` mirrors this for the native backend.
pub fn compare_floats(left: f64, right: f64) -> core::cmp::Ordering {
    match left.partial_cmp(&right) {
        Some(ordering) => ordering,
        // Unordered, so at least one side is NaN.
        None => match (left.is_nan(), right.is_nan()) {
            (true, true) => core::cmp::Ordering::Equal,
            (true, false) => core::cmp::Ordering::Greater,
            (false, true) => core::cmp::Ordering::Less,
            // `partial_cmp` answers `None` only for a NaN, so this cannot happen;
            // it is spelled out so the relation stays total if that ever changes.
            (false, false) => core::cmp::Ordering::Equal,
        },
    }
}

#[cfg(test)]
mod compare_floats_tests {
    use super::compare_floats;
    // `alloc`, not the std prelude: this crate also builds without an OS.
    use alloc::vec::Vec;
    use core::cmp::Ordering;

    /// The property `sort_by` needs: transitivity across the NaN.
    ///
    /// `partial_cmp(..).unwrap_or(Equal)` fails exactly here — NaN == 1.0 and
    /// NaN == 2.0 while 1.0 < 2.0 — and Rust's sort notices and panics.
    #[test]
    fn the_order_is_total_across_nan() {
        let nan = f64::NAN;
        assert_eq!(compare_floats(nan, nan), Ordering::Equal);
        assert_eq!(compare_floats(nan, 1.0), Ordering::Greater);
        assert_eq!(compare_floats(1.0, nan), Ordering::Less);
        assert_eq!(compare_floats(nan, f64::INFINITY), Ordering::Greater);
        // Zeroes stay equal, unlike `total_cmp`.
        assert_eq!(compare_floats(-0.0, 0.0), Ordering::Equal);
        assert_eq!(compare_floats(1.0, 2.0), Ordering::Less);

        // And a sort over the values that used to abort now completes.
        let mut values: Vec<f64> = (0..60)
            .map(|i| if i % 4 == 0 { f64::NAN } else { f64::from(60 - i) })
            .collect();
        values.sort_by(|left, right| compare_floats(*left, *right));
        assert!(
            values[..45].windows(2).all(|pair| pair[0] <= pair[1]),
            "the numbers come out ascending: {values:?}"
        );
        assert!(
            values[45..].iter().all(|value| value.is_nan()),
            "and the NaNs are gathered at the end: {values:?}"
        );
    }
}

/// Whether a runtime value may be stored where `declared` is written.
///
/// Scalars only. A container's declared element type is not something a single
/// value carries — `List<Int>` and `List<String>` are the same `HeapValue::List`
/// at run time — so a container-typed field is not checked here, and neither is
/// `Any`, a union, or a named type. What is left is exactly the set a wrong
/// store corrupts silently, and the set the type checker's own assignability
/// rules answer the same way: an `Int` satisfies a `Float` field (the language
/// never coerces at a typed boundary, so it stays an `Int`), and `nil`
/// satisfies a nullable one.
pub fn value_satisfies_declared(value: &RuntimeVal, declared: &Type, heap: &HeapStore) -> bool {
    if let Type::Optional(inner) = declared {
        return matches!(value, RuntimeVal::Nil) || value_satisfies_declared(value, inner, heap);
    }
    let is_string = match value {
        RuntimeVal::ShortStr(_) => true,
        RuntimeVal::Obj(handle) => matches!(heap.get(*handle), Some(HeapValue::String(_))),
        _ => false,
    };
    match declared {
        Type::Int => matches!(value, RuntimeVal::Int(_)),
        Type::Float => matches!(value, RuntimeVal::Int(_) | RuntimeVal::Float(_)),
        Type::Bool => matches!(value, RuntimeVal::Bool(_)),
        Type::String => is_string,
        _ => true,
    }
}
