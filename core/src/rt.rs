use alloc::sync::Arc;

use crate::val::HeapStore;

#[cfg(feature = "async-runtime")]
mod runtime;
#[cfg(not(feature = "async-runtime"))]
mod unsupported;

#[cfg(feature = "async-runtime")]
pub use runtime::*;
#[cfg(not(feature = "async-runtime"))]
pub use unsupported::*;

/// A raise carrying a value out of the heap it was raised in.
///
/// A task runs against a `HeapStore` of its own, and its *result* leaves as a
/// [`RuntimePayload`] — value plus the heap it lives in — precisely so the
/// awaiting side can copy it into its own. A raise carries a value the same
/// way and had no such carrier: the error propagated with a bare handle, and
/// by the time anyone read it the task's heap was gone. `error([1, 2, 3])`
/// inside `spawn` came back as whatever object now sat at that index —
/// `<native fn println(...)>` — with no error reported.
///
/// [`detach`](Self::detach) is called on the task's side while its heap is
/// still alive, [`reattach`](Self::reattach) on the awaiting side.
#[derive(Debug)]
pub struct RaisedPayload {
    pub payload: RuntimePayload,
    pub rendered: Arc<str>,
}

impl core::fmt::Display for RaisedPayload {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.rendered.as_ref())
    }
}

impl core::error::Error for RaisedPayload {}

impl RaisedPayload {
    /// Take a raise out of `heap`, so it can outlive it.
    ///
    /// Anything that is not a first-class raise, and any raise whose payload is
    /// stored inline (an Int, a short string), is returned untouched — those
    /// carry no handle and were never at risk.
    pub fn detach(error: anyhow::Error, heap: &HeapStore) -> anyhow::Error {
        let Some(raised) = error.root_cause().downcast_ref::<crate::vm::LkRaisedValue>() else {
            return error;
        };
        if !matches!(raised.value, crate::val::RuntimeVal::Obj(_)) {
            return error;
        }
        let rendered = Arc::clone(&raised.rendered);
        match RuntimePayload::copy_from_value(&raised.value, heap) {
            Ok(payload) => anyhow::anyhow!(RaisedPayload { payload, rendered }),
            // A payload that cannot be copied at all (a bare closure) keeps the
            // message it already rendered, rather than a handle that faults.
            Err(_) => anyhow::anyhow!("{rendered}"),
        }
    }

    /// Put a detached raise back into `heap`, as the raise it was.
    pub fn reattach(error: anyhow::Error, heap: &mut HeapStore) -> anyhow::Error {
        let Some(detached) = error.root_cause().downcast_ref::<RaisedPayload>() else {
            return error;
        };
        let rendered = Arc::clone(&detached.rendered);
        match detached.payload.clone_value_into(heap) {
            Ok(value) => anyhow::anyhow!(crate::vm::LkRaisedValue { value, rendered }),
            Err(_) => anyhow::anyhow!("{rendered}"),
        }
    }
}
