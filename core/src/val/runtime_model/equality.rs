//! `==` on runtime values — the only implementation.
//!
//! Equality needs the heap, so it cannot be a `PartialEq` impl (see
//! [`super::RuntimeVal`]). That made it a function, and a function got copied:
//! the executor had one, `vm::context::core_methods` had a second for
//! `contains` / `index_of` / `position` / `unique`, and they disagreed. The
//! second one materialized each list element as a `RuntimeVal` before
//! comparing, and a string element longer than seven bytes has no `RuntimeVal`
//! short of allocating — so it became `Nil`, and two `Nil`s are equal:
//!
//! ```text
//! [["abcdefghij"]].contains(["zzzzzzzzzz"])   → true
//! [["abc"]].contains(["zzz"])                 → false
//! ```
//!
//! The same seven-byte boundary that [`super::RuntimeVal`]'s doc comment
//! describes, reintroduced one layer down. Copies of a relation do not stay
//! equal to each other; there is one here now, and callers pass the heap.
//!
//! Comparison is depth-bounded — see [`super::MAX_VALUE_DEPTH`] for why.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow};

use super::{
    HeapRef, HeapStore, HeapValue, MAX_VALUE_DEPTH, RuntimeMapKey, RuntimeSet, RuntimeVal, TypedList, TypedMap,
};

/// `left == right`, by value, through `heap`.
pub fn runtime_values_equal(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore) -> Result<bool> {
    Comparison { heap }.values(left, right, 0)
}

/// `value == text`, where `text` is a string the caller already holds.
///
/// The list comparisons need this: a `TypedList::String` element is an
/// `Arc<str>` with no `RuntimeVal` short of a heap allocation.
pub fn runtime_value_equals_str(value: &RuntimeVal, text: &str, heap: &HeapStore) -> Result<bool> {
    Comparison { heap }.value_equals_str(value, text)
}

struct Comparison<'a> {
    heap: &'a HeapStore,
}

