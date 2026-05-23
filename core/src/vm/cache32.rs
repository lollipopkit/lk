use crate::vm::analysis::{PerfCallFact, PerfIndexFact};

#[derive(Clone, Debug, Default)]
pub struct InlineCaches32 {
    pub globals: Vec<Option<u16>>,
    pub indexes: Vec<Option<PerfIndexFact>>,
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

    pub fn index(&self, pc: usize) -> Option<PerfIndexFact> {
        self.indexes.get(pc).copied().flatten()
    }

    pub fn set_index(&mut self, pc: usize, fact: PerfIndexFact) {
        if self.indexes.len() <= pc {
            self.indexes.resize(pc + 1, None);
        }
        self.indexes[pc] = Some(fact);
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
