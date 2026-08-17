//! Call opcodes: direct/indirect calls, method dispatch, closure construction.

use super::LowerCtx;
use crate::*;

pub(super) fn lower(
    ctx: &mut LowerCtx<'_>,
    block: usize,
    insts: &mut Vec<Inst>,
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    let ssa = &mut *ctx.ssa;
    let globals = &mut *ctx.globals;
    let sig = &mut *ctx.sig;
    let func = ctx.func;
    let funcs = ctx.funcs;
    let entry = ctx.entry;
    let capture_params = ctx.capture_params;
    let ctx_func_index = ctx.func_index;
    // Where an onward capture (`ClosureCapture::CellParam`) reads from: this
    // function's own hidden trailing parameters.
    let cap_ctx = CaptureCtx {
        params: capture_params,
        index: ctx_func_index,
        param_count: func.param_count as usize,
    };
    match instr.opcode() {
        Opcode::CallMethodK => {
            lower_method_call_k(ssa, insts, globals, func, funcs, entry, sig, instr, block, pc)?;
        }
        Opcode::CallDirect => {
            // Register-window call: `a`=dst register, `b`=callee function index,
            // `c`=argument count; the args occupy registers `[a+1, a+1+c)`. Each
            // argument's observed scalar type refines the callee's parameter type
            // (`sig.param_obs`); disagreeing sites mark the callee polymorphic
            // (`sig.conflict` → whole-module fallback). The result takes the callee's
            // inferred return type, so `f64`/`bool`-returning calls type correctly.
            let callee_idx = instr.b() as usize;
            lower_user_call(
                ssa,
                insts,
                funcs,
                entry,
                sig,
                cap_ctx,
                callee_idx,
                instr.a(),
                instr.c() as usize,
                &[],
                block,
                pc,
            )?;
        }
        // A function value in a register. Usually the compiler's global-table
        // bookkeeping, which is a no-op natively — but also how a call to a
        // function past index 255 is spelled, because `CallDirect` names its
        // target in a byte. The index rides along so the `Call` arm can
        // devirtualize it.
        Opcode::LoadFunction => {
            ssa.builtin_regs
                .insert((block, instr.a()), GlobalRef::UserFn(u32::from(instr.bx())));
        }
        Opcode::MakeClosure => {
            // `a` = dst, `b` = function index, `c` = capture window base. A
            // zero-capture closure is a statically known function reference; a
            // capturing one additionally snapshots the capture window by value
            // (exactly the VM's `capture_values` copy) — the values become
            // hidden trailing arguments at each call. Mutable captures compile
            // to cells (`LoadCellVal`/`StoreCellVal`), which stay unsupported.
            let fidx = instr.b() as usize;
            let callee = funcs.get(fidx).ok_or(Unsupported::BadConst { pc })?;
            if callee.capture_count == 0 {
                ssa.builtin_regs
                    .insert((block, instr.a()), GlobalRef::Lambda(fidx as u32));
                return Ok(());
            }
            let mut captures = Vec::with_capacity(callee.capture_count as usize);
            for k in 0..callee.capture_count {
                let reg = instr.c().wrapping_add(k as u8);
                // The compiler captures locals through upvalue cells (shared
                // mutable boxes); a plain value is captured directly.
                match ssa.builtin_ref_at(reg, block) {
                    Some(GlobalRef::Cell(cid)) => {
                        // A cell whose content is a callable *reference* has no
                        // runtime value to pass: the meaning goes to the callee
                        // through `sig.ref_captures`, and the slot carries a
                        // dead `0` so the ABI arity is unchanged.
                        if let Some(callable) = ssa.cell_refs.get(&cid).cloned() {
                            let key = (fidx as u32, k as usize);
                            if sig.ref_captures.get(&key) != Some(&callable) {
                                sig.ref_captures.insert(key, callable);
                                return Err(Unsupported::TypeMismatch { pc });
                            }
                            captures.push(ClosureCapture::StaticRef);
                            continue;
                        }
                        captures.push(ClosureCapture::Cell(cid));
                        continue;
                    }
                    // A closure nested in a closure captures what its parent
                    // captured; the parent holds that as a capture parameter.
                    Some(GlobalRef::CellParam(k)) => {
                        captures.push(ClosureCapture::CellParam(k));
                        continue;
                    }
                    _ => {}
                }
                let (v, ty) = ssa.read(reg, block, pc)?;
                // Same set as call arguments: scalars and handles pass through,
                // `Maybe`/nil carriers stay out of the function ABI.
                if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                captures.push(ClosureCapture::Value(v, ty));
            }
            // Nothing to pass: this is a plain function reference, which is
            // what lets the list HOFs' typed fast paths accept it.
            let global_ref = if captures.iter().all(|c| matches!(c, ClosureCapture::StaticRef)) {
                GlobalRef::Lambda(fidx as u32)
            } else {
                GlobalRef::Closure(fidx as u32, captures)
            };
            ssa.bind_ref(block, instr.a(), global_ref);
        }
        // `abx(CallNamed, call_base, (named_count << 7) | positional_count)`:
        // the callee sits at `call_base`, the positional arguments follow it,
        // then `named_count` (name, value) pairs. The callee is resolved the
        // same way `Call` does; the arguments are permuted into frame order by
        // name (see `lower_named_call`).
        Opcode::CallNamed => {
            let base = instr.a();
            let payload = instr.bx();
            let positional_count = (payload & 0x7F) as usize;
            let named_count = (payload >> 7) as usize;
            let callee_idx = match ssa.builtin_ref_at(base, block) {
                Some(GlobalRef::Lambda(fidx)) | Some(GlobalRef::UserFn(fidx)) => fidx as usize,
                // A stdlib member called by name — `regex.replace(s, pattern: p,
                // replacement: r)`. The names come from the member's own row
                // rather than from a user function's metadata; everything else
                // (the permutation, the rejection rules) is the same problem.
                Some(GlobalRef::ModuleFn(module, name)) => {
                    return lower_named_module_call(
                        ssa,
                        insts,
                        &module,
                        &name,
                        base,
                        positional_count,
                        named_count,
                        block,
                        pc,
                    );
                }
                _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            };
            lower_named_call(
                ssa,
                insts,
                funcs,
                entry,
                sig,
                callee_idx,
                base,
                positional_count,
                named_count,
                block,
                pc,
            )?;
        }
        Opcode::Call => {
            // Register-window call: `a` = window base (the callee slot), `c` =
            // positional count, args at `[a+1, a+1+c)`. Only calls whose callee
            // register holds a recognized global ref lower; everything else
            // (closures, runtime values) rejects.
            let base = instr.a();
            match ssa.builtin_ref_at(base, block) {
                Some(GlobalRef::Builtin(Builtin::Spawn)) => {
                    lower_spawn(
                        ssa,
                        insts,
                        funcs,
                        entry,
                        sig,
                        cap_ctx,
                        base,
                        instr.c() as usize,
                        block,
                        pc,
                    )?;
                }
                Some(GlobalRef::Builtin(Builtin::MergeFields)) => {
                    lower_merge_fields(ssa, insts, base, instr.c() as usize, block, pc)?;
                }
                Some(GlobalRef::Builtin(Builtin::MakeStruct)) => {
                    lower_make_struct(ssa, insts, sig, base, instr.c() as usize, block, pc)?;
                }
                Some(GlobalRef::Builtin(Builtin::CallMethod)) => {
                    if instr.c() != 3 {
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                    }
                    lower_method_call(ssa, insts, globals, base, block, pc)?;
                }
                Some(GlobalRef::Builtin(builtin)) => {
                    // Auto-Display (plan J1): a struct-instance print argument
                    // with a registered `show` prints its result, like the VM's
                    // `try_runtime_display_show`.
                    //
                    // The converted value is a temporary for this print's own
                    // argument read only: the call-window register keeps its
                    // original SSA definition, so a later read in this block or
                    // a successor still sees the struct, not its `show` string.
                    let mut display_saved: Vec<(u8, (ValueId, Ty))> = Vec::new();
                    if matches!(builtin, Builtin::Println | Builtin::Print) {
                        for i in 0..instr.c() as usize {
                            let reg = base.wrapping_add(1).wrapping_add(i as u8);
                            if let Ok((v, ty)) = ssa.read(reg, block, pc) {
                                let (nv, nty) = apply_display_show(ssa, insts, funcs, entry, sig, v, ty, pc)?;
                                if nv != v {
                                    display_saved.push((reg, (v, ty)));
                                    ssa.write(reg, block, (nv, nty));
                                }
                            }
                        }
                    }
                    let printed = lower_builtin_call(ssa, insts, globals, builtin, base, instr.c() as usize, block, pc);
                    for (reg, original) in display_saved {
                        ssa.write(reg, block, original);
                    }
                    printed?;
                }
                Some(GlobalRef::ModuleFn(module, name)) => {
                    // `iter.map(xs, f)`, `iter.take(xs, n)`, … are the
                    // module-function spellings of the list methods (the VM
                    // routes both through the same core_methods) — forward
                    // to the same lowering with the receiver at `base+1`.
                    let argc = instr.c() as usize;
                    if let Some(method) = forwards_to_method(module.as_str(), &name)
                        && argc >= 1
                    {
                        let (receiver, receiver_ty) = ssa.read(base.wrapping_add(1), block, pc)?;
                        // The HOF spellings reuse the lambda-aware method
                        // path (the lambda register offset matches with the
                        // window base shifted one slot right).
                        if matches!(method, "map" | "filter" | "reduce") {
                            if matches!(receiver_ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn)
                                && let Some(result) = lower_list_hof_k(
                                    ssa,
                                    insts,
                                    funcs,
                                    entry,
                                    sig,
                                    receiver,
                                    receiver_ty,
                                    method,
                                    base.wrapping_add(1),
                                    argc - 1,
                                    block,
                                    pc,
                                )?
                            {
                                ssa.write(base, block, result);
                                return Ok(());
                            }
                            return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                        }
                        let mut args = Vec::with_capacity(argc - 1);
                        for i in 0..argc - 1 {
                            args.push(ssa.read(base.wrapping_add(2).wrapping_add(i as u8), block, pc)?);
                        }
                        let result = lower_method_dispatch(
                            ssa,
                            insts,
                            globals,
                            receiver,
                            receiver_ty,
                            method,
                            &args,
                            block,
                            pc,
                        )?;
                        ssa.write(base, block, result);
                        return Ok(());
                    }
                    lower_module_call(ssa, insts, &module, &name, base, argc, block, pc)?;
                }
                // An indirect call through a statically known capture-free
                // closure devirtualizes to a direct call (same register-window
                // layout: result at `base`, args at `[base+1, base+1+c)`).
                Some(GlobalRef::Lambda(fidx)) => {
                    lower_user_call(
                        ssa,
                        insts,
                        funcs,
                        entry,
                        sig,
                        cap_ctx,
                        fidx as usize,
                        base,
                        instr.c() as usize,
                        &[],
                        block,
                        pc,
                    )?;
                }
                // A capturing closure devirtualizes the same way; each cell
                // capture resolves to the cell's content *at this call site*
                // (the VM's shared-mutable-cell semantics) and is appended as
                // a hidden trailing argument.
                Some(GlobalRef::Closure(fidx, captures)) => {
                    let site = CaptureSite::new(cap_ctx, fidx, CaptureMode::Share, block, pc);
                    let mut resolved = Vec::with_capacity(captures.len());
                    // A capture the body *assigns* to travels as a runtime cell
                    // this call site seeds and reads back afterwards
                    // (`SigInfer::cell_captures`); one it only reads keeps
                    // passing as a plain value.
                    let mut writebacks: Vec<(u32, ValueId, Ty)> = Vec::new();
                    for (k, capture) in captures.iter().enumerate() {
                        let (v, ty) = match (site.resolve(ssa, insts, sig, capture, k)?, capture) {
                            (Some(resolved), _) => resolved,
                            (None, ClosureCapture::Cell(cid)) => {
                                let slot = ssa.cell_slot(*cid);
                                let (cur, cur_ty) = ssa.read_slot(slot, block, pc)?;
                                if sig.cell_captures.contains(&(fidx, k)) {
                                    // What the callee's reads of this cell
                                    // unbox to. Recorded here because this is
                                    // where the type is known; the callee never
                                    // sees anything but the pointer.
                                    let content =
                                        join_cell_content(sig.cell_capture_tys.get(&(fidx, k)).copied(), cur_ty);
                                    sig.cell_capture_tys.insert((fidx, k), content);
                                    let boxed = to_dyn_any(ssa, insts, cur, cur_ty, pc)?;
                                    let cell = ssa.new_val();
                                    insts.push(Inst::Call {
                                        dst: Some(cell),
                                        callee: AbiRef::new("rt", "cell_new"),
                                        args: vec![boxed],
                                    });
                                    writebacks.push((*cid, cell, content));
                                    (cell, Ty::Cell)
                                } else {
                                    (cur, cur_ty)
                                }
                            }
                            (None, _) => unreachable!("only `Cell` is left to the call site"),
                        };
                        if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                            return Err(Unsupported::TypeMismatch { pc });
                        }
                        resolved.push((v, ty));
                    }
                    lower_user_call(
                        ssa,
                        insts,
                        funcs,
                        entry,
                        sig,
                        cap_ctx,
                        fidx as usize,
                        base,
                        instr.c() as usize,
                        &resolved,
                        block,
                        pc,
                    )?;
                    // Re-sync the parent's tracked cell content from the cell
                    // the callee wrote through.
                    for (cid, cell, content) in writebacks {
                        let cur = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(cur),
                            callee: AbiRef::new("rt", "cell_get"),
                            args: vec![cell],
                        });
                        // Back under the type the callee read through, so the
                        // caller's own later uses stay typed as well.
                        let (value, ty) = match unbox_cell_value(ssa, insts, cur, content) {
                            Some(value) if content != Ty::Dyn => (value, content),
                            _ => (cur, Ty::Dyn),
                        };
                        let slot = ssa.cell_slot(cid);
                        ssa.write_slot(slot, block, (value, ty));
                    }
                }
                // A plain function value, called through the register the
                // bytecode had to load it into. No captures: a `fn` has none.
                Some(GlobalRef::UserFn(fidx)) => {
                    lower_user_call(
                        ssa,
                        insts,
                        funcs,
                        entry,
                        sig,
                        cap_ctx,
                        fidx as usize,
                        base,
                        instr.c() as usize,
                        &[],
                        block,
                        pc,
                    )?;
                }
                Some(GlobalRef::Module(_))
                | Some(GlobalRef::UserModule(_))
                | Some(GlobalRef::ArgList(_))
                | Some(GlobalRef::Cell(_))
                | Some(GlobalRef::CellParam(_))
                | None => {
                    return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                }
            }
        }
        op => return Err(Unsupported::Opcode { pc, op }),
    }
    Ok(())
}
