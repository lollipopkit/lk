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
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if argc != 1 {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    }
    let arg_reg = base.wrapping_add(1);
    let (fidx, caps) = match ssa.builtin_ref_at(arg_reg, block) {
        Some(GlobalRef::Closure(f, caps)) => (f as usize, caps),
        Some(GlobalRef::Lambda(f)) => (f as usize, Vec::new()),
        _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
    };
    if fidx >= funcs.len()
        || fidx == entry as usize
        || funcs[fidx].param_count != 0
        || caps.len() != funcs[fidx].capture_count as usize
        || caps.len() > 4
    {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
        for (k, capture) in caps.iter().enumerate() {
            let (v, ty) = match capture {
                ClosureCapture::Cell(cid) => {
                    let slot = ssa.cell_slot(*cid);
                    ssa.read_slot(slot, block, pc)?
                }
                ClosureCapture::Value(v, ty) => (*v, *ty),
            };
            let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "spawn_args_push"),
                args: vec![b, boxed],
            });
            let want = sig.observe_param(fidx, k, Ty::Dyn);
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
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    }
    let (bv, bty) = ssa.read(base.wrapping_add(1), block, pc)?;
    let (ov, oty) = ssa.read(base.wrapping_add(2), block, pc)?;
    let base_map = to_dyn_map_handle(ssa, insts, bv, bty, pc)?;
    let overlay_map = to_dyn_map_handle(ssa, insts, ov, oty, pc)?;
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("map_h", "str_dyn_merge"),
        args: vec![base_map, overlay_map],
    });
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
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    }
    let name_reg = base.wrapping_add(1);
    let type_name = {
        let nv = ssa.read(name_reg, block, pc).ok().map(|(v, _)| v);
        nv.and_then(|v| ssa.const_strs.get(&v).cloned())
            .or_else(|| ssa.reg_const_str(name_reg, block))
    }
    .ok_or(Unsupported::Opcode { pc, op: Opcode::Call })?;
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

