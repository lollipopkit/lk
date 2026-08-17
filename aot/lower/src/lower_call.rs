use super::*;

/// `spawn(closure)` / the `go` desugar (plan H): the closure's captures all
/// cross the isolate boundary *boxed* (its signature joins to
/// `fn(Dyn, …) -> Dyn`), so a per-arity lkrt trampoline launches the
/// compiled body on a fresh OS thread. Captures snapshot by value —
/// including cells (isolate: a goroutine's mutation never leaks back; the
/// body's cell writes land in a thread-private virtual slot).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_spawn(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    cap_ctx: CaptureCtx<'_>,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if argc != 1 {
        return Err(Unsupported::CallShape {
            pc,
            reason: "a spawned callee must be a statically known function with scalar arguments",
        });
    }
    let arg_reg = base.wrapping_add(1);
    let (fidx, caps) = match ssa.builtin_ref_at(arg_reg, block) {
        Some(GlobalRef::Closure(f, caps)) => (f as usize, caps),
        Some(GlobalRef::Lambda(f)) => (f as usize, Vec::new()),
        _ => {
            return Err(Unsupported::CallShape {
                pc,
                reason: "a spawned callee must be a statically known function with scalar arguments",
            });
        }
    };
    if fidx >= funcs.len()
        || fidx == entry as usize
        || funcs[fidx].param_count != 0
        || caps.len() != funcs[fidx].capture_count as usize
        || caps.len() > 4
    {
        return Err(Unsupported::CallShape {
            pc,
            reason: "a spawned callee must be a statically known function with scalar arguments",
        });
    }
    sig.spawned_isolate.insert(fidx as u32);
    // Snapshot the captures into the argument block, boxed.
    let block_v = if caps.is_empty() {
        None
    } else {
        let b = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(b),
            callee: AbiRef::new("rt", "spawn_args_new"),
            args: Vec::new(),
        });
        let site = CaptureSite::new(cap_ctx, fidx as u32, CaptureMode::Snapshot, block, pc);
        for (k, capture) in caps.iter().enumerate() {
            // Isolate: every capture crosses as a private copy taken here, so a
            // cell is read for its *content* rather than passed by pointer.
            let (v, ty) = match site.resolve(ssa, insts, sig, capture, k)? {
                Some(resolved) => resolved,
                None => {
                    let ClosureCapture::Cell(cid) = capture else {
                        unreachable!("only `Cell` is left to the call site")
                    };
                    let slot = ssa.cell_slot(*cid);
                    ssa.read_slot(slot, block, pc)?
                }
            };
            let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "spawn_args_push"),
                args: vec![b, boxed],
            });
            // Boxed into `Dyn` on the way in, so the callee's parameter is
            // never a typed struct: no provenance to carry.
            let want = sig.observe_param(fidx, k, Ty::Dyn, None);
            if want != Ty::Dyn {
                return Err(Unsupported::TypeMismatch { pc });
            }
        }
        Some(b)
    };
    // The body's result crosses back boxed on `task.await`.
    if !sig.dyn_rets.contains(&(fidx as u32)) {
        sig.dyn_rets.insert(fidx as u32);
        return Err(Unsupported::TypeMismatch { pc });
    }
    if sig.ret_types.get(fidx).copied() != Some(Ty::Dyn) {
        return Err(Unsupported::TypeMismatch { pc });
    }
    let fnaddr = ssa.new_val();
    insts.push(Inst::Const {
        dst: fnaddr,
        value: Const::FnAddr(FuncId(fidx as u32)),
    });
    let spawn_fn: &'static str = match caps.len() {
        0 => "spawn0",
        1 => "spawn1",
        2 => "spawn2",
        3 => "spawn3",
        _ => "spawn4",
    };
    let mut args = vec![fnaddr];
    if let Some(b) = block_v {
        args.push(b);
    }
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("rt", spawn_fn),
        args,
    });
    ssa.write(base, block, (dst, Ty::I64));
    Ok(())
}

