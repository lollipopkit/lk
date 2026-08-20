//! The signatures of the built-in container methods.
//!
//! `xs.take(2)` is as much a part of the language as `math.abs(x)`, but until
//! this table existed only the second one had a declared type. The checker knew
//! seven method names — `len`, `is_empty`, `get`, `set`, `add`, `push`, `clear`
//! — and everything else was `Any`: `xs.first()` told you nothing, and
//! `xs.take("2")` was not an error until it ran.
//!
//! It also knew them by hand, in a `match` that was the *fourth* copy of this
//! knowledge:
//!
//! | copy | what it held |
//! | --- | --- |
//! | `vm/context/core_methods*` | the implementation and its arity checks |
//! | `typ/type_checker/expressions.rs` | types, for seven of them |
//! | `completion/src/lib.rs` | which names to offer per receiver |
//! | `lsp/src/server/handlers.rs` | the text shown in signature help |
//!
//! They had already drifted — completion offered no `slice`/`sort`/`pop`, and
//! signature help still described `take(list, n)` as "n <= 0 returns []", which
//! stopped being true when a negative count started raising. This module is the
//! one they now derive from, the same move `stdlib_sig` made for the modules.
//!
//! # The placeholders
//!
//! A module function's parameters are concrete; a method's are relative to its
//! receiver. So four names in the type texts below stand for parts of it, and
//! [`builtin_method_signature`] substitutes them:
//!
//! - `Elem` — a list's, set's or window's element type
//! - `Key`, `Val` — a map's
//! - `Self` — the receiver type itself, for the methods that hand it back
//!
//! Anything else is ordinary LK type text and is parsed as such.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::val::Type;

/// Which receiver a method belongs to — the type-level mirror of
/// `vm::context::core_methods`'s `BuiltinReceiver`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinReceiverKind {
    List,
    /// A `Bytes` handle. A sequence like the two below it, with the *read* half
    /// of the list surface and none of the transforming half: `map` cannot
    /// answer a `Bytes`, because a callback may return something that is not a
    /// byte. `to_list` is the way across.
    Bytes,
    /// A window over a list (`xs.slice(a, b)`), which is its own type: it has
    /// `to_list` and no `push`.
    Slice,
    Map,
    Set,
    Str,
}

/// One declared parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinParam {
    pub name: &'static str,
    /// LK type text, possibly using the placeholders above.
    pub ty: &'static str,
    /// May be omitted at the call site.
    pub optional: bool,
}

const fn p(name: &'static str, ty: &'static str) -> BuiltinParam {
    BuiltinParam {
        name,
        ty,
        optional: false,
    }
}

const fn opt(name: &'static str, ty: &'static str) -> BuiltinParam {
    BuiltinParam {
        name,
        ty,
        optional: true,
    }
}

/// One built-in method, as declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinMethodSig {
    pub receiver: BuiltinReceiverKind,
    pub name: &'static str,
    pub params: &'static [BuiltinParam],
    pub returns: &'static str,
    /// One line, shown on hover and in completion.
    pub docs: &'static str,
    /// Which parameter, if any, is a callback applied to each element — its
    /// first parameter is the receiver's element type.
    ///
    /// This is what lets `xs.map(|x| …)` type `x` at all: the closure arrives
    /// with a fresh variable for its parameter, and nothing but the receiver
    /// can say what it holds.
    pub elementwise_callback: Option<usize>,
    /// The last declared parameter may repeat, so a call may pass more
    /// arguments than there are parameters. Only `format` needs it — a template
    /// takes as many values as it has placeholders — and without it the checker
    /// would reject `"{} {}".format(a, b)` for having "too many" arguments.
    pub variadic: bool,
}

use BuiltinReceiverKind::{Bytes, List, Map, Set, Slice, Str};

const fn m(
    receiver: BuiltinReceiverKind,
    name: &'static str,
    params: &'static [BuiltinParam],
    returns: &'static str,
    docs: &'static str,
) -> BuiltinMethodSig {
    BuiltinMethodSig {
        receiver,
        name,
        params,
        returns,
        docs,
        elementwise_callback: None,
        variadic: false,
    }
}

/// [`m`] for a method whose last parameter may repeat.
const fn variadic_m(
    receiver: BuiltinReceiverKind,
    name: &'static str,
    params: &'static [BuiltinParam],
    returns: &'static str,
    docs: &'static str,
) -> BuiltinMethodSig {
    BuiltinMethodSig {
        receiver,
        name,
        params,
        returns,
        docs,
        elementwise_callback: None,
        variadic: true,
    }
}

/// [`m`] for a method whose parameter `callback` is applied to each element.
const fn hof(
    receiver: BuiltinReceiverKind,
    name: &'static str,
    params: &'static [BuiltinParam],
    returns: &'static str,
    docs: &'static str,
    callback: usize,
) -> BuiltinMethodSig {
    BuiltinMethodSig {
        receiver,
        name,
        params,
        returns,
        docs,
        elementwise_callback: Some(callback),
        variadic: false,
    }
}

