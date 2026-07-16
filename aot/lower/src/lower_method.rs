use super::*;

/// Lowers `__lk_call_method(receiver, name, args_list)` — the compiler's
/// generic method dispatch. The method name must be a compile-time constant
/// and the argument pack must be a lowering-tracked [`GlobalRef::ArgList`];
/// dispatch is per (receiver type, method name, argument types), each entry
/// mapped to a typed lkrt ABI call with VM-exact semantics.
pub(crate) fn lower_method_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    base: u8,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let (receiver, receiver_ty) = ssa.read(base.wrapping_add(1), block, pc)?;
    let name_reg = base.wrapping_add(2);
    let name = {
        let name_v = ssa.read(name_reg, block, pc).ok().map(|(v, _)| v);
        name_v
            .and_then(|v| ssa.const_strs.get(&v).cloned())
            .or_else(|| ssa.reg_const_str(name_reg, block))
    };
    let Some(name) = name else {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    };
    let args = match ssa.builtin_regs.get(&(block, base.wrapping_add(3))) {
        Some(GlobalRef::ArgList(elems)) => elems.clone(),
        _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
    };
    let result = lower_method_dispatch(ssa, insts, globals, receiver, receiver_ty, &name, &args, block, pc)?;
    ssa.write(base, block, result);
    Ok(())
}

/// `CallMethodK` — the boxing-free method-call opcode: receiver at the window
/// base, args in the window, method name a string constant. Shares the
/// per-(receiver type, method) dispatch with the legacy
/// `__lk_call_method` shape.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_method_call_k(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    func: &FunctionData,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    instr: &Instr,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let base = instr.a();
    let name = func
        .consts
        .strings
        .get(instr.b() as usize)
        .ok_or(Unsupported::BadConst { pc })?
        .clone();
    let (receiver, receiver_ty) = ssa.read(base, block, pc)?;
    let argc = instr.c() as usize;
    // A boxed Dyn receiver unwraps through the as_list guard for list-only
    // method names (a non-list tag aborts — the VM's method-on-wrong-type is
    // a loud error too). Names shared with str/map receivers stay boxed.
    let role = method_role(&name);
    let (receiver, receiver_ty) = if receiver_ty == Ty::Dyn && role.is_some_and(|role| role.unbox_list) {
        let unboxed = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(unboxed),
            callee: AbiRef::new("dyn", "as_list"),
            args: vec![receiver],
        });
        (unboxed, Ty::ListDyn)
    } else if receiver_ty == Ty::Dyn && role.is_some_and(|role| role.unbox_map) {
        // Map-only method names unbox through the as_map guard (a parsed
        // json/yaml value flows as Dyn); `get` stays ambiguous (lists have
        // it too) and keeps rejecting.
        let unboxed = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(unboxed),
            callee: AbiRef::new("dyn", "as_map"),
            args: vec![receiver],
        });
        (unboxed, Ty::MapStrDyn)
    } else {
        (receiver, receiver_ty)
    };
    // Trait method dispatch (plan J1): a receiver with known `NewObject`
    // provenance devirtualizes to a direct call of the registered impl; a
    // boxed receiver dispatches at runtime over the arena type marks.
    if let Some(result) = lower_trait_method_k(
        ssa,
        insts,
        funcs,
        entry,
        sig,
        receiver,
        receiver_ty,
        &name,
        base,
        argc,
        block,
        pc,
    )? {
        ssa.write(base, block, result);
        return Ok(());
    }
    // List HOF with a compiled zero-capture lambda callback (fn-pointer ABI):
    // handled before the generic argument reads, because the lambda register
    // carries a `GlobalRef::Lambda`, not an SSA value.
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
            base,
            argc,
            block,
            pc,
        )?
    {
        ssa.write(base, block, result);
        return Ok(());
    }
    let mut args = Vec::with_capacity(argc);
    for i in 0..argc {
        args.push(ssa.read(base.wrapping_add(1).wrapping_add(i as u8), block, pc)?);
    }
    let result = lower_method_dispatch(ssa, insts, globals, receiver, receiver_ty, &name, &args, block, pc)?;
    ssa.write(base, block, result);
    Ok(())
}