/// `__lk_merge_fields(base, overlay)` — the struct-update desugar
/// (`P { ..base, k: v }`). Mirrors the VM's `merge_field_maps` two-step
/// insertion (base entries the overlay doesn't shadow, then the overlay),
/// so the result's iteration order is VM-exact.
pub(crate) fn lower_merge_fields(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if argc != 2 {
        return Err(Unsupported::CallShape {
            pc,
            reason: "a field merge needs two map operands",
        });
    }
    let (bv, bty) = ssa.read(base.wrapping_add(1), block, pc)?;
    let (ov, oty) = ssa.read(base.wrapping_add(2), block, pc)?;
    let base_map = to_dyn_map_handle(ssa, insts, bv, bty, pc)?;
    let dst = ssa.new_val();
    // The overlay is walked where it lives rather than converted. A struct
    // update's overlay is the `{x: 42}` field literal — a *typed* map — and
    // converting it meant re-inserting its entries into a fresh table in its
    // iteration order, which is not the sequence that built it. The overlay's
    // order is the tail of the merged result's, so that was a reorder waiting
    // to be noticed (see `map_h.str_dyn_merge_typed`).
    match typed_map_kind(oty) {
        Some(kind) => {
            let kind_v = ssa.new_val();
            insts.push(Inst::Const {
                dst: kind_v,
                value: Const::I64(kind),
            });
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", "str_dyn_merge_typed"),
                args: vec![base_map, ov, kind_v],
            });
        }
        None => {
            let overlay_map = to_dyn_map_handle(ssa, insts, ov, oty, pc)?;
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", "str_dyn_merge"),
                args: vec![base_map, overlay_map],
            });
        }
    }
    ssa.write(base, block, (dst, Ty::MapStrDyn));
    Ok(())
}

/// `__lk_make_struct(name, fields)` — the struct-update desugar's object
/// constructor. The VM copies the merged field map into a fresh
/// `RuntimeObject` (`runtime_object_fields_from_map`); the native carrier
/// replays that fresh zero-capacity rebuild to keep the iteration order
/// identical, then records struct provenance (trait dispatch, plan J1).
pub(crate) fn lower_make_struct(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    sig: &mut SigInfer,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if argc != 2 {
        return Err(Unsupported::CallShape {
            pc,
            reason: "a struct construction needs a constant type name and a map of fields",
        });
    }
    let name_reg = base.wrapping_add(1);
    let type_name = ssa.const_str_at(name_reg, block, pc).ok_or(Unsupported::CallShape {
        pc,
        reason: "a struct construction needs a constant type name and a map of fields",
    })?;
    let (fv, fty) = ssa.read(base.wrapping_add(2), block, pc)?;
    let fields = to_dyn_map_handle(ssa, insts, fv, fty, pc)?;
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("map_h", "str_dyn_rebuild"),
        args: vec![fields],
    });
    if let Some(&tid) = sig.traits.type_ids.get(&type_name) {
        let tid_v = ssa.new_val();
        insts.push(Inst::Const {
            dst: tid_v,
            value: Const::I64(tid),
        });
        insts.push(Inst::Call {
            dst: None,
            callee: AbiRef::new("map_h", "obj_mark"),
            args: vec![dst, tid_v],
        });
    }
    ssa.struct_types.insert(dst, type_name);
    ssa.write(base, block, (dst, Ty::MapStrDyn));
    Ok(())
}