/// Every built-in method the language has.
///
/// The return types describe what the implementation *does*, not what would be
/// tidy. `remove_at` really does hand back a two-element `[rest, removed]`, and
/// saying so here is what lets the checker reject `xs.remove_at(0) + 1`;
/// writing `Elem` because that reads better would make the table a wish.
pub const BUILTIN_METHODS: &[BuiltinMethodSig] = &[
    // ---- List ----
    m(List, "len", &[], "Int", "Number of elements"),
    m(List, "is_empty", &[], "Bool", "Whether the list has no elements"),
    m(List, "first", &[], "Elem?", "First element, or nil when empty"),
    m(List, "last", &[], "Elem?", "Last element, or nil when empty"),
    m(
        List,
        "get",
        &[p("index", "Int")],
        "Elem?",
        "Element at `index` (negative counts from the end), or nil when out of range",
    ),
    m(
        List,
        "index_of",
        &[p("value", "Any")],
        "Int?",
        "Position of the first equal element, or nil",
    ),
    // `index_of`'s sibling — how many rather than where. It was declared on
    // `Str` alone, so `"aa".count("a")` answered 2 while `[1, 1].count(1)` was
    // "List has no method 'count'".
    m(
        List,
        "count",
        &[p("value", "Any")],
        "Int",
        "How many elements equal `value`",
    ),
    m(
        List,
        "contains",
        &[p("value", "Any")],
        "Bool",
        "Whether an element is equal to `value`",
    ),
    m(
        List,
        "push",
        &[p("value", "Elem")],
        "Self",
        "The list with `value` appended",
    ),
    m(
        List,
        "clear",
        &[],
        "Self",
        "Removes every element, in place; answers the list",
    ),
    m(
        List,
        "pop",
        &[],
        "Elem?",
        "Removes and returns the last element, or nil when empty",
    ),
    m(
        List,
        "insert",
        &[p("index", "Int"), p("value", "Elem")],
        "Self",
        "Inserts at `index`, in place; answers the list so calls chain",
    ),
    m(
        List,
        "remove_at",
        &[p("index", "Int")],
        "Elem",
        "Removes the element at `index`, in place, and returns it",
    ),
    m(
        List,
        "set",
        &[p("index", "Int"), p("value", "Elem")],
        "Self",
        "Writes `index` in place; answers the list so calls chain",
    ),
    m(List, "sort", &[], "Self", "A sorted copy (the receiver is untouched)"),
    // The three reductions. `min`/`max` answer `Elem?` for the same reason
    // `first` does — an empty list has none — and they use `sort`'s order, so
    // `xs.sort().first()` and `xs.min()` cannot disagree.
    m(
        List,
        "min",
        &[],
        "Elem?",
        "The smallest element by `sort`'s order, or nil when empty",
    ),
    m(
        List,
        "max",
        &[],
        "Elem?",
        "The largest element by `sort`'s order, or nil when empty",
    ),
    m(
        List,
        "sum",
        &[],
        "Any",
        "The numbers added up (0 when empty); a non-number raises",
    ),
    m(List, "reverse", &[], "Self", "A reversed copy"),
    // The inverse of `Bytes::to_list`, whose only spelling was the constructor
    // `bytes.from_list(xs)` in another module.
    m(
        List,
        "to_bytes",
        &[],
        "Bytes",
        "The list as bytes; every item must be an Int in 0..=255",
    ),
    m(
        List,
        "unique",
        &[],
        "Self",
        "A copy without later duplicates, order preserved",
    ),
    m(
        List,
        "take",
        &[p("count", "Int")],
        "Self",
        "The first `count` elements; a negative count raises",
    ),
    m(
        List,
        "skip",
        &[p("count", "Int")],
        "Self",
        "Everything after the first `count` elements; a negative count raises",
    ),
    m(List, "concat", &[p("other", "Self")], "Self", "The two lists joined"),
    m(List, "chain", &[p("other", "Self")], "Self", "The two lists joined"),
    m(
        List,
        "chunk",
        &[p("size", "Int")],
        "List<Self>",
        "Groups of `size` elements; the size must be positive",
    ),
    m(
        List,
        "enumerate",
        &[],
        "List<Tuple<Int, Elem>>",
        "Each element paired with its position",
    ),
    m(
        List,
        "zip",
        &[p("other", "List<_>")],
        "List<List<Any>>",
        "Elements paired positionally, up to the shorter length",
    ),
    m(List, "flatten", &[], "List<Any>", "One nesting level removed"),
    m(
        List,
        "join",
        &[p("separator", "String")],
        "String",
        "The elements joined; every element must be a String",
    ),
    m(
        List,
        "slice",
        &[p("start", "Int"), opt("end", "Int")],
        "Slice<Elem>",
        "A window over `[start, end)` — a view, not a copy (`to_list` copies)",
    ),
    // The higher-order three. `map`'s result element type is the callback's
    // return type, which nothing here can name — `CallbackResult` is the
    // placeholder the *call site* fills in, and `Any` is what it means when the
    // argument is not a function literal the checker can read.
    hof(
        List,
        "map",
        &[p("transform", "Fn")],
        "List<CallbackResult>",
        "Each element through `transform`",
        0,
    ),
    hof(
        List,
        "filter",
        &[p("predicate", "Fn")],
        "Self",
        "The elements `predicate` keeps (only nil and false drop one)",
        0,
    ),
    m(
        List,
        "reduce",
        &[p("initial", "Any"), p("accumulate", "Fn")],
        "Any",
        "Folds `accumulate` over the elements from `initial`",
    ),
    // ---- Slice (a window over a list) ----
    m(Slice, "len", &[], "Int", "Number of elements in the window"),
    m(Slice, "is_empty", &[], "Bool", "Whether the window is empty"),
    m(
        Slice,
        "get",
        &[p("index", "Int")],
        "Elem?",
        "Element at `index` within the window, or nil when outside it",
    ),
    m(
        Slice,
        "slice",
        &[p("start", "Int"), opt("end", "Int")],
        "Self",
        "A narrower window, resolved against the original list",
    ),
    m(Slice, "to_list", &[], "List<Elem>", "A copy of the window's elements"),
    m(
        Slice,
        "first",
        &[],
        "Elem?",
        "First element, or nil when the window is empty",
    ),
    m(
        Slice,
        "last",
        &[],
        "Elem?",
        "Last element, or nil when the window is empty",
    ),
    m(
        Slice,
        "min",
        &[],
        "Elem?",
        "The smallest element in the window, or nil when empty",
    ),
    m(
        Slice,
        "max",
        &[],
        "Elem?",
        "The largest element in the window, or nil when empty",
    ),
    m(Slice, "sum", &[], "Any", "The window's numbers added up (0 when empty)"),
    m(
        Slice,
        "contains",
        &[p("value", "Any")],
        "Bool",
        "Whether an element of the window equals `value`",
    ),
    m(
        Slice,
        "index_of",
        &[p("value", "Any")],
        "Int?",
        "Position within the window of the first equal element, or nil",
    ),
    m(
        Slice,
        "count",
        &[p("value", "Any")],
        "Int",
        "How many elements in the window equal `value`",
    ),
    // A window is a range of its source, and a reversed range is not one — so
    // this materializes where `take`/`skip`/`slice` answer sub-windows, the
    // same rule `map` follows here.
    m(Slice, "reverse", &[], "List<Elem>", "The window's elements, reversed"),
    m(
        Slice,
        "sort",
        &[],
        "List<Elem>",
        "The window's elements in ascending order",
    ),
    m(Slice, "enumerate", &[], "List<List<Any>>", "`[index, element]` pairs"),
    m(
        Slice,
        "zip",
        &[p("other", "List<_>")],
        "List<List<Any>>",
        "Pairs with `other`",
    ),
    m(
        Slice,
        "chain",
        &[p("other", "List<_>")],
        "List<Any>",
        "The window's elements then `other`'s",
    ),
    m(
        Slice,
        "chunk",
        &[p("size", "Int")],
        "List<List<Elem>>",
        "Groups of `size` elements",
    ),
    // `concat` is `chain` under its other name; the window answers a list for
    // the same reason.
    m(
        Slice,
        "concat",
        &[p("other", "List<_>")],
        "List<Any>",
        "The window's elements then `other`'s",
    ),
    m(
        Slice,
        "join",
        &[p("separator", "String")],
        "String",
        "Elements joined by `separator`",
    ),
    m(
        Slice,
        "unique",
        &[],
        "List<Elem>",
        "The window's elements with later duplicates dropped, order kept",
    ),
    // A contiguous run of a window is still a window; what `filter` keeps is
    // not contiguous, so it materializes a list.
    m(
        Slice,
        "take",
        &[p("count", "Int")],
        "Self",
        "The window's first `count` elements",
    ),
    m(
        Slice,
        "skip",
        &[p("count", "Int")],
        "Self",
        "The window without its first `count` elements",
    ),
    hof(
        Slice,
        "map",
        &[p("transform", "Fn")],
        "List<CallbackResult>",
        "Each element through `transform`",
        0,
    ),
    hof(
        Slice,
        "filter",
        &[p("predicate", "Fn")],
        "List<Elem>",
        "The elements `predicate` keeps",
        0,
    ),
    m(
        Slice,
        "reduce",
        &[p("initial", "Any"), p("accumulate", "Fn")],
        "Any",
        "Folds `accumulate` over the window from `initial`",
    ),
    // ---- Bytes ----
    //
    // The read half of the list surface, and the elements are `Int`. Every one
    // of these means on a `Bytes` exactly what it means on a `List`, which is
    // the test for belonging here.
    m(Bytes, "len", &[], "Int", "Number of bytes"),
    m(Bytes, "is_empty", &[], "Bool", "Whether there are no bytes"),
    m(Bytes, "first", &[], "Int?", "First byte, or nil when empty"),
    m(Bytes, "last", &[], "Int?", "Last byte, or nil when empty"),
    m(Bytes, "min", &[], "Int?", "Smallest byte, or nil when empty"),
    m(Bytes, "max", &[], "Int?", "Largest byte, or nil when empty"),
    m(Bytes, "sum", &[], "Int", "The bytes added up (0 when empty)"),
    m(
        Bytes,
        "get",
        &[p("index", "Int")],
        "Int?",
        "Byte at `index` (negative counts from the end), or nil when out of range",
    ),
    m(
        Bytes,
        "contains",
        &[p("value", "Any")],
        "Bool",
        "Whether a byte equals `value`",
    ),
    m(
        Bytes,
        "index_of",
        &[p("value", "Any")],
        "Int?",
        "Position of the first byte equal to `value`, or nil",
    ),
    m(
        Bytes,
        "count",
        &[p("value", "Any")],
        "Int",
        "How many bytes equal `value`",
    ),
    // Shape-preserving and element-type-independent, so a `Bytes` again — the
    // reading `take`, `skip`, `slice` and `concat` already take here.
    m(Bytes, "reverse", &[], "Bytes", "The bytes in reverse order"),
    // Byte values are ordered scalars, so both mean here what they mean on a
    // `List<Int>`, and both keep the carrier.
    m(Bytes, "sort", &[], "Bytes", "The bytes in ascending order"),
    // The operations whose answer is a *list of the elements*: they mean the
    // same here as on a `List` and cannot keep the carrier, so they answer one.
    m(Bytes, "enumerate", &[], "List<List<Int>>", "`[index, byte]` pairs"),
    m(
        Bytes,
        "zip",
        &[p("other", "List<_>")],
        "List<List<Any>>",
        "Pairs with `other`",
    ),
    m(
        Bytes,
        "chain",
        &[p("other", "List<_>")],
        "List<Any>",
        "The bytes then `other`'s elements",
    ),
    m(
        Bytes,
        "chunk",
        &[p("size", "Int")],
        "List<List<Int>>",
        "Groups of `size` bytes",
    ),
    m(
        Bytes,
        "join",
        &[p("separator", "String")],
        "String",
        "Byte values joined by `separator`",
    ),
    m(
        Bytes,
        "unique",
        &[],
        "Bytes",
        "The bytes with later duplicates dropped, order kept",
    ),
    m(
        Bytes,
        "slice",
        &[p("start", "Int"), opt("end", "Int")],
        "Bytes",
        "The bytes in `[start, end)` — a copy, since `Bytes` has no cheap sub-range",
    ),
    m(Bytes, "to_list", &[], "List<Int>", "The bytes as a list of numbers"),
    // The three that used to be reachable only as `bytes.f(b, …)`.
    m(
        Bytes,
        "to_string_utf8",
        &[],
        "String",
        "The bytes decoded as UTF-8; raises when they are not",
    ),
    m(
        Bytes,
        "to_string_lossy",
        &[],
        "String",
        "The bytes decoded as UTF-8, with every invalid sequence replaced",
    ),
    m(
        Bytes,
        "concat",
        &[p("other", "Bytes")],
        "Bytes",
        "These bytes followed by `other`'s",
    ),
    // Transforms. The rule is whether the result's elements can be something
    // the receiver could not hold: `filter` keeps a subset, so it is still
    // `Bytes`; `map` may answer anything, so it is a list.
    m(Bytes, "take", &[p("count", "Int")], "Bytes", "The first `count` bytes"),
    m(
        Bytes,
        "skip",
        &[p("count", "Int")],
        "Bytes",
        "Everything after the first `count` bytes",
    ),
    hof(
        Bytes,
        "map",
        &[p("transform", "Fn")],
        "List<CallbackResult>",
        "Each byte through `transform`",
        0,
    ),
    hof(
        Bytes,
        "filter",
        &[p("predicate", "Fn")],
        "Bytes",
        "The bytes `predicate` keeps",
        0,
    ),
    m(
        Bytes,
        "reduce",
        &[p("initial", "Any"), p("accumulate", "Fn")],
        "Any",
        "Folds `accumulate` over the bytes from `initial`",
    ),
    // ---- Map ----
    m(Map, "len", &[], "Int", "Number of entries"),
    m(Map, "is_empty", &[], "Bool", "Whether the map has no entries"),
    // The default is optional *here* too: the runtime has always accepted
    // `get(key, default)` and only the declaration said otherwise, so the one
    // form that avoids a nil check was a type error.
    m(
        Map,
        "get",
        &[p("key", "Key"), opt("default", "Val")],
        "Val?",
        "Value for `key`, or `default` when absent (nil without one)",
    ),
    m(
        Map,
        "set",
        &[p("key", "Key"), p("value", "Val")],
        "Self",
        "Writes an entry, in place; answers the map so calls chain",
    ),
    m(Map, "has", &[p("key", "Any")], "Bool", "Whether `key` is present"),
    m(
        Map,
        "delete",
        &[p("key", "Any")],
        "Val?",
        "Removes `key`, returning its value",
    ),
    m(
        Map,
        "clear",
        &[],
        "Self",
        "Removes every entry, in place; answers the map",
    ),
    m(Map, "keys", &[], "List<Key>", "The keys, in the map's iteration order"),
    m(
        Map,
        "values",
        &[],
        "List<Val>",
        "The values, in the map's iteration order",
    ),
    // ---- Set ----
    m(Set, "len", &[], "Int", "Number of members"),
    m(Set, "is_empty", &[], "Bool", "Whether the set has no members"),
    // Membership is `contains` on every value container — the four sequence
    // types and this one. `has` is a *map's* spelling, where the question is
    // about a key and "contains" would not say which of the two it means.
    m(
        Set,
        "contains",
        &[p("value", "Any")],
        "Bool",
        "Whether `value` is a member",
    ),
    m(
        Set,
        "add",
        &[p("value", "Elem")],
        "Bool",
        "Adds a member, reporting whether it was new",
    ),
    m(
        Set,
        "delete",
        &[p("value", "Any")],
        "Bool",
        "Removes a member, reporting whether it was there",
    ),
    m(
        Set,
        "clear",
        &[],
        "Self",
        "Removes every member, in place; answers the set",
    ),
    m(Set, "values", &[], "List<Elem>", "The members"),
    // The set operations. A `Set` that can only add, delete, test a member and
    // hand back a list is a deduplicating bag; these are what make it a set.
    // The answers are filled in a stated order — the receiver's members first,
    // then the argument's — because a set's iteration order is its hash order,
    // so two sets with the same members can still iterate differently.
    m(Set, "union", &[p("other", "Set<Elem>")], "Self", "The members of both"),
    m(
        Set,
        "intersection",
        &[p("other", "Set<Elem>")],
        "Self",
        "The members present in both",
    ),
    m(
        Set,
        "difference",
        &[p("other", "Set<Elem>")],
        "Self",
        "The members not in `other`",
    ),
    m(
        Set,
        "symmetric_difference",
        &[p("other", "Set<Elem>")],
        "Self",
        "The members in exactly one of the two",
    ),
    m(
        Set,
        "is_subset",
        &[p("other", "Set<Elem>")],
        "Bool",
        "Whether every member is also in `other`",
    ),
    m(
        Set,
        "is_superset",
        &[p("other", "Set<Elem>")],
        "Bool",
        "Whether every member of `other` is also here",
    ),
    m(
        Set,
        "is_disjoint",
        &[p("other", "Set<Elem>")],
        "Bool",
        "Whether the two share no member",
    ),
    // ---- String ----
    //
    // Every position here is a *character* position, not a byte offset — see
    // `util::text`. `bytes()` is the way down to bytes, deliberately explicit.
    m(Str, "len", &[], "Int", "Number of characters"),
    m(Str, "is_empty", &[], "Bool", "Whether the string has no characters"),
    m(Str, "lower", &[], "String", "Lowercased"),
    m(Str, "upper", &[], "String", "Uppercased"),
    m(Str, "trim", &[], "String", "Without leading or trailing whitespace"),
    m(Str, "reverse", &[], "String", "Characters in reverse order"),
    m(
        Str,
        "repeat",
        &[p("count", "Int")],
        "String",
        "The string repeated `count` times",
    ),
    m(
        Str,
        "starts_with",
        &[p("prefix", "String")],
        "Bool",
        "Whether it starts with `prefix`",
    ),
    m(
        Str,
        "ends_with",
        &[p("suffix", "String")],
        "Bool",
        "Whether it ends with `suffix`",
    ),
    m(
        Str,
        "contains",
        &[p("needle", "Any")],
        "Bool",
        "Whether `needle` occurs",
    ),
    // The read surface every other sequence has. `slice` in particular reads
    // the same as `List`/`Slice`/`Bytes` — start and end, not start and length
    // — because `xs.slice(1, 3)` and `s.substring(1, 3)` taking different
    // windows from the same numbers is a trap, not a feature.
    m(
        Str,
        "slice",
        &[p("start", "Int"), opt("end", "Int")],
        "String",
        "Characters in `[start, end)`, clamped; to the end without `end`",
    ),
    m(
        Str,
        "index_of",
        &[p("needle", "Any")],
        "Int?",
        "Character position of the first occurrence, or nil",
    ),
    m(
        Str,
        "get",
        &[p("index", "Int")],
        "String?",
        "The character at `index`, or nil",
    ),
    m(Str, "first", &[], "String?", "First character, or nil when empty"),
    m(Str, "last", &[], "String?", "Last character, or nil when empty"),
    m(
        Str,
        "take",
        &[p("count", "Int")],
        "String",
        "The first `count` characters",
    ),
    m(
        Str,
        "skip",
        &[p("count", "Int")],
        "String",
        "Everything after the first `count` characters",
    ),
    m(
        Str,
        "replace",
        &[p("from", "String"), p("to", "String"), opt("all", "Bool")],
        "String",
        "Occurrences of `from` replaced; `all: false` replaces only the first",
    ),
    m(
        Str,
        "split",
        &[p("delimiter", "String")],
        "List<String>",
        "Split on `delimiter`",
    ),
    m(Str, "chars", &[], "List<String>", "One string per character"),
    m(
        Str,
        "bytes",
        &[],
        "Bytes",
        "The UTF-8 bytes — the explicit way down from characters",
    ),
    m(
        Str,
        "byte_at",
        &[p("index", "Int")],
        "Int?",
        "The byte at a *byte* offset, or nil when out of range",
    ),
    // The nine that used to exist only as `string` module functions. A module
    // function whose first parameter is the receiver *is* a method, and having
    // it in only one of the two places meant `s.strip("-")` did not exist while
    // `string.strip(s, "-")` did.
    m(
        Str,
        "capitalize",
        &[],
        "String",
        "First character upper, the rest lower",
    ),
    m(
        Str,
        "title",
        &[],
        "String",
        "First character of each whitespace-separated word upper, the rest lower",
    ),
    m(
        Str,
        "count",
        &[p("needle", "Any")],
        "Int",
        "How many non-overlapping occurrences of `needle` there are",
    ),
    m(
        Str,
        "strip",
        &[p("chars", "String")],
        "String",
        "Without leading or trailing characters that are in `chars`",
    ),
    m(
        Str,
        "strip_prefix",
        &[p("prefix", "String")],
        "String?",
        "Without `prefix`, or nil when it does not start with it",
    ),
    m(
        Str,
        "strip_suffix",
        &[p("suffix", "String")],
        "String?",
        "Without `suffix`, or nil when it does not end with it",
    ),
    m(
        Str,
        "pad_left",
        &[p("width", "Int"), opt("fill", "String")],
        "String",
        "Widened to `width` characters by repeating `fill` (a space) on the left",
    ),
    m(
        Str,
        "pad_right",
        &[p("width", "Int"), opt("fill", "String")],
        "String",
        "Widened to `width` characters by repeating `fill` (a space) on the right",
    ),
    variadic_m(
        Str,
        "format",
        &[opt("values", "Any")],
        "String",
        "The receiver as a template: each `{}` takes the next value",
    ),
];