/// Trait-method dispatch for `CallMethodK` (plan J1). Two shapes:
///
///  - **Static devirtualization**: the receiver is a `MapStrDyn` whose
///    `NewObject` provenance names a type with a registered `(type, method)`
///    impl — a plain direct call (`self` first, then the window arguments),
///    through the same monomorphization lattice as user calls.
///  - **Runtime dispatch**: the receiver is boxed (`Dyn` — a struct instance
///    that flowed through a mixed list or a `Dyn` parameter) and the method
///    name has registered impls — [`Inst::TraitDispatch`] reads the arena
///    type mark and calls the matching impl. Every arm is forced to the
///    uniform boxed signature (`Dyn` self via the parameter lattice, `Dyn`
///    return via `dyn_rets` — both retriable discoveries).
///
/// Returns `Ok(None)` when neither shape applies (generic dispatch decides).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_trait_method_k(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    receiver: ValueId,
    receiver_ty: Ty,
    name: &str,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<Option<(ValueId, Ty)>, Unsupported> {
    if receiver_ty == Ty::MapStrDyn
        && let Some(type_name) = ssa.struct_types.get(&receiver).cloned()
        && let Some(&fidx) = sig.traits.impls.get(&(type_name, name.to_string()))
    {
        let mut call_args = Vec::with_capacity(argc + 1);
        call_args.push((receiver, Ty::MapStrDyn));
        for i in 0..argc {
            call_args.push(ssa.read(base.wrapping_add(1).wrapping_add(i as u8), block, pc)?);
        }
        return emit_trait_call(ssa, insts, funcs, entry, sig, fidx as usize, call_args, pc).map(Some);
    }
    if receiver_ty == Ty::Dyn
        && argc == 0
        && let Some(arms) = sig.traits.methods.get(name).cloned()
        && !arms.is_empty()
    {
        let mut retry = false;
        for &(_, fidx) in &arms {
            let f = fidx as usize;
            if f >= funcs.len()
                || fidx == entry
                || funcs[f].param_count != 1
                || funcs[f].capture_count != 0
                || sig.specialized.get(f).copied().unwrap_or(false)
            {
                return Err(Unsupported::TypeMismatch { pc });
            }
            if let Some(flag) = sig.plain_called.get_mut(f) {
                *flag = true;
            }
            sig.observe_param(f, 0, Ty::Dyn);
            if !sig.dyn_rets.contains(&fidx) {
                sig.dyn_rets.insert(fidx);
                retry = true;
            }
            if sig.ret_types.get(f).copied() != Some(Ty::Dyn) {
                retry = true;
            }
        }
        // Boxed-signature discoveries converge through the fixpoint like
        // every other retriable widening.
        if retry {
            return Err(Unsupported::TypeMismatch { pc });
        }
        let dst = ssa.new_val();
        insts.push(Inst::TraitDispatch {
            dst,
            self_arg: receiver,
            arms: arms.iter().map(|&(tid, f)| (tid, FuncId(f))).collect(),
        });
        return Ok(Some((dst, Ty::Dyn)));
    }
    Ok(None)
}

/// Emits a devirtualized trait-impl call (`self` is the first argument),
/// refining the callee's signature through the shared parameter lattice.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_trait_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    fidx: usize,
    call_args: Vec<(ValueId, Ty)>,
    pc: usize,
) -> Result<(ValueId, Ty), Unsupported> {
    if fidx >= funcs.len()
        || fidx as u32 == entry
        || funcs[fidx].param_count as usize != call_args.len()
        || funcs[fidx].capture_count != 0
    {
        return Err(Unsupported::Opcode {
            pc,
            op: Opcode::CallMethodK,
        });
    }
    if sig.specialized.get(fidx).copied().unwrap_or(false) {
        sig.conflict = true;
        return Err(Unsupported::TypeMismatch { pc });
    }
    if let Some(flag) = sig.plain_called.get_mut(fidx) {
        *flag = true;
    }
    let mut args = Vec::with_capacity(call_args.len());
    for (i, (v, ty)) in call_args.into_iter().enumerate() {
        let want = sig.observe_param(fidx, i, ty);
        args.push(coerce_arg(ssa, insts, v, ty, want, pc)?);
    }
    let ret = sig.ret_types.get(fidx).copied().unwrap_or(Ty::I64);
    if ret == Ty::Nil {
        insts.push(Inst::CallFn {
            dst: None,
            func: FuncId(fidx as u32),
            args,
        });
        let nil = ssa.new_val();
        insts.push(Inst::Const {
            dst: nil,
            value: Const::Nil,
        });
        return Ok((nil, Ty::Nil));
    }
    let dst = ssa.new_val();
    insts.push(Inst::CallFn {
        dst: Some(dst),
        func: FuncId(fidx as u32),
        args,
    });
    Ok((dst, ret))
}

