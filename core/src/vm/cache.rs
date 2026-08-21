#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{val::HeapRef, vm::analysis::PerfIndexFact};

#[derive(Clone, Copy, Debug)]
pub struct IndexInlineCache {
    pub handle: HeapRef,
    pub generation: u64,
    pub fact: PerfIndexFact,
    pub object_field_slot: Option<u16>,
}

#[derive(Clone, Debug, Default)]
pub struct InlineCaches {
    pub indexes: Vec<Option<IndexInlineCache>>,
}

impl InlineCaches {
    pub fn index(&self, pc: usize, handle: HeapRef, generation: u64) -> Option<IndexInlineCache> {
        self.indexes
            .get(pc)
            .copied()
            .flatten()
            .filter(|cache| cache.handle == handle && cache.generation == generation)
    }

    pub fn set_index(
        &mut self,
        pc: usize,
        handle: HeapRef,
        generation: u64,
        fact: PerfIndexFact,
        object_field_slot: Option<u16>,
    ) {
        if self.indexes.len() <= pc {
            self.indexes.resize(pc + 1, None);
        }
        self.indexes[pc] = Some(IndexInlineCache {
            handle,
            generation,
            fact,
            object_field_slot,
        });
    }

    pub fn index_fact_for_tests(&self, pc: usize) -> Option<PerfIndexFact> {
        self.indexes.get(pc).copied().flatten().map(|cache| cache.fact)
    }

    pub fn index_cache_for_tests(&self, pc: usize) -> Option<IndexInlineCache> {
        self.indexes.get(pc).copied().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::analysis::{PerfIndexTargetKind, PerfValueKind};

    #[test]
    fn index_inline_cache_is_guarded_by_handle_and_generation() {
        let mut caches = InlineCaches::default();
        let handle = HeapRef::new(7);
        let fact = PerfIndexFact {
            target_kind: PerfIndexTargetKind::Map,
            value_kind: PerfValueKind::Int,
        };

        caches.set_index(3, handle, 11, fact, Some(2));

        let cache = caches.index(3, handle, 11).expect("cache hit");
        assert_eq!(cache.fact, fact);
        assert_eq!(cache.object_field_slot, Some(2));
        assert!(caches.index(3, HeapRef::new(8), 11).is_none());
        assert!(caches.index(3, handle, 12).is_none());
        assert_eq!(caches.index_fact_for_tests(3), Some(fact));
    }
}