/// A built-in method's signature with the receiver's types substituted in.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedBuiltinMethod {
    pub name: &'static str,
    pub params: Vec<(&'static str, Type)>,
    /// How many leading arguments a call must supply.
    pub required: usize,
    pub return_type: Type,
    pub docs: &'static str,
    /// See [`BuiltinMethodSig::elementwise_callback`].
    pub elementwise_callback: Option<usize>,
    /// The receiver's element type, for a caller that needs to constrain a
    /// callback's parameter against it.
    pub elem: Type,
    /// See [`BuiltinMethodSig::variadic`].
    pub variadic: bool,
}

/// What the placeholders stand for, given a concrete receiver.
struct Bindings {
    kind: BuiltinReceiverKind,
    elem: Type,
    key: Type,
    val: Type,
    receiver: Type,
    /// What the call site says a callback parameter returns, if it could tell.
    callback_result: Option<Type>,
}

/// The declared signature of `method` on `receiver`, or `None` when the
/// receiver is not a built-in container or has no such method.
///
/// `None` is the "say nothing" answer, not "this is an error": a receiver whose
/// type is still a variable, or a user type with a trait method of the same
/// name, must be left to the rest of the checker.
pub fn builtin_method_signature(receiver: &Type, method: &str) -> Option<ResolvedBuiltinMethod> {
    builtin_method_signature_with(receiver, method, None)
}