/// Lowers a call to user function `callee_idx` with the register-window layout
/// shared by `CallDirect` and indirect `Call` (callee/result at `dst_reg`,
/// args at `[dst_reg+1, dst_reg+1+argc)`): reads the typed arguments, refines
/// the callee's per-callsite-monomorphized signature, and writes the typed
/// result. A capturing closure passes its environment snapshot as hidden
/// trailing arguments (`captures`); their count must match the callee's
/// `capture_count` (a `CallDirect` to a capturing lambda has no environment
/// and rejects).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_user_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    cap_ctx: CaptureCtx<'_>,
    callee_idx: usize,
    dst_reg: u8,
    argc: usize,
    captures: &[(ValueId, Ty)],
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if callee_idx >= funcs.len() || callee_idx == entry as usize {
        return Err(Unsupported::Opcode {
            pc,
            op: Opcode::CallDirect,
        });
    }
    if argc != funcs[callee_idx].param_count as usize {
        return Err(Unsupported::Opcode {
            pc,
            op: Opcode::CallDirect,
        });
    }
    let capture_count = funcs[callee_idx].capture_count as usize;
    if captures.len() != capture_count && !sig.captures_all_static(callee_idx, capture_count) {
        return Err(Unsupported::Opcode {
            pc,
            op: Opcode::CallDirect,
        });
    }
    // Zero-capture lambda arguments are erased from the native signature:
    // collect the call site's lambda identity vector; a non-empty vector
    // retargets the call to a per-identity *clone* of the callee (created on
    // demand, byte-identical body, `lambda_params` pre-filled so its
    // parameters seed static refs instead of binding values).
    // Identity resolution backtracks across blocks like the hidden-env
    // lookup below (an argument register may inherit its lambda/closure ref
    // from a predecessor), so both paths agree on what the register holds.
    let identity: Vec<Option<LambdaIdentity>> = (0..argc)
        .map(|i| {
            let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
            match ssa.builtin_ref_at(arg_reg, block) {
                Some(GlobalRef::Lambda(fidx)) => Some(LambdaIdentity { fidx, captures: 0 }),
                Some(GlobalRef::Closure(fidx, caps)) => Some(LambdaIdentity {
                    fidx,
                    captures: caps.len() as u16,
                }),
                _ => None,
            }
        })
        .collect();
    let callee_idx = if identity.iter().any(Option::is_some) {
        // The clone carries `lambda_params`; the original body would treat the
        // parameter as a plain value and reject. A function called both ways
        // is polymorphic over functions vs values — outside the subset.
        if let Some(flag) = sig.specialized.get_mut(callee_idx) {
            *flag = true;
        }
        if sig.plain_called.get(callee_idx).copied().unwrap_or(false) {
            sig.conflict = true;
            return Err(Unsupported::TypeMismatch { pc });
        }
        let key = (callee_idx as u32, identity.clone());
        match sig.specializations.get(&key) {
            Some(&clone) => clone as usize,
            None => {
                // Cap the clone count per original so a pathological program
                // cannot explode the module (falls back loudly instead).
                const MAX_SPECIALIZATIONS: usize = 8;
                let existing = sig
                    .specializations
                    .keys()
                    .filter(|(orig, _)| *orig == callee_idx as u32)
                    .count();
                if existing >= MAX_SPECIALIZATIONS {
                    sig.conflict = true;
                    return Err(Unsupported::TypeMismatch { pc });
                }
                let env_total: usize = identity.iter().flatten().map(|id| id.captures as usize).sum();
                let arity =
                    funcs[callee_idx].param_count as usize + env_total + funcs[callee_idx].capture_count as usize;
                let ret_known = sig.ret_known.get(callee_idx).copied().unwrap_or(false);
                let clone = sig.push_function(vec![None; arity], sig.ret_types[callee_idx]);
                sig.ret_known[clone as usize] = ret_known;
                sig.lambda_params[clone as usize] = identity.clone();
                sig.specializations.insert(key, clone);
                sig.pending_clones.push(callee_idx as u32);
                clone as usize
            }
        }
    } else {
        if sig.specialized.get(callee_idx).copied().unwrap_or(false) {
            sig.conflict = true;
            return Err(Unsupported::TypeMismatch { pc });
        }
        if let Some(flag) = sig.plain_called.get_mut(callee_idx) {
            *flag = true;
        }
        callee_idx
    };
    // A summarized callee (its single return is a closure whose captures map
    // to parameters) is consumed statically: the result register is seeded
    // with the closure ref built from this call site's argument values. The
    // effect-free body is never emitted and no call happens at runtime.
    if let Some((lf, srcs)) = sig.ret_closures.get(callee_idx).cloned().flatten() {
        let mut caps = Vec::with_capacity(srcs.len());
        for RetCaptureSrc::Param(k) in srcs {
            let arg_reg = dst_reg.wrapping_add(1).wrapping_add(k as u8);
            let (v, ty) = ssa.read(arg_reg, block, pc)?;
            // Nullable shapes have no typed capture form: box to Dyn, so the
            // eventual consumer joins its parameter to Dyn like any call site.
            let (v, ty) = if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                (to_dyn_any(ssa, insts, v, ty, pc)?, Ty::Dyn)
            } else {
                (v, ty)
            };
            caps.push(ClosureCapture::Value(v, ty));
        }
        if (dst_reg as usize) < ssa.reg_count {
            ssa.current_def[block][dst_reg as usize] = None;
        }
        ssa.bind_ref(block, dst_reg, GlobalRef::Closure(lf, caps));
        return Ok(());
    }
    // Tier 1 bridge call (`docs/aot/tier1-hybrid.md`): the callee runs on the
    // embedded VM. Arguments must match the recorded scalar marshaling types;
    // the destination register binds as `Dyn` (v2: the bridge returns an
    // `LkDyn` by value) — codegen degrades a never-read destination back to
    // the void bridge call, so statement-position calls stay v1-shaped.
    if let Some(param_count) = sig.vm_functions.get(&(callee_idx as u32)).copied() {
        if !captures.is_empty() {
            return Err(Unsupported::Opcode {
                pc,
                op: Opcode::CallDirect,
            });
        }
        let mut args = Vec::with_capacity(argc);
        let mut arg_tys = Vec::with_capacity(argc);
        for i in 0..param_count.min(argc) {
            let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
            let (aval, aty) = read_scalar(ssa, insts, arg_reg, block, pc)?;
            // Each argument is tagged individually at the bridge, so the type
            // only has to be marshalable *here* — it need not agree with what
            // another call site passes for the same parameter.
            if !matches!(aty, Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str | Ty::Nil) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            args.push(aval);
            arg_tys.push(aty);
        }
        let dst = ssa.new_val();
        insts.push(Inst::CallVm {
            dst: Some(dst),
            func: FuncId(callee_idx as u32),
            args,
            arg_tys,
        });
        ssa.builtin_regs.remove(&(block, dst_reg));
        ssa.write(dst_reg, block, (dst, Ty::Dyn));
        return Ok(());
    }
    let mut args = Vec::with_capacity(argc + captures.len());
    // Kept alongside, for an `#[extern]` callee: its signature is not in any
    // table, so the call site is where the argument types come from.
    let mut arg_tys: Vec<Ty> = Vec::with_capacity(argc + captures.len());
    let mut env_args: Vec<(ValueId, Ty)> = Vec::new();
    for (i, id) in identity.iter().enumerate() {
        let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
        match *id {
            // Erased zero-capture lambda: nothing is passed at runtime.
            Some(LambdaIdentity { captures: 0, .. }) => continue,
            // Erased capturing closure: its environment (resolved to current
            // cell contents at this call site) travels as hidden trailing
            // arguments, in parameter order.
            Some(LambdaIdentity { fidx: lambda, .. }) => {
                let Some(GlobalRef::Closure(_, caps)) = ssa.builtin_ref_at(arg_reg, block) else {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "the callee does not resolve to a statically known function",
                    });
                };
                let site = CaptureSite::new(cap_ctx, lambda, CaptureMode::Share, block, pc);
                for (k, capture) in caps.iter().enumerate() {
                    let (v, ty) = match site.resolve(ssa, insts, sig, capture, k)? {
                        Some(resolved) => resolved,
                        None => {
                            let ClosureCapture::Cell(cid) = capture else {
                                unreachable!("only `Cell` is left to the call site")
                            };
                            let slot = ssa.cell_slot(*cid);
                            ssa.read_slot(slot, block, pc)?
                        }
                    };
                    env_args.push((v, ty));
                }
                continue;
            }
            None => {}
        }
        // `Str` and container handles pass as `ptr` (arena-owned until exit,
        // so no ownership transfer is involved). The raw register read keeps
        // nullable carriers intact: they observe as `Dyn` and box, so the
        // callee receives nil as nil (VM call semantics) instead of the
        // scalar-context unwrap abort.
        // Through `read_value`: an argument that is a lambda the callee cannot
        // erase — a struct constructor's field, say — becomes a closure value
        // here. A lambda the callee *can* erase never reaches this line; the
        // identity vector above took it.
        let (aval, aty) = read_value(ssa, insts, sig, funcs, cap_ctx, arg_reg, block, pc)?;
        let want = sig.observe_param(callee_idx, i, aty, ssa.struct_types.get(&aval).map(String::as_str));
        // A typed container reaching an erased parameter has to be built Dyn.
        //
        // `want` is `Dyn` here because two call sites disagreed on the
        // carrier, so the callee sees the list only through its tag and a
        // `push` goes to `dyn.list_push`. That push may widen — and a
        // `Vec<i64>` cannot become a `Vec<LkDyn>` after the fact, because the
        // caller's aliases read the old allocation. The VM widens the carrier
        // in place, so the only representation both backends can agree on is
        // a Dyn list from the literal onward. Same retry channel as a
        // contradicted `[]`: the fixpoint rebuilds the literal and this call
        // site then passes a `ListDyn`, which does not re-trigger.
        //
        // Two ways to learn that: `want` is `Dyn` (two call sites disagreed on
        // the carrier, so the callee pushes through `dyn.list_push` and the
        // widening is invisible to it at compile time), or the callee was
        // lowered once and reported the push itself (`dyn_params`), which is
        // the monomorphic case a single call site produces.
        if matches!(
            aty,
            Ty::ListI64
                | Ty::ListF64
                | Ty::ListStr
                | Ty::MapStrI64
                | Ty::MapStrF64
                | Ty::MapStrBool
                | Ty::MapI64I64
                | Ty::MapI64F64
        ) && (want == Ty::Dyn || sig.dyn_params.contains(&(callee_idx as u32, i as u8)))
            && let Some(unsupported) = crate::inst::container::carrier_contradicted(ssa, aval, aty)
        {
            return Err(unsupported);
        }
        arg_tys.push(want);
        args.push(coerce_arg(ssa, insts, aval, aty, want, pc)?);
    }
    // Hidden trailing arguments, in signature order: the erased closures'
    // environment values first, then the callee's own captures. Their types
    // refine the same monomorphization lattice as visible parameters.
    for (k, &(ev, ety)) in env_args.iter().enumerate() {
        let want = sig.observe_param(callee_idx, argc + k, ety, ssa.struct_types.get(&ev).map(String::as_str));
        arg_tys.push(want);
        args.push(coerce_arg(ssa, insts, ev, ety, want, pc)?);
    }
    let env_total = env_args.len();
    for (k, &(cval, cty)) in captures.iter().enumerate() {
        let want = sig.observe_param(
            callee_idx,
            argc + env_total + k,
            cty,
            ssa.struct_types.get(&cval).map(String::as_str),
        );
        arg_tys.push(want);
        args.push(coerce_arg(ssa, insts, cval, cty, want, pc)?);
    }
    let ret = sig.ret_types.get(callee_idx).copied().unwrap_or(Ty::I64);
    // A callee the source marked `#[extern]` is implemented outside the
    // program: the call goes to that symbol and its body is never emitted. The
    // body is not dead — it is what the interpreter runs — but nothing native
    // uses it.
    if let Some(symbol) = funcs.get(callee_idx).and_then(|f| f.extern_name.clone()) {
        if ret == Ty::Nil {
            insts.push(Inst::CallExtern {
                dst: None,
                symbol,
                args,
                arg_tys,
                ret,
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            ssa.write(dst_reg, block, (nil, Ty::Nil));
        } else {
            let dst = ssa.new_val();
            insts.push(Inst::CallExtern {
                dst: Some(dst),
                symbol,
                args,
                arg_tys,
                ret,
            });
            ssa.write(dst_reg, block, (dst, ret));
        }
        return Ok(());
    }
    if ret == Ty::Nil {
        insts.push(Inst::CallFn {
            dst: None,
            func: FuncId(callee_idx as u32),
            args,
        });
        let nil = ssa.new_val();
        insts.push(Inst::Const {
            dst: nil,
            value: Const::Nil,
        });
        ssa.write(dst_reg, block, (nil, Ty::Nil));
    } else {
        let dst = ssa.new_val();
        insts.push(Inst::CallFn {
            dst: Some(dst),
            func: FuncId(callee_idx as u32),
            args,
        });
        seed_ret_struct(ssa, sig, callee_idx, dst);
        ssa.write(dst_reg, block, (dst, ret));
    }
    Ok(())
}

/// `CallNamed` — a call written with `name: value` arguments.
///
/// The whole opcode had no native lowering, so every named call dropped its
/// module to the VM. That became load-bearing when `module.Type { … }` started
/// desugaring to one (`stmt::struct_ctors`), which is how a cross-module struct
/// literal is built.
///
/// It devirtualizes the same way a positional call does, plus one step: the
/// argument *order*. The window is `[base]` callee, `positional` values, then
/// `named_count` (name, value) pairs — and every name is a string constant the
/// compiler emitted, so the permutation into the callee's frame order is a
/// compile-time fact. `FunctionData::param_names` is that order, and
/// `positional_param_count` is where the named ones begin.
///
/// Rejects rather than guesses when anything is not statically known: a name
/// that is not a constant, a callee with no name metadata, a missing or
/// duplicate name, or a parameter with a default the call site omits (the
/// default expression lives in the callee's body, which the VM evaluates on
/// entry — there is nothing to read here).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_named_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    cap_ctx: CaptureCtx<'_>,
    callee_idx: usize,
    base: u8,
    positional_count: usize,
    named_count: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let reject = || Unsupported::Opcode {
        pc,
        op: Opcode::CallNamed,
    };
    let callee = funcs.get(callee_idx).ok_or_else(reject)?;
    if callee_idx == entry as usize || callee.capture_count != 0 {
        return Err(reject());
    }
    let param_count = callee.param_count as usize;
    let declared_positional = callee.positional_param_count as usize;
    if callee.param_names.len() != param_count
        || positional_count != declared_positional
        || positional_count + named_count != param_count
    {
        return Err(reject());
    }

    // Frame order: the positional prefix as written, then each named parameter
    // filled from whichever pair carries its name.
    let mut args: Vec<Option<(ValueId, Ty)>> = vec![None; param_count];
    for (i, slot) in args.iter_mut().enumerate().take(positional_count) {
        // Through `read_value`, so a lambda written as a field of a struct
        // literal becomes a closure: `H { f: |x| x + 1 }` desugars to a named
        // call, and this is where its arguments are read.
        let arg_reg = base.wrapping_add(1).wrapping_add(i as u8);
        *slot = Some(read_value(ssa, insts, sig, funcs, cap_ctx, arg_reg, block, pc)?);
    }
    for pair in 0..named_count {
        let name_reg = base
            .wrapping_add(1)
            .wrapping_add(positional_count as u8)
            .wrapping_add((pair * 2) as u8);
        let value_reg = name_reg.wrapping_add(1);
        let name = ssa.const_str_at(name_reg, block, pc).ok_or_else(reject)?;
        let slot = callee.param_names[declared_positional..]
            .iter()
            .position(|param| &**param == name.as_str())
            .ok_or_else(reject)?
            + declared_positional;
        if args[slot].is_some() {
            return Err(reject());
        }
        args[slot] = Some(read_value(ssa, insts, sig, funcs, cap_ctx, value_reg, block, pc)?);
    }
    let args = args.into_iter().collect::<Option<Vec<_>>>().ok_or_else(reject)?;

    let (dst, ty) = emit_call_with_args(ssa, insts, funcs, entry, sig, callee_idx, args, Opcode::CallNamed, pc)?;
    ssa.write(base, block, (dst, ty));
    Ok(())
}