impl<'a> Comparison<'a> {
    fn fetch(&self, handle: HeapRef) -> Result<&'a HeapValue> {
        self.heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))
    }

    /// The text of a value that is a string, without allocating.
    ///
    /// A `ShortStr` lives inline in the value, a long one in the heap, so the
    /// answer borrows from whichever is shorter-lived.
    fn as_str<'v>(&'v self, value: &'v RuntimeVal) -> Result<Option<&'v str>> {
        Ok(match value {
            RuntimeVal::ShortStr(value) => Some(value.as_str()),
            RuntimeVal::Obj(handle) => match self.fetch(*handle)? {
                HeapValue::String(text) => Some(text.as_ref()),
                _ => None,
            },
            _ => None,
        })
    }

    fn value_equals_str(&self, value: &RuntimeVal, text: &str) -> Result<bool> {
        Ok(self.as_str(value)? == Some(text))
    }

    fn values(&self, left: &RuntimeVal, right: &RuntimeVal, depth: u32) -> Result<bool> {
        Ok(match (left, right) {
            (RuntimeVal::Nil, RuntimeVal::Nil) => true,
            (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left == right,
            (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left == right,
            // By value, not by bits: `0.0 == -0.0` and `NaN != NaN`, as IEEE
            // says and as every other arm here does.
            (RuntimeVal::Float(left), RuntimeVal::Float(right)) => left == right,
            (RuntimeVal::Int(left), RuntimeVal::Float(right)) => *left as f64 == *right,
            (RuntimeVal::Float(left), RuntimeVal::Int(right)) => *left == *right as f64,
            (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) if left == right => true,
            (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) => {
                let left = self.fetch(*left)?;
                let right = self.fetch(*right)?;
                self.heap_values(left, right, depth)?
            }
            // The one remaining mixed pair that can be equal: the same text
            // reaches `ShortStr` or the heap depending only on its length.
            _ => match (self.as_str(left)?, self.as_str(right)?) {
                (Some(left), Some(right)) => left == right,
                _ => false,
            },
        })
    }

    /// One level down. Every recursive step goes through here so the bound is
    /// stated once.
    fn nested(&self, left: &RuntimeVal, right: &RuntimeVal, depth: u32) -> Result<bool> {
        if depth >= MAX_VALUE_DEPTH {
            return Err(anyhow!(
                "comparison nested deeper than {MAX_VALUE_DEPTH} levels; the values are cyclic or too deeply nested to compare"
            ));
        }
        self.values(left, right, depth + 1)
    }

    fn heap_values(&self, left: &HeapValue, right: &HeapValue, depth: u32) -> Result<bool> {
        Ok(match (left, right) {
            (HeapValue::String(left), HeapValue::String(right)) => left == right,
            (HeapValue::Bytes(left), HeapValue::Bytes(right)) => left == right,
            (HeapValue::List(left), HeapValue::List(right)) => self.lists(left, right, depth)?,
            // A window compares by its elements, like everything else that has
            // elements — including against the list it windows.
            (HeapValue::Slice(left), HeapValue::Slice(right)) => self.slice_ranges(
                left.source,
                left.start,
                left.len,
                right.source,
                right.start,
                right.len,
                depth,
            )?,
            (HeapValue::Slice(left), HeapValue::List(right)) => {
                self.slice_and_list(left.source, left.start, left.len, right, depth)?
            }
            (HeapValue::List(left), HeapValue::Slice(right)) => {
                self.slice_and_list(right.source, right.start, right.len, left, depth)?
            }
            (HeapValue::Map(left), HeapValue::Map(right)) => self.maps(left, right, depth)?,
            (HeapValue::Set(left), HeapValue::Set(right)) => sets_equal(left, right),
            _ => false,
        })
    }

    /// The list a window reads through to, or `None` if the source is gone.
    fn slice_source_list(&self, source: RuntimeVal) -> Option<&'a TypedList> {
        let RuntimeVal::Obj(handle) = source else {
            return None;
        };
        match self.heap.get(handle) {
            Some(HeapValue::List(list)) => Some(list),
            _ => None,
        }
    }

    /// Two windows, compared through their sources. Nothing is materialized:
    /// element comparison already works by index, so a window only offsets the
    /// index it asks for.
    #[allow(clippy::too_many_arguments)]
    fn slice_ranges(
        &self,
        left_source: RuntimeVal,
        left_start: usize,
        left_len: usize,
        right_source: RuntimeVal,
        right_start: usize,
        right_len: usize,
        depth: u32,
    ) -> Result<bool> {
        if left_len != right_len {
            return Ok(false);
        }
        let (Some(left), Some(right)) = (
            self.slice_source_list(left_source),
            self.slice_source_list(right_source),
        ) else {
            return Ok(false);
        };
        if left_start + left_len > left.len() || right_start + right_len > right.len() {
            return Ok(false);
        }
        for index in 0..left_len {
            if !self.list_items(left, left_start + index, right, right_start + index, depth)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// A window against a whole list.
    fn slice_and_list(
        &self,
        source: RuntimeVal,
        start: usize,
        len: usize,
        other: &TypedList,
        depth: u32,
    ) -> Result<bool> {
        if len != other.len() {
            return Ok(false);
        }
        let Some(list) = self.slice_source_list(source) else {
            return Ok(false);
        };
        if start + len > list.len() {
            return Ok(false);
        }
        for index in 0..len {
            if !self.list_items(list, start + index, other, index, depth)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn lists(&self, left: &TypedList, right: &TypedList, depth: u32) -> Result<bool> {
        if left.len() != right.len() {
            return Ok(false);
        }
        // Same representation on both sides: the whole vector at once, and no
        // heap lookups at all.
        match (left, right) {
            (TypedList::Int(left), TypedList::Int(right)) => return Ok(left == right),
            (TypedList::Float(left), TypedList::Float(right)) => return Ok(left == right),
            (TypedList::Bool(left), TypedList::Bool(right)) => return Ok(left == right),
            (TypedList::String(left), TypedList::String(right)) => return Ok(left == right),
            _ => {}
        }
        for index in 0..left.len() {
            if !self.list_items(left, index, right, index, depth)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn list_items(
        &self,
        left: &TypedList,
        left_index: usize,
        right: &TypedList,
        right_index: usize,
        depth: u32,
    ) -> Result<bool> {
        match (left, right) {
            (TypedList::Mixed(left), TypedList::Mixed(right)) => {
                self.nested(&left[left_index], &right[right_index], depth)
            }
            (TypedList::Mixed(left), TypedList::String(right)) => {
                self.value_equals_str(&left[left_index], &right[right_index])
            }
            (TypedList::String(left), TypedList::Mixed(right)) => {
                self.value_equals_str(&right[right_index], &left[left_index])
            }
            (TypedList::Int(left), _) => {
                self.item_against(RuntimeVal::Int(left[left_index]), right, right_index, depth)
            }
            (TypedList::Float(left), _) => {
                self.item_against(RuntimeVal::Float(left[left_index]), right, right_index, depth)
            }
            (TypedList::Bool(left), _) => {
                self.item_against(RuntimeVal::Bool(left[left_index]), right, right_index, depth)
            }
            (TypedList::String(left), _) => self.string_item_against(&left[left_index], right, right_index),
            (TypedList::Mixed(left), _) => self.item_against(left[left_index], right, right_index, depth),
        }
    }

    fn item_against(&self, left: RuntimeVal, right: &TypedList, right_index: usize, depth: u32) -> Result<bool> {
        match right {
            TypedList::Mixed(right) => self.nested(&left, &right[right_index], depth),
            TypedList::Int(right) => self.nested(&left, &RuntimeVal::Int(right[right_index]), depth),
            TypedList::Float(right) => self.nested(&left, &RuntimeVal::Float(right[right_index]), depth),
            TypedList::Bool(right) => self.nested(&left, &RuntimeVal::Bool(right[right_index]), depth),
            TypedList::String(right) => self.value_equals_str(&left, &right[right_index]),
        }
    }

    fn string_item_against(&self, left: &Arc<str>, right: &TypedList, right_index: usize) -> Result<bool> {
        match right {
            TypedList::Mixed(right) => self.value_equals_str(&right[right_index], left),
            TypedList::String(right) => Ok(left == &right[right_index]),
            _ => Ok(false),
        }
    }

    /// Maps compare by key lookup, not by scanning the other side — the same
    /// answer as a pairwise search, without its quadratic cost.
    fn maps(&self, left: &TypedMap, right: &TypedMap, depth: u32) -> Result<bool> {
        if left.len() != right.len() {
            return Ok(false);
        }
        match left {
            TypedMap::Mixed(entries) => {
                for (key, value) in entries {
                    if !self.map_value(right, key, value, depth)? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringMixed(entries) => {
                for (key, value) in entries {
                    if !self.map_value(right, &RuntimeMapKey::String(key.clone()), value, depth)? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringInt(entries) => {
                for (key, value) in entries {
                    let key = RuntimeMapKey::String(key.clone());
                    if !self.map_value(right, &key, &RuntimeVal::Int(*value), depth)? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringFloat(entries) => {
                for (key, value) in entries {
                    let key = RuntimeMapKey::String(key.clone());
                    if !self.map_value(right, &key, &RuntimeVal::Float(*value), depth)? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringBool(entries) => {
                for (key, value) in entries {
                    let key = RuntimeMapKey::String(key.clone());
                    if !self.map_value(right, &key, &RuntimeVal::Bool(*value), depth)? {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    fn map_value(&self, right: &TypedMap, key: &RuntimeMapKey, left_value: &RuntimeVal, depth: u32) -> Result<bool> {
        let Some(right_value) = right.get(key) else {
            return Ok(false);
        };
        self.nested(left_value, &right_value, depth)
    }
}

fn sets_equal(left: &RuntimeSet, right: &RuntimeSet) -> bool {
    left.len() == right.len() && left.entries().all(|key| right.contains(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::val::HeapValue;

    /// The bug that made one implementation two: a list element longer than
    /// `ShortStr`'s seven inline bytes had no `RuntimeVal`, became `Nil`, and
    /// two `Nil`s compared equal.
    #[test]
    fn long_string_elements_are_compared_by_text_not_flattened_to_nil() {
        let mut heap = HeapStore::new();
        let left = heap.alloc(HeapValue::List(TypedList::String(vec![Arc::from("abcdefghij")])));
        let right = heap.alloc(HeapValue::List(TypedList::String(vec![Arc::from("zzzzzzzzzz")])));
        let same = heap.alloc(HeapValue::List(TypedList::String(vec![Arc::from("abcdefghij")])));

        let equal = |a, b| runtime_values_equal(&RuntimeVal::Obj(a), &RuntimeVal::Obj(b), &heap).expect("compare");
        assert!(!equal(left, right));
        assert!(equal(left, same));
    }

    /// A chain deeper than the bound raises instead of overflowing the Rust
    /// stack — which used to abort the process outright.
    #[test]
    fn nesting_past_the_bound_raises_instead_of_aborting() {
        let mut heap = HeapStore::new();
        let mut build = || {
            let mut node = RuntimeVal::Int(1);
            for _ in 0..(MAX_VALUE_DEPTH + 8) {
                node = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(vec![node]))));
            }
            node
        };
        let left = build();
        let right = build();

        let error = runtime_values_equal(&left, &right, &heap).expect_err("too deep to compare");
        assert!(error.to_string().contains("nested deeper than"), "{error}");
    }

    /// Shallow nesting still compares all the way down.
    #[test]
    fn nesting_within_the_bound_still_compares_structurally() {
        let mut heap = HeapStore::new();
        let mut build = |leaf: i64| {
            let mut node = RuntimeVal::Int(leaf);
            for _ in 0..16 {
                node = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(vec![node]))));
            }
            node
        };
        let left = build(1);
        let same = build(1);
        let different = build(2);

        assert!(runtime_values_equal(&left, &same, &heap).expect("compare"));
        assert!(!runtime_values_equal(&left, &different, &heap).expect("compare"));
    }
}