/// [`builtin_method_signature`] with the call site's answer for
/// `CallbackResult` — the type the callback argument returns.
///
/// `None` means the call site could not tell, and the placeholder widens to
/// `Any`, which is what every `map` used to be.
pub fn builtin_method_signature_with(
    receiver: &Type,
    method: &str,
    callback_result: Option<Type>,
) -> Option<ResolvedBuiltinMethod> {
    let mut bindings = bind(receiver)?;
    bindings.callback_result = callback_result;
    let declared = BUILTIN_METHODS
        .iter()
        .find(|sig| sig.receiver == bindings.kind && sig.name == method)?;
    Some(ResolvedBuiltinMethod {
        name: declared.name,
        params: declared
            .params
            .iter()
            .map(|param| (param.name, resolve(param.ty, &bindings)))
            .collect(),
        required: declared.params.iter().filter(|param| !param.optional).count(),
        return_type: resolve(declared.returns, &bindings),
        docs: declared.docs,
        elementwise_callback: declared.elementwise_callback,
        elem: bindings.elem.clone(),
        variadic: declared.variadic,
    })
}

/// Every method available on `receiver`, for completion and signature help.
/// How many positional arguments a declared method takes: `(required, most)`.
///
/// Answers `None` for a name this table does not declare, which is the signal
/// to leave the question to the implementation.
///
/// The runtime dispatchers used to state their own arity in a `bail!` guard,
/// so the declaration and the implementation were two sources that drifted:
/// `bytes.slice`, `map.get` and `str.slice` each accepted a form the checker
/// rejected, or the reverse. This is the one source.
pub fn builtin_method_arity(receiver: BuiltinReceiverKind, method: &str) -> Option<(usize, usize)> {
    let sig = BUILTIN_METHODS
        .iter()
        .find(|sig| sig.receiver == receiver && sig.name == method)?;
    let required = sig.params.iter().filter(|param| !param.optional).count();
    Some((required, sig.params.len()))
}

