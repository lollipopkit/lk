use anyhow::{Result, anyhow};

use crate::{
    util::fast_map::{FastHashMap, fast_hash_map_new},
    val::Val,
};

#[derive(Debug, Clone)]
pub(super) struct ValBindingContext {
    globals: FastHashMap<String, Val>,
    locals: Vec<FastHashMap<String, Val>>,
}

impl ValBindingContext {
    pub(super) fn new() -> Self {
        Self {
            globals: fast_hash_map_new(),
            locals: Vec::new(),
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<&Val> {
        for scope in self.locals.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Some(value);
            }
        }
        self.globals.get(name)
    }

    pub(super) fn set(&mut self, name: String, value: Val) -> Option<Val> {
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name, value)
        } else {
            self.globals.insert(name, value)
        }
    }

    pub(super) fn assign(&mut self, name: &str, value: Val) -> Result<()> {
        for scope in self.locals.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = value;
                return Ok(());
            }
        }
        if let Some(slot) = self.globals.get_mut(name) {
            *slot = value;
            Ok(())
        } else {
            Err(anyhow!("Undefined variable: {}", name))
        }
    }

    pub(super) fn remove(&mut self, name: &str) -> Option<Val> {
        if let Some(scope) = self.locals.last_mut()
            && let Some(prev) = scope.remove(name)
        {
            return Some(prev);
        }
        self.globals.remove(name)
    }

    pub(super) fn remove_global(&mut self, name: &str) {
        self.globals.remove(name);
    }

    pub(super) fn push_scope(&mut self) {
        self.locals.push(fast_hash_map_new());
    }

    pub(super) fn pop_scope(&mut self) -> bool {
        self.locals.pop().is_some()
    }

    pub(super) fn has_local_scope(&self) -> bool {
        !self.locals.is_empty()
    }

    pub(super) fn bind_param_at_slot(&mut self, name: String, value: Val) {
        if self.locals.is_empty() {
            self.push_scope();
        }
        if let Some(scope) = self.locals.last_mut() {
            scope.insert(name, value);
        }
    }
}

impl Default for ValBindingContext {
    fn default() -> Self {
        Self::new()
    }
}
