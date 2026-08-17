use super::*;

/// Did anything before `pc` write `reg`?
///
/// A textual scan rather than a question to the SSA: this runs while the exit
/// table is being built, before any block exists to ask about. The same
/// over-approximation as `written_registers` applies, and in the same
/// direction — a false positive costs a rejection.
/// How a value of this type is taken back out of a cell, if it can be.
///
/// Boxing into a `Dyn` works for everything; coming back out is per type, and
/// the ones missing here are missing on purpose — a `Maybe` carrier, a channel,
/// a closure. Guessing at one produces a wrong value, so their regions reject.
/// How a register's value comes back out of the cell it travelled in.
///
/// `None` means it cannot, and the region rejects.
/// See [`unbox_from_dyn`].
enum CellReadBack {
    /// The cell's content is the value; nothing to do.
    Identity,
    /// The ABI entry that takes the value back out.
    Unbox(&'static str, &'static str),
}

/// Whether a register of this type crosses a region as a **raw handle** rather
/// than a boxed value.
///
/// A typed container cannot be boxed and read back: its boxing is an
/// element-wise conversion, so the round trip is a copy and the body's writes to
/// the original are lost. It is parked as-is instead, under `DYN_RAW`, and both
/// ends check the tag — crossing the two families is a loud failure rather than
/// a `Vec<i64>` walked as `Vec<LkDyn>`.
///
/// One function, read by the caller (which seeds and reads back) and by the body
/// (which writes on each assignment), so the two cannot disagree about a cell.
pub(crate) fn cell_is_raw(ty: Ty) -> bool {
    matches!(
        ty,
        Ty::ListI64
            | Ty::ListF64
            | Ty::ListStr
            | Ty::MapStrI64
            | Ty::MapI64I64
            | Ty::MapStrF64
            | Ty::MapI64F64
            | Ty::MapStrBool
            | Ty::Set
            | Ty::Bytes
            | Ty::SliceI64
    )
}

fn unbox_from_dyn(ty: Ty) -> Option<CellReadBack> {
    Some(match ty {
        // Already a boxed value: what the cell holds *is* the register's
        // value, so there is nothing to convert. Not the same shape as the
        // typed cases below — those name an ABI entry that reinterprets the
        // cell's contents, and a container reinterpreted that way loses the
        // mutation it travelled to carry (see this module's docs).
        Ty::Dyn => CellReadBack::Identity,
        // A register that holds nil *going in* says nothing about what the body
        // will put there, and the body boxes whatever it writes — so the honest
        // readback type is `Dyn`, not `Nil`. Reading it back as `Nil` would
        // describe the seed rather than the value, which is why
        // `let x = nil; try { x = 5; } catch e {}` was rejected outright.
        Ty::Nil => CellReadBack::Identity,
        Ty::I64 => CellReadBack::Unbox("dyn", "as_i64"),
        // Answers 0/1 in an `i64`, so the caller narrows it back to a `Bool`.
        Ty::Bool => CellReadBack::Unbox("dyn", "as_bool"),
        Ty::F64 => CellReadBack::Unbox("dyn", "as_f64"),
        Ty::Str => CellReadBack::Unbox("dyn", "as_str"),
        // A container that is *already* boxed round-trips by pointer:
        // `dyn.from_list` / `dyn.from_map` only tag the handle, and
        // `dyn.as_list` / `dyn.as_map` check the tag and hand the same pointer
        // back — so the register keeps its identity and the mutations it
        // travelled to carry.
        //
        // A **typed** container cannot join them, and the reason is not caution:
        // its boxing (`dyn_box`) is an element-wise *conversion*
        // (`list_h::i64_to_dyn` builds a second list), so a round trip would
        // hand back a copy — a different handle, with the body's writes to the
        // original lost. Tagging the typed handle as `DYN_LIST` instead is worse
        // than wrong: a `Vec<i64>` read as a `Vec<LkDyn>` is a memory-safety
        // bug. Giving them a round trip means an identity-preserving cell (a raw
        // handle slot, not a boxed one), not another entry in this table.
        Ty::ListDyn => CellReadBack::Unbox("dyn", "as_list"),
        // A window is parked raw by `cell_is_raw` when the register already
        // holds one; this is the other case — a register seeded `nil` that the
        // body assigns a window to, which travels boxed like any other value.
        Ty::SliceI64 => CellReadBack::Unbox("dyn", "as_slice"),
        Ty::MapStrDyn => CellReadBack::Unbox("dyn", "as_map"),
        _ => return None,
    })
}

/// Whether a value of this type can be taken back out of a cell at all.
///
/// The same table [`unbox_cell_value`] uses, asked without emitting anything —
/// what a call site consults before promising a callee that its cell holds one.
pub(crate) fn unbox_cell_value_supported(ty: Ty) -> bool {
    unbox_from_dyn(ty).is_some()
}

/// Joins what a cell was already agreed to hold with what this call site is
/// seeding it from.
///
/// **Monotone, and that is the whole point.** The fixpoint's early passes
/// observe *provisional* types — a callee's return type is its `I64` default
/// until its body has been lowered once — so a rule that simply overwrote made
/// the agreement flip type every pass, the snapshot never settle, and the
/// budget run out: `examples/syntax/closure.lk` stopped lowering entirely.
/// Disagreement goes to `Dyn`, which is where a cell was before any of this,
/// and `Dyn` is absorbing.
pub(crate) fn join_cell_content(previous: Option<Ty>, seeded: Ty) -> Ty {
    match previous {
        Some(prev) if prev == seeded => prev,
        Some(_) => Ty::Dyn,
        None if unbox_cell_value_supported(seeded) => seeded,
        None => Ty::Dyn,
    }
}

/// Takes a value of type `ty` back out of the boxed `Dyn` a cell holds.
///
/// One function for the three places that do it — the region's output cells,
/// the return channel, and a cell *input*'s reads inside the body — so the
/// `Bool` narrowing below cannot be remembered at two of them and forgotten at
/// the third. `None` means the type has no readback and the caller rejects.
pub(crate) fn unbox_cell_value(ssa: &mut Ssa, insts: &mut Vec<Inst>, boxed: ValueId, ty: Ty) -> Option<ValueId> {
    match unbox_from_dyn(ty)? {
        CellReadBack::Identity => Some(boxed),
        CellReadBack::Unbox(module, name) => {
            let raw = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(raw),
                callee: AbiRef::new(module, name),
                args: vec![boxed],
            });
            // `dyn.as_bool` answers an `i64`; a `Bool` operand is narrower, and
            // the Cranelift verifier rejects the wide value.
            if ty != Ty::Bool {
                return Some(raw);
            }
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let narrow = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: narrow,
                op: CmpOp::Ne,
                float: false,
                lhs: raw,
                rhs: zero,
            });
            Some(narrow)
        }
    }
}

/// The function a region's body became, or a rejection naming the region.
fn body_index_of(sig: &SigInfer, func_index: u32, begin_pc: usize) -> Result<u32, Unsupported> {
    sig.try_bodies
        .get(&(func_index, begin_pc))
        .copied()
        .ok_or(Unsupported::TryRegion {
            pc: begin_pc,
            reason: "the body was not outlined",
        })
}

/// Can a value of this type travel through the trampoline's argument buffer?
///
/// The buffer is machine words, so the test is "does one word hold it": an
/// integer, and a container handle, which is a pointer. `F64` cannot — the ABI
/// passes it in XMM while the trampoline passes integers — and neither can the
/// two-register carriers (`Dyn`, the `Maybe`s), which have no single word to be.
fn crosses_as_word(ty: Ty) -> bool {
    matches!(
        ty,
        Ty::I64
            // A `Bool` is 0/1 and an `F64` is eight bytes — both are machine
            // words. Leaving `Bool` out is what made
            // `fn probe(c: Bool) { let r = try { … } catch e { … }; }` reject
            // while the same function with an `Int` parameter lowered.
            //
            // `F64` needs one more step, because the trampoline's signature is
            // all `long long`: the body declares the parameter `I64` and reads
            // the float back out of those bits (`Inst::BitsToFloat`). Declaring
            // it `F64` instead made Cranelift read a *float* register — that
            // compiled and segfaulted.
            | Ty::Bool
            | Ty::F64
            | Ty::Str
            | Ty::ListI64
            | Ty::ListF64
            | Ty::ListStr
            | Ty::ListDyn
            | Ty::MapStrI64
            | Ty::MapI64I64
            | Ty::MapStrF64
            | Ty::MapI64F64
            | Ty::MapStrBool
            | Ty::MapStrDyn
            | Ty::Set
            | Ty::Bytes
            // A window is a handle like the rest — it was the one carrier
            // missing from this list, so a `try` that so much as *mentioned* a
            // `xs.slice(a, b)` dropped the whole program to the VM while the
            // same body over the list itself lowered.
            | Ty::SliceI64
    )
}

/// Writes every tracked register whose definition changed into the cell its
/// caller allocated for it.
///
/// `before` is what those registers held at the last mirror, in `cell_handles`
/// order. Asking the SSA what changed — rather than reading an opcode's `a`
/// field — is what makes this correct for *any* producer of a definition: an
/// ordinary instruction, and equally a nested region's write-back, which
/// produces its definitions in the exit handling where no opcode is in sight.
#[allow(clippy::too_many_arguments)]
fn mirror_cells(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    sig: &SigInfer,
    func_index: u32,
    cell_handles: &[(u8, ValueId)],
    before: &[Option<Reg>],
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    for (index, (reg, handle)) in cell_handles.iter().enumerate() {
        let now = ssa.current_def[block][*reg as usize];
        if now == before.get(index).copied().flatten() {
            continue;
        }
        let Some((value, ty)) = now else { continue };
        // Written immediately, not at the end of the body: a raise can happen
        // in the next call, and the VM shows whatever was assigned before it.
        // Storing only on the way out would lose exactly the writes a handler
        // is most likely to look at.
        // The cell's kind is the caller's, not this store's type: a raw handle
        // written into a value cell is a loud failure at the read
        // (`cell_get_raw` checks the tag), and a register the caller saw as
        // `nil` gets a *value* cell however containery the body's assignment
        // turns out to be.
        if cell_is_raw(ty) && sig.try_body_raw_cells.contains(&(func_index, *reg)) {
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "cell_set_raw"),
                args: vec![*handle, value],
            });
            continue;
        }
        // Into a value cell: boxing a typed *list* rebuilds it, so a mutation
        // made after this store would not travel — reject rather than answer
        // with a stale copy. Every other container boxes in place
        // (`DYN_SET`/`DYN_BYTES`/`DYN_SLICE`/the typed map tags), so it carries
        // whatever the body does to it.
        if matches!(ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr) {
            return Err(Unsupported::TryRegion {
                pc,
                reason: "a nil-seeded register is assigned a typed list, which cannot be boxed in place",
            });
        }
        let boxed = crate::dyn_box::to_dyn_any(ssa, insts, value, ty, pc)?;
        insts.push(Inst::Call {
            dst: None,
            callee: AbiRef::new("rt", "cell_set"),
            args: vec![*handle, boxed],
        });
    }
    Ok(())
}