pub fn builtin_methods_for(receiver: BuiltinReceiverKind) -> impl Iterator<Item = &'static BuiltinMethodSig> {
    BUILTIN_METHODS.iter().filter(move |sig| sig.receiver == receiver)
}

/// The receiver kind a concrete type dispatches as, mirroring
/// `builtin_receiver_kind` in the VM.
pub fn receiver_kind(receiver: &Type) -> Option<BuiltinReceiverKind> {
    bind(receiver).map(|b| b.kind)
}

fn bind(receiver: &Type) -> Option<Bindings> {
    let (kind, elem, key, val) = match receiver {
        Type::List(elem) => (List, (**elem).clone(), Type::Any, Type::Any),
        Type::Set(elem) => (Set, (**elem).clone(), Type::Any, Type::Any),
        Type::Map(k, v) => (Map, Type::Any, (**k).clone(), (**v).clone()),
        Type::String => (Str, Type::Any, Type::Any, Type::Any),
        // A window carries its element type: `xs.slice(..)` on a `List<Int>` is
        // a `Slice<Int>`, and losing that is what made `let s: String = w[0]`
        // type-check for as long as the window was an opaque handle.
        Type::Named(name) if name == "Bytes" => (Bytes, Type::Int, Type::Any, Type::Any),
        Type::Generic { name, params } if name == "Slice" => (
            Slice,
            params.first().cloned().unwrap_or(Type::Any),
            Type::Any,
            Type::Any,
        ),
        // A tuple is a list (`is_assignable_to` says so), so it answers the
        // list methods — with the element type it can honestly state.
        Type::Tuple(elems) => {
            let elem = if elems.is_empty() {
                Type::Any
            } else if elems.iter().all(|e| *e == elems[0]) {
                elems[0].clone()
            } else {
                Type::Any
            };
            (List, elem, Type::Any, Type::Any)
        }
        _ => return None,
    };
    Some(Bindings {
        kind,
        elem,
        key,
        val,
        receiver: receiver.clone(),
        callback_result: None,
    })
}

