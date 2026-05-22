use anyhow::{Result, bail};

use crate::val::{ErrorVal, HeapValue, RuntimeVal};

#[derive(Clone, Debug)]
pub(super) struct LanguageRaise32 {
    pub(super) message: std::sync::Arc<str>,
}

impl std::fmt::Display for LanguageRaise32 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message.as_ref())
    }
}

impl std::error::Error for LanguageRaise32 {}

#[derive(Clone, Debug)]
pub(super) struct ErrorHandler32 {
    pub(super) catch_reg: u8,
    pub(super) catch_pc: usize,
    pub(super) frame_base: usize,
    pub(super) stack_top: usize,
    pending_error: Option<RuntimeVal>,
}

impl ErrorHandler32 {
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

impl super::Executor32 {
    pub(super) fn handle_language_raise(&mut self, raise: &LanguageRaise32) -> Result<()> {
        let Some(handler) = self.handler_stack.pop() else {
            bail!("{}", raise.message);
        };
        let error = RuntimeVal::Obj(self.state.heap.alloc(HeapValue::ErrorVal(ErrorVal {
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