/// Whether this type occupies **two** machine registers.
///
/// `Dyn` is `{tag, payload}` and each `Maybe` is `{value, present}`. Everything
/// else in the type set is one word or is not a value at all. A carrier crosses
/// the `try`-region boundary as two words rather than one
/// (`Inst::CarrierWord`), which is the only correct way to move it through a
/// buffer of `long long`s: the alternatives are to unwrap it — which aborts
/// when a `Maybe` is absent, where the body may only have asked
/// `x ?? default` — or to refuse, which is what it used to do.
///
/// Exhaustive on purpose. A type missing from the two-register side would be
/// split into halves that do not exist; one wrongly on it would cross as two
/// words the body then binds as one parameter too many.
fn crosses_as_two_words(ty: Ty) -> bool {
    match ty {
        Ty::Dyn | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => true,
        Ty::I64
        | Ty::F64
        | Ty::Bool
        | Ty::Str
        | Ty::Nil
        | Ty::Cell
        | Ty::ListDyn
        | Ty::ListI64
        | Ty::ListF64
        | Ty::ListStr
        | Ty::SliceI64
        | Ty::MapStrDyn
        | Ty::MapStrI64
        | Ty::MapI64I64
        | Ty::MapStrF64
        | Ty::MapI64F64
        | Ty::MapStrBool
        | Ty::Set
        | Ty::Bytes => false,
    }
}

/// How many machine words the `try`-region trampoline's arity switch covers
/// (`lkrt/src/try_trampoline.c`), mirrored from the codegen constant of the
/// same name — the lowering refuses a region past it so codegen never has to.
const LK_TRY_MAX_ARGS: usize = 8;

/// The closure identity a region input names, when it is one.
///
/// Asked *before* the register is read, because reading it is what fails: a
/// lambda is a compile-time `GlobalRef` with no SSA value behind it, and the
/// generic reader reports that as `ReferenceAsValue`. Recording the identity is
/// a fixpoint discovery like every other — the body has already been lowered
/// once with this register as a plain word, so the answer only takes effect on
/// the pass after it is written down.
fn lambda_region_input(ssa: &mut Ssa, sig: &mut SigInfer, body: u32, reg: u8, block: usize) -> Option<LambdaIdentity> {
    let identity = match ssa.builtin_ref_at(reg, block)? {
        GlobalRef::Lambda(fidx) => LambdaIdentity { fidx, captures: 0 },
        GlobalRef::Closure(fidx, caps) => LambdaIdentity {
            fidx,
            captures: caps.len() as u16,
        },
        // A module or a builtin is re-derived inside the body by its own
        // `GetGlobal`; a cell is an input, but a different kind of one
        // ([`cell_region_input`]).
        _ => return None,
    };
    sig.try_body_lambdas.insert((body, reg), identity);
    Some(identity)
}

/// The upvalue cell a region input names, when it is one.
///
/// A variable some closure captures is a *cell* — the register holds a
/// `GlobalRef::Cell` and its content lives in a virtual slot — so a region that
/// so much as reads one had no word to marshal and rejected on the body's
/// `LoadCellVal`. That is not a rare shape: a function parameter mentioned by
/// any lambda in the function is one, which is why the generated corpus of
/// nested regions lowered two programs in a hundred and ninety-one.
///
/// It crosses as a **runtime** cell, the same object a closure's mutable
/// capture crosses as: the caller seeds one from the slot, the body names it as
/// a capture parameter (`inst::global` already reads and writes those through
/// `rt.cell_get`/`rt.cell_set`), and the caller reads the slot back afterwards.
/// So the body's writes are visible to the parent whether it returned or
/// raised, which is the property the region's register cells exist for.
fn cell_region_input(
    ssa: &mut Ssa,
    sig: &mut SigInfer,
    capture_params: &[(ValueId, Ty)],
    body: u32,
    reg: u8,
    block: usize,
) -> Option<CellInput> {
    let input = match ssa.builtin_ref_at(reg, block)? {
        GlobalRef::Cell(cid) => {
            // A cell holding a callable *reference* has no runtime content at
            // all; the lambda path is the one that carries those.
            if ssa.cell_refs.contains_key(&cid) {
                return None;
            }
            CellInput::Slot(cid)
        }
        // This function is itself a region's body (or a closure), so it holds
        // the cell as a pointer already: the region one frame in gets the same
        // pointer, and all three frames name one cell.
        GlobalRef::CellParam(k) => match capture_params.get(k) {
            Some(&(handle, Ty::Cell)) => CellInput::Handle(handle),
            _ => return None,
        },
        _ => return None,
    };
    sig.try_body_cell_inputs.insert((body, reg));
    Some(input)
}

/// Where the runtime cell a region input travels in comes from.
enum CellInput {
    /// A cell of the enclosing function: the caller makes the runtime cell from
    /// its virtual slot and reads the slot back after the region.
    Slot(u32),
    /// A cell the enclosing function already holds a pointer to, because it is
    /// itself a body or a closure. Passed on as is — there is one cell, so
    /// there is nothing to resync.
    Handle(ValueId),
}

/// Everything [`try_lambda_env`] needs about the one input it is marshaling.
struct LambdaEnvSite<'a> {
    cap_ctx: CaptureCtx<'a>,
    body: u32,
    reg: u8,
    identity: LambdaIdentity,
    /// The runtime cell each cell input of this region travels in, and what it
    /// is agreed to hold, by cell id.
    ///
    /// A capture of the crossing closure that names one of them must get *that*
    /// cell rather than a snapshot of its content: the body writes through it,
    /// and the closure is called after those writes. `try { a = a * 2; a =
    /// clo(); }`, with `clo` capturing `a`, read the value `a` had entering the
    /// region and answered `3` where the VM answered `6`.
    region_cells: &'a std::collections::HashMap<u32, (ValueId, Ty)>,
    /// The lambda's own parameter count, which is where its capture slots begin
    /// ([`SigInfer::require_cell_capture`] keys on it).
    callee_param_count: usize,
}

