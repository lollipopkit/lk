use std::sync::Arc;

use anyhow::{Result, anyhow};
use lkr_core::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};
use lkr_core::val::Val;

/// Trait describing a sequence container that supports mutation with
/// copy-on-write semantics.
///
/// Implementations should attempt to reuse the underlying buffer when the
/// container is uniquely owned, and fall back to cloning when aliases exist.
pub trait MutableSequence {
    /// Returns the current length of the sequence view.
    fn len(&self) -> usize;

    /// Returns true when the sequence is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an immutable view of the sequence.
    fn as_slice(&self) -> &[Val];

    /// Ensures capacity for at least `len() + additional` items.
    fn reserve(&mut self, additional: usize);

    /// Pushes a new item to the end of the sequence.
    fn push(&mut self, value: Val);

    /// Extends the sequence with the provided iterator.
    fn extend<I>(&mut self, values: I)
    where
        I: IntoIterator<Item = Val>;

    /// Replaces the value at the given index, returning the previous value.
    fn replace(&mut self, index: usize, value: Val) -> Result<Val>;

    /// Removes the value at the given index, returning it if present.
    fn remove(&mut self, index: usize) -> Option<Val>;

    /// Finalises the mutation and returns the resulting `Val` representation.
    fn finish(self) -> Val;
}

/// Trait describing a map container that supports mutation with copy-on-write
/// semantics.
pub trait MutableMap {
    /// Returns the current number of entries in the map view.
    fn len(&self) -> usize;

    /// Returns true when the map is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns true if the map contains a key.
    fn contains_key(&self, key: &str) -> bool;

    /// Inserts or replaces a key with the provided value, returning the previous value.
    fn insert(&mut self, key: Arc<str>, value: Val) -> Option<Val>;

    /// Removes a key from the map, returning the removed value when it existed.
    fn remove(&mut self, key: &str) -> Option<Val>;

    /// Retains only the entries that satisfy the predicate.
    fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&Arc<str>, &mut Val) -> bool;

    /// Finalises the mutation and returns the resulting `Val`.
    fn finish(self) -> Val;
}

/// Copy-on-write mutation guard for list values.
pub struct ListMutation {
    original: Arc<[Val]>,
    scratch: Option<Vec<Val>>,
}

impl ListMutation {
    /// Creates a new mutation guard from a list value.
    pub fn from_val(val: &Val) -> Result<Self> {
        match val {
            Val::List(list) => Ok(Self::new(list.clone())),
            other => Err(anyhow!("expected list, got {}", other.type_name())),
        }
    }

    /// Wraps the provided list arc.
    pub fn new(list: Arc<[Val]>) -> Self {
        Self {
            original: list,
            scratch: None,
        }
    }

    /// Returns true if no mutations have been performed.
    fn pristine(&self) -> bool {
        self.scratch.is_none()
    }

    fn ensure_owned(&mut self) -> &mut Vec<Val> {
        if self.scratch.is_none() {
            let mut owned = Vec::with_capacity(self.original.len());
            owned.extend(self.original.iter().cloned());
            self.scratch = Some(owned);
        }
        self.scratch.as_mut().expect("scratch initialised above")
    }
}

impl MutableSequence for ListMutation {
    fn len(&self) -> usize {
        self.scratch
            .as_ref()
            .map(|v| v.len())
            .unwrap_or_else(|| self.original.len())
    }

    fn as_slice(&self) -> &[Val] {
        match &self.scratch {
            Some(vec) => vec.as_slice(),
            None => self.original.as_ref(),
        }
    }

    fn reserve(&mut self, additional: usize) {
        let new_cap = self.len().saturating_add(additional);
        if let Some(vec) = self.scratch.as_mut() {
            vec.reserve(additional);
        } else if additional > 0 {
            let mut owned = Vec::with_capacity(new_cap);
            owned.extend(self.original.iter().cloned());
            self.scratch = Some(owned);
        }
    }

    fn push(&mut self, value: Val) {
        self.ensure_owned().push(value);
    }

    fn extend<I>(&mut self, values: I)
    where
        I: IntoIterator<Item = Val>,
    {
        self.ensure_owned().extend(values);
    }

    fn replace(&mut self, index: usize, value: Val) -> Result<Val> {
        let len = self.len();
        if index >= len {
            return Err(anyhow!("index {} out of bounds for len {}", index, len));
        }
        let slot = self.ensure_owned().get_mut(index).expect("index bounds checked above");
        Ok(std::mem::replace(slot, value))
    }