/// The runtime's closure arity switch (`lkrt::lkclosure`), counting visible
/// parameters and captures together.
pub(crate) const LK_CLOSURE_MAX_ARGS: usize = 8;

/// Reads a register **as a value**, building a closure for it when it names a
/// lambda.
///
/// The one entry point for "I need a value here". A register that names a
/// lambda holds a compile-time reference and no SSA value, and the sites that
/// need one — a container store, an argument, an indirect call — are exactly
/// the sites that reported `ReferenceAsValue`. Materializing *here*, at the
/// consumer, is what keeps a register to one meaning: binding both a reference
/// and a value to it was tried and every mover that carried one and not the
/// other produced a different wrong answer (`docs/aot/aot-gaps-and-lkrt.md`
/// §30).
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_value(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    sig: &mut SigInfer,
    funcs: &[FunctionData],
    cap_ctx: CaptureCtx<'_>,
    reg: u8,
    block: usize,
    pc: usize,
) -> Result<Reg, Unsupported> {
    if let Some(global_ref) = ssa.builtin_ref_at(reg, block)
        && let Some(value) = materialize_closure(ssa, insts, sig, funcs, cap_ctx, &global_ref, block, pc)?
    {
        return Ok(value);
    }
    ssa.read(reg, block, pc)
}

