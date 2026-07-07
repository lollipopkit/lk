#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{ErrorVal, HeapValue, RuntimeVal};

#[derive(Clone, Debug)]
pub(super) struct LanguageRaise {
    pub(super) message: alloc::sync::Arc<str>,
}

impl core::fmt::Display for LanguageRaise {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message.as_ref())
    }
}

impl core::error::Error for LanguageRaise {}

/// A recoverable error carrying a first-class LK value. `error(v)` raises this
/// and `pcall` extracts `value`, so an errored value round-trips as itself
/// rather than a string — including heap objects (String/List/…), which are
/// pinned as a GC root while unwinding (see `RuntimeModuleState::pending_raise_root`,
/// plan M2.2). `rendered` is the display captured at raise time, used for the
/// top-level message when the error is *uncaught*: by then the heap is gone, so a
/// live `Obj` handle can no longer be formatted. Public so the stdlib's
/// `error`/`pcall` can construct and downcast it.
#[derive(Clone, Debug)]
pub struct LkRaisedValue {
    pub value: RuntimeVal,
    pub rendered: Arc<str>,
}

impl core::fmt::Display for LkRaisedValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.rendered.as_ref())
    }
}

impl core::error::Error for LkRaisedValue {}

#[derive(Clone, Debug)]
pub(super) struct ErrorHandler {
    pub(super) catch_reg: u8,
    pub(super) catch_pc: usize,
    pub(super) frame_base: usize,
    pub(super) stack_top: usize,
    pending_error: Option<RuntimeVal>,
}

impl ErrorHandler {
    pub(super) fn new(catch_reg: u8, catch_pc: usize, frame_base: usize, stack_top: usize) -> Self {
        Self {
            catch_reg,
            catch_pc,
            frame_base,
            stack_top,
            pending_error: None,
        }
    }

    pub(super) fn roots(&self) -> impl Iterator<Item = &RuntimeVal> + '_ {
        self.pending_error.iter()
    }
}

impl super::Executor {
    pub(super) fn raise_language_message(&mut self, message: &str) -> Result<()> {
        if let Some(handler_index) = self
            .handler_stack
            .iter()
            .rposition(|handler| handler.frame_base == self.frame_base)
        {
            let handler = self.handler_stack.remove(handler_index);
            let error = RuntimeVal::Obj(self.alloc_heap_value(HeapValue::ErrorVal(ErrorVal {
                message: Arc::<str>::from(message),
                trace: Vec::new(),
            })));
            self.frame_base = handler.frame_base;
            self.state.stack_top = handler.stack_top;
            self.write(handler.catch_reg, error)?;
            self.pc = handler.catch_pc;
            Ok(())
        } else {
            Err(anyhow!(LanguageRaise {
                message: Arc::<str>::from(message),
            }))
        }
    }

    pub(super) fn begin_try(&mut self, catch_reg: u8, catch_offset: i32) -> Result<()> {
        let catch_pc = self.relative_pc(catch_offset)?;
        self.handler_stack.push(ErrorHandler::new(
            catch_reg,
            catch_pc,
            self.frame_base,
            self.state.stack_top,
        ));
        self.pc += 1;
        Ok(())
    }

    pub(super) fn end_try(&mut self) {
        let _ = self.handler_stack.pop();
        self.pc += 1;
    }

    pub(super) fn handle_language_raise(&mut self, raise: &LanguageRaise) -> Result<()> {
        let Some(handler) = self.handler_stack.pop() else {
            bail!("{}", raise.message);
        };
        let error = RuntimeVal::Obj(self.alloc_heap_value(HeapValue::ErrorVal(ErrorVal {
            message: raise.message.clone(),
            trace: Vec::new(),
        })));
        self.frame_base = handler.frame_base;
        self.state.stack_top = handler.stack_top;
        self.write(handler.catch_reg, error)?;
        self.pc = handler.catch_pc;
        Ok(())
    }
}
