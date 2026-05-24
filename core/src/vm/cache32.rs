use crate::{
    val::HeapRef,
    vm::analysis::{PerfCallFact, PerfIndexFact},
};

#[derive(Clone, Copy, Debug)]
pub struct IndexInlineCache32 {
    pub handle: HeapRef,
    pub generation: u64,
    pub fact: PerfIndexFact,
    pub object_field_slot: Option<u16>,
}

#[derive(Clone, Debug, Default)]
pub struct InlineCaches32 {
    pub globals: Vec<Option<u16>>,
    pub indexes: Vec<Option<IndexInlineCache32>>,
    pub calls: Vec<Option<PerfCallFact>>,
}

impl InlineCaches32 {
    pub fn global(&self, pc: usize) -> Option<u16> {
        self.globals.get(pc).copied().flatten()
    }

    pub fn set_global(&mut self, pc: usize, slot: u16) {
        if self.globals.len() <= pc {
            self.globals.resize(pc + 1, None);
        }
        self.globals[pc] = Some(slot);
    }

    pub fn index(&self, pc: usize, handle: HeapRef, generation: u64) -> Option<IndexInlineCache32> {
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
        self.indexes[pc] = Some(IndexInlineCache32 {
            handle,
            generation,
            fact,
            object_field_slot,
        });
    }

    pub fn index_fact_for_tests(&self, pc: usize) -> Option<PerfIndexFact> {
        self.indexes.get(pc).copied().flatten().map(|cache| cache.fact)
    }

    pub fn index_cache_for_tests(&self, pc: usize) -> Option<IndexInlineCache32> {
        self.indexes.get(pc).copied().flatten()
    }

    pub fn call(&self, pc: usize) -> Option<PerfCallFact> {
        self.calls.get(pc).copied().flatten()
    }

    pub fn set_call(&mut self, pc: usize, fact: PerfCallFact) {
        if self.calls.len() <= pc {
            self.calls.resize(pc + 1, None);
        }
        self.calls[pc] = Some(fact);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::analysis::{PerfIndexTargetKind, PerfValueKind};

    #[test]
    fn index_inline_cache_is_guarded_by_handle_and_generation() {
        let mut caches = InlineCaches32::default();
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
