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
                callee_idx,
                instr.a(),
                instr.c() as usize,
                &[],
                block,
                pc,
            )?;
        }
        // Direct calls address the callee by index, so the loaded function
        // value itself only flows into the compiler's global-table storage
        // (`SetGlobal`), which stays a no-op.
        Opcode::LoadFunction => {
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::UserFn);
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
                if let Some(GlobalRef::Cell(cid)) = ssa.builtin_regs.get(&(block, reg)) {
                    captures.push(ClosureCapture::Cell(*cid));
                    continue;
                }
                let (v, ty) = ssa.read(reg, block, pc)?;
                // Same set as call arguments: scalars and handles pass through,
                // `Maybe`/nil carriers stay out of the function ABI.
                if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                captures.push(ClosureCapture::Value(v, ty));
            }
            ssa.builtin_regs
                .insert((block, instr.a()), GlobalRef::Closure(fidx as u32, captures));
        }
        Opcode::Call => {
            // Register-window call: `a` = window base (the callee slot), `c` =
            // positional count, args at `[a+1, a+1+c)`. Only calls whose callee
            // register holds a recognized global ref lower; everything else
            // (closures, runtime values) rejects.
            let base = instr.a();
            match ssa.builtin_ref_at(base, block) {
                Some(GlobalRef::Builtin(Builtin::TryCall)) => {
                    lower_try_call(ssa, insts, funcs, entry, sig, base, instr.c() as usize, block, pc)?;
                }
                Some(GlobalRef::Builtin(Builtin::Spawn)) => {
                    lower_spawn(ssa, insts, funcs, entry, sig, base, instr.c() as usize, block, pc)?;
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
                    if matches!(builtin, Builtin::Println | Builtin::Print) {
                        for i in 0..instr.c() as usize {
                            let reg = base.wrapping_add(1).wrapping_add(i as u8);
                            if let Ok((v, ty)) = ssa.read(reg, block, pc) {
                                let (nv, nty) = apply_display_show(ssa, insts, funcs, entry, sig, v, ty, pc)?;
                                if nv != v {
                                    ssa.write(reg, block, (nv, nty));
                                }
                            }
                        }
                    }
                    lower_builtin_call(ssa, insts, globals, builtin, base, instr.c() as usize, block, pc)?;
                }
                Some(GlobalRef::ModuleFn(module, name)) => {
                    // `iter.map(xs, f)`, `iter.take(xs, n)`, … are the
                    // module-function spellings of the list methods (the VM
                    // routes both through the same core_methods) — forward
                    // to the same lowering with the receiver at `base+1`.
                    let argc = instr.c() as usize;
                    if matches!(module.as_str(), "iter" | "stream")
                        && method_role(&name).is_some_and(|role| role.forward)
                        && argc >= 1
                    {
                        let (receiver, receiver_ty) = ssa.read(base.wrapping_add(1), block, pc)?;
                        // The HOF spellings reuse the lambda-aware method
                        // path (the lambda register offset matches with the
                        // window base shifted one slot right).
                        if matches!(name.as_str(), "map" | "filter" | "reduce") {
                            if matches!(receiver_ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn)
                                && let Some(result) = lower_list_hof_k(
                                    ssa,
                                    insts,
                                    funcs,
                                    entry,
                                    sig,
                                    receiver,
                                    receiver_ty,
                                    &name,
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
                        let result =
                            lower_method_dispatch(ssa, insts, globals, receiver, receiver_ty, &name, &args, block, pc)?;
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
                    let mut resolved = Vec::with_capacity(captures.len());
                    for capture in &captures {
                        let (v, ty) = match capture {
                            ClosureCapture::Cell(cid) => {
                                let slot = ssa.cell_slot(*cid);
                                ssa.read_slot(slot, block, pc)?
                            }
                            ClosureCapture::Value(v, ty) => (*v, *ty),
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
                        fidx as usize,
                        base,
                        instr.c() as usize,
                        &resolved,
                        block,
                        pc,
                    )?;
                }
                Some(GlobalRef::Module(_))
                | Some(GlobalRef::UserModule(_))
                | Some(GlobalRef::UserFn)
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