    fn remove(&mut self, index: usize) -> Option<Val> {
        if index >= self.len() {
            return None;
        }
        Some(self.ensure_owned().remove(index))
    }

    fn finish(self) -> Val {
        if self.pristine() {
            Val::List(self.original)
        } else {
            let vec = self.scratch.expect("scratch must exist when mutated");
            Val::List(Arc::<[Val]>::from(vec))
        }
    }
}

/// Copy-on-write mutation guard for map values.
pub struct MapMutation {
    original: Arc<FastHashMap<Arc<str>, Val>>,
    scratch: Option<FastHashMap<Arc<str>, Val>>,
}

impl MapMutation {
    pub fn from_val(val: &Val) -> Result<Self> {
        match val {
            Val::Map(map) => Ok(Self::new(map.clone())),
            other => Err(anyhow!("expected map, got {}", other.type_name())),
        }
    }

    pub fn new(map: Arc<FastHashMap<Arc<str>, Val>>) -> Self {
        Self {
            original: map,
            scratch: None,
        }
    }

    fn ensure_owned(&mut self) -> &mut FastHashMap<Arc<str>, Val> {
        if self.scratch.is_none() {
            let mut owned = fast_hash_map_with_capacity(self.original.len());
            for (k, v) in self.original.iter() {
                owned.insert(k.clone(), v.clone());
            }
            self.scratch = Some(owned);
        }
        self.scratch.as_mut().expect("scratch initialised above")
    }

    fn pristine(&self) -> bool {
        self.scratch.is_none()
    }
}

impl MutableMap for MapMutation {
    fn len(&self) -> usize {
        self.scratch
            .as_ref()
            .map(|m| m.len())
            .unwrap_or_else(|| self.original.len())
    }

    fn contains_key(&self, key: &str) -> bool {
        match &self.scratch {
            Some(map) => map.contains_key(key),
            None => self.original.contains_key(key),
        }
    }

    fn insert(&mut self, key: Arc<str>, value: Val) -> Option<Val> {
        self.ensure_owned().insert(key, value)
    }

    fn remove(&mut self, key: &str) -> Option<Val> {
        self.ensure_owned().remove(key)
    }

    fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&Arc<str>, &mut Val) -> bool,
    {
        let map = self.ensure_owned();
        map.retain(|k, v| f(k, v));
    }

    fn finish(self) -> Val {
        if self.pristine() {
            Val::Map(self.original)
        } else {
            let map = self.scratch.expect("scratch must exist when mutated");
            Val::Map(Arc::new(map))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_mutation_reuses_original_when_pristine() {
        let original: Arc<[Val]> = vec![Val::Int(1), Val::Int(2)].into();
        let guard = ListMutation::new(original.clone());
        let result = guard.finish();
        let Val::List(list_arc) = result else {
            panic!("expected list");
        };
        assert!(Arc::ptr_eq(&list_arc, &original));
    }

    #[test]
    fn list_mutation_clones_on_write() {
        let original: Arc<[Val]> = vec![Val::Int(1), Val::Int(2)].into();
        let mut guard = ListMutation::new(original.clone());
        guard.push(Val::Int(3));
        let result = guard.finish();
        let Val::List(list_arc) = result else {
            panic!("expected list");
        };
        assert_eq!(list_arc.len(), 3);
        assert!(!Arc::ptr_eq(&list_arc, &original));
    }

    #[test]
    fn map_mutation_reuses_original_when_pristine() {
        let mut map = fast_hash_map_with_capacity(1);
        map.insert(Arc::<str>::from("k"), Val::Int(1));
        let original = Arc::new(map);
        let guard = MapMutation::new(original.clone());
        let result = guard.finish();
        let Val::Map(map_arc) = result else {
            panic!("expected map");
        };
        assert!(Arc::ptr_eq(&map_arc, &original));
    }

    #[test]
    fn map_mutation_clone_on_write() {
        let mut map = fast_hash_map_with_capacity(1);
        map.insert(Arc::<str>::from("k"), Val::Int(1));
        let original = Arc::new(map);
        let mut guard = MapMutation::new(original.clone());
        guard.insert(Arc::<str>::from("k2"), Val::Int(2));
        let result = guard.finish();
        let Val::Map(map_arc) = result else {
            panic!("expected map");
        };
        assert_eq!(map_arc.len(), 2);
        assert!(!Arc::ptr_eq(&map_arc, &original));
    }
}
