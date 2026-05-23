use super::{CallableValue, HeapValue, RuntimeMapKey, RuntimeVal, TypedList, TypedMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeapRef(u32);

impl HeapRef {
    #[inline]
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    #[inline]
    pub const fn index(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct HeapStore {
    slots: Vec<Option<HeapValue>>,
    marks: Vec<u8>,
    free_list: Vec<u32>,
    live_len: usize,
    alloc_since_gc: u32,
    gc_threshold: u32,
}

impl HeapStore {
    const WHITE: u8 = 0;
    const BLACK: u8 = 2;
    pub const DEFAULT_GC_THRESHOLD: u32 = 1024;

    #[inline]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            marks: Vec::new(),
            free_list: Vec::new(),
            live_len: 0,
            alloc_since_gc: 0,
            gc_threshold: Self::DEFAULT_GC_THRESHOLD,
        }
    }

    #[inline]
    pub fn alloc(&mut self, value: HeapValue) -> HeapRef {
        let index = if let Some(index) = self.free_list.pop() {
            self.slots[index as usize] = Some(value);
            self.marks[index as usize] = Self::WHITE;
            index
        } else {
            let index = self.slots.len();
            assert!(u32::try_from(index).is_ok(), "heap object index overflow");
            self.slots.push(Some(value));
            self.marks.push(Self::WHITE);
            index as u32
        };
        self.live_len += 1;
        self.alloc_since_gc = self.alloc_since_gc.saturating_add(1);
        HeapRef::new(index)
    }

    #[inline]
    pub fn get(&self, reference: HeapRef) -> Option<&HeapValue> {
        self.slots.get(reference.index() as usize)?.as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, reference: HeapRef) -> Option<&mut HeapValue> {
        self.slots.get_mut(reference.index() as usize)?.as_mut()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.live_len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live_len == 0
    }

    #[inline]
    pub fn should_collect(&self) -> bool {
        self.alloc_since_gc >= self.gc_threshold
    }

    #[inline]
    pub fn gc_threshold(&self) -> u32 {
        self.gc_threshold
    }

    #[inline]
    pub fn set_gc_threshold(&mut self, threshold: u32) {
        self.gc_threshold = threshold.max(1);
    }

    pub fn collect(&mut self, roots: impl IntoIterator<Item = HeapRef>) {
        for mark in &mut self.marks {
            *mark = Self::WHITE;
        }
        for root in roots {
            self.mark_ref(root);
        }
        self.sweep();
        self.alloc_since_gc = 0;
    }

    fn mark_ref(&mut self, reference: HeapRef) {
        let index = reference.index() as usize;
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        if slot.is_none() || self.marks.get(index).copied() == Some(Self::BLACK) {
            return;
        }
        self.marks[index] = Self::BLACK;
        let value = slot.as_ref().expect("checked live slot").clone();
        self.mark_heap_value(value);
    }

    fn mark_heap_value(&mut self, value: HeapValue) {
        match value {
            HeapValue::String(_)
            | HeapValue::Task(_)
            | HeapValue::Channel(_)
            | HeapValue::Stream(_)
            | HeapValue::StreamCursor(_) => {}
            HeapValue::List(values) => self.mark_typed_list(values),
            HeapValue::Map(values) => self.mark_typed_map(values),
            HeapValue::Object(object) => {
                for value in object.fields.values() {
                    self.mark_runtime_value(value);
                }
            }
            HeapValue::Callable(CallableValue::Closure { captures, .. }) => {
                for value in &captures {
                    self.mark_runtime_value(value);
                }
            }
            HeapValue::Callable(CallableValue::Runtime32(function)) => {
                let _ = function.collect_garbage();
            }
            HeapValue::Callable(CallableValue::RuntimeNative32 { .. } | CallableValue::Aot(_)) => {}
            HeapValue::UpvalCell(value) => self.mark_runtime_value(&value),
            HeapValue::ErrorVal(error) => {
                for value in &error.trace {
                    self.mark_runtime_value(value);
                }
            }
        }
    }

    fn mark_typed_list(&mut self, values: TypedList) {
        if let TypedList::Mixed(values) = values {
            for value in &values {
                self.mark_runtime_value(value);
            }
        }
    }

    fn mark_typed_map(&mut self, values: TypedMap) {
        match values {
            TypedMap::Mixed(values) => {
                for (key, value) in &values {
                    self.mark_runtime_map_key(key);
                    self.mark_runtime_value(value);
                }
            }
            TypedMap::StringMixed(values) => {
                for value in values.values() {
                    self.mark_runtime_value(value);
                }
            }
            TypedMap::StringInt(_) | TypedMap::StringFloat(_) | TypedMap::StringBool(_) => {}
        }
    }

    fn mark_runtime_map_key(&mut self, key: &RuntimeMapKey) {
        if let RuntimeMapKey::Obj(reference) = key {
            self.mark_ref(*reference);
        }
    }

    fn mark_runtime_value(&mut self, value: &RuntimeVal) {
        if let RuntimeVal::Obj(reference) = value {
            self.mark_ref(*reference);
        }
    }

    fn sweep(&mut self) {
        self.free_list.clear();
        let mut live_len = 0;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                self.free_list.push(index as u32);
                continue;
            }
            if self.marks[index] == Self::BLACK {
                self.marks[index] = Self::WHITE;
                live_len += 1;
            } else {
                *slot = None;
                self.free_list.push(index as u32);
            }
        }
        self.live_len = live_len;
    }
}