/// `try$call(closure)` — the try/catch desugar's protected call (plan G).
/// The body closure lowers as a normal `Dyn`-returning function; the call
/// site emits [`Inst::TryCall`], which codegen expands into `rt.try_push` +
/// `_setjmp` + a conditional body call joining into the `[ok, value]` dyn
/// list the desugared destructuring consumes. Mutable captures (`UpvalCell`)
/// materialize as *runtime cells* across the boundary: the body writes
/// through the shared slot, and the caller re-reads it afterwards, so the
/// SSA-tracked cell world stays coherent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_try_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if argc != 1 {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    }
    let arg_reg = base.wrapping_add(1);
    let (fidx, caps) = match ssa.builtin_ref_at(arg_reg, block) {
        Some(GlobalRef::Closure(f, caps)) => (f as usize, caps),
        Some(GlobalRef::Lambda(f)) => (f as usize, Vec::new()),
        _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
    };
    if fidx >= funcs.len()
        || fidx == entry as usize
        || funcs[fidx].param_count != 0
        || caps.len() != funcs[fidx].capture_count as usize
    {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    }
    let mut args = Vec::with_capacity(caps.len());
    let mut cell_writebacks: Vec<(u32, ValueId)> = Vec::new();
    for (k, capture) in caps.iter().enumerate() {
        let (v, ty) = match capture {
            ClosureCapture::Cell(cid) => {
                // Seed a runtime cell with the current content; the body
                // mutates through it, the write-back below re-syncs.
                let slot = ssa.cell_slot(*cid);
                let (cur, cur_ty) = ssa.read_slot(slot, block, pc)?;
                let boxed = to_dyn_any(ssa, insts, cur, cur_ty, pc)?;
                let cell = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(cell),
                    callee: AbiRef::new("rt", "cell_new"),
                    args: vec![boxed],
                });
                cell_writebacks.push((*cid, cell));
                (cell, Ty::Cell)
            }
            ClosureCapture::Value(v, ty) => (*v, *ty),
        };
        let want = sig.observe_param(fidx, k, ty);
        args.push(coerce_arg(ssa, insts, v, ty, want, pc)?);
    }
    // The body's return crosses the boundary boxed (`dyn_rets`, the same
    // retriable convergence the dyn HOF family uses).
    if !sig.dyn_rets.contains(&(fidx as u32)) {
        sig.dyn_rets.insert(fidx as u32);
        return Err(Unsupported::TypeMismatch { pc });
    }
    if sig.ret_types.get(fidx).copied() != Some(Ty::Dyn) {
        return Err(Unsupported::TypeMismatch { pc });
    }
    let dst = ssa.new_val();
    insts.push(Inst::TryCall {
        dst,
        func: FuncId(fidx as u32),
        args,
    });
    for (cid, cell) in cell_writebacks {
        let cur = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(cur),
            callee: AbiRef::new("rt", "cell_get"),
            args: vec![cell],
        });
        let slot = ssa.cell_slot(cid);
        ssa.write_slot(slot, block, (cur, Ty::Dyn));
    }
    ssa.write(base, block, (dst, Ty::ListDyn));
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
    if captures.len() != funcs[callee_idx].capture_count as usize {
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
                let clone = sig.param_obs.len() as u32;
                let env_total: usize = identity.iter().flatten().map(|id| id.captures as usize).sum();
                sig.param_obs.push(vec![
                    None;
                    funcs[callee_idx].param_count as usize
                        + env_total
                        + funcs[callee_idx].capture_count as usize
                ]);
                sig.ret_types.push(sig.ret_types[callee_idx]);
                sig.ret_known
                    .push(sig.ret_known.get(callee_idx).copied().unwrap_or(false));
                sig.ret_closures.push(None);
                sig.ret_closure_poisoned.push(false);
                sig.lambda_params.push(identity.clone());
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
        ssa.builtin_regs.insert((block, dst_reg), GlobalRef::Closure(lf, caps));
        return Ok(());
    }
    // Tier 1 bridge call (`docs/llvm/tier1-hybrid.md`): the callee runs on the
    // embedded VM. Arguments must match the recorded scalar marshaling types;
    // the destination register binds as `Dyn` (v2: the bridge returns an
    // `LkDyn` by value) — codegen degrades a never-read destination back to
    // the void bridge call, so statement-position calls stay v1-shaped.
    if let Some(param_tys) = sig.vm_functions.get(&(callee_idx as u32)).cloned() {
        if !captures.is_empty() {
            return Err(Unsupported::Opcode {
                pc,
                op: Opcode::CallDirect,
            });
        }
        let mut args = Vec::with_capacity(argc);
        for (i, param_ty) in param_tys.iter().enumerate().take(argc) {
            let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
            let (aval, aty) = read_scalar(ssa, insts, arg_reg, block, pc)?;
            if aty != *param_ty {
                return Err(Unsupported::TypeMismatch { pc });
            }
            args.push(aval);
        }
        let dst = ssa.new_val();
        insts.push(Inst::CallVm {
            dst: Some(dst),
            func: FuncId(callee_idx as u32),
            args,
        });
        ssa.builtin_regs.remove(&(block, dst_reg));
        ssa.write(dst_reg, block, (dst, Ty::Dyn));
        return Ok(());
    }
    let mut args = Vec::with_capacity(argc + captures.len());
    let mut env_args: Vec<(ValueId, Ty)> = Vec::new();
    for (i, id) in identity.iter().enumerate() {
        let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
        match *id {
            // Erased zero-capture lambda: nothing is passed at runtime.
            Some(LambdaIdentity { captures: 0, .. }) => continue,
            // Erased capturing closure: its environment (resolved to current
            // cell contents at this call site) travels as hidden trailing
            // arguments, in parameter order.
            Some(_) => {
                let Some(GlobalRef::Closure(_, caps)) = ssa.builtin_ref_at(arg_reg, block) else {
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                };
                for capture in &caps {
                    let (v, ty) = match capture {
                        ClosureCapture::Cell(cid) => {
                            let slot = ssa.cell_slot(*cid);
                            ssa.read_slot(slot, block, pc)?
                        }
                        ClosureCapture::Value(v, ty) => (*v, *ty),
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
        let (aval, aty) = ssa.read(arg_reg, block, pc)?;
        let want = sig.observe_param(callee_idx, i, aty);
        args.push(coerce_arg(ssa, insts, aval, aty, want, pc)?);
    }
    // Hidden trailing arguments, in signature order: the erased closures'
    // environment values first, then the callee's own captures. Their types
    // refine the same monomorphization lattice as visible parameters.
    for (k, &(ev, ety)) in env_args.iter().enumerate() {
        let want = sig.observe_param(callee_idx, argc + k, ety);
        args.push(coerce_arg(ssa, insts, ev, ety, want, pc)?);
    }
    let env_total = env_args.len();
    for (k, &(cval, cty)) in captures.iter().enumerate() {
        let want = sig.observe_param(callee_idx, argc + env_total + k, cty);
        args.push(coerce_arg(ssa, insts, cval, cty, want, pc)?);
    }
    let ret = sig.ret_types.get(callee_idx).copied().unwrap_or(Ty::I64);
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
        ssa.write(dst_reg, block, (dst, ret));
    }
    Ok(())
}