/// Appends a lambda region input's environment to the region call's arguments.
///
/// The identity itself is not passed: the body seeds the register with the ref
/// (see the `try_params` binding), exactly the way an erased lambda argument
/// reaches an ordinary call. What crosses is one machine word per capture, in
/// capture order, so both sides walk `try_body_params` and agree on the layout
/// without either of them writing it down.
fn try_lambda_env(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    sig: &mut SigInfer,
    site: LambdaEnvSite<'_>,
    call_args: &mut Vec<ValueId>,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let LambdaEnvSite {
        cap_ctx,
        body,
        reg,
        identity,
        region_cells,
        callee_param_count,
    } = site;
    if identity.captures == 0 {
        return Ok(());
    }
    let Some(GlobalRef::Closure(_, caps)) = ssa.builtin_ref_at(reg, block) else {
        return Err(Unsupported::TryRegion {
            pc,
            reason: "a closure region input stopped resolving to the closure it was recorded as",
        });
    };
    let resolver = CaptureSite::new(cap_ctx, identity.fidx, CaptureMode::Share, block, pc);
    for (k, capture) in caps.iter().enumerate() {
        let (v, ty) = match resolver.resolve(ssa, insts, sig, capture, k)? {
            Some(resolved) => resolved,
            None => {
                let ClosureCapture::Cell(cid) = capture else {
                    unreachable!("only `Cell` is left to the call site")
                };
                // The body carries this very variable in a runtime cell, and it
                // is called *after* the body has written through it: the closure
                // gets the cell, not a copy of what it held on the way in. That
                // means the callee's capture has to be a cell at every call
                // site, which is a demand the fixpoint already knows how to
                // propagate.
                if let Some(&(handle, content)) = region_cells.get(cid) {
                    // Written down *before* any retry is asked for. The pass
                    // that asks does not reach the record at the bottom of this
                    // loop, so the next pass's body would bind this env word as
                    // the by-value type it had before — and calling the closure
                    // with it re-observes the parameter, joins it away from
                    // `Cell`, and the pin is re-requested forever. That is a
                    // fixpoint that never converges, reported as a rejection at
                    // the region's first pc.
                    sig.try_body_lambda_env_tys.insert((body, reg, k as u8), Ty::Cell);
                    // The closure reads through the same agreement the body
                    // does — one cell, one opinion about what is in it.
                    let joined = join_cell_content(sig.cell_capture_tys.get(&(identity.fidx, k)).copied(), content);
                    let mut retry = sig.require_cell_capture(identity.fidx as usize, callee_param_count, k);
                    retry |= sig.cell_capture_tys.insert((identity.fidx, k), joined) != Some(joined);
                    if retry {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    (handle, Ty::Cell)
                } else {
                    // Nothing in the region touches it, so its content at entry
                    // is its content throughout — a snapshot is exact.
                    //
                    // A capture the closure *writes* is a different matter: the
                    // cell would have to travel and be read back, and there is
                    // no region cell to hang that on.
                    if sig.cell_captures.contains(&(identity.fidx, k)) {
                        return Err(Unsupported::TryRegion {
                            pc,
                            reason: "a closure crossing into the region assigns what it captured",
                        });
                    }
                    ssa.read_slot(ssa.cell_slot(*cid), block, pc)?
                }
            }
        };
        // A cell is a pointer, which is a word; `crosses_as_word` is about
        // *values*, and answers no for it.
        if ty != Ty::Cell && !crosses_as_word(ty) {
            return Err(Unsupported::TryRegion {
                pc,
                reason: "a closure crossing into the region captures a value wider than a machine word",
            });
        }
        sig.try_body_lambda_env_tys.insert((body, reg, k as u8), ty);
        // Stored into the trampoline's word buffer as-is, exactly as an
        // ordinary input is: an `F64` goes in by its eight bytes and the body
        // reads it back out of them (`Inst::BitsToFloat`).
        call_args.push(v);
    }
    Ok(())
}

/// Lowers a single function to a [`MirFunction`]. User (non-entry) functions use
/// the `(i64, ...) -> i64` ABI in this slice: params and return are `I64`, verified
/// via typed reads / a return-type check — a mismatch rejects (falls back) rather
/// than miscompiles.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_function(
    func: &FunctionData,
    funcs: &[FunctionData],
    func_index: u32,
    entry: u32,
    is_entry: bool,
    globals: &mut Vec<String>,
    module_globals: &[String],
    sig: &mut SigInfer,
) -> Result<MirFunction, Unsupported> {
    if is_entry && func.capture_count != 0 {
        return Err(Unsupported::EntryHasCaptures(func.capture_count));
    }
    if is_entry && func.param_count != 0 {
        return Err(Unsupported::EntryHasParams(func.param_count));
    }
    let param_count = func.param_count as usize;
    let capture_count = func.capture_count as usize;

    let code_len = func.code.len();
    let instrs = func
        .code
        .iter()
        .enumerate()
        .map(|(pc, raw)| Instr::try_from_raw(*raw).map_err(|_| Unsupported::BadInstr { pc }))
        .collect::<Result<Vec<_>, _>>()?;

    // 0. Protected regions. Each body was outlined into a function of its own
    //    before this ran (see `lower_module`), so the parent must not see those
    //    instructions as control flow at all: they are marked consumed, and the
    //    `TryBegin` becomes one exit with two successors.
    let regions = crate::try_region::scan(func, &instrs)?;

    // 1. Classify control-flow exits; a fused `TestXxx`+`Jmp` consumes the `Jmp`.
    let mut consumed = vec![false; code_len];
    for region in &regions {
        for flag in consumed.iter_mut().take(region.body_end + 1).skip(region.body_start) {
            *flag = true;
        }
        // The `Jmp` over the handler belongs to the region, not to the body.
        if region.body_end + 1 < code_len && instrs[region.body_end + 1].opcode() == Opcode::Jmp {
            consumed[region.body_end + 1] = true;
        }
    }
    let mut exits: Vec<Option<Exit>> = (0..code_len)
        .map(|pc| exit_of(pc, &instrs, code_len, &mut consumed, &func.performance))
        .collect::<Result<Vec<_>, _>>()?;
    for region in &regions {
        // A body that writes a register the enclosing function already defined
        // would, outlined, write it in the *body's* frame and leave the
        // parent's copy untouched. The program then computes a different
        // answer with nothing said — the one outcome worse than not compiling.
        //
        // Registers the parent has no definition for are safe: the body owns
        // them, and a later read of one is undefined, which rejects on its own.
        // Carrying a write back out needs the value to live in memory rather
        // than a register, which is the next piece of work.
        // Registers the body assigns that the enclosing function already had:
        // they travel through cells, because a write in the body's own frame is
        // invisible here otherwise — and on the raise path the body never
        // returns to hand anything back, while the VM still shows what it wrote
        // before raising.
        // No cell is created on the strength of "the parent wrote this
        // register before the region". That was an over-approximation of the
        // question that matters — *does anything read it after* — and it paid
        // for the approximation twice: a dead call-window temporary the body
        // happened to reuse got a cell, and its value at the region had no type
        // that could come back out, so the whole region rejected.
        //
        // Instead every register the body writes and does not carry back is
        // poisoned at the region's exit, and a later read of one fails naming
        // itself. That error is what the fixpoint already turns into a cell.
        // So the set below starts empty and is filled by being asked.
        let mut cells: Vec<u8> = Vec::new();
        // Registers a later read proved the body had to write back: they are
        // not visible to the scan above, because nothing in this function
        // defines them — the body does.
        let body_index = body_index_of(sig, func_index, region.begin_pc)?;
        // What the body rebound, as the body itself reported. Until it has been
        // lowered once there is no report, and the syntactic scan stands in —
        // conservative, and replaced on the next pass.
        let body_writes: Vec<u8> = match sig.try_body_rebound.get(&body_index) {
            Some(set) => {
                let mut v: Vec<u8> = set.iter().copied().collect();
                v.sort_unstable();
                v
            }
            None => crate::try_region::written_registers(&instrs, region.body_start, region.body_end),
        };
        if let Some(extra) = sig.try_body_extra_cells.get(&body_index) {
            for &reg in extra {
                if reg != region.catch_reg && !cells.contains(&reg) && body_writes.contains(&reg) {
                    cells.push(reg);
                }
            }
        }
        // A cell of *this* function, when this function is itself a region's
        // body: somebody past this frame reads it, which is what having a cell
        // means, so a nested region that writes it has to carry it back even
        // though nothing here reads it afterwards.
        //
        // The read-after-the-region evidence the set above is built from cannot
        // see that reader — it is a frame away. `try { try { r = f(); } catch e
        // { r = -1; } } catch e2 { r = -2; }` is the whole shape: the outer
        // body's only statement is the inner `try`, so it never reads `r`, and
        // the inner body's assignment was dropped with nothing said.
        for &reg in sig.try_body_cells.get(&func_index).unwrap_or(&Vec::new()) {
            if reg != region.catch_reg && !cells.contains(&reg) && body_writes.contains(&reg) {
                cells.push(reg);
            }
        }
        cells.sort_unstable();
        // The trampoline passes machine words and its arity switch caps them at
        // `LK_TRY_MAX_ARGS`; past that its `default` arm traps, so the count has
        // to be exact rather than indicative. Everything that becomes an
        // argument is counted: each input — a lambda one contributing a word per
        // capture and nothing for its identity — each cell, and the two-cell
        // return channel.
        //
        // It used to count inputs and cells only. A body with seven inputs that
        // also returned built a nine-argument call, and the program compiled,
        // linked, and died on `SIGILL` the first time the region ran.
        let inputs: usize = sig
            .try_body_params
            .get(&body_index)
            .map(|params| {
                params
                    .iter()
                    .map(|reg| match sig.try_body_lambdas.get(&(body_index, *reg)) {
                        Some(identity) => identity.captures as usize,
                        // A carrier crosses as two words.
                        None => match sig.try_body_param_tys.get(&(body_index, *reg)) {
                            Some(ty) if crosses_as_two_words(*ty) => 2,
                            _ => 1,
                        },
                    })
                    .sum()
            })
            .unwrap_or(0);
        let channel = if sig.try_body_returns.contains(&body_index) {
            2
        } else {
            0
        };
        if inputs + cells.len() + channel > LK_TRY_MAX_ARGS {
            return Err(Unsupported::TryRegion {
                pc: region.begin_pc,
                reason: "too many values cross the region boundary",
            });
        }
        sig.try_body_cells.insert(body_index, cells);
        let body = sig
            .try_bodies
            .get(&(func_index, region.begin_pc))
            .copied()
            .ok_or(Unsupported::TryRegion {
                pc: region.begin_pc,
                reason: "the body was not outlined",
            })?;
        exits[region.begin_pc] = Some(Exit::TryRegion {
            body,
            catch_reg: region.catch_reg,
            handler: region.handler,
            fallthrough: region.fallthrough,
        });
    }

    // 2. Block leaders.
    let mut leaders = std::collections::BTreeSet::new();
    leaders.insert(0usize);
    let mut implicit_ret = false;
    for (pc, exit) in exits.iter().enumerate() {
        match exit {
            None => {}
            Some(Exit::Ret(_)) => {
                if pc + 1 < code_len {
                    leaders.insert(pc + 1);
                }
            }
            Some(Exit::Jump(t)) => {
                mark_target(*t, code_len, &mut leaders, &mut implicit_ret);
                if pc + 1 < code_len {
                    leaders.insert(pc + 1);
                }
            }
            Some(Exit::Cond { then_pc, else_pc, .. }) => {
                mark_target(*then_pc, code_len, &mut leaders, &mut implicit_ret);
                mark_target(*else_pc, code_len, &mut leaders, &mut implicit_ret);
                if pc + 1 < code_len {
                    leaders.insert(pc + 1);
                }
            }
            Some(Exit::FusedCmp { taken, fallthrough, .. })
            | Some(Exit::FusedCmp2 { taken, fallthrough, .. })
            | Some(Exit::ForLoop { taken, fallthrough, .. })
            | Some(Exit::FusedModZero { taken, fallthrough, .. })
            | Some(Exit::NilBranch { taken, fallthrough, .. }) => {
                mark_target(*taken, code_len, &mut leaders, &mut implicit_ret);
                mark_target(*fallthrough, code_len, &mut leaders, &mut implicit_ret);
            }
            Some(Exit::TryRegion {
                handler, fallthrough, ..
            }) => {
                mark_target(*handler, code_len, &mut leaders, &mut implicit_ret);
                mark_target(*fallthrough, code_len, &mut leaders, &mut implicit_ret);
            }
        }
    }

    // A function whose code simply runs out reaches the same one-past-end
    // target an explicit exit would name, so it needs the same block. Only the
    // last block can do this, and only when nothing in it is an exit.
    let last_leader = *leaders.iter().next_back().expect("block 0 is always a leader");
    if block_span(&exits, &consumed, last_leader, code_len).1.is_none() {
        implicit_ret = true;
    }

    // 3. Block ids (+ optional synthetic implicit-nil-return block).
    let leader_vec: Vec<usize> = leaders.iter().copied().collect();
    let pc_to_block: BTreeMap<usize, u32> = leader_vec.iter().enumerate().map(|(i, &pc)| (pc, i as u32)).collect();
    let implicit_ret_block = if implicit_ret {
        Some(leader_vec.len() as u32)
    } else {
        None
    };
    let block_of = |pc: usize| -> usize {
        if pc >= code_len {
            implicit_ret_block.expect("marked when a one-past-end target exists") as usize
        } else {
            *pc_to_block.range(..=pc).next_back().map(|(_, id)| id).unwrap() as usize
        }
    };

    // 4. Predecessors per block (edges over the CFG).
    let total_blocks = leader_vec.len() + usize::from(implicit_ret);
    let reg_count = func.register_count as usize;
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); total_blocks];
    let block_bounds: Vec<(usize, usize)> = leader_vec
        .iter()
        .enumerate()
        .map(|(bi, &start)| (start, leader_vec.get(bi + 1).copied().unwrap_or(code_len)))
        .collect();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); total_blocks];
    for (bi, &(start, end)) in block_bounds.iter().enumerate() {
        let (_, exit) = block_span(&exits, &consumed, start, end);
        successors[bi] = exit_successors(exit, end).into_iter().map(block_of).collect();
    }

    // Blocks control can actually get to, from the entry. Code after a `return`
    // is not lowered and contributes no edges: with no predecessors of its own
    // it has no definition for any register, and `read_recursive`'s empty-preds
    // case answers "read before any definition" — which then propagated into
    // every block it fell into, so `if c { return 1; } else { return 2; }`
    // followed by one more line rejected the whole function over its own
    // parameter. Bytecode also arrives from `.lkm` files, so the backend cannot
    // rest on the compiler never emitting unreachable code.
    let mut reachable = vec![false; total_blocks];
    let mut worklist = vec![0usize];
    reachable[0] = true;
    while let Some(bi) = worklist.pop() {
        for &succ in &successors[bi] {
            if !reachable[succ] {
                reachable[succ] = true;
                worklist.push(succ);
            }
        }
    }
    for (bi, succs) in successors.iter().enumerate() {
        if !reachable[bi] {
            continue;
        }
        for &succ in succs {
            preds[succ].push(bi);
        }
    }

    // 5. Lower each block in leader order via Braun on-demand SSA construction.
    // One virtual cell slot per `LoadHeapConst UpvalCell` site (cell ids are
    // assigned in lowering order, so the site count bounds them).
    let cell_capacity = instrs
        .iter()
        .filter(|i| {
            i.opcode() == Opcode::LoadHeapConst
                && matches!(
                    func.consts.heap_values.get(i.bx() as usize),
                    Some(ConstHeapValueData::UpvalCell(_))
                )
        })
        .count();
    let mut ssa = Ssa::new(reg_count, cell_capacity, capture_count, preds, total_blocks);
    ssa.dyn_loop_slots = sig
        .dyn_loop_phis
        .iter()
        .filter(|&&(fi, _, _)| fi == func_index)
        .map(|&(_, b, s)| (b, s))
        .collect();
    ssa.dyn_literal_pcs = sig
        .dyn_literals
        .iter()
        .filter(|&&(fi, _)| fi == func_index)
        .map(|&(_, p)| p)
        .collect();
    // Function parameters occupy r0..r(param_count-1) at entry; each takes its
    // inferred type (the argument type observed at call sites, `I64` by default).
    // They seed the entry block's register file as its first SSA values.
    let identities: Vec<Option<LambdaIdentity>> =
        sig.lambda_params.get(func_index as usize).cloned().unwrap_or_default();
    let env_total: usize = identities.iter().flatten().map(|id| id.captures as usize).sum();
    let mut fn_params: Vec<(ValueId, Ty)> = Vec::with_capacity(param_count + env_total + capture_count);
    for r in 0..param_count {
        // An erased zero-capture lambda parameter has no runtime value: the
        // register holds the statically known function ref (indirect calls
        // devirtualize). Erased capturing identities bind below, after the
        // visible parameters, so signature order matches the call site.
        if let Some(id) = identities.get(r).copied().flatten() {
            if id.captures == 0 {
                ssa.bind_ref(0, r as u8, GlobalRef::Lambda(id.fidx));
            }
            continue;
        }
        let pty = sig.param_ty(func_index as usize, r);
        let pv = ssa.new_val();
        ssa.current_def[0][r] = Some((pv, pty));
        // `self` in `impl T { … }` *is* a `T`. Provenance otherwise comes only
        // from a `NewObject`, so inside an impl method the receiver had none
        // and `self.other()` fell out of the devirtualizing path — the whole
        // "a method built on the type's other methods" shape, which is most of
        // what methods are for, and the reason a trait default body could not
        // be lowered at all.
        //
        // An ordinary parameter gets it from the call sites instead
        // (`sig.param_structs`), which is the same carry `ret_structs` does for
        // a returned struct — passing one to a function is at least as common
        // as returning one, and without this `fn area(q: P) { return q.norm(); }`
        // dropped the module to the VM while `q.w * q.h` in the same position
        // lowered fine.
        if pty == Ty::MapStrDyn {
            let provenance = if r == 0 {
                sig.traits.impl_owner(func_index)
            } else {
                None
            }
            .or_else(|| sig.param_structs.get(&(func_index as usize, r)).cloned().flatten());
            if let Some(type_name) = provenance {
                ssa.struct_types.insert(pv, type_name);
            }
        }
        fn_params.push((pv, pty));
    }
    // A try body's inputs: registers of the *enclosing* function, bound here as
    // ordinary trailing parameters. They are all `I64` because the trampoline
    // passes machine words; a body that needs something wider rejects when it
    // reads it, which is the honest failure.
    // Whether this function *is* a region's body, which is what makes the
    // per-register snapshot below worth taking.
    let is_try_body = sig.try_bodies.values().any(|&b| b == func_index);
    let mut rebound: std::collections::HashSet<u8> = std::collections::HashSet::new();
    let try_params: Vec<u8> = sig.try_body_params.get(&func_index).cloned().unwrap_or_default();
    let mut try_param_bitcasts: Vec<(u8, ValueId)> = Vec::new();
    // The same read-back for a lambda input's `F64` *capture*, which has no
    // register of its own: the closure ref already names the float value, so
    // only the bitcast producing it is outstanding.
    let mut entry_bitcasts: Vec<(ValueId, ValueId)> = Vec::new();
    // Carriers to reassemble at entry, from the two words they crossed as.
    let mut entry_carriers: Vec<(ValueId, ValueId, ValueId, Ty)> = Vec::new();
    // Upvalue-cell inputs, bound here in `try_params` order (so the caller's
    // argument layout is matched) and wired into `capture_params` below, once
    // that exists — `inst::global` reads and writes a `CellParam` backed by a
    // runtime cell through `rt.cell_get`/`rt.cell_set`, which is exactly the
    // sharing this input needs.
    let mut cell_input_params: Vec<(u8, ValueId)> = Vec::new();
    for &reg in &try_params {
        if sig.try_body_cell_inputs.contains(&(func_index, reg)) {
            let pv = ssa.new_val();
            fn_params.push((pv, Ty::Cell));
            cell_input_params.push((reg, pv));
            continue;
        }
        // A lambda input: its identity is a compile-time fact the caller wrote
        // down, so the register is seeded with the reference and only the
        // environment is bound — one parameter per capture, in capture order,
        // which is the order the caller pushed them.
        if let Some(identity) = sig.try_body_lambdas.get(&(func_index, reg)).copied() {
            let mut caps = Vec::with_capacity(identity.captures as usize);
            for k in 0..identity.captures {
                let ety = sig
                    .try_body_lambda_env_tys
                    .get(&(func_index, reg, k as u8))
                    .copied()
                    .unwrap_or(Ty::I64);
                let ev = ssa.new_val();
                if ety == Ty::F64 {
                    fn_params.push((ev, Ty::I64));
                    let f = ssa.new_val();
                    entry_bitcasts.push((f, ev));
                    caps.push(ClosureCapture::Value(f, Ty::F64));
                } else {
                    fn_params.push((ev, ety));
                    caps.push(ClosureCapture::Value(ev, ety));
                }
            }
            let global_ref = if caps.is_empty() {
                GlobalRef::Lambda(identity.fidx)
            } else {
                GlobalRef::Closure(identity.fidx, caps)
            };
            ssa.bind_ref(0, reg, global_ref);
            continue;
        }
        let ty = sig
            .try_body_param_tys
            .get(&(func_index, reg))
            .copied()
            .unwrap_or(Ty::I64);
        // The two words a carrier crossed as, fused back into one.
        if crosses_as_two_words(ty) {
            let lo = ssa.new_val();
            fn_params.push((lo, Ty::I64));
            let hi = ssa.new_val();
            fn_params.push((hi, Ty::I64));
            let carrier = ssa.new_val();
            entry_carriers.push((carrier, lo, hi, ty));
            ssa.current_def[0][reg as usize] = Some((carrier, ty));
            continue;
        }
        let pv = ssa.new_val();
        // An `F64` input is declared `I64` and read back out of those bits at
        // entry: the trampoline calls this body through a `(long long, …)`
        // signature (`lkrt/src/try_trampoline.c`), so every input arrives in an
        // integer register. Declaring the parameter `F64` made Cranelift read a
        // *float* register instead — it compiled and segfaulted.
        if ty == Ty::F64 {
            fn_params.push((pv, Ty::I64));
            try_param_bitcasts.push((reg, pv));
        } else {
            ssa.current_def[0][reg as usize] = Some((pv, ty));
            fn_params.push((pv, ty));
        }
    }
    // The cells this body writes through, in the same order the caller passes
    // them. They are handles, not values: the register keeps its own value in
    // SSA, and every change to it is *also* written to the cell, so the caller
    // sees it whether the body returned or raised.
    let try_cells: Vec<u8> = sig.try_body_cells.get(&func_index).cloned().unwrap_or_default();
    let mut cell_handles: Vec<(u8, ValueId)> = Vec::with_capacity(try_cells.len());
    for &reg in &try_cells {
        let pv = ssa.new_val();
        fn_params.push((pv, Ty::Cell));
        cell_handles.push((reg, pv));
    }

    // A body that `return`s from the enclosing function takes two more cells:
    // a flag saying it did, and the value. They come last, so nothing else
    // shifts. (`SigInfer::try_body_returns`.)
    let return_channel = sig.try_body_returns.contains(&func_index).then(|| {
        let flag = ssa.new_val();
        fn_params.push((flag, Ty::Cell));
        let value = ssa.new_val();
        fn_params.push((value, Ty::Cell));
        (flag, value)
    });

    // An erased *capturing* closure argument: its environment (resolved at
    // the call site) arrives as hidden trailing parameters, one block per
    // erased parameter in parameter order. The register holds a Closure ref
    // whose captures alias those parameters by value.
    let mut env_offset = 0usize;
    for r in 0..param_count {
        let Some(id) = identities.get(r).copied().flatten() else {
            continue;
        };
        if id.captures == 0 {
            continue;
        }
        let mut caps = Vec::with_capacity(id.captures as usize);
        for _ in 0..id.captures {
            let ety = sig.param_ty(func_index as usize, param_count + env_offset);
            let ev = ssa.new_val();
            fn_params.push((ev, ety));
            caps.push(ClosureCapture::Value(ev, ety));
            env_offset += 1;
        }
        ssa.bind_ref(0, r as u8, GlobalRef::Closure(id.fidx, caps));
    }
    // A capturing lambda's own environment arrives after any erased-argument
    // env blocks (the closure's by-value snapshot, appended by the `Call`
    // lowering); it occupies no register — `LoadCapture k` reads it directly.
    let spawned_isolate = sig.spawned_isolate.contains(&func_index);
    ssa.spawned_isolate = spawned_isolate;
    // An environment that is entirely static references carries nothing at
    // runtime, so it is not declared at all (`SigInfer::captures_all_static`).
    let erased_environment = sig.captures_all_static(func_index as usize, capture_count);
    let mut capture_params: Vec<(ValueId, Ty)> = Vec::with_capacity(capture_count);
    for k in 0..capture_count {
        let cty = sig.param_ty(func_index as usize, param_count + env_total + k);
        let cv = ssa.new_val();
        capture_params.push((cv, cty));
        if !erased_environment {
            fn_params.push((cv, cty));
        }
        // What reads of this capture unbox to, when it arrived as a runtime
        // cell. The call site wrote it down (`SigInfer::cell_capture_tys`);
        // unset means `Dyn`, which is what a cell answered everywhere before.
        if cty == Ty::Cell {
            ssa.cellparam_content.insert(
                k,
                sig.cell_capture_tys.get(&(func_index, k)).copied().unwrap_or(Ty::Dyn),
            );
        }
        // A spawned goroutine's cell captures are thread-private copies:
        // seed the virtual slot so body writes (isolate — never visible to
        // the spawner) go through plain SSA.
        if spawned_isolate {
            let slot = ssa.cellparam_slot(k);
            ssa.write_slot(slot, 0, (cv, cty));
        }
    }
    // The upvalue-cell inputs join the environment: a try body has no captures
    // of its own (`capture_count` is 0), so these are all of it, and naming them
    // as capture parameters is what lets `LoadCellVal`/`StoreCellVal` reach them
    // through the arm that already knows how to read and write a runtime cell.
    for (reg, pv) in cell_input_params {
        let k = capture_params.len();
        ssa.bind_ref(0, reg, GlobalRef::CellParam(k));
        capture_params.push((pv, Ty::Cell));
        // What reads of this cell unbox to, and what a store into it must
        // agree with. Unset (a closure's own capture) means `Dyn`, which is
        // what a cell answered everywhere before this.
        ssa.set_cellparam_content_ty(
            k,
            reg,
            sig.try_body_cell_input_tys
                .get(&(func_index, reg))
                .copied()
                .unwrap_or(Ty::Dyn),
        );
    }
    let mut block_insts: Vec<Vec<Inst>> = vec![Vec::new(); total_blocks];
    let mut block_exit: Vec<Option<Exit>> = vec![None; total_blocks];
    let mut ret_ty: Option<Ty> = None;
    // Resolved terminator value reads (filled during each block's lowering).
    // Regions whose body may `return` from this function: the ok edge gets a
    // check block, and a `return` block behind it. Collected here and emitted
    // after the block loop, where this function's own return type is known.
    let mut try_return_checks: Vec<(usize, ValueId, ValueId, bool)> = Vec::new();
    let mut ret_val: Vec<Option<ValueId>> = vec![None; total_blocks];
    let mut cond_val: Vec<Option<ValueId>> = vec![None; total_blocks];

    for (bi, &(start, end)) in block_bounds.iter().enumerate() {
        ssa.seal_ready()?;
        if !reachable[bi] {
            // Filled and left empty: it keeps its block id (successors are
            // addressed by it) and gets a terminator in step 6.
            ssa.mark_filled(bi);
            continue;
        }
        let (body_end, exit) = block_span(&exits, &consumed, start, end);
        if exit.is_none() {
            ssa.single_fallthrough_target[bi] = Some(end);
        }
        let mut insts = Vec::new();
        // A float input arrives as bits (see the parameter binding above): read
        // it back as a float before the body's first instruction.
        if bi == 0 {
            for &(reg, bits) in &try_param_bitcasts {
                let f = ssa.new_val();
                insts.push(Inst::BitsToFloat { dst: f, src: bits });
                ssa.current_def[0][reg as usize] = Some((f, Ty::F64));
            }
            for &(dst, bits) in &entry_bitcasts {
                insts.push(Inst::BitsToFloat { dst, src: bits });
            }
            for &(dst, lo, hi, ty) in &entry_carriers {
                insts.push(Inst::CarrierFromParts { dst, lo, hi, ty });
            }
        }
        // The entry describes every declared struct to the runtime before any
        // user code runs: its type id, name, and field names in declaration
        // order. `display` needs them where the *mark* is — at runtime — because
        // a field holding another struct is a bare map by then and the display
        // site cannot tell (see `docs/aot/aot-gaps-and-lkrt.md`).
        if is_entry && bi == 0 {
            for (tid, name, fields) in sig.traits.struct_fields.clone() {
                let id = ssa.new_val();
                insts.push(Inst::Const {
                    dst: id,
                    value: Const::I64(tid),
                });
                let name_v = const_str_value(&mut ssa, &mut insts, globals, &name);
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("obj_ty", "begin"),
                    args: vec![id, name_v],
                });
                for field in &fields {
                    let field_v = const_str_value(&mut ssa, &mut insts, globals, field);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("obj_ty", "field"),
                        args: vec![id, field_v],
                    });
                }
            }
        }
        #[allow(clippy::needless_range_loop)] // `pc` is the semantic bytecode index
        for pc in start..body_end {
            // What the tracked registers held before this instruction, so a
            // change can be noticed afterwards. Asking the SSA what changed is
            // the same device the inputs use: no table of which opcode writes
            // where, and therefore no entry in such a table to get wrong.
            let before: Vec<Option<Reg>> = cell_handles
                .iter()
                .map(|(reg, _)| ssa.current_def[bi][*reg as usize])
                .collect();
            // And the same question asked of *every* register, which is what
            // tells the parent which of them this body rebound. A mutation
            // through a shared handle changes no `current_def` and so does not
            // appear here — which is the whole difference the `a` field could
            // not express.
            let before_all: Vec<Option<Reg>> = (0..ssa.reg_count).map(|r| ssa.current_def[bi][r]).collect();
            lower_inst(
                &mut LowerCtx {
                    ssa: &mut ssa,
                    globals,
                    sig,
                    func,
                    func_index,
                    funcs,
                    entry,
                    module_globals,
                    capture_params: &capture_params,
                },
                bi,
                &mut insts,
                &instrs[pc],
                pc,
            )?;
            if is_try_body {
                for r in 0..ssa.reg_count {
                    if ssa.current_def[bi][r] != before_all[r] {
                        rebound.insert(r as u8);
                    }
                }
            }
            mirror_cells(&mut ssa, &mut insts, sig, func_index, &cell_handles, &before, bi, pc)?;
        }
        // The terminator can produce definitions too, and one of them is a
        // *nested* region's write-back: an inner `try` hands its cells' values
        // back here, in the exit handling, where the per-instruction mirror
        // above has already run. Snapshotting across the terminator and
        // mirroring after it is what carries an inner region's writes out
        // through this body's own cells — without it, a `try` inside a `try`
        // compiled to a program that dropped the inner body's assignments and
        // said nothing.
        let before_exit: Vec<Option<Reg>> = cell_handles
            .iter()
            .map(|(reg, _)| ssa.current_def[bi][*reg as usize])
            .collect();
        let before_all_exit: Vec<Option<Reg>> = (0..ssa.reg_count).map(|r| ssa.current_def[bi][r]).collect();
        // Resolve the terminator's value reads while this block is current.
        match exit {
            Some(Exit::Ret(Some(reg))) => {
                // A return of a closure ref has no SSA value. When it is the
                // function's only return, the body is effect-free, and every
                // capture resolves to a parameter, record a summary — call
                // sites construct the closure from their argument values and
                // this body is never emitted. Everything else rejects below.
                if !is_entry && let Some(candidate) = ret_closure_candidate(&mut ssa, reg, bi, &fn_params, param_count)
                {
                    let single_ret =
                        !implicit_ret && exits.iter().flatten().filter(|e| matches!(e, Exit::Ret(_))).count() == 1;
                    if single_ret
                        && capture_count == 0
                        && identities.iter().all(Option::is_none)
                        && ret_closure_body_is_pure(&instrs)
                    {
                        record_ret_closure(sig, func_index as usize, candidate);
                    }
                    return Err(Unsupported::Opcode {
                        pc: start,
                        op: Opcode::Return1,
                    });
                }
                let (v, ty) = ssa.read(reg, bi, start)?;
                // A try body's `return` is the enclosing function's, not this
                // one's: set the flag, park the value, and return normally so
                // the trampoline reports "did not raise". The caller checks the
                // flag on the ok edge.
                let parked = if let Some((flag, slot)) = return_channel {
                    let boxed = to_dyn_any(&mut ssa, &mut insts, v, ty, start)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("rt", "cell_set"),
                        args: vec![slot, boxed],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    let marked = to_dyn_any(&mut ssa, &mut insts, one, Ty::I64, start)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("rt", "cell_set"),
                        args: vec![flag, marked],
                    });
                    ret_val[bi] = None;
                    ret_ty = Some(Ty::Nil);
                    true
                } else {
                    false
                };
                // Everything below is about *this* function's return value, and
                // a parked one is not that. Guarded rather than `continue`d:
                // the loop's tail is what stores this block's instructions.
                if !parked {
                    // The struct this return constructs, carried out to callers so
                    // a method on the result devirtualizes (`sig.ret_structs`).
                    // Joined across return points: two different structs, or one
                    // return that is not a struct, answer "unknown" rather than a
                    // name that is right only sometimes.
                    if !is_entry {
                        let returned = ssa.struct_types.get(&v).cloned();
                        match sig.ret_structs.entry(func_index) {
                            std::collections::hash_map::Entry::Vacant(slot) => {
                                slot.insert(returned);
                            }
                            std::collections::hash_map::Entry::Occupied(mut slot) => {
                                if *slot.get() != returned {
                                    slot.insert(None);
                                }
                            }
                        }
                    }
                    // A function discovered to mix return types boxes every
                    // return point: it returns `Dyn`, callers consume through
                    // the Dyn arms (plan M4.2 cross-function Dyn flow).
                    let force_dyn = !is_entry && sig.dyn_rets.contains(&func_index);
                    let (v, ty) = if force_dyn && ty != Ty::Dyn {
                        (to_dyn_any(&mut ssa, &mut insts, v, ty, start)?, Ty::Dyn)
                    } else {
                        (v, ty)
                    };
                    match ret_ty {
                        Some(prev) if prev != ty => {
                            // Heterogeneous but boxable returns are retriable:
                            // record the function, the fixpoint re-lowers it with
                            // every return boxed (the snapshot includes the set's
                            // size). Everything else stays a real reject.
                            if !is_entry && dyn_boxable_ty(prev) && dyn_boxable_ty(ty) {
                                sig.dyn_rets.insert(func_index);
                            }
                            return Err(Unsupported::ReturnTypeConflict);
                        }
                        _ => {
                            // Eagerly publish the first concrete return type so a
                            // self-recursive call later in this same body observes
                            // it instead of the stale `I64` default (a Bool-typed
                            // `return f(xs.skip(1))` chain would otherwise look
                            // heterogeneous forever).
                            if ret_ty.is_none()
                                && !is_entry
                                && let Some(slot) = sig.ret_types.get_mut(func_index as usize)
                            {
                                *slot = ty;
                                if let Some(known) = sig.ret_known.get_mut(func_index as usize) {
                                    *known = true;
                                }
                            }
                            ret_ty = Some(ty);
                        }
                    }
                    // A `Nil` return value renders as `ret void`.
                    ret_val[bi] = if ty == Ty::Nil { None } else { Some(v) };
                }
            }
            Some(Exit::TryRegion {
                body,
                catch_reg,
                handler: region_handler,
                fallthrough: region_fallthrough,
            }) => {
                // Run the body under a handler, and bind what it raised.
                //
                // The caught value is written unconditionally, on both edges.
                // Writing it only on the raise edge would leave the register
                // undefined on the other one, and SSA has to agree about a
                // register's definition at a join whether or not the path that
                // defined it was taken.
                // The body's inputs, read here where the enclosing function's
                // values are still current. All `I64`: the trampoline passes
                // machine words, and a body wanting something wider rejects
                // when it reads it.
                // Two passes, because a closure crossing the boundary may
                // capture a variable that is *also* crossing as a cell: the
                // cells are made first so the lambda can be handed the same
                // object rather than a copy of what it held. Each input's words
                // are collected positionally and flattened afterwards, so the
                // argument order is still `try_body_params` order — which is
                // what the body walks.
                let region_params = sig.try_body_params.get(&body).cloned().unwrap_or_default();
                let mut input_words: Vec<Option<Vec<ValueId>>> = vec![None; region_params.len()];
                // Upvalue-cell inputs, resynced from their runtime cells after
                // the call: the body may have written through one, and the
                // parent's slot is the only place that write can land.
                let mut cell_input_values: Vec<(u32, ValueId, Ty)> = Vec::new();
                let mut region_cells: std::collections::HashMap<u32, (ValueId, Ty)> = std::collections::HashMap::new();
                for (index, &reg) in region_params.iter().enumerate() {
                    // A variable a closure captured: it lives in a slot behind a
                    // compile-time cell ref, so what crosses is a runtime cell
                    // seeded from that slot.
                    match cell_region_input(&mut ssa, sig, &capture_params, body, reg, bi) {
                        Some(CellInput::Slot(cid)) => {
                            let (cur, cur_ty) = ssa.read_slot(ssa.cell_slot(cid), bi, start)?;
                            // The content type the body reads through, unless a
                            // store inside it has already disagreed (which pins
                            // the entry to `Dyn` — see `try_body_cell_input_tys`).
                            let content =
                                join_cell_content(sig.try_body_cell_input_tys.get(&(body, reg)).copied(), cur_ty);
                            sig.try_body_cell_input_tys.insert((body, reg), content);
                            let boxed = crate::dyn_box::to_dyn_any(&mut ssa, &mut insts, cur, cur_ty, start)?;
                            let handle = ssa.new_val();
                            insts.push(Inst::Call {
                                dst: Some(handle),
                                callee: AbiRef::new("rt", "cell_new"),
                                args: vec![boxed],
                            });
                            input_words[index] = Some(vec![handle]);
                            region_cells.insert(cid, (handle, content));
                            cell_input_values.push((cid, handle, content));
                            continue;
                        }
                        Some(CellInput::Handle(handle)) => {
                            // The pointer is passed on unchanged, and so is what
                            // it is agreed to hold: this frame reads the same
                            // cell under the same type.
                            let content = ssa
                                .cellparam_content_ty(reg)
                                .filter(|_| sig.try_body_cell_input_tys.get(&(body, reg)) != Some(&Ty::Dyn))
                                .unwrap_or(Ty::Dyn);
                            sig.try_body_cell_input_tys.insert((body, reg), content);
                            input_words[index] = Some(vec![handle]);
                            if let Some(GlobalRef::Cell(cid)) = ssa.builtin_ref_at(reg, bi) {
                                region_cells.insert(cid, (handle, content));
                            }
                            continue;
                        }
                        None => {}
                    }
                    // Read as whatever it is, then decide whether it can cross.
                    // Forcing `I64` here is what used to reject a body that
                    // merely *looked at* a list the parent owned — a handle is a
                    // machine word, and the buffer the trampoline marshals into
                    // is machine words.
                    //
                    // A lambda is neither, and is left to the second pass.
                    if lambda_region_input(&mut ssa, sig, body, reg, bi).is_some() {
                        continue;
                    }
                    let (v, ty) = ssa.read(reg, bi, start)?;
                    // A two-register carrier crosses as its two raw words, put
                    // back together by the body (`Inst::CarrierFromParts`).
                    if crosses_as_two_words(ty) {
                        let mut word = |half| {
                            let dst = ssa.new_val();
                            insts.push(Inst::CarrierWord { dst, src: v, half });
                            dst
                        };
                        let lo = word(lk_aot_mir::CarrierHalf::Lo);
                        let hi = word(lk_aot_mir::CarrierHalf::Hi);
                        sig.try_body_param_tys.insert((body, reg), ty);
                        input_words[index] = Some(vec![lo, hi]);
                        continue;
                    }
                    if crosses_as_word(ty) {
                        sig.try_body_param_tys.insert((body, reg), ty);
                        input_words[index] = Some(vec![v]);
                    } else {
                        // Not a word and not a carrier this knows how to split.
                        // Named, rather than reported as "some operand": which
                        // *type* could not cross is the whole content of the
                        // answer, and it is what a reader needs to know whether
                        // to widen this rule or to change the program.
                        sig.try_body_param_tys.remove(&(body, reg));
                        return Err(Unsupported::OperandType {
                            pc: start,
                            want: "machine word",
                            got: lk_aot_mir::ty_name(ty),
                        });
                    }
                }
                for (index, &reg) in region_params.iter().enumerate() {
                    let Some(identity) = sig.try_body_lambdas.get(&(body, reg)).copied() else {
                        continue;
                    };
                    let mut env = Vec::new();
                    try_lambda_env(
                        &mut ssa,
                        &mut insts,
                        sig,
                        LambdaEnvSite {
                            cap_ctx: CaptureCtx {
                                params: &capture_params,
                                index: func_index,
                                param_count,
                            },
                            body,
                            reg,
                            identity,
                            region_cells: &region_cells,
                            callee_param_count: funcs.get(identity.fidx as usize).map_or(0, |f| f.param_count as usize),
                        },
                        &mut env,
                        bi,
                        start,
                    )?;
                    input_words[index] = Some(env);
                }
                let mut call_args: Vec<ValueId> = Vec::new();
                for words in input_words {
                    let Some(words) = words else {
                        return Err(Unsupported::TryRegion {
                            pc: start,
                            reason: "a region input resolved to nothing the trampoline can carry",
                        });
                    };
                    call_args.extend(words);
                }
                // One cell per register the body assigns that this function
                // already had. Seeded with the value it holds now, because a
                // body that raises before assigning must leave it alone.
                let cell_regs: Vec<u8> = sig.try_body_cells.get(&body).cloned().unwrap_or_default();
                let mut cell_values: Vec<(u8, ValueId, Ty)> = Vec::with_capacity(cell_regs.len());
                for &reg in &cell_regs {
                    let (v, ty) = ssa.read(reg, bi, start)?;
                    // A typed container is parked as a raw handle: no boxing, so
                    // the same handle comes back and the body's writes stand.
                    if cell_is_raw(ty) {
                        // The kind is decided *here*, and written down: the
                        // body must not decide it a second time from the type
                        // it happens to store (see `try_body_raw_cells`).
                        sig.try_body_raw_cells.insert((body, reg));
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("rt", "cell_new_raw"),
                            args: vec![v],
                        });
                        call_args.push(handle);
                        cell_values.push((reg, handle, ty));
                        continue;
                    }
                    sig.try_body_raw_cells.remove(&(body, reg));
                    // A value crosses back only if it can be taken out of a
                    // cell again. Boxing is universal; unboxing is per type,
                    // and a type with no unboxer is a rejection rather than a
                    // guess.
                    if unbox_from_dyn(ty).is_none() {
                        return Err(Unsupported::TryRegion {
                            pc: start,
                            reason: "the body assigns a value that cannot be read back out of a cell",
                        });
                    }
                    let boxed = crate::dyn_box::to_dyn_any(&mut ssa, &mut insts, v, ty, start)?;
                    let handle = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(handle),
                        callee: AbiRef::new("rt", "cell_new"),
                        args: vec![boxed],
                    });
                    call_args.push(handle);
                    cell_values.push((reg, handle, ty));
                }
                // The return channel: a flag cell (seeded false) and a value
                // cell (seeded nil). Only for a body that `return`s — every
                // other region passes exactly what it always did.
                let return_channel = sig.try_body_returns.contains(&body).then(|| {
                    let mut fresh_cell = |seed: Ty| -> Result<ValueId, Unsupported> {
                        let raw = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: raw,
                            value: Const::I64(0),
                        });
                        let boxed = crate::dyn_box::to_dyn_any(&mut ssa, &mut insts, raw, seed, start)?;
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("rt", "cell_new"),
                            args: vec![boxed],
                        });
                        Ok(handle)
                    };
                    let flag = fresh_cell(Ty::I64)?;
                    let value = fresh_cell(Ty::Nil)?;
                    call_args.push(flag);
                    call_args.push(value);
                    Ok::<_, Unsupported>((flag, value))
                });
                let return_channel = match return_channel {
                    Some(result) => Some(result?),
                    None => None,
                };
                if let Some((flag, value)) = return_channel {
                    // A body every path of which returns has no jump over the
                    // handler, so the region's "fallthrough" *is* the handler —
                    // there is no ok edge to fall to, and the check's else
                    // branch is unreachable. It goes to the return block too,
                    // rather than into the handler on a path that did not raise.
                    let always_returns = region_handler == region_fallthrough;
                    try_return_checks.push((bi, flag, value, always_returns));
                }
                let ok = ssa.new_val();
                insts.push(Inst::TryRegionCall {
                    dst: ok,
                    func: FuncId(body),
                    args: call_args,
                });
                // The upvalue cells first, on the same principle.
                for (cid, handle, content) in cell_input_values {
                    let cur = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(cur),
                        callee: AbiRef::new("rt", "cell_get"),
                        args: vec![handle],
                    });
                    // Read back under the same type the body read through, so
                    // the parent's own later uses stay typed too.
                    let (value, ty) = match unbox_cell_value(&mut ssa, &mut insts, cur, content) {
                        Some(value) if content != Ty::Dyn => (value, content),
                        _ => (cur, Ty::Dyn),
                    };
                    ssa.write_slot(ssa.cell_slot(cid), bi, (value, ty));
                }
                // Read every cell back, before the branch, so both edges see
                // what the body managed to write — including a body that
                // raised half way through, which is what the VM shows.
                for (reg, handle, ty) in cell_values {
                    if cell_is_raw(ty) {
                        let raw = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(raw),
                            callee: AbiRef::new("rt", "cell_get_raw"),
                            args: vec![handle],
                        });
                        ssa.write(reg, bi, (raw, ty));
                        continue;
                    }
                    let got = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(got),
                        callee: AbiRef::new("rt", "cell_get"),
                        args: vec![handle],
                    });
                    // See `unbox_from_dyn`: a nil seed comes back as whatever
                    // the body boxed, which is a `Dyn`.
                    let ty = if ty == Ty::Nil { Ty::Dyn } else { ty };
                    let value = unbox_cell_value(&mut ssa, &mut insts, got, ty).expect("checked above");
                    ssa.write(reg, bi, (value, ty));
                }
                // Everything else the body wrote is gone: it was written in the
                // body's frame, and nothing carried it back. Saying so is what
                // makes a later read report itself instead of silently reading
                // the value the parent had before the region.
                //
                // After the write-backs, so a register that *was* carried back
                // keeps the definition it was just given.
                // The same set the cells were chosen from: registers the body
                // *rebound*. A container it merely mutated is not among them,
                // and must not be — poisoning it would make the next read
                // report itself, the fixpoint would give it a cell, and the
                // round trip a cell implies is what loses the mutation.
                let body_may_write: Vec<u8> = match sig.try_body_rebound.get(&body) {
                    Some(set) => set.iter().copied().collect(),
                    None => regions
                        .iter()
                        .find(|r| sig.try_bodies.get(&(func_index, r.begin_pc)) == Some(&body))
                        .map(|span| crate::try_region::written_registers(&instrs, span.body_start, span.body_end))
                        .unwrap_or_default(),
                };
                for reg in &body_may_write {
                    if *reg != catch_reg && !cell_regs.contains(reg) {
                        ssa.poison(*reg, bi, body);
                    }
                }
                // A nested region's writes are *this* body's writes too, and
                // this body has to report them to *its* caller whether or not
                // the nested region managed to perform them on *this* pass —
                // it cannot, until the nested region has its cells, which is a
                // pass later.
                //
                // From the nested body's own report, which is `current_def`-based
                // and therefore precise. The syntactic scan is not usable here:
                // it names the `a` field of every instruction, so a container
                // the region merely *mutates* (`ListPush a=receiver`) would be
                // reported as rebound, the fixpoint would give it a cell, and
                // the round trip a cell implies is what loses the mutation. Both
                // spellings were tried; that one stopped
                // `examples/syntax/closure.lk` lowering at all.
                //
                // Without the transitive report:
                //
                //     try { try { b = clo(); } catch c1 { } } catch c2 { }
                //     acc.push(b);
                //
                // printed `b` from before the region, natively, with nothing
                // said.
                if is_try_body && let Some(nested) = sig.try_body_rebound.get(&body) {
                    rebound.extend(nested.iter().copied());
                }
                let caught = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(caught),
                    callee: AbiRef::new("rt", "current_error"),
                    args: vec![],
                });
                ssa.write(catch_reg, bi, (caught, Ty::Dyn));
                // The flag is an `i64` (1/0) and the terminator wants a Bool.
                let flag = ssa.new_val();
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                insts.push(Inst::Cmp {
                    dst: flag,
                    op: CmpOp::Ne,
                    float: false,
                    lhs: ok,
                    rhs: zero,
                });
                cond_val[bi] = Some(flag);
            }
            Some(Exit::Cond { cond, .. }) => {
                // VM truthiness (`truthy_unchecked`): only nil and false are
                // falsy — every number (0 included), string, and container is
                // truthy. Typed conditions fold at compile time; a Dyn
                // condition tests tag/payload at runtime; a Maybe tests its
                // present bit (its payload is truthy except for MaybeBool).
                let (v, ty) = ssa.read(cond, bi, start)?;
                let v = match ty {
                    Ty::Bool => v,
                    Ty::Nil => {
                        let c = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: c,
                            value: Const::Bool(false),
                        });
                        c
                    }
                    Ty::I64
                    | Ty::F64
                    | Ty::Str
                    | Ty::ListI64
                    | Ty::SliceI64
                    | Ty::ListF64
                    | Ty::ListStr
                    | Ty::ListDyn
                    | Ty::MapStrI64
                    | Ty::MapI64I64
                    | Ty::MapStrF64
                    | Ty::MapI64F64
                    | Ty::MapStrBool
                    | Ty::MapStrDyn
                    | Ty::Set
                    | Ty::Bytes
                    | Ty::Cell => {
                        let c = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: c,
                            value: Const::Bool(true),
                        });
                        c
                    }
                    Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr => {
                        let present = ssa.new_val();
                        insts.push(Inst::MaybePresent {
                            dst: present,
                            src: v,
                            maybe_ty: ty,
                        });
                        present
                    }
                    Ty::MaybeBool => {
                        // Absent is nil (falsy); present carries the payload.
                        let present = ssa.new_val();
                        insts.push(Inst::MaybePresent {
                            dst: present,
                            src: v,
                            maybe_ty: ty,
                        });
                        let value = ssa.new_val();
                        insts.push(Inst::MaybeValue {
                            dst: value,
                            src: v,
                            maybe_ty: ty,
                        });
                        let zero = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: zero,
                            value: Const::I64(0),
                        });
                        let value_b = ssa.new_val();
                        insts.push(Inst::Cmp {
                            dst: value_b,
                            op: CmpOp::Ne,
                            float: false,
                            lhs: value,
                            rhs: zero,
                        });
                        let both = ssa.new_val();
                        insts.push(Inst::BoolAnd {
                            dst: both,
                            lhs: present,
                            rhs: value_b,
                        });
                        both
                    }
                    Ty::Dyn => {
                        let wide = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(wide),
                            callee: AbiRef::new("dyn", "truthy"),
                            args: vec![v],
                        });
                        let zero = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: zero,
                            value: Const::I64(0),
                        });
                        let cond_b = ssa.new_val();
                        insts.push(Inst::Cmp {
                            dst: cond_b,
                            op: CmpOp::Ne,
                            float: false,
                            lhs: wide,
                            rhs: zero,
                        });
                        cond_b
                    }
                };
                cond_val[bi] = Some(v);
            }
            Some(Exit::FusedCmp { reg_a, rhs, op, .. }) => {
                // Dispatch on the tested register's type (int vs float compare).
                // A `Maybe` operand unwraps first (aborting when absent — the
                // VM's halt on comparing nil).
                let (lv, lty) = read_scalar(&mut ssa, &mut insts, reg_a, bi, start)?;
                let (float, lhs, rhs_val) = match lty {
                    Ty::I64 => {
                        let rhs_val = match rhs {
                            FusedRhs::Imm(n) => {
                                let c = ssa.new_val();
                                insts.push(Inst::Const {
                                    dst: c,
                                    value: Const::I64(n),
                                });
                                c
                            }
                            FusedRhs::Reg(r) => read_typed_scalar(&mut ssa, &mut insts, r, bi, Ty::I64, start)?,
                        };
                        (false, lv, rhs_val)
                    }
                    Ty::F64 => {
                        let rhs_val = match rhs {
                            FusedRhs::Imm(n) => {
                                let c = ssa.new_val();
                                insts.push(Inst::Const {
                                    dst: c,
                                    value: Const::F64(n as f64),
                                });
                                c
                            }
                            FusedRhs::Reg(r) => {
                                let (rv, rty) = ssa.read(r, bi, start)?;
                                coerce_to_f64(&mut ssa, &mut insts, rv, rty)
                            }
                        };
                        (true, lv, rhs_val)
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc: start }),
                };
                let cond = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cond,
                    op,
                    float,
                    lhs,
                    rhs: rhs_val,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::ForLoop {
                index_reg,
                end_reg,
                step_reg,
                inclusive,
                positive_step,
                ..
            }) => {
                // next = index + step (wrapping, like the VM); the register is
                // updated *before* the branch so the back-edge phi carries it.
                let index = ssa.read_typed(index_reg, bi, Ty::I64, start)?;
                let end = ssa.read_typed(end_reg, bi, Ty::I64, start)?;
                let step = ssa.read_typed(step_reg, bi, Ty::I64, start)?;
                let next = ssa.new_val();
                insts.push(Inst::IntBin {
                    dst: next,
                    op: IntBinOp::Add,
                    lhs: index,
                    rhs: step,
                });
                ssa.write(index_reg, bi, (next, Ty::I64));
                let op = match (positive_step, inclusive) {
                    (true, true) => CmpOp::Le,
                    (true, false) => CmpOp::Lt,
                    (false, true) => CmpOp::Ge,
                    (false, false) => CmpOp::Gt,
                };
                let cond = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cond,
                    op,
                    float: false,
                    lhs: next,
                    rhs: end,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::FusedCmp2 {
                reg_a,
                imm_a,
                reg_b,
                imm_b,
                ..
            }) => {
                let a = ssa.read_typed(reg_a, bi, Ty::I64, start)?;
                let b = ssa.read_typed(reg_b, bi, Ty::I64, start)?;
                let ka = ssa.new_val();
                insts.push(Inst::Const {
                    dst: ka,
                    value: Const::I64(imm_a),
                });
                let kb = ssa.new_val();
                insts.push(Inst::Const {
                    dst: kb,
                    value: Const::I64(imm_b),
                });
                let ca = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: ca,
                    op: CmpOp::Eq,
                    float: false,
                    lhs: a,
                    rhs: ka,
                });
                let cb = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cb,
                    op: CmpOp::Eq,
                    float: false,
                    lhs: b,
                    rhs: kb,
                });
                let cond = ssa.new_val();
                insts.push(Inst::BoolAnd {
                    dst: cond,
                    lhs: ca,
                    rhs: cb,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::FusedModZero { reg_a, divisor, op, .. }) => {
                // `r_a % divisor <op> 0`: guarded modulo (aborts on a zero divisor,
                // matching the VM) then a compare against zero.
                let lhs = ssa.read_typed(reg_a, bi, Ty::I64, start)?;
                let d = ssa.new_val();
                insts.push(Inst::Const {
                    dst: d,
                    value: Const::I64(divisor),
                });
                let m = ssa.new_val();
                insts.push(Inst::IntBin {
                    dst: m,
                    op: IntBinOp::Mod,
                    lhs,
                    rhs: d,
                });
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                let cond = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cond,
                    op,
                    float: false,
                    lhs: m,
                    rhs: zero,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::NilBranch {
                reg_a, jump_when_nil, ..
            }) => {
                // Resolve nil-ness by the operand's static type: a `Maybe` tests its
                // present bit; any other scalar is provably non-nil (and `Ty::Nil` is
                // provably nil), so the branch folds to a constant. The `cond` is true
                // exactly when the `taken` edge should be followed.
                let (v, ty) = ssa.read(reg_a, bi, start)?;
                let cond = match ty {
                    Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                        let present = ssa.new_val();
                        insts.push(Inst::MaybePresent {
                            dst: present,
                            src: v,
                            maybe_ty: ty,
                        });
                        if jump_when_nil {
                            // taken when nil = when NOT present.
                            let c = ssa.new_val();
                            insts.push(Inst::Not { dst: c, src: present });
                            c
                        } else {
                            // taken when not-nil = present.
                            present
                        }
                    }
                    Ty::Nil => {
                        let c = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: c,
                            value: Const::Bool(jump_when_nil),
                        });
                        c
                    }
                    // A boxed Dyn's nil-ness is its runtime tag — folding it
                    // like a scalar would silently take the wrong branch.
                    Ty::Dyn => {
                        let tag = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(tag),
                            callee: AbiRef::new("dyn", "tag"),
                            args: vec![v],
                        });
                        let zero = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: zero,
                            value: Const::I64(0),
                        });
                        let c = ssa.new_val();
                        insts.push(Inst::Cmp {
                            dst: c,
                            op: if jump_when_nil { CmpOp::Eq } else { CmpOp::Ne },
                            float: false,
                            lhs: tag,
                            rhs: zero,
                        });
                        c
                    }
                    _ => {
                        let c = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: c,
                            value: Const::Bool(!jump_when_nil),
                        });
                        c
                    }
                };
                cond_val[bi] = Some(cond);
            }
            // A Dyn-returning function's bare `return` returns boxed nil
            // (`ret void` is invalid once the signature is `{i64,i64}`);
            // `build_term` picks the resolved value up via `ret_val`.
            Some(Exit::Ret(None)) if !is_entry && sig.dyn_rets.contains(&func_index) => {
                let dummy = ssa.new_val();
                let boxed = to_dyn(&mut ssa, &mut insts, dummy, Ty::Nil, start).expect("nil always boxes");
                match ret_ty {
                    Some(prev) if prev != Ty::Dyn => return Err(Unsupported::ReturnTypeConflict),
                    _ => ret_ty = Some(Ty::Dyn),
                }
                ret_val[bi] = Some(boxed);
            }
            _ => {}
        }
        // What the terminator itself rebound — for a nested region, the
        // registers its cells handed back — reported to this body's own caller
        // and written through this body's own cells.
        if is_try_body {
            for (r, was) in before_all_exit.iter().enumerate() {
                if ssa.current_def[bi][r] != *was {
                    rebound.insert(r as u8);
                }
            }
        }
        mirror_cells(
            &mut ssa,
            &mut insts,
            sig,
            func_index,
            &cell_handles,
            &before_exit,
            bi,
            start,
        )?;
        block_insts[bi] = insts;
        block_exit[bi] = exit;
        ssa.mark_filled(bi);
        ssa.seal_ready()?;
    }
    if let Some(id) = implicit_ret_block {
        ssa.mark_filled(id as usize);
    }
    ssa.seal_ready()?;

    // 6. Build MIR blocks: block params come from the constructed phis; branch args
    //    come from each successor phi's operand contributed by this block.
    let block_id = |pc: usize| -> u32 {
        if pc >= code_len {
            implicit_ret_block.expect("implicit ret block present")
        } else {
            *pc_to_block.range(..=pc).next_back().map(|(_, id)| id).unwrap()
        }
    };
    // Two synthetic blocks per returning region, after every real block and the
    // implicit-return block (the same allocation `implicit_ret_block` uses).
    let synthetic_base = total_blocks as u32 + u32::from(implicit_ret_block.is_some());
    let check_block_ids: Vec<u32> = (0..try_return_checks.len())
        .map(|i| synthetic_base + (i as u32) * 2)
        .collect();
    let ret_block_ids: Vec<u32> = check_block_ids.iter().map(|id| id + 1).collect();
    let mut forwarded_args: Vec<(usize, Vec<ValueId>, BlockId)> = Vec::new();
    let mut mir_blocks: Vec<Block> = Vec::with_capacity(total_blocks);
    for bi in 0..leader_vec.len() {
        if !reachable[bi] {
            // No instructions, no params, and a terminator that names only
            // itself — nothing about the rest of the function has to hold for
            // a block control cannot enter.
            mir_blocks.push(Block {
                id: BlockId(bi as u32),
                params: Vec::new(),
                insts: Vec::new(),
                term: Term::Br {
                    target: BlockId(bi as u32),
                    args: Vec::new(),
                },
            });
            continue;
        }
        let params: Vec<(ValueId, Ty)> = ssa.phis[bi].iter().map(|p| (p.param, p.ty)).collect();
        let exit = block_exit[bi];
        // Phi-edge conversions land after the block's own instructions,
        // before the terminator.
        let edge_tail = std::mem::take(&mut ssa.edge_insts[bi]);
        let mut term = build_term(bi, exit, &ssa, &block_id, ret_val[bi], cond_val[bi]);
        // A region whose body may return: its ok edge goes to the check block
        // instead, which forwards to the real fallthrough with the *same*
        // arguments. Rewriting the edge rather than re-keying the phis is what
        // keeps this local — the target's operands are still recorded against
        // this block, and this is where they are read from.
        if let Some(index) = try_return_checks.iter().position(|(rb, _, _, _)| *rb == bi)
            && let Term::CondBr {
                then_blk, then_args, ..
            } = &mut term
        {
            forwarded_args.push((index, core::mem::take(then_args), *then_blk));
            *then_blk = BlockId(check_block_ids[index]);
        }
        let mut insts = std::mem::take(&mut block_insts[bi]);
        insts.extend(edge_tail);
        mir_blocks.push(Block {
            id: BlockId(bi as u32),
            params,
            insts,
            term,
        });
    }
    if let Some(id) = implicit_ret_block {
        let params: Vec<(ValueId, Ty)> = ssa.phis[id as usize].iter().map(|p| (p.param, p.ty)).collect();
        // A Dyn-returning function's implicit return (falling off the end)
        // returns boxed nil — `ret void` in a `{i64,i64}` function is invalid.
        let (insts, term) = if !reachable[id as usize] {
            (
                Vec::new(),
                Term::Br {
                    target: BlockId(id),
                    args: Vec::new(),
                },
            )
        } else if !is_entry && sig.dyn_rets.contains(&func_index) {
            let dummy = ssa.new_val();
            let mut iv = Vec::new();
            let boxed = to_dyn(&mut ssa, &mut iv, dummy, Ty::Nil, 0).expect("nil always boxes");
            (iv, Term::Ret(Some(boxed)))
        } else if ret_ty.is_some() {
            // One path returns a value and another falls off the end, which
            // answers nil. `ret void` in a value-returning function is not
            // valid MIR, and there is no value of the return type that means
            // nil — the same conflict two disagreeing `return`s produce.
            return Err(Unsupported::ReturnTypeConflict);
        } else {
            (Vec::new(), Term::Ret(None))
        };
        mir_blocks.push(Block {
            id: BlockId(id),
            params,
            insts,
            term,
        });
    }

    // The check/return pair for each region whose body may return. Emitted here
    // because the *enclosing* function's return type is only settled once every
    // block has been lowered, and the parked value has to come back out of its
    // cell as that type.
    let ret = ret_ty.unwrap_or(Ty::Nil);
    for (index, (_, flag, value, always_returns)) in try_return_checks.iter().enumerate() {
        let (_, fallthrough_args, fallthrough) = forwarded_args
            .iter()
            .find(|(i, _, _)| *i == index)
            .cloned()
            .expect("every recorded check redirects exactly one edge");
        let mut check_insts = Vec::new();
        let raised = ssa.new_val();
        check_insts.push(Inst::Call {
            dst: Some(raised),
            callee: AbiRef::new("rt", "cell_get"),
            args: vec![*flag],
        });
        let as_int = ssa.new_val();
        check_insts.push(Inst::Call {
            dst: Some(as_int),
            callee: AbiRef::new("dyn", "as_i64"),
            args: vec![raised],
        });
        let zero = ssa.new_val();
        check_insts.push(Inst::Const {
            dst: zero,
            value: Const::I64(0),
        });
        let returned = ssa.new_val();
        check_insts.push(Inst::Cmp {
            dst: returned,
            op: CmpOp::Ne,
            float: false,
            lhs: as_int,
            rhs: zero,
        });
        mir_blocks.push(Block {
            id: BlockId(check_block_ids[index]),
            params: Vec::new(),
            insts: check_insts,
            term: Term::CondBr {
                cond: returned,
                then_blk: BlockId(ret_block_ids[index]),
                then_args: Vec::new(),
                else_blk: if *always_returns {
                    BlockId(ret_block_ids[index])
                } else {
                    fallthrough
                },
                else_args: if *always_returns { Vec::new() } else { fallthrough_args },
            },
        });

        let mut ret_insts = Vec::new();
        let boxed = ssa.new_val();
        ret_insts.push(Inst::Call {
            dst: Some(boxed),
            callee: AbiRef::new("rt", "cell_get"),
            args: vec![*value],
        });
        // This function may itself be a region's body, and then the `return` it
        // is about to perform is not its own either: it belongs to whoever is
        // two frames out. Forward it into *this* body's channel instead — the
        // value is already boxed, so the hand-off is two `cell_set`s — and
        // return normally, so the trampoline still reports "did not raise".
        //
        // Without the forward, the inner region's parked value was read back
        // and then dropped, because a body's own return type is `Nil`: `try {
        // try { return 11; } catch e { … } } catch e { … }` answered whatever
        // the function fell through to.
        if let Some((outer_flag, outer_value)) = return_channel {
            ret_insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "cell_set"),
                args: vec![outer_value, boxed],
            });
            let one = ssa.new_val();
            ret_insts.push(Inst::Const {
                dst: one,
                value: Const::I64(1),
            });
            let marked = to_dyn_any(&mut ssa, &mut ret_insts, one, Ty::I64, 0)?;
            ret_insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "cell_set"),
                args: vec![outer_flag, marked],
            });
            mir_blocks.push(Block {
                id: BlockId(ret_block_ids[index]),
                params: Vec::new(),
                insts: ret_insts,
                term: Term::Ret(None),
            });
            continue;
        }
        let returned_value = match ret {
            Ty::Nil => None,
            // The same unboxing the output cells use; a type with no readback
            // never got here, because the body's `return` had to box it in the
            // first place.
            other => match unbox_cell_value(&mut ssa, &mut ret_insts, boxed, other) {
                Some(value) => Some(value),
                None => {
                    return Err(Unsupported::TryRegion {
                        pc: 0,
                        reason: "the body returns a value that cannot be read back out of a cell",
                    });
                }
            },
        };
        mir_blocks.push(Block {
            id: BlockId(ret_block_ids[index]),
            params: Vec::new(),
            insts: ret_insts,
            term: Term::Ret(returned_value),
        });
    }

    // User (non-entry) functions return scalars, `Str`/handle pointers
    // (arena-owned until exit), or nothing (`Nil` renders as `void`).
    // A `Maybe` carrier has no direct-call return form: retriable — the
    // fixpoint re-lowers with every return boxed, so the function returns
    // `Dyn` (nil crosses as nil, VM-exact).
    if !is_entry && matches!(ret, Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
        sig.dyn_rets.insert(func_index);
        return Err(Unsupported::ReturnTypeConflict);
    }
    // The entry can return scalars (printed), but not a container handle (printing
    // a list is not modelled yet) — reject so it falls back rather than print wrong.
    if is_entry
        && matches!(
            ret,
            Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::MapStrI64 | Ty::MapI64I64 | Ty::MapStrF64 | Ty::MapI64F64
        )
    {
        return Err(Unsupported::ReturnTypeConflict);
    }
    if is_try_body {
        sig.try_body_rebound.insert(func_index, rebound);
    }
    Ok(MirFunction {
        id: FuncId(func_index),
        params: fn_params,
        entry: BlockId(0),
        ret,
        blocks: mir_blocks,
        // The entry already has a fixed exported name (`main`), so an
        // `#[export]` on it would be a second name for the same symbol.
        export_name: if is_entry { None } else { func.export_name.clone() },
    })
}

/// A string constant as an SSA value, interned into the module's global table.
fn const_str_value(ssa: &mut Ssa, insts: &mut Vec<Inst>, globals: &mut Vec<String>, text: &str) -> ValueId {
    let gid = crate::prescan::intern_global(globals, text);
    let dst = ssa.new_val();
    insts.push(Inst::Const {
        dst,
        value: Const::Str(GlobalId(gid)),
    });
    dst
}