/// Builds a lambda's runtime closure value.
///
/// `None` when the program has not asked for one: a closure that is only built
/// and called stays a compile-time reference and keeps devirtualizing, which is
/// why this is demand-driven rather than uniform.
///
/// The address taken is the *clone*'s (`SigInfer::value_lambdas`), whose
/// signature is all-`Dyn`. The environment travels in the same argument block a
/// `spawn` builds, and the runtime appends it at the call — the order the
/// native signature already declares.
#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize_closure(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    sig: &mut SigInfer,
    funcs: &[FunctionData],
    cap_ctx: CaptureCtx<'_>,
    global_ref: &GlobalRef,
    block: usize,
    pc: usize,
) -> Result<Option<Reg>, Unsupported> {
    let (fidx, captures) = match global_ref {
        GlobalRef::Lambda(fidx) | GlobalRef::UserFn(fidx) => (*fidx, Vec::new()),
        GlobalRef::Closure(fidx, captures) => (*fidx, captures.clone()),
        _ => return Ok(None),
    };
    let Some(&body) = sig.value_lambdas.get(&fidx) else {
        return Ok(None);
    };
    let callee = funcs.get(fidx as usize).ok_or(Unsupported::BadConst { pc })?;
    // A lambda whose environment is *entirely* static references carries
    // nothing at run time, so `MakeClosure` recorded it as a bare `Lambda`
    // (`captures_all_static`) — correct for a call that resolves those
    // references statically, and wrong for a value, whose clone still has that
    // many capture parameters and nothing to fill them with. It read past the
    // end of an empty environment and called whatever it found:
    //
    //     let add = |x| x + 1;
    //     let fs = [|y| add(y) * 10];
    //     fs[0](2)                      // 30 interpreted, "value is not callable" compiled
    //
    // So the environment is rebuilt from the references themselves, which the
    // loop below then materializes one by one.
    let captures = if captures.is_empty() && callee.capture_count > 0 {
        vec![ClosureCapture::StaticRef; callee.capture_count as usize]
    } else {
        captures
    };
    if callee.param_count as usize + captures.len() > LK_CLOSURE_MAX_ARGS {
        return Err(Unsupported::CallShape {
            pc,
            reason: "a closure value with this many parameters and captures is past the runtime's arity switch",
        });
    }
    let env = if captures.is_empty() {
        None
    } else {
        let block_v = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(block_v),
            callee: AbiRef::new("rt", "spawn_args_new"),
            args: Vec::new(),
        });
        // A closure outlives the frame that built it, so a cell's *content*
        // crosses into it — the same snapshot a goroutine takes.
        let site = CaptureSite::new(cap_ctx, body, CaptureMode::Snapshot, block, pc);
        for (k, capture) in captures.iter().enumerate() {
            // A capture whose whole meaning is a *callable reference* — the
            // lambda captured another lambda, and the environment slot carries
            // a dead `0` because the callee resolves it statically. A closure
            // *value* cannot: nothing resolves its environment later, so the
            // reference has to become a value too, recursively.
            //
            // Without this the slot really would carry the `0`, and calling the
            // capture answered "value is not callable" for a function that
            // exists. `fn twice(f) { return |x| f(f(x)); }` is the shape.
            if matches!(capture, ClosureCapture::StaticRef) {
                let Some(referenced) = sig.ref_captures.get(&(fidx, k)).cloned() else {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "a closure value captures a callable this lowering cannot name",
                    });
                };
                let Some((v, ty)) = materialize_closure(ssa, insts, sig, funcs, cap_ctx, &referenced, block, pc)?
                else {
                    // The referenced callable is not a value lambda *yet*: ask
                    // for it the way every other consumer does, so the fixpoint
                    // records the demand and the next pass finds it.
                    return Err(Unsupported::ReferenceAsValue {
                        pc,
                        reg: 0,
                        what: referenced.describe(),
                        lambda: match referenced {
                            GlobalRef::Lambda(f) | GlobalRef::Closure(f, _) | GlobalRef::UserFn(f) => Some(f),
                            _ => None,
                        },
                    });
                };
                let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "spawn_args_push"),
                    args: vec![block_v, boxed],
                });
                continue;
            }
            let (v, ty) = match site.resolve(ssa, insts, sig, capture, k)? {
                Some(resolved) => resolved,
                None => {
                    let ClosureCapture::Cell(cid) = capture else {
                        unreachable!("only `Cell` is left to the call site")
                    };
                    ssa.read_slot(ssa.cell_slot(*cid), block, pc)?
                }
            };
            let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "spawn_args_push"),
                args: vec![block_v, boxed],
            });
        }
        Some(block_v)
    };
    let code = ssa.new_val();
    insts.push(Inst::Const {
        dst: code,
        value: Const::FnAddr(FuncId(body)),
    });
    let env_is_empty = env.is_none();
    let env_ptr = match env {
        Some(block_v) => block_v,
        None => {
            let null = ssa.new_val();
            insts.push(Inst::Const {
                dst: null,
                value: Const::I64(0),
            });
            null
        }
    };
    let params = ssa.new_val();
    insts.push(Inst::Const {
        dst: params,
        value: Const::I64(i64::from(callee.param_count)),
    });
    // Only so `display` prints what the interpreter prints. The *original*
    // index, not the clone's: the clone is this pipeline's bookkeeping and no
    // program can observe it.
    let index = ssa.new_val();
    insts.push(Inst::Const {
        dst: index,
        value: Const::I64(i64::from(fidx)),
    });
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("rt", "closure_new"),
        args: vec![code, env_ptr, params, index],
    });
    ssa.closure_values.insert(dst);
    if env_is_empty {
        ssa.closure_fidx.insert(dst, fidx);
    }
    Ok(Some((dst, Ty::Dyn)))
}

