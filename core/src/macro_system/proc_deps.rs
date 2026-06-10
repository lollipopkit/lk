use super::procedural::ProcMacroDependency;
use std::{cell::RefCell, collections::HashSet, rc::Rc};

#[derive(Debug, Clone, Default)]
pub struct ProcMacroDependencyRecorder {
    dependencies: Rc<RefCell<Vec<ProcMacroDependency>>>,
}

impl ProcMacroDependencyRecorder {
    pub fn record(&self, dependencies: &[ProcMacroDependency]) {
        self.dependencies.borrow_mut().extend(dependencies.iter().cloned());
    }

    pub fn dependencies(&self) -> Vec<ProcMacroDependency> {
        let mut seen = HashSet::new();
        self.dependencies
            .borrow()
            .iter()
            .filter(|dependency| seen.insert((dependency.path.clone(), dependency.digest.clone())))
            .cloned()
            .collect()
    }
}