fn resolve(text: &str, bindings: &Bindings) -> Type {
    let parsed = Type::parse(text).unwrap_or(Type::Any);
    substitute(&parsed, bindings)
}

/// Replace the placeholder names with what they stand for.
///
/// They arrive as `Type::Named` because the parser has never heard of them,
/// which is exactly what makes this a substitution rather than a special case
/// in the parser.
fn substitute(ty: &Type, bindings: &Bindings) -> Type {
    match ty {
        Type::Named(name) => match name.as_str() {
            "Elem" => bindings.elem.clone(),
            "Key" => bindings.key.clone(),
            "Val" => bindings.val.clone(),
            "Self" => bindings.receiver.clone(),
            "CallbackResult" => bindings.callback_result.clone().unwrap_or(Type::Any),
            // Not a placeholder: hand it to the same table the standard
            // library's declarations go through, so `Fn` and `Bytes` mean here
            // what they mean there rather than becoming a named type nothing
            // is assignable to.
            _ => crate::typ::type_from_text(name),
        },
        Type::List(inner) => Type::List(Box::new(substitute(inner, bindings))),
        Type::Set(inner) => Type::Set(Box::new(substitute(inner, bindings))),
        Type::Optional(inner) => Type::Optional(Box::new(substitute(inner, bindings))),
        Type::Boxed(inner) => Type::Boxed(Box::new(substitute(inner, bindings))),
        Type::Map(k, v) => Type::Map(Box::new(substitute(k, bindings)), Box::new(substitute(v, bindings))),
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| substitute(e, bindings)).collect()),
        Type::Union(arms) => Type::Union(arms.iter().map(|a| substitute(a, bindings)).collect()),
        Type::Generic { name, params } => Type::Generic {
            name: name.clone(),
            params: params.iter().map(|p| substitute(p, bindings)).collect(),
        },
        other => other.clone(),
    }
}