/// `f(args…)` where `f` is an ordinary value: a closure built by
/// [`materialize_closure`], reached through the runtime's arity switch.
pub(crate) fn lower_dyn_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    // Through `read_scalar`, so a carrier unwraps first: a closure that came
    // out of a list is a `Maybe`, and handing the carrier to the runtime made
    // it answer "value is not callable" for a value that is one.
    let callee = read_scalar(ssa, insts, base, block, pc)?;
    lower_dyn_call_to(ssa, insts, callee, base, argc, block, pc)
}

/// [`lower_dyn_call`] with the callee already in hand — for the sites where the
/// register names it rather than holding it, which a capture parameter does.
pub(crate) fn lower_dyn_call_to(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    callee: Reg,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if argc > LK_CLOSURE_MAX_ARGS {
        return Err(Unsupported::CallShape {
            pc,
            reason: "a call through a closure value with this many arguments is past the runtime's arity switch",
        });
    }
    let (callee, callee_ty) = callee;
    let callee = if callee_ty == Ty::Dyn {
        callee
    } else {
        to_dyn_any(ssa, insts, callee, callee_ty, pc)?
    };
    let args = if argc == 0 {
        let null = ssa.new_val();
        insts.push(Inst::Const {
            dst: null,
            value: Const::I64(0),
        });
        null
    } else {
        let block_v = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(block_v),
            callee: AbiRef::new("rt", "spawn_args_new"),
            args: Vec::new(),
        });
        for i in 0..argc {
            let (v, ty) = ssa.read(base.wrapping_add(1).wrapping_add(i as u8), block, pc)?;
            let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "spawn_args_push"),
                args: vec![block_v, boxed],
            });
        }
        block_v
    };
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("rt", "closure_call"),
        args: vec![callee, args],
    });
    ssa.write(base, block, (dst, Ty::Dyn));
    Ok(())
}
