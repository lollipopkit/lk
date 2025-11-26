use std::{
    any::Any,
    sync::{Arc, Mutex},
};

use anyhow::{Result, anyhow};
use core::fmt;

use crate::vm::VmContext;

use crate::val::Val;

/// Trait implemented by iterator state machines exposed to the runtime.
pub trait IteratorState: Send + Sync + 'static {
    /// Advance the iterator and return the next value if available.
    fn next(&mut self, ctx: &mut VmContext) -> Result<Option<Val>>;

    /// Optional size hint used by collectors for pre-allocation.
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }

    /// Human readable identifier used for diagnostics.
    fn debug_name(&self) -> &'static str {
        "iterator"
    }
}

/// Runtime handle for immutable iterators.
pub struct IteratorValue {
    origin: Option<Arc<str>>,
    state: Mutex<Box<dyn IteratorState>>,
}

impl fmt::Debug for IteratorValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IteratorValue").field("origin", &self.origin).finish()
    }
}

impl IteratorValue {
    pub fn new<S>(state: S) -> Arc<Self>
    where
        S: IteratorState,
    {
        Arc::new(Self {
            origin: None,
            state: Mutex::new(Box::new(state)),
        })
    }

    pub fn with_origin<S>(state: S, origin: Arc<str>) -> Arc<Self>
    where
        S: IteratorState,
    {
        Arc::new(Self {
            origin: Some(origin),
            state: Mutex::new(Box::new(state)),
        })
    }

    pub fn next(&self, ctx: &mut VmContext) -> Result<Option<Val>> {
        let mut guard = self.state.lock().map_err(|_| anyhow!("iterator poisoned"))?;
        guard.next(ctx)
    }

    pub fn size_hint(&self) -> (usize, Option<usize>) {
        if let Ok(guard) = self.state.lock() {
            guard.size_hint()
        } else {
            (0, None)
        }
    }

    pub fn origin(&self) -> Option<&Arc<str>> {
        self.origin.as_ref()
    }
}

/// State machine backing a mutation guard value.
pub trait MutationGuardState: Send + 'static {
    /// Returns the type name exposed to the language (e.g. "ListMut").
    fn guard_type(&self) -> &'static str;

    /// Consumes pending mutations and returns the updated collection value.
    fn commit(&mut self) -> Result<Val>;

    /// Returns an immutable snapshot of the current collection view.
    fn snapshot(&mut self) -> Result<Val>;

    /// Returns `true` if the guard has performed mutations.
    fn has_mutated(&self) -> bool {
        true
    }

    /// Downcast support for guard-specific native methods.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Downcast support for read-only guard access.
    fn as_any(&self) -> &dyn Any;
}

/// Runtime wrapper for collection mutation guards.
pub struct MutationGuardValue {
    state: Mutex<Box<dyn MutationGuardState>>,
}

impl fmt::Debug for MutationGuardValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutationGuardValue")
            .field("guard_type", &self.guard_type())
            .finish()
    }
}

impl MutationGuardValue {
    pub fn new<S>(state: S) -> Arc<Self>
    where
        S: MutationGuardState,
    {
        Arc::new(Self {
            state: Mutex::new(Box::new(state)),
        })
    }

    pub fn guard_type(&self) -> &'static str {
        match self.state.lock() {
            Ok(state) => state.guard_type(),
            Err(_) => "MutationGuard",
        }
    }

    pub fn commit(&self) -> Result<Val> {
        let mut guard = self.state.lock().map_err(|_| anyhow!("mutation guard poisoned"))?;
        guard.commit()
    }

    pub fn with_state_mut<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut dyn MutationGuardState) -> Result<R>,
    {
        let mut state = self.state.lock().map_err(|_| anyhow!("mutation guard poisoned"))?;
        f(state.as_mut())
    }

    pub fn with_state<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&dyn MutationGuardState) -> Result<R>,
    {
        let state = self.state.lock().map_err(|_| anyhow!("mutation guard poisoned"))?;
        f(state.as_ref())
    }

    pub fn snapshot(&self) -> Result<Val> {
        let mut guard = self.state.lock().map_err(|_| anyhow!("mutation guard poisoned"))?;
        guard.snapshot()
    }

    pub fn has_mutated(&self) -> bool {
        self.state.lock().map(|state| state.has_mutated()).unwrap_or(true)
    }
}
