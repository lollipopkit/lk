#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use super::{CallableValue, HeapValue, RuntimeVal, TypedList, TypedMap};
use crate::vm::RuntimeCallable;

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
    generations: Vec<u64>,
    free_list: Vec<u32>,
    live_len: usize,
    alloc_since_gc: u32,
    gc_threshold: u32,
    /// An embedder asked for a specific threshold, so the live-set scaling in
    /// [`Self::should_collect`] is off: a number given by name means that
    /// number, not a floor under a policy the caller did not ask for.
    pinned_threshold: bool,
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
            generations: Vec::new(),
            free_list: Vec::new(),
            live_len: 0,
            alloc_since_gc: 0,
            gc_threshold: Self::DEFAULT_GC_THRESHOLD,
            pinned_threshold: false,
        }
    }

    #[inline]
    pub fn alloc(&mut self, value: HeapValue) -> HeapRef {
        let index = if let Some(index) = self.free_list.pop() {
            self.slots[index as usize] = Some(value);
            self.marks[index as usize] = Self::WHITE;
            self.generations[index as usize] = self.generations[index as usize].wrapping_add(1);
            index
        } else {
            let index = self.slots.len();
            assert!(u32::try_from(index).is_ok(), "heap object index overflow");
            self.slots.push(Some(value));
            self.marks.push(Self::WHITE);
            self.generations.push(0);
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
    pub fn shape_generation(&self, reference: HeapRef) -> Option<u64> {
        self.slots.get(reference.index() as usize)?.as_ref()?;
        self.generations.get(reference.index() as usize).copied()
    }

    #[inline]
    pub fn bump_shape_generation(&mut self, reference: HeapRef) {
        let idx = reference.index() as usize;
        if idx < self.slots.len() && self.slots[idx].is_some() {
            self.generations[idx] = self.generations[idx].wrapping_add(1);
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.live_len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live_len == 0
    }

    /// Whether enough has been allocated since the last collection to be worth
    /// another one.
    ///
    /// The bound is the *live set*, floored at [`Self::gc_threshold`]. A mark
    /// and sweep costs O(live) whatever triggered it, so collecting every fixed
    /// number of allocations makes the collector's share of a program grow with
    /// the data it holds: a workload with a hundred thousand live objects paid a
    /// hundred-thousand-object walk every thousand allocations. Waiting for the
    /// heap to grow by its own size instead keeps the amortized cost per
    /// allocation constant — the standard bound, and the reason `collect` was
    /// 6% of `bench/workloads_business_algorithms.lk` before it.
    ///
    /// `gc_threshold` is the *floor* on how often this can fire, so a small
    /// heap keeps the old behaviour exactly. A threshold set by name turns the
    /// scaling off entirely (`pinned_threshold`): an embedder that asks to
    /// collect every allocation is asking for a policy, not for a floor under
    /// one.
    #[inline]
    pub fn should_collect(&self) -> bool {
        if self.pinned_threshold {
            return self.alloc_since_gc >= self.gc_threshold;
        }
        self.alloc_since_gc as usize >= (self.gc_threshold as usize).max(self.live_len / 2)
    }

    #[inline]
    pub fn gc_threshold(&self) -> u32 {
        self.gc_threshold
    }

    #[inline]
    pub fn set_gc_threshold(&mut self, threshold: u32) {
        self.gc_threshold = threshold.max(1);
        self.pinned_threshold = true;
    }

    /// Mark and sweep.
    ///
    /// Marking walks an explicit worklist rather than recursing. Recursion put
    /// the object graph's *depth* on the Rust stack, so a chain a program can
    /// build in a loop —
    ///
    /// ```lk
    /// let node: Any = [1];
    /// for i in 0..200000 { node = [node]; }
    /// ```
    ///
    /// — aborted the process with `fatal runtime error: stack overflow` at the
    /// next collection, with no way for a script to catch it and no line to
    /// blame: the allocation that tripped the threshold, not the one at fault.
    /// The worklist also lets edges land straight in it, so marking no longer
    /// allocates a fresh `Vec` per object visited.
    pub fn collect(&mut self, roots: impl IntoIterator<Item = HeapRef>) {
        // No whitening pass. Every mark is already `WHITE` when a collection
        // starts, and the three places that could say otherwise all maintain
        // it: `sweep` turns each surviving `BLACK` back, a swept slot was never
        // marked in the first place, and both `alloc` paths write `WHITE`. The
        // loop that used to be here walked every slot the heap had *ever* held
        // to write a value each of them already had — the same O(slots) the
        // sweep costs, spent twice.
        debug_assert!(
            self.marks.iter().all(|mark| *mark == Self::WHITE),
            "a collection starts from an all-white heap"
        );
        let mut worklist: Vec<HeapRef> = roots.into_iter().collect();
        let mut runtime_callables = Vec::new();
        while let Some(reference) = worklist.pop() {
            let index = reference.index() as usize;
            if index >= self.slots.len() || self.slots[index].is_none() || self.marks[index] == Self::BLACK {
                continue;
            }
            self.marks[index] = Self::BLACK;
            let value = self.slots[index].as_ref().expect("checked live slot");
            collect_heap_value_edges(value, &mut worklist, &mut runtime_callables);
        }
        // Deferred to here rather than done mid-walk: each of these collects a
        // *different* heap (the callable's own module state), so the order
        // relative to this heap's marking cannot matter.
        for function in runtime_callables {
            let _ = function.collect_garbage();
        }
        self.sweep();
        self.alloc_since_gc = 0;
    }
}

fn collect_heap_value_edges(
    value: &HeapValue,
    refs: &mut Vec<HeapRef>,
    runtime_callables: &mut Vec<Arc<RuntimeCallable>>,
) {
    match value {
        HeapValue::String(_)
        | HeapValue::Bytes(_)
        | HeapValue::Task(_)
        | HeapValue::Channel(_)
        | HeapValue::Resource(_) => {}
        HeapValue::Stream(stream) => {
            for value in &stream.roots {
                collect_runtime_value_edge(value, refs);
            }
        }
        HeapValue::StreamCursor(cursor) => {
            for value in &cursor.roots {
                collect_runtime_value_edge(value, refs);
            }
        }
        HeapValue::Slice(slice) => collect_runtime_value_edge(&slice.source, refs),
        HeapValue::List(values) => collect_typed_list_edges(values, refs),
        HeapValue::Map(values) => collect_typed_map_edges(values, refs),
        // A set's members are `RuntimeMapKey`s, and none of those is a heap
        // handle any more: a long string key is an `Arc<str>` held inline, and
        // a container cannot be a key at all (see `RuntimeMapKey::from_value`).
        // So a set has no outgoing edges, like a string or a byte buffer.
        HeapValue::Set(_) => {}
        HeapValue::Object(object) => {
            for value in object.fields.values() {
                collect_runtime_value_edge(value, refs);
            }
        }
        HeapValue::Callable(CallableValue::Closure { captures, .. }) => {
            for value in captures.iter() {
                collect_runtime_value_edge(value, refs);
            }
        }
        HeapValue::Callable(CallableValue::Runtime(function)) => {
            runtime_callables.push(Arc::clone(function));
        }
        HeapValue::Callable(CallableValue::RuntimeNative { .. }) => {}
        HeapValue::UpvalCell(value) => collect_runtime_value_edge(value, refs),
        HeapValue::ErrorVal(error) => {
            for value in &error.trace {
                collect_runtime_value_edge(value, refs);
            }
        }
    }
}

fn collect_typed_list_edges(values: &TypedList, refs: &mut Vec<HeapRef>) {
    if let TypedList::Mixed(values) = values {
        for value in values {
            collect_runtime_value_edge(value, refs);
        }
    }
}

fn collect_typed_map_edges(values: &TypedMap, refs: &mut Vec<HeapRef>) {
    match values {
        TypedMap::Mixed(values) => {
            // Keys hold no handles — see the `Set` arm above.
            for value in values.values() {
                collect_runtime_value_edge(value, refs);
            }
        }
        TypedMap::StringMixed(values) => {
            for value in values.values() {
                collect_runtime_value_edge(value, refs);
            }
        }
        TypedMap::StringInt(_) | TypedMap::StringFloat(_) | TypedMap::StringBool(_) => {}
    }
}

fn collect_runtime_value_edge(value: &RuntimeVal, refs: &mut Vec<HeapRef>) {
    if let RuntimeVal::Obj(reference) = value {
        refs.push(*reference);
    }
}

impl HeapStore {
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
    use crate::compat::sync::Mutex;
    use alloc::sync::Arc;

    use super::*;
    use crate::{
        val::{ErrorVal, StreamCursorValue, StreamValue, Type},
        vm::RuntimeModuleState,
    };

    /// The collector's share of a program must not grow with the data it holds.
    ///
    /// A mark and sweep costs O(live) whatever triggered it, so a *fixed*
    /// allocation threshold makes total GC work O(allocations x live) — a
    /// program with a large live set paid a full walk every thousand
    /// allocations. Scaling the trigger with the live set makes it O(1)
    /// amortized per allocation, which is what this measures: ten times the
    /// live set must not mean ten times the collections per allocation.
    ///
    /// Counted rather than timed, so it says the same thing on any machine.
    #[test]
    fn collections_do_not_multiply_with_the_live_set() {
        fn collections_for(live: usize, allocations: usize) -> usize {
            let mut heap = HeapStore::new();
            let roots: Vec<HeapRef> = (0..live)
                .map(|i| heap.alloc(HeapValue::String(Arc::<str>::from(alloc::format!("live{i}")))))
                .collect();
            let mut collections = 0;
            for i in 0..allocations {
                heap.alloc(HeapValue::String(Arc::<str>::from(alloc::format!("tmp{i}"))));
                if heap.should_collect() {
                    heap.collect(roots.iter().copied());
                    collections += 1;
                }
            }
            collections
        }
        // Ten times the live set, the same number of allocations. With a fixed
        // threshold both answers are the same and the *work* is ten times as
        // much; scaling the trigger trades that for a tenth of the collections.
        let small = collections_for(2_000, 20_000);
        let large = collections_for(20_000, 20_000);
        assert!(small > 0, "the small heap has to collect at all, got {small}");
        assert!(
            large * 5 <= small,
            "ten times the live set should collect far less often per allocation, \
             got {large} collections against {small}"
        );
    }

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
    fn heap_store_shape_generation_tracks_mutation_and_slot_reuse() {
        let mut heap = HeapStore::new();
        let handle = heap.alloc(HeapValue::List(TypedList::Int(vec![1])));
        let initial = heap.shape_generation(handle).expect("live handle generation");

        heap.bump_shape_generation(handle);
        assert_eq!(heap.shape_generation(handle), Some(initial.wrapping_add(1)));

        heap.collect([]);
        assert_eq!(heap.shape_generation(handle), None);

        let reused = heap.alloc(HeapValue::Map(TypedMap::StringInt(
            crate::util::value_map::value_map_new(),
        )));
        assert_eq!(reused.index(), handle.index());
        assert_eq!(heap.shape_generation(reused), Some(initial.wrapping_add(2)));
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
        let map = heap.alloc(HeapValue::Map(TypedMap::StringMixed(
            crate::util::value_map::value_map_from_iter([(Arc::<str>::from("list"), RuntimeVal::Obj(list))]),
        )));
        let object = heap.alloc(HeapValue::Object(crate::val::RuntimeObject::new(
            Arc::new(crate::val::DeclaredType::new(
                crate::val::TypeScope::anonymous(),
                Arc::<str>::from("Box"),
            )),
            crate::util::value_map::value_map_from_iter([(Arc::<str>::from("map"), RuntimeVal::Obj(map))]),
        )));
        let closure = heap.alloc(HeapValue::Callable(CallableValue::Closure {
            function_index: 7,
            captures: Arc::new(vec![RuntimeVal::Obj(object)]),
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
    fn heap_store_gc_marks_stream_and_cursor_roots() {
        let mut heap = HeapStore::new();
        let stream_root = heap.alloc(HeapValue::String(Arc::<str>::from("stream-root")));
        let cursor_root = heap.alloc(HeapValue::String(Arc::<str>::from("cursor-root")));
        let garbage = heap.alloc(HeapValue::String(Arc::<str>::from("garbage")));
        let stream = heap.alloc(HeapValue::Stream(Arc::new(StreamValue {
            id: 1,
            inner_type: Type::Any,
            roots: vec![RuntimeVal::Obj(stream_root)],
        })));
        let cursor = heap.alloc(HeapValue::StreamCursor(Arc::new(StreamCursorValue {
            id: 1,
            stream_id: 1,
            roots: vec![RuntimeVal::Obj(cursor_root)],
        })));

        heap.collect([stream, cursor]);

        assert!(heap.get(stream).is_some());
        assert!(heap.get(cursor).is_some());
        assert!(heap.get(stream_root).is_some());
        assert!(heap.get(cursor_root).is_some());
        assert!(heap.get(garbage).is_none());
    }

    /// Marking used to recurse, so the *depth* of the object graph sat on the
    /// Rust stack and a chain a loop can build aborted the process at the next
    /// collection. There is no depth bound here on purpose: a collection cannot
    /// be allowed to fail.
    #[test]
    fn heap_store_gc_marks_a_chain_far_deeper_than_the_rust_stack() {
        let mut heap = HeapStore::new();
        let mut node = heap.alloc(HeapValue::String(Arc::<str>::from("leaf")));
        for _ in 0..200_000 {
            node = heap.alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Obj(node)])));
        }
        let garbage = heap.alloc(HeapValue::String(Arc::<str>::from("garbage")));

        heap.collect([node]);

        assert_eq!(heap.len(), 200_001);
        assert!(heap.get(node).is_some());
        assert!(heap.get(garbage).is_none());
    }

    #[test]
    fn heap_store_gc_collects_runtime_callable_shared_state_without_marking_dest_heap_captures() {
        let mut source_heap = HeapStore::new();
        let source_capture = source_heap.alloc(HeapValue::String(Arc::<str>::from("source-capture")));
        let source_garbage = source_heap.alloc(HeapValue::String(Arc::<str>::from("source-garbage")));
        let callable = crate::vm::RuntimeCallable::with_state(
            Arc::new(crate::vm::Module::default()),
            0,
            Arc::new(vec![RuntimeVal::Obj(source_capture)]),
            Arc::new(Mutex::new(RuntimeModuleState::new(source_heap, Vec::new()))),
        );

        let mut dest_heap = HeapStore::new();
        let same_index_garbage = dest_heap.alloc(HeapValue::String(Arc::<str>::from("dest-garbage")));
        assert_eq!(same_index_garbage.index(), source_capture.index());
        let callable_handle = dest_heap.alloc(HeapValue::Callable(CallableValue::Runtime(Arc::new(
            callable.shallow_clone_shared(),
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