/// The type a window over `List<T>` has.
pub fn slice_of(elem: Type) -> Type {
    Type::Generic {
        name: "Slice".to_string(),
        params: vec![elem],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(receiver: Type, method: &str) -> ResolvedBuiltinMethod {
        builtin_method_signature(&receiver, method).unwrap_or_else(|| panic!("no signature for {method}"))
    }

    fn list_of(elem: Type) -> Type {
        Type::List(Box::new(elem))
    }

    #[test]
    fn a_methods_types_come_from_its_receiver() {
        assert_eq!(
            sig(list_of(Type::Int), "first").return_type,
            Type::Optional(Box::new(Type::Int))
        );
        assert_eq!(
            sig(list_of(Type::String), "get").return_type,
            Type::Optional(Box::new(Type::String))
        );
        // `Self` really is the receiver, element type included.
        assert_eq!(sig(list_of(Type::Int), "sort").return_type, list_of(Type::Int));
        assert_eq!(
            sig(list_of(Type::Int), "chunk").return_type,
            list_of(list_of(Type::Int))
        );
        assert_eq!(
            sig(Type::Map(Box::new(Type::String), Box::new(Type::Int)), "keys").return_type,
            list_of(Type::String)
        );
        assert_eq!(
            sig(Type::Map(Box::new(Type::String), Box::new(Type::Int)), "get").return_type,
            Type::Optional(Box::new(Type::Int))
        );
    }

    #[test]
    fn a_window_keeps_the_element_type_of_the_list_it_windows() {
        // The whole reason `Slice` is generic: as an opaque handle it made
        // `let s: String = xs.slice(0, 1)[0];` type-check.
        let window = sig(list_of(Type::Int), "slice").return_type;
        assert_eq!(window, slice_of(Type::Int));
        assert_eq!(
            sig(window.clone(), "get").return_type,
            Type::Optional(Box::new(Type::Int))
        );
        assert_eq!(sig(window.clone(), "to_list").return_type, list_of(Type::Int));
        // A window of a window is still a window over the same elements.
        assert_eq!(sig(window, "slice").return_type, slice_of(Type::Int));
    }

    #[test]
    fn optional_parameters_are_the_ones_that_may_be_left_out() {
        let slice = sig(list_of(Type::Int), "slice");
        assert_eq!(slice.params.len(), 2);
        assert_eq!(slice.required, 1);
        assert_eq!(slice.params[0].1, Type::Int);
    }

    /// A callback parameter must not become a named type nothing can satisfy.
    #[test]
    fn a_callback_parameter_accepts_a_function() {
        let map = sig(list_of(Type::Int), "map");
        assert_eq!(map.params[0].1, Type::Any);
        assert_eq!(map.return_type, list_of(Type::Any));
        // `filter` keeps the element type; only `map` cannot know it.
        assert_eq!(sig(list_of(Type::Int), "filter").return_type, list_of(Type::Int));
    }

    #[test]
    fn a_receiver_the_table_has_nothing_for_says_nothing() {
        assert!(builtin_method_signature(&Type::Int, "len").is_none());
        assert!(builtin_method_signature(&Type::Variable("a".into()), "len").is_none());
        // A list has no `clear` — the checker used to accept `xs.clear()` and
        // the VM answers "List has no method 'clear'".
        assert!(builtin_method_signature(&list_of(Type::Int), "clear").is_some());
        assert!(builtin_method_signature(&Type::Set(Box::new(Type::Int)), "clear").is_some());
    }

    /// Every entry must name a type the checker can act on. A typo in a
    /// declaration would otherwise widen that method to `Any` silently, which
    /// is the state this table exists to end.
    #[test]
    fn every_declared_type_resolves() {
        let receivers = [
            (List, list_of(Type::Int)),
            (Bytes, Type::Named("Bytes".to_string())),
            (Slice, slice_of(Type::Int)),
            (Map, Type::Map(Box::new(Type::String), Box::new(Type::Int))),
            (Set, Type::Set(Box::new(Type::Int))),
            (Str, Type::String),
        ];
        for declared in BUILTIN_METHODS {
            let receiver = receivers
                .iter()
                .find(|(kind, _)| *kind == declared.receiver)
                .map(|(_, ty)| ty.clone())
                .expect("every receiver kind has a sample");
            let resolved = builtin_method_signature(&receiver, declared.name)
                .unwrap_or_else(|| panic!("{:?}.{} did not resolve", declared.receiver, declared.name));
            for ((name, ty), param) in resolved.params.iter().zip(declared.params) {
                // `Fn` and `Any` are the two texts that *mean* "unconstrained":
                // a callback's signature is not stated here, and `reduce`'s
                // seed genuinely is any value. Every other text reaching `Any`
                // is a name the checker does not know — a typo, or a type this
                // table has not been taught — and the method would be silently
                // unchecked, which is the state it exists to end.
                if matches!(param.ty, "Fn" | "Any") {
                    continue;
                }
                assert_ne!(
                    *ty,
                    Type::Any,
                    "{:?}.{}({name}: {}) resolved to Any",
                    declared.receiver,
                    declared.name,
                    param.ty
                );
            }
            assert!(
                declared.returns == "Any" || resolved.return_type != Type::Any,
                "{:?}.{} -> {} resolved to Any",
                declared.receiver,
                declared.name,
                declared.returns
            );
        }
    }
}
