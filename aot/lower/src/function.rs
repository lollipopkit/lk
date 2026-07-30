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
        // Containers come back as an untyped handle (`dyn.as_list` /
        // `dyn.as_map` answer `Ptr`), and which *typed* handle that is depends
        // on the register. Getting it wrong is a container read as the wrong
        // element type, so they wait until there is a test that pins each one.
        _ => return None,
    })
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
    )
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
            cells.sort_unstable();
        }
        // The trampoline passes machine words and the arity switch caps them;
        // inputs and cells share that budget.
        if cells.len()
            + sig
                .try_body_params
                .get(&body_index_of(sig, func_index, region.begin_pc)?)
                .map_or(0, Vec::len)
            > 8
        {
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
    for (bi, &(start, end)) in block_bounds.iter().enumerate() {
        let (_, exit) = block_span(&exits, &consumed, start, end);
        for succ in exit_successors(exit, end) {
            preds[block_of(succ)].push(bi);
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
    ssa.dyn_empty_pcs = sig
        .dyn_empty_lists
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
                ssa.builtin_regs.insert((0, r as u8), GlobalRef::Lambda(id.fidx));
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
        if r == 0
            && pty == Ty::MapStrDyn
            && let Some(type_name) = sig.traits.impl_owner(func_index)
        {
            ssa.struct_types.insert(pv, type_name);
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
    for &reg in &try_params {
        let ty = sig
            .try_body_param_tys
            .get(&(func_index, reg))
            .copied()
            .unwrap_or(Ty::I64);
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
        ssa.builtin_regs.insert((0, r as u8), GlobalRef::Closure(id.fidx, caps));
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
        // A spawned goroutine's cell captures are thread-private copies:
        // seed the virtual slot so body writes (isolate — never visible to
        // the spawner) go through plain SSA.
        if spawned_isolate {
            let slot = ssa.cellparam_slot(k);
            ssa.write_slot(slot, 0, (cv, cty));
        }
    }
    let mut block_insts: Vec<Vec<Inst>> = vec![Vec::new(); total_blocks];
    let mut block_exit: Vec<Option<Exit>> = vec![None; total_blocks];
    let mut ret_ty: Option<Ty> = None;
    // Resolved terminator value reads (filled during each block's lowering).
    // Regions whose body may `return` from this function: the ok edge gets a
    // check block, and a `return` block behind it. Collected here and emitted
    // after the block loop, where this function's own return type is known.
    let mut try_return_checks: Vec<(usize, ValueId, ValueId)> = Vec::new();
    let mut ret_val: Vec<Option<ValueId>> = vec![None; total_blocks];
    let mut cond_val: Vec<Option<ValueId>> = vec![None; total_blocks];

    for (bi, &(start, end)) in block_bounds.iter().enumerate() {
        ssa.seal_ready()?;
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
            for (index, (reg, handle)) in cell_handles.iter().enumerate() {
                let now = ssa.current_def[bi][*reg as usize];
                if now == before[index] {
                    continue;
                }
                let Some((value, ty)) = now else { continue };
                // Written immediately, not at the end of the body: a raise can
                // happen in the next call, and the VM shows whatever was
                // assigned before it. Storing only on the way out would lose
                // exactly the writes a handler is most likely to look at.
                let boxed = crate::dyn_box::to_dyn_any(&mut ssa, &mut insts, value, ty, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "cell_set"),
                    args: vec![*handle, boxed],
                });
            }
        }
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
            Some(Exit::TryRegion { body, catch_reg, .. }) => {
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
                let mut call_args = Vec::new();
                for &reg in sig.try_body_params.get(&body).cloned().unwrap_or_default().iter() {
                    // Read as whatever it is, then decide whether it can cross.
                    // Forcing `I64` here is what used to reject a body that
                    // merely *looked at* a list the parent owned — a handle is a
                    // machine word, and the buffer the trampoline marshals into
                    // is machine words.
                    let (v, ty) = ssa.read(reg, bi, start)?;
                    if crosses_as_word(ty) {
                        sig.try_body_param_tys.insert((body, reg), ty);
                        call_args.push(v);
                    } else {
                        // Not a word: the honest failure is the body rejecting
                        // when it reads it, which is what `I64` produces.
                        sig.try_body_param_tys.remove(&(body, reg));
                        call_args.push(ssa.read_typed(reg, bi, Ty::I64, start)?);
                    }
                }
                // One cell per register the body assigns that this function
                // already had. Seeded with the value it holds now, because a
                // body that raises before assigning must leave it alone.
                let cell_regs: Vec<u8> = sig.try_body_cells.get(&body).cloned().unwrap_or_default();
                let mut cell_values: Vec<(u8, ValueId, Ty)> = Vec::with_capacity(cell_regs.len());
                for &reg in &cell_regs {
                    let (v, ty) = ssa.read(reg, bi, start)?;
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
                    try_return_checks.push((bi, flag, value));
                }
                let ok = ssa.new_val();
                insts.push(Inst::TryRegionCall {
                    dst: ok,
                    func: FuncId(body),
                    args: call_args,
                });
                // Read every cell back, before the branch, so both edges see
                // what the body managed to write — including a body that
                // raised half way through, which is what the VM shows.
                for (reg, handle, ty) in cell_values {
                    let got = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(got),
                        callee: AbiRef::new("rt", "cell_get"),
                        args: vec![handle],
                    });
                    let raw = match unbox_from_dyn(ty).expect("checked above") {
                        CellReadBack::Identity => got,
                        CellReadBack::Unbox(module, name) => {
                            let raw = ssa.new_val();
                            insts.push(Inst::Call {
                                dst: Some(raw),
                                callee: AbiRef::new(module, name),
                                args: vec![got],
                            });
                            raw
                        }
                    };
                    // `dyn.as_bool` answers an `i64`; the register holds a
                    // `Bool`, which is a narrower machine type. Writing the
                    // wide value back under the narrow type is what the
                    // Cranelift verifier rejects — "arg has type i64, expected
                    // i8" — so it is narrowed here.
                    // See `unbox_from_dyn`: a nil seed comes back as whatever
                    // the body boxed, which is a `Dyn`.
                    let ty = if ty == Ty::Nil { Ty::Dyn } else { ty };
                    let value = if ty == Ty::Bool {
                        let zero = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: zero,
                            value: Const::I64(0),
                        });
                        let narrowed = ssa.new_val();
                        insts.push(Inst::Cmp {
                            dst: narrowed,
                            op: CmpOp::Ne,
                            float: false,
                            lhs: raw,
                            rhs: zero,
                        });
                        narrowed
                    } else {
                        raw
                    };
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
                let rebound: Vec<u8> = match sig.try_body_rebound.get(&body) {
                    Some(set) => set.iter().copied().collect(),
                    None => regions
                        .iter()
                        .find(|r| sig.try_bodies.get(&(func_index, r.begin_pc)) == Some(&body))
                        .map(|span| crate::try_region::written_registers(&instrs, span.body_start, span.body_end))
                        .unwrap_or_default(),
                };
                for reg in rebound {
                    if reg != catch_reg && !cell_regs.contains(&reg) {
                        ssa.poison(reg, bi);
                    }
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
        if let Some(index) = try_return_checks.iter().position(|(rb, _, _)| *rb == bi)
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
        let (insts, term) = if !is_entry && sig.dyn_rets.contains(&func_index) {
            let dummy = ssa.new_val();
            let mut iv = Vec::new();
            let boxed = to_dyn(&mut ssa, &mut iv, dummy, Ty::Nil, 0).expect("nil always boxes");
            (iv, Term::Ret(Some(boxed)))
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
    for (index, (_, flag, value)) in try_return_checks.iter().enumerate() {
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
                else_blk: fallthrough,
                else_args: fallthrough_args,
            },
        });

        let mut ret_insts = Vec::new();
        let boxed = ssa.new_val();
        ret_insts.push(Inst::Call {
            dst: Some(boxed),
            callee: AbiRef::new("rt", "cell_get"),
            args: vec![*value],
        });
        let returned_value = match ret {
            Ty::Nil => None,
            Ty::Dyn => Some(boxed),
            other => {
                // The same unboxing table the output cells use; a type with no
                // unboxer never got here, because the body's `return` had to box
                // it in the first place.
                let Some(read_back) = unbox_from_dyn(other) else {
                    return Err(Unsupported::TryRegion {
                        pc: 0,
                        reason: "the body returns a value that cannot be read back out of a cell",
                    });
                };
                match read_back {
                    CellReadBack::Identity => Some(boxed),
                    CellReadBack::Unbox(module, name) => {
                        let raw = ssa.new_val();
                        ret_insts.push(Inst::Call {
                            dst: Some(raw),
                            callee: AbiRef::new(module, name),
                            args: vec![boxed],
                        });
                        // `dyn.as_bool` answers an `i64`; a `Bool` operand is
                        // narrower, and the verifier rejects the wide value.
                        if other == Ty::Bool {
                            let zero = ssa.new_val();
                            ret_insts.push(Inst::Const {
                                dst: zero,
                                value: Const::I64(0),
                            });
                            let narrow = ssa.new_val();
                            ret_insts.push(Inst::Cmp {
                                dst: narrow,
                                op: CmpOp::Ne,
                                float: false,
                                lhs: raw,
                                rhs: zero,
                            });
                            Some(narrow)
                        } else {
                            Some(raw)
                        }
                    }
                }
            }
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
