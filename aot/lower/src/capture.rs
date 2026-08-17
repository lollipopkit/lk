//! Resolving a closure's environment at a call site.
//!
//! Four sites hand a closure its captures — the ordinary closure call, a
//! protected call (`try$call`), `spawn`, and the erased-lambda environment
//! [`lower_user_call`] appends as hidden trailing arguments. They differ in what
//! they do with a *cell* the enclosing frame owns, and in nothing else, so the
//! three arms that do not differ ([`ClosureCapture::Value`],
//! [`ClosureCapture::StaticRef`], [`ClosureCapture::CellParam`]) live here once
//! and each site keeps only its own `Cell` arm.
//!
//! [`ClosureCapture::CellParam`] is the arm that was missing at three of the
//! four. A closure nested in a closure captures what its parent captured, and
//! the parent holds that as a capture *parameter* rather than as a cell of its
//! own — so resolving it means reading the parent's parameter, not a slot. Only
//! the ordinary call did that; the rest refused, and a program as plain as
//!
//! ```lk
//! let running = 0;
//! let post = |amount| {
//!     let entry = || { running = running + amount; return running; };
//!     return call_it(entry);
//! };
//! ```
//!
//! fell back to the VM for it (`examples/syntax/closure.lk`, section 13).

use crate::*;

/// What a call site does with a capture the enclosing frame owns.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureMode {
    /// Caller and callee name the same cell, so a write by either is visible to
    /// the other. This is the VM's semantics for every ordinary call.
    Share,
    /// The callee gets a private copy taken at the call. `spawn` is the only
    /// site with this shape: a goroutine's mutation never leaks back.
    Snapshot,
}

/// Where a capture is being resolved *from*: the function currently being
/// lowered, whose capture parameters an onward capture reads.
#[derive(Clone, Copy)]
pub(crate) struct CaptureCtx<'a> {
    /// The enclosing function's hidden trailing parameters.
    pub(crate) params: &'a [(ValueId, Ty)],
    /// Its index — what [`SigInfer::require_cell_capture`] keys a demand on.
    pub(crate) index: u32,
    /// Its visible parameter count, which is where its capture slots begin.
    pub(crate) param_count: usize,
}

/// One call site handing one closure its whole environment: everything that is
/// the same for every capture in the loop, so [`CaptureSite::resolve`] takes
/// only what varies.
#[derive(Clone, Copy)]
pub(crate) struct CaptureSite<'a> {
    ctx: CaptureCtx<'a>,
    callee: u32,
    mode: CaptureMode,
    block: usize,
    pc: usize,
}

impl<'a> CaptureSite<'a> {
    pub(crate) fn new(ctx: CaptureCtx<'a>, callee: u32, mode: CaptureMode, block: usize, pc: usize) -> Self {
        Self {
            ctx,
            callee,
            mode,
            block,
            pc,
        }
    }

    /// Resolves capture `k` to the value this call site passes, or `None` when
    /// the capture is a [`ClosureCapture::Cell`] and the site has to decide.
    ///
    /// `Cell` is deliberately not answered here: the four sites genuinely
    /// disagree about it (seed a fresh runtime cell and read it back, snapshot
    /// its content, or pass the content by value), and that disagreement is the
    /// only real difference between them.
    pub(crate) fn resolve(
        &self,
        ssa: &mut Ssa,
        insts: &mut Vec<Inst>,
        sig: &mut SigInfer,
        capture: &ClosureCapture,
        k: usize,
    ) -> Result<Option<(ValueId, Ty)>, Unsupported> {
        Ok(Some(match capture {
            ClosureCapture::Cell(_) => return Ok(None),
            ClosureCapture::Value(v, ty) => (*v, *ty),
            // A static reference carries no runtime value; the slot exists only
            // to keep the ABI arity, so it carries a dead `0`.
            ClosureCapture::StaticRef => {
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                (zero, Ty::I64)
            }
            ClosureCapture::CellParam(outer) => self.resolve_cell_param(ssa, insts, sig, *outer, k)?,
        }))
    }

    /// Capture `outer` of the enclosing function, handed onward to the callee's
    /// capture `k`.
    fn resolve_cell_param(
        &self,
        ssa: &mut Ssa,
        insts: &mut Vec<Inst>,
        sig: &mut SigInfer,
        outer: usize,
        k: usize,
    ) -> Result<(ValueId, Ty), Unsupported> {
        let pc = self.pc;
        let &(v, ty) = self.ctx.params.get(outer).ok_or(Unsupported::BadConst { pc })?;
        // The enclosing frame already holds a runtime cell for this capture.
        if ty == Ty::Cell {
            return Ok(match self.mode {
                // The pointer passes through, so parent and child name one cell
                // — which is what the VM does.
                CaptureMode::Share => (v, Ty::Cell),
                // The goroutine reads the content once, at the spawn, and never
                // sees a later write to it.
                CaptureMode::Snapshot => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("rt", "cell_get"),
                        args: vec![v],
                    });
                    (dst, Ty::Dyn)
                }
            });
        }
        // The enclosing function is *itself* a goroutine body: its capture
        // parameters are thread-private, and its own writes went to a virtual
        // slot rather than to `v` (see `inst::global`'s `StoreCellVal`). The
        // slot holds the current content; `v` is only its value at entry.
        if ssa.spawned_isolate {
            // A child that writes needs somewhere for the write to land, and a
            // thread-private slot is not addressable from another frame.
            if self.mode == CaptureMode::Share && sig.cell_captures.contains(&(self.callee, k)) {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "a goroutine's private capture cannot be written through a nested closure",
                });
            }
            return ssa.read_slot(ssa.cellparam_slot(outer), self.block, pc);
        }
        // A by-value capture parameter is one nothing writes: `StoreCellVal` on
        // a non-cell `CellParam` demands a cell before it lowers, so reaching
        // here means the enclosing function only reads it. Passing the value on
        // keeps the child's reads correct and allocates nothing.
        //
        // Unless the child writes. That demand was recorded when *its* body
        // lowered, and it propagates up exactly one frame here: the enclosing
        // function's own capture has to become a cell, which its caller seeds.
        if sig.cell_captures.contains(&(self.callee, k)) {
            if sig.require_cell_capture(self.ctx.index as usize, self.ctx.param_count, outer) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            // Already demanded, and it still arrived by value: whoever calls
            // the enclosing function cannot give this capture a cell.
            return Err(Unsupported::CallShape {
                pc,
                reason: "a capture written through two closure frames has no native cell to write to",
            });
        }
        Ok((v, ty))
    }
}
