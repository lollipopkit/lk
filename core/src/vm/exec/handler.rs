use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeVal};

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

/// `panic(msg)` — an abort, and the one raise `catch` refuses.
///
/// The language has two ways to stop: `error(v)` is recoverable and `catch`
/// binds it, `panic(msg)` is not. That was the documented design and no host
/// implemented it: the desktop one called Rust's `panic!` (which works on a
/// desktop and is an unrecoverable trap in wasm and has no unwinder on bare
/// metal), while the web and bare hosts returned an ordinary error — which
/// `catch` catches, making `panic` recoverable there and not here.
///
/// Being a distinct type is the whole mechanism: the unwinder checks for it
/// before consulting the handler stack, so no `try` can swallow it, on any
/// host, without anyone having to remember.
#[derive(Clone, Debug)]
pub struct LkPanic {
    pub message: alloc::sync::Arc<str>,
}

impl core::fmt::Display for LkPanic {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message.as_ref())
    }
}

impl core::error::Error for LkPanic {}

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
    /// The value a `catch` binds for a message-only raise.
    ///
    /// A **string**, not an `ErrorVal`. This is the observable contract of
    /// today's try/catch (`try { 1/0 } catch e` → `typeof(e) == "String"`),
    /// which the parse-time desugar implements through `pcall`. These opcodes
    /// are the path that replaces it, so they have to bind the same thing —
    /// binding an `ErrorVal` here would silently change `typeof(e)` the moment
    /// the compiler starts emitting them.
    pub(super) fn caught_message_value(&mut self, message: &str) -> RuntimeVal {
        match crate::val::ShortStr::new(message) {
            Some(short) => RuntimeVal::ShortStr(short),
            None => RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(Arc::<str>::from(message)))),
        }
    }

    /// Hands `value` to `handler` and resumes at its catch block.
    ///
    /// Also drops the GC-root pin on a first-class error value now that it is
    /// about to be bound by a live register, mirroring what `pcall` does when it
    /// catches (plan M2.2): the pin exists only to carry a heap error through
    /// unwinding.
    pub(super) fn enter_handler(&mut self, handler: ErrorHandler, value: RuntimeVal) -> Result<()> {
        self.state.set_pending_raise_root(None);
        self.frame_base = handler.frame_base;
        self.state.stack_top = handler.stack_top;
        self.write(handler.catch_reg, value)?;
        self.pc = handler.catch_pc;
        Ok(())
    }

    pub(super) fn raise_language_message(&mut self, message: &str) -> Result<()> {
        if let Some(handler_index) = self
            .handler_stack
            .iter()
            .rposition(|handler| handler.frame_base == self.frame_base)
        {
            let handler = self.handler_stack.remove(handler_index);
            let value = self.caught_message_value(message);
            self.enter_handler(handler, value)
        } else {
            Err(anyhow!(LanguageRaise {
                message: Arc::<str>::from(message),
            }))
        }
    }

    /// Catches an `error(v)` raise in the current frame, binding **`v` itself**
    /// rather than its rendering — the round-trip `pcall` guarantees (plan M2.2)
    /// and the other half of the value contract described on
    /// [`Self::caught_message_value`].
    ///
    /// Without this, a first-class raise crossing these opcodes was not
    /// catchable at all: every catch site downcast `LanguageRaise` only, so
    /// `try { error([1, 2]) } catch e` escaped an opcode-emitted handler.
    pub(super) fn handle_raised_value(&mut self, raised: &LkRaisedValue) -> Result<()> {
        let Some(handler_index) = self
            .handler_stack
            .iter()
            .rposition(|handler| handler.frame_base == self.frame_base)
        else {
            return Err(anyhow!(raised.clone()));
        };
        let handler = self.handler_stack.remove(handler_index);
        self.enter_handler(handler, raised.value)
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
        let value = self.caught_message_value(raise.message.as_ref());
        self.enter_handler(handler, value)
    }
}