/// The VM's auto-Display (`try_runtime_display_show`): `print`/`println`
/// formatting and string interpolation call a struct instance's registered
/// `show` method. Mirrors it in display contexts: an operand with struct
/// provenance and a `(type, "show")` impl is replaced by that call's result
/// before generic display conversion.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_display_show(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<(ValueId, Ty), Unsupported> {
    if ty == Ty::MapStrDyn
        && let Some(type_name) = ssa.struct_types.get(&v).cloned()
        && let Some(&fidx) = sig.traits.impls.get(&(type_name, "show".to_string()))
    {
        return emit_trait_call(ssa, insts, funcs, entry, sig, fidx as usize, vec![(v, ty)], pc);
    }
    Ok((v, ty))
}

/// `xs.map(|x| …)` / `filter` / `reduce(init, |acc, x| …)` with a
/// zero-capture lambda: the compiled `@lk_fn_N` address is passed to an lkrt
/// fold helper. Three ABI families, chosen from the receiver's element type
/// and the lambda's converged signature:
///  - `i64` (typed fast path, `i64 → i64`/`Bool`, `(i64, i64) → i64`);
///  - `str` (`str → str`/`Bool` — `words.map(|w| w.lower())`);
///  - boxed `dyn` (everything else): the receiver converts to a dyn list,
///    the lambda's parameters seed `Dyn` and its returns box (`dyn_rets`),
///    so one compiled body serves runtime-polymorphic call sites.
///
/// The lambda's signature converges through the same monomorphization
/// lattice as direct calls. Returns `Ok(None)` when the shape doesn't apply
/// (the generic path then rejects loudly — never a silent semantic change).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_list_hof_k(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    receiver: ValueId,
    receiver_ty: Ty,
    name: &str,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<Option<Reg>, Unsupported> {
    let lambda_at = |ssa: &Ssa, reg: u8| match ssa.builtin_regs.get(&(block, reg)) {
        Some(GlobalRef::Lambda(fidx)) => Some(*fidx as usize),
        _ => None,
    };
    let elem = match receiver_ty {
        Ty::ListI64 => Ty::I64,
        Ty::ListF64 => Ty::F64,
        Ty::ListStr => Ty::Str,
        Ty::ListDyn => Ty::Dyn,
        _ => return Ok(None),
    };
    let seed_params = |sig: &mut SigInfer, fidx: usize, arity: usize, ty: Ty| {
        for i in 0..arity {
            sig.observe_param(fidx, i, ty);
        }
    };
    // The dyn family: convert the receiver, seed `Dyn` parameters; `map`/
    // `reduce` callbacks must *return* boxed values, so the lambda joins
    // `dyn_rets` (a fresh entry re-runs the fixpoint with boxed returns).
    let dyn_list_of = |ssa: &mut Ssa, insts: &mut Vec<Inst>, receiver: ValueId| -> Result<ValueId, Unsupported> {
        to_dyn_list_handle(ssa, insts, receiver, receiver_ty, pc)
    };
    match (name, argc) {
        ("map" | "filter", 1) => {
            let Some(fidx) = lambda_at(ssa, base.wrapping_add(1)) else {
                return Ok(None);
            };
            if fidx >= funcs.len() || fidx == entry as usize || funcs[fidx].param_count != 1 {
                return Err(Unsupported::Opcode {
                    pc,
                    op: Opcode::CallMethodK,
                });
            }
            let is_filter = name == "filter";
            // Typed fast paths only while nothing has widened the lambda.
            let widened = sig.dyn_rets.contains(&(fidx as u32))
                || sig
                    .param_obs
                    .get(fidx)
                    .is_some_and(|p| p.first().copied().flatten() == Some(Ty::Dyn));
            let family = match elem {
                Ty::I64 | Ty::Str if !widened => elem,
                _ => Ty::Dyn,
            };
            let fnaddr = ssa.new_val();
            insts.push(Inst::Const {
                dst: fnaddr,
                value: Const::FnAddr(FuncId(fidx as u32)),
            });
            match family {
                Ty::I64 | Ty::Str => {
                    seed_params(sig, fidx, 1, family);
                    if sig.param_ty(fidx, 0) != family {
                        // Joined wider by another call site: re-route through
                        // the dyn family on the re-run.
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let want_ret = if is_filter { Ty::Bool } else { family };
                    if sig.ret_types.get(fidx).copied() != Some(want_ret) {
                        // Transiently wrong before the fixpoint converges; a
                        // dyn-boxable mismatch re-routes the *map* through the
                        // dyn family (`|x| tostr(x)` over ints); filter's Bool
                        // is a hard requirement.
                        if !is_filter
                            && sig.ret_known.get(fidx).copied().unwrap_or(false)
                            && sig
                                .ret_types
                                .get(fidx)
                                .copied()
                                .is_some_and(|t| t != want_ret && dyn_boxable_ty(t))
                        {
                            sig.dyn_rets.insert(fidx as u32);
                        }
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let hof: &'static str = match (family, is_filter) {
                        (Ty::I64, false) => "i64_map_fn",
                        (Ty::I64, true) => "i64_filter_fn",
                        (_, false) => "str_map_fn",
                        (_, true) => "str_filter_fn",
                    };
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("list_h", hof),
                        args: vec![receiver, fnaddr],
                    });
                    Ok(Some((dst, receiver_ty)))
                }
                _ => {
                    seed_params(sig, fidx, 1, Ty::Dyn);
                    if sig.param_ty(fidx, 0) != Ty::Dyn {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    if is_filter {
                        if sig.ret_types.get(fidx).copied() != Some(Ty::Bool) {
                            return Err(Unsupported::TypeMismatch { pc });
                        }
                    } else {
                        if !sig.dyn_rets.contains(&(fidx as u32)) {
                            sig.dyn_rets.insert(fidx as u32);
                            return Err(Unsupported::TypeMismatch { pc });
                        }
                        if sig.ret_types.get(fidx).copied() != Some(Ty::Dyn) {
                            return Err(Unsupported::TypeMismatch { pc });
                        }
                    }
                    let list = dyn_list_of(ssa, insts, receiver)?;
                    let hof = if is_filter { "dyn_filter_fn" } else { "dyn_map_fn" };
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("list_h", hof),
                        args: vec![list, fnaddr],
                    });
                    Ok(Some((dst, Ty::ListDyn)))
                }
            }
        }
        ("reduce", 2) => {
            let Some(fidx) = lambda_at(ssa, base.wrapping_add(2)) else {
                return Ok(None);
            };
            if fidx >= funcs.len() || fidx == entry as usize || funcs[fidx].param_count != 2 {
                return Err(Unsupported::Opcode {
                    pc,
                    op: Opcode::CallMethodK,
                });
            }
            let (init_raw, init_ty) = ssa.read(base.wrapping_add(1), block, pc)?;
            let widened = sig.dyn_rets.contains(&(fidx as u32))
                || sig
                    .param_obs
                    .get(fidx)
                    .is_some_and(|p| p.iter().take(2).any(|slot| *slot == Some(Ty::Dyn)));
            let fnaddr = ssa.new_val();
            insts.push(Inst::Const {
                dst: fnaddr,
                value: Const::FnAddr(FuncId(fidx as u32)),
            });
            if elem == Ty::I64 && init_ty == Ty::I64 && !widened {
                // Typed fast path: `(i64, i64) → i64`.
                seed_params(sig, fidx, 2, Ty::I64);
                if sig.param_ty(fidx, 0) != Ty::I64 || sig.param_ty(fidx, 1) != Ty::I64 {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                if sig.ret_types.get(fidx).copied() != Some(Ty::I64) {
                    if sig.ret_known.get(fidx).copied().unwrap_or(false)
                        && sig
                            .ret_types
                            .get(fidx)
                            .copied()
                            .is_some_and(|t| t != Ty::I64 && dyn_boxable_ty(t))
                    {
                        sig.dyn_rets.insert(fidx as u32);
                    }
                    return Err(Unsupported::TypeMismatch { pc });
                }
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("list_h", "i64_reduce_fn"),
                    args: vec![receiver, init_raw, fnaddr],
                });
                return Ok(Some((dst, Ty::I64)));
            }
            // Dyn accumulator: `(Dyn, Dyn) → Dyn` — a list-building reduce
            // (`xs.reduce([], |sorted, item| …)`), a Maybe/nil init, or a
            // runtime-polymorphic receiver.
            seed_params(sig, fidx, 2, Ty::Dyn);
            if sig.param_ty(fidx, 0) != Ty::Dyn || sig.param_ty(fidx, 1) != Ty::Dyn {
                return Err(Unsupported::TypeMismatch { pc });
            }
            if !sig.dyn_rets.contains(&(fidx as u32)) {
                sig.dyn_rets.insert(fidx as u32);
                return Err(Unsupported::TypeMismatch { pc });
            }
            if sig.ret_types.get(fidx).copied() != Some(Ty::Dyn) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let list = dyn_list_of(ssa, insts, receiver)?;
            let init = to_dyn_any(ssa, insts, init_raw, init_ty, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_reduce_fn"),
                args: vec![list, init, fnaddr],
            });
            Ok(Some((dst, Ty::Dyn)))
        }
        _ => Ok(None),
    }
}

