//! Per-instruction lowering: one bytecode instruction → MIR instructions.
//!
//! [`lower_inst`] is a router over *semantic families*, not an arbitrary
//! split: its match is the routing table and each submodule owns a disjoint
//! set of opcodes. Adding an opcode means picking the family it belongs to and
//! adding one arm there — there is no "last file" that new cases pile up in.
//!
//! Every lowering routine takes a [`LowerCtx`], which carries the per-function
//! state (SSA builder, interned globals, signature inference) plus the
//! read-only module context, so the routines stay at five arguments instead of
//! threading a dozen positional parameters through every call.

use crate::*;

mod call;
mod container;
mod control;
mod global;
mod scalar;
mod string;

/// The state one instruction lowering may touch.
///
/// Split deliberately: the first three fields are *mutable* per-function
/// state, the rest is read-only module context. Submodules destructure only
/// what they need (`let LowerCtx { ssa, .. } = ctx;` style borrows), which
/// keeps each family's actual dependencies visible at a glance.
pub(crate) struct LowerCtx<'a> {
    /// On-demand SSA construction (registers, cells, phis) for this function.
    pub(crate) ssa: &'a mut Ssa,
    /// Interned module globals (C-string constants); [`intern_global`] appends.
    pub(crate) globals: &'a mut Vec<String>,
    /// Cross-function inference (parameter/return types, global slot types,
    /// imports, traits) — mutable because lowering *observes* new facts.
    pub(crate) sig: &'a mut SigInfer,
    /// The function being lowered (constant pools, performance facts).
    pub(crate) func: &'a FunctionData,
    /// Every function in the module (call targets, capture counts).
    pub(crate) funcs: &'a [FunctionData],
    /// The module's entry function index.
    pub(crate) entry: u32,
    /// Module-level global *names*, indexed by slot.
    pub(crate) module_globals: &'a [String],
    /// This function's capture parameters (hidden trailing arguments).
    pub(crate) capture_params: &'a [(ValueId, Ty)],
}

/// Lowers one bytecode instruction into `insts`.
///
/// This match is the routing table: it is the single place that answers "which
/// family owns this opcode". A new opcode is added here *and* in that family's
/// module; an opcode missing from the table falls through to `Unsupported`
/// (a clean fallback, never a miscompile).
///
/// Control-flow opcodes are terminators handled by the caller (`function.rs`);
/// reaching the default arm means a branch targeted the middle of a fused pair
/// or an otherwise malformed shape, which rejects cleanly rather than panicking.
pub(crate) fn lower_inst(
    ctx: &mut LowerCtx<'_>,
    block: usize,
    insts: &mut Vec<Inst>,
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    use Opcode::*;
    match instr.opcode() {
        LoadInt | LoadFloat | LoadBool | LoadNil | Move | Move2 | IsNil | IsList | IsMap | Not | AddInt | SubInt
        | MulInt | DivInt | ModInt | MidInt | MinInt | MaxInt | AddMulInt | Add2Int | AddListInt | SubListInt
        | AddIntI | MulIntI | ModIntI | Neg | FloorDivInt | AddFloat | SubFloat | MulFloat | DivFloat | ModFloat
        | CmpInt | CmpNeInt | CmpLtInt | CmpLeInt | CmpGtInt | CmpGeInt | CastTo => {
            scalar::lower(ctx, block, insts, instr, pc)
        }

        LoadString | ToString | ConcatString | ConcatN | ListJoin | StringSplit => {
            string::lower(ctx, block, insts, instr, pc)
        }

        CallMethodK | CallDirect | LoadFunction | MakeClosure | Call => call::lower(ctx, block, insts, instr, pc),

        LoadCapture | LoadCellVal | StoreCellVal | SetGlobal | GetGlobal => global::lower(ctx, block, insts, instr, pc),

        NewList | GetIndexStrI | SetIndexStrI | LoadHeapConst | Len | SliceFrom | NewRange | ToIter | NewObject
        | ListPush | GetList | GetIndex | SetIndex | GetFieldK | SetFieldK | Contains | MapRest => {
            container::lower(ctx, block, insts, instr, pc)
        }

        Raise => control::lower(ctx, block, insts, instr, pc),

        op => Err(Unsupported::Opcode { pc, op }),
    }
}