impl Default for HeapStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::val::ErrorVal;

    #[test]
    fn heap_store_returns_stable_refs() {
        let mut heap = HeapStore::new();
        let name = heap.alloc(HeapValue::String(Arc::<str>::from("customer")));

        assert_eq!(name.index(), 0);
        assert_eq!(heap.len(), 1);
        assert!(matches!(heap.get(name), Some(HeapValue::String(text)) if text.as_ref() == "customer"));
    }

    #[test]
    fn heap_store_reuses_collected_slots_and_dangling_refs_return_none() {
        let mut heap = HeapStore::new();
        let live = heap.alloc(HeapValue::String(Arc::<str>::from("live")));
        let dead = heap.alloc(HeapValue::String(Arc::<str>::from("dead")));

        heap.collect([live]);

        assert_eq!(heap.len(), 1);
        assert!(heap.get(live).is_some());
        assert_eq!(heap.get(dead).map(HeapValue::type_name), None);

        let reused = heap.alloc(HeapValue::String(Arc::<str>::from("reused")));
        assert_eq!(reused.index(), dead.index());
        assert_eq!(heap.len(), 2);
        assert!(matches!(heap.get(reused), Some(HeapValue::String(value)) if value.as_ref() == "reused"));
    }

    #[test]
    fn heap_store_tracks_gc_threshold_without_collecting_implicitly() {
        let mut heap = HeapStore::new();
        heap.set_gc_threshold(2);

        assert_eq!(heap.gc_threshold(), 2);
        assert!(!heap.should_collect());

        let first = heap.alloc(HeapValue::String(Arc::<str>::from("first")));
        assert!(!heap.should_collect());
        let second = heap.alloc(HeapValue::String(Arc::<str>::from("second")));
        assert!(heap.should_collect());

        heap.collect([first, second]);

        assert!(!heap.should_collect());
        assert_eq!(heap.len(), 2);
    }

    #[test]
    fn heap_store_gc_marks_nested_runtime_refs() {
        let mut heap = HeapStore::new();
        let leaf = heap.alloc(HeapValue::String(Arc::<str>::from("leaf")));
        let list = heap.alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Obj(leaf)])));
        let map = heap.alloc(HeapValue::Map(TypedMap::StringMixed(BTreeMap::from([(
            Arc::<str>::from("list"),
            RuntimeVal::Obj(list),
        )]))));
        let object = heap.alloc(HeapValue::Object(super::super::RuntimeObject {
            type_name: Arc::<str>::from("Box"),
            fields: BTreeMap::from([(Arc::<str>::from("map"), RuntimeVal::Obj(map))]),
        }));
        let closure = heap.alloc(HeapValue::Callable(CallableValue::Closure {
            function_index: 7,
            captures: vec![RuntimeVal::Obj(object)],
        }));
        let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Obj(closure)));
        let error = heap.alloc(HeapValue::ErrorVal(ErrorVal {
            message: Arc::<str>::from("boom"),
            trace: vec![RuntimeVal::Obj(cell)],
        }));
        let garbage = heap.alloc(HeapValue::String(Arc::<str>::from("garbage")));

        heap.collect([error]);

        for handle in [leaf, list, map, object, closure, cell, error] {
            assert!(
                heap.get(handle).is_some(),
                "live handle {} should survive",
                handle.index()
            );
        }
        assert!(heap.get(garbage).is_none());
    }

    #[test]
    fn heap_store_gc_marks_mixed_map_object_keys() {
        let mut heap = HeapStore::new();
        let key_object = heap.alloc(HeapValue::String(Arc::<str>::from("key-object")));
        let map = heap.alloc(HeapValue::Map(TypedMap::Mixed(BTreeMap::from([(
            RuntimeMapKey::Obj(key_object),
            RuntimeVal::Int(1),
        )]))));

        heap.collect([map]);

        assert!(heap.get(map).is_some());
        assert!(heap.get(key_object).is_some());
    }

    #[test]
    fn heap_store_gc_collects_runtime32_callable_shared_state_without_marking_dest_heap_captures() {
        let mut source_heap = HeapStore::new();
        let source_capture = source_heap.alloc(HeapValue::String(Arc::<str>::from("source-capture")));
        let source_garbage = source_heap.alloc(HeapValue::String(Arc::<str>::from("source-garbage")));
        let callable = crate::vm::RuntimeCallable32::new(
            Arc::new(crate::vm::Module32::default()),
            0,
            vec![RuntimeVal::Obj(source_capture)],
            source_heap,
            Vec::new(),
        );

        let mut dest_heap = HeapStore::new();
        let same_index_garbage = dest_heap.alloc(HeapValue::String(Arc::<str>::from("dest-garbage")));
        assert_eq!(same_index_garbage.index(), source_capture.index());
        let callable_handle = dest_heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(
            callable.clone(),
        ))));

        dest_heap.collect([callable_handle]);
        let state = callable.state.lock().expect("runtime callable state");

        assert!(dest_heap.get(callable_handle).is_some());
        assert!(
            dest_heap.get(same_index_garbage).is_none(),
            "source capture handle must not mark same-index object in destination heap"
        );
        assert!(state.heap.get(source_capture).is_some());
        assert!(state.heap.get(source_garbage).is_none());
    }
}