/// The shared per-(receiver type, method name, argument types) dispatch table.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_method_dispatch(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    receiver: ValueId,
    receiver_ty: Ty,
    name: &str,
    args: &[(ValueId, Ty)],
    block: usize,
    pc: usize,
) -> Result<Reg, Unsupported> {
    let result: Reg = match (receiver_ty, name, args) {
        // Boxed-element list long tail (runtime-polymorphic receivers).
        (Ty::ListDyn, "take", [(n, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_take"),
                args: vec![receiver, *n],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::ListDyn, "skip", [(n, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_skip"),
                args: vec![receiver, *n],
            });
            (dst, Ty::ListDyn)
        }
        // `concat` with any dyn-list side: both sides normalize to dyn lists
        // (typed sides convert element-wise, cold path) and chain.
        (Ty::ListDyn | Ty::ListI64 | Ty::ListF64 | Ty::ListStr, "concat", [(other, oty)])
            if receiver_ty == Ty::ListDyn || *oty == Ty::ListDyn || *oty == Ty::Dyn =>
        {
            let lhs = to_dyn_list_handle(ssa, insts, receiver, receiver_ty, pc)?;
            let rhs = match *oty {
                Ty::Dyn => {
                    let unboxed = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(unboxed),
                        callee: AbiRef::new("dyn", "as_list"),
                        args: vec![*other],
                    });
                    unboxed
                }
                oty => to_dyn_list_handle(ssa, insts, *other, oty, pc)?,
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_chain"),
                args: vec![lhs, rhs],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::ListI64, "unique", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_unique"),
                args: vec![receiver],
            });
            (dst, Ty::ListI64)
        }
        // `xs.sort()` / `xs.reverse()` — fresh copies (the VM sorts/reverses
        // a snapshot; the receiver is untouched).
        (Ty::ListI64, "sort", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_sort"),
                args: vec![receiver],
            });
            (dst, Ty::ListI64)
        }
        (Ty::ListI64, "reverse", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_reverse"),
                args: vec![receiver],
            });
            (dst, Ty::ListI64)
        }
        // `.is_empty()` — `len == 0` over the same per-type len ABI.
        (
            Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn | Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrDyn,
            "is_empty",
            [],
        ) => {
            let (module, len_fn) = match receiver_ty {
                Ty::ListI64 => ("list_h", "i64_len"),
                Ty::ListF64 => ("list_h", "f64_len"),
                Ty::ListStr => ("list_h", "str_len"),
                Ty::ListDyn => ("list_h", "dyn_len"),
                Ty::MapStrI64 => ("map_h", "str_i64_len"),
                Ty::MapStrF64 => ("map_h", "str_f64_len"),
                _ => ("map_h", "str_dyn_len"),
            };
            let len = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(len),
                callee: AbiRef::new(module, len_fn),
                args: vec![receiver],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Eq,
                float: false,
                lhs: len,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `.slice(start[, end])` — negative aborts (VM loud), end clamps.
        (Ty::ListI64, "slice", [(start, Ty::I64), (end, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_slice_method"),
                args: vec![receiver, *start, *end],
            });
            (dst, Ty::ListI64)
        }
        // Map iteration family (order = the VM's, layout mirror): keys/
        // values snapshots (Mixed → dyn lists), delete-with-removed-value.
        (Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn, "keys" | "values", []) => {
            let family = match receiver_ty {
                Ty::MapStrI64 => "str_i64",
                Ty::MapStrF64 => "str_f64",
                Ty::MapStrBool => "str_bool",
                _ => "str_dyn",
            };
            let abi_name: &'static str = match (family, name) {
                ("str_i64", "keys") => "str_i64_keys",
                ("str_i64", _) => "str_i64_values",
                ("str_f64", "keys") => "str_f64_keys",
                ("str_f64", _) => "str_f64_values",
                ("str_bool", "keys") => "str_bool_keys",
                ("str_bool", _) => "str_bool_values",
                (_, "keys") => "str_dyn_keys",
                _ => "str_dyn_values",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", abi_name),
                args: vec![receiver],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn, "delete" | "remove", [(k, Ty::Str)]) => {
            let abi_name = match receiver_ty {
                Ty::MapStrI64 => "str_i64_delete",
                Ty::MapStrF64 => "str_f64_delete",
                Ty::MapStrBool => "str_bool_delete",
                _ => "str_dyn_delete",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", abi_name),
                args: vec![receiver, *k],
            });
            (dst, Ty::Dyn)
        }
        // `m.has(k)` on typed string maps — the dynamic-lookup present bit.
        (Ty::MapStrI64 | Ty::MapStrBool, "has", [(k, Ty::Str)]) => {
            let looked = ssa.new_val();
            insts.push(Inst::MapGetMaybe {
                dst: looked,
                handle: receiver,
                key: *k,
            });
            let present = ssa.new_val();
            insts.push(Inst::MaybePresent {
                dst: present,
                src: looked,
                maybe_ty: Ty::MaybeI64,
            });
            (present, Ty::Bool)
        }
        (Ty::MapStrF64, "has", [(k, Ty::Str)]) => {
            let looked = ssa.new_val();
            insts.push(Inst::MapGetMaybeStrF64 {
                dst: looked,
                handle: receiver,
                key: *k,
            });
            let present = ssa.new_val();
            insts.push(Inst::MaybePresent {
                dst: present,
                src: looked,
                maybe_ty: Ty::MaybeF64,
            });
            (present, Ty::Bool)
        }
        // Set methods (VM `core_methods` set family): membership/mutation
        // return Bool, `len` Int, `clear` Nil. Elements box to Dyn — a Float
        // aborts inside lkrt (the VM's loud "cannot be used as a key").
        (Ty::Set, "len", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("set", "len"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        (Ty::Set, "is_empty", []) => {
            let len = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(len),
                callee: AbiRef::new("set", "len"),
                args: vec![receiver],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Eq,
                float: false,
                lhs: len,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        (Ty::Set, "has" | "contains" | "add" | "delete" | "remove", [(v, vty)]) => {
            let boxed = to_dyn_any(ssa, insts, *v, *vty, pc)?;
            let abi_name = match name {
                "has" | "contains" => "has",
                "add" => "add",
                _ => "delete",
            };
            let wide = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(wide),
                callee: AbiRef::new("set", abi_name),
                args: vec![receiver, boxed],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: wide,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        (Ty::Set, "clear", []) => {
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("set", "clear"),
                args: vec![receiver],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
        }
        // `s.starts_with(prefix)` — byte-prefix test, exactly Rust/VM semantics.
        (Ty::Str, "starts_with", [(prefix, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "starts_with"),
                args: vec![receiver, *prefix],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: dst,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `s.contains(needle)` — byte-substring test, exactly Rust/VM semantics.
        // `m.has(key)` on a mixed-value map — key membership (stored-nil
        // still counts, see `str_dyn_has`).
        // `xs.first()` / `xs.last()` — nil when empty: exactly the dynamic-
        // index `Maybe` model (an OOB/absent `get_pair` is `present = 0`),
        // so both reuse the existing ListGetMaybe machinery, no new ABI.
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, "first", []) => {
            let idx = ssa.new_val();
            insts.push(Inst::Const {
                dst: idx,
                value: Const::I64(0),
            });
            let dst = ssa.new_val();
            let maybe_ty = match receiver_ty {
                Ty::ListI64 => {
                    insts.push(Inst::ListGetMaybe {
                        dst,
                        handle: receiver,
                        index: idx,
                    });
                    Ty::MaybeI64
                }
                Ty::ListF64 => {
                    insts.push(Inst::ListGetMaybeF64 {
                        dst,
                        handle: receiver,
                        index: idx,
                    });
                    Ty::MaybeF64
                }
                _ => {
                    insts.push(Inst::ListGetMaybeStr {
                        dst,
                        handle: receiver,
                        index: idx,
                    });
                    Ty::MaybeStr
                }
            };
            (dst, maybe_ty)
        }
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, "last", []) => {
            let (len_module, len_fn) = match receiver_ty {
                Ty::ListI64 => ("list_h", "i64_len"),
                Ty::ListF64 => ("list_h", "f64_len"),
                _ => ("list_h", "str_len"),
            };
            let len = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(len),
                callee: AbiRef::new(len_module, len_fn),
                args: vec![receiver],
            });
            let one = ssa.new_val();
            insts.push(Inst::Const {
                dst: one,
                value: Const::I64(1),
            });
            let idx = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: idx,
                op: IntBinOp::Sub,
                lhs: len,
                rhs: one,
            });
            let dst = ssa.new_val();
            let maybe_ty = match receiver_ty {
                Ty::ListI64 => {
                    insts.push(Inst::ListGetMaybe {
                        dst,
                        handle: receiver,
                        index: idx,
                    });
                    Ty::MaybeI64
                }
                Ty::ListF64 => {
                    insts.push(Inst::ListGetMaybeF64 {
                        dst,
                        handle: receiver,
                        index: idx,
                    });
                    Ty::MaybeF64
                }
                _ => {
                    insts.push(Inst::ListGetMaybeStr {
                        dst,
                        handle: receiver,
                        index: idx,
                    });
                    Ty::MaybeStr
                }
            };
            (dst, maybe_ty)
        }
        // `xs.concat(ys)` — same semantics as chain (the VM implements both
        // as lhs ++ rhs into a fresh list).
        (Ty::ListI64, "concat", [(other, Ty::ListI64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_chain"),
                args: vec![receiver, *other],
            });
            (dst, Ty::ListI64)
        }
        // `xs.join(sep)` on a string list → one string.
        (Ty::ListStr, "join", [(sep, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "str_join"),
                args: vec![receiver, *sep],
            });
            (dst, Ty::Str)
        }
        // `xs.get(i)` — safe index: nil on OOB, i.e. exactly the dynamic-
        // index Maybe model (reused, no new ABI).
        (Ty::ListI64, "get", [(idx, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::ListGetMaybe {
                dst,
                handle: receiver,
                index: *idx,
            });
            (dst, Ty::MaybeI64)
        }
        (Ty::ListF64, "get", [(idx, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::ListGetMaybeF64 {
                dst,
                handle: receiver,
                index: *idx,
            });
            (dst, Ty::MaybeF64)
        }
        (Ty::ListStr, "get", [(idx, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::ListGetMaybeStr {
                dst,
                handle: receiver,
                index: *idx,
            });
            (dst, Ty::MaybeStr)
        }
        // `List<i64>` slicing/concat helpers (VM core_methods semantics).
        (Ty::ListI64, "take", [(n, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_take"),
                args: vec![receiver, *n],
            });
            (dst, Ty::ListI64)
        }
        (Ty::ListI64, "skip", [(n, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_skip"),
                args: vec![receiver, *n],
            });
            (dst, Ty::ListI64)
        }
        (Ty::ListI64, "chain", [(other, Ty::ListI64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_chain"),
                args: vec![receiver, *other],
            });
            (dst, Ty::ListI64)
        }
        (Ty::MapStrDyn, "has", [(key, Ty::Str)]) => {
            let raw = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(raw),
                callee: AbiRef::new("map_h", "str_dyn_has"),
                args: vec![receiver, *key],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: raw,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `m.len()` / `xs.len()` on Dyn containers (method form of `Len`).
        (Ty::MapStrDyn, "len", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", "str_dyn_len"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        (Ty::ListDyn, "len", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_len"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        // Methods whose VM result is a mixed list regardless of the receiver
        // (chunk/enumerate/zip pairs are nested; unique/flatten come back
        // `TypedList::Mixed`): the receiver converts to a dyn-list handle up
        // front, one lkrt helper per method mirrors core_methods.rs.
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "chunk", [(n, Ty::I64)]) => {
            let handle = to_dyn_list_handle(ssa, insts, receiver, receiver_ty, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_chunk"),
                args: vec![handle, *n],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "enumerate", []) => {
            let handle = to_dyn_list_handle(ssa, insts, receiver, receiver_ty, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_enumerate"),
                args: vec![handle],
            });
            (dst, Ty::ListDyn)
        }
        (
            Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn,
            "zip",
            [(other, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn)],
        ) => {
            let lhs = to_dyn_list_handle(ssa, insts, receiver, receiver_ty, pc)?;
            let rhs = to_dyn_list_handle(ssa, insts, *other, args[0].1, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_zip"),
                args: vec![lhs, rhs],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "unique", []) => {
            let handle = to_dyn_list_handle(ssa, insts, receiver, receiver_ty, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_unique"),
                args: vec![handle],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::ListDyn, "flatten", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_flatten"),
                args: vec![receiver],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::Str, "contains", [(needle, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "contains"),
                args: vec![receiver, *needle],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: dst,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `s.len()` — Unicode scalar count (the VM's `chars().count()`).
        (Ty::Str, "len", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "char_len"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        // `s.is_empty()` — char_len == 0 (an empty string is empty in both
        // byte and char terms), no new ABI.
        (Ty::Str, "is_empty", []) => {
            let len = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(len),
                callee: AbiRef::new("str", "char_len"),
                args: vec![receiver],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Eq,
                float: false,
                lhs: len,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `s.ends_with(suffix)` — byte-suffix test (see `starts_with`).
        (Ty::Str, "ends_with", [(suffix, Ty::Str)]) => {
            let raw = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(raw),
                callee: AbiRef::new("str", "ends_with"),
                args: vec![receiver, *suffix],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: raw,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `s.find(needle)` — byte index or -1 (the VM's `str::find`).
        (Ty::Str, "find", [(needle, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "find"),
                args: vec![receiver, *needle],
            });
            (dst, Ty::I64)
        }
        // Fresh-string unary transforms (VM core_methods semantics: `lower`/
        // `upper` are Unicode `to_lowercase`/`to_uppercase`, `reverse` is
        // char-wise, `trim` is Rust `str::trim`).
        (Ty::Str, "lower" | "upper" | "trim" | "reverse", []) => {
            let helper = match name {
                "lower" => "lower",
                "upper" => "upper",
                "trim" => "trim",
                _ => "reverse",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", helper),
                args: vec![receiver],
            });
            (dst, Ty::Str)
        }
        (Ty::Str, "repeat", [(n, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "repeat"),
                args: vec![receiver, *n],
            });
            (dst, Ty::Str)
        }
        // `s.substring(start, length)` — byte-indexed in the VM (a
        // non-boundary index aborts loudly, like the VM's panic).
        (Ty::Str, "substring", [(start, Ty::I64), (length, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "substring"),
                args: vec![receiver, *start, *length],
            });
            (dst, Ty::Str)
        }
        (Ty::Str, "replace", [(from, Ty::Str), (to, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "replace"),
                args: vec![receiver, *from, *to],
            });
            (dst, Ty::Str)
        }
        // `s.chars()` — the VM returns a *Mixed* list (bare-text display),
        // so the native carrier is a dyn list, not a typed string list.
        (Ty::Str, "chars", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "chars"),
                args: vec![receiver],
            });
            (dst, Ty::ListDyn)
        }
        // `m.get(key)` on string-keyed maps: the missing-key `Maybe` model.
        (Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool, "get", [(key, Ty::Str)]) => {
            let dst = ssa.new_val();
            let maybe_ty = match receiver_ty {
                Ty::MapStrF64 => {
                    insts.push(Inst::MapGetMaybeStrF64 {
                        dst,
                        handle: receiver,
                        key: *key,
                    });
                    Ty::MaybeF64
                }
                Ty::MapStrBool => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle: receiver,
                        key: *key,
                    });
                    Ty::MaybeBool
                }
                _ => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle: receiver,
                        key: *key,
                    });
                    Ty::MaybeI64
                }
            };
            (dst, maybe_ty)
        }
        // `m.set(key, value)` on string-keyed maps.
        (Ty::MapStrI64, "set", [(key, Ty::Str), (value, Ty::I64)])
        | (Ty::MapStrBool, "set", [(key, Ty::Str), (value, Ty::I64)]) => {
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("map_h", "str_i64_set"),
                args: vec![receiver, *key, *value],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
        }
        (Ty::MapStrF64, "set", [(key, Ty::Str), (value, Ty::F64)]) => {
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("map_h", "str_f64_set"),
                args: vec![receiver, *key, *value],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
        }
        // `xs.contains(v)` on typed lists (fcmp semantics for f64, like the VM).
        (Ty::ListI64, "contains", [(v, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_contains"),
                args: vec![receiver, *v],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: dst,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
    };
    let _ = globals;
    let _ = block;
    Ok(result)
}
