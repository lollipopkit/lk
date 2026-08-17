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
        return Err(Unsupported::CallShape {
            pc,
            reason: "no native lowering for this method on this receiver type",
        });
    };
    let args = match ssa.builtin_regs.get(&(block, base.wrapping_add(3))) {
        Some(GlobalRef::ArgList(elems)) => elems.clone(),
        _ => {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this method on this receiver type",
            });
        }
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
    let argc = instr.c() as usize;
    // A **module object** receiver is a module function call, not a method
    // call: `encoding.json.parse(s)` compiles to `CallMethodK` with `parse` as
    // the name and `encoding.json` as the receiver, and there was no arm for
    // that — the receiver holds a lowering-time ref, not an SSA value, so the
    // read below reported "register r7 is read before any definition" and the
    // whole program fell back. Only the selective import
    // (`use { json } from encoding;`) lowered.
    if let Some(GlobalRef::Module(module)) = ssa.builtin_ref_at(base, block) {
        // `lower_module_call` reads its arguments from `base + 1`, which is
        // where a method call's arguments already sit.
        return lower_module_call(ssa, insts, &module, &name, base, argc, block, pc);
    }
    // A `Maybe` receiver unwraps first, which is what the VM does: calling a
    // method on an absent one raises (`lkrt_maybe_*_unwrap` raises too, so the
    // two agree, including on being catchable). Without it a list's loop
    // variable — a `Maybe`, since the element read is bounds-checked — could be
    // printed but not asked anything: `for s in ["ab", "cde"] { s.len() }`
    // dropped the whole program to the VM.
    let (receiver, receiver_ty) = read_scalar(ssa, insts, base, block, pc)?;
    // A boxed Dyn receiver unwraps through the as_list guard for list-only
    // method names (a non-list tag aborts — the VM's method-on-wrong-type is
    // a loud error too). Names shared with str/map receivers stay boxed.
    let role = method_role(&name);
    /// The methods whose answer is a list of the receiver's elements, whatever
    /// carrier held them. See the arm below.
    ///
    /// `flatten` is not here: a `Bytes` and an `i64` window hold scalars, so
    /// flattening one is a no-op and the checker declines it. `join` is not
    /// here either — the bytecode compiler matches it by name into the fused
    /// `ListJoin`, so no method call by that name reaches this.
    ///
    /// `concat` is here for a window and **not** for a `Bytes`: two byte
    /// strings joined are a byte string, so that one keeps its carrier and has
    /// its own arm. Taking it away would answer a `List` for a shape the
    /// language already spells as `Bytes`.
    fn answers_a_list_of_the_elements(receiver_ty: Ty, name: &str) -> bool {
        match receiver_ty {
            Ty::Bytes => matches!(name, "enumerate" | "zip" | "chain" | "chunk"),
            Ty::SliceI64 => matches!(name, "enumerate" | "zip" | "chain" | "chunk" | "concat"),
            _ => false,
        }
    }
    let (receiver, receiver_ty) = if receiver_ty == Ty::Dyn && role.is_some_and(|role| role.unbox_list) {
        let unboxed = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(unboxed),
            callee: AbiRef::new("dyn", "as_list"),
            args: vec![receiver],
        });
        (unboxed, Ty::ListDyn)
    } else if answers_a_list_of_the_elements(receiver_ty, &name) {
        // The operations whose answer is a *list of the elements*: they mean
        // the same on `Bytes` and on a window as on a `List`, and cannot keep
        // the carrier, so they are the list's — reached by materializing once
        // and letting the list arms run. Six arms per carrier would be six
        // copies of `enumerate`'s pairing and `chunk`'s grouping, and the VM
        // delegates for exactly that reason.
        let list = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(list),
            callee: match receiver_ty {
                Ty::Bytes => AbiRef::new("bytes_h", "to_i64_list"),
                _ => AbiRef::new("slice_h", "i64_to_list"),
            },
            args: vec![receiver],
        });
        (list, Ty::ListI64)
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
    //
    // `Bytes` joins by *becoming* an `Int` list first. Its elements are byte
    // values, so `to_i64_list` loses nothing, and the channel below then answers
    // the same shapes the VM does: `map` and `reduce` are already list-shaped
    // there, and only `filter` has to come back — the VM keeps a filtered
    // `Bytes` as `Bytes`, because filtering removes elements without changing
    // any. Without this the three closure methods were the last of the fourteen
    // still dropping their module to the VM.
    //
    // A `Slice` joins the same way and for the same reason, with one difference
    // in the other direction: `w.filter(f)` answers a **List**, not a window
    // (`builtin_method_sig` says so — a window is a range of its source, and a
    // filtered window is not one), so nothing has to come back.
    let hof_receiver =
        if matches!(receiver_ty, Ty::Bytes | Ty::SliceI64) && matches!(name.as_str(), "map" | "filter" | "reduce") {
            let listed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(listed),
                callee: AbiRef::new(
                    if receiver_ty == Ty::Bytes { "bytes_h" } else { "slice_h" },
                    if receiver_ty == Ty::Bytes {
                        "to_i64_list"
                    } else {
                        "i64_to_list"
                    },
                ),
                args: vec![receiver],
            });
            Some(listed)
        } else {
            None
        };
    let hof_ty = if hof_receiver.is_some() {
        Ty::ListI64
    } else {
        receiver_ty
    };
    if matches!(hof_ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn)
        && let Some(result) = lower_list_hof_k(
            ssa,
            insts,
            funcs,
            entry,
            sig,
            hof_receiver.unwrap_or(receiver),
            hof_ty,
            &name,
            base,
            argc,
            block,
            pc,
        )?
    {
        let result = match (hof_receiver, name.as_str()) {
            (Some(_), "filter") if receiver_ty == Ty::Bytes => {
                let bytes = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(bytes),
                    callee: AbiRef::new("bytes_h", "from_i64_list"),
                    args: vec![result.0],
                });
                (bytes, Ty::Bytes)
            }
            _ => result,
        };
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
        return emit_call_with_args(
            ssa,
            insts,
            funcs,
            entry,
            sig,
            fidx as usize,
            call_args,
            Opcode::CallMethodK,
            pc,
        )
        .map(Some);
    }
    // Runtime dispatch. The receiver may be boxed already (`Dyn`) or a struct
    // carrier whose type the lowering could not name — a `MapStrDyn` parameter
    // two call sites pass different structs to, which `param_structs` poisons
    // on purpose. Both know their type at *run time*, in the arena mark this
    // instruction reads, so both dispatch; only the `Dyn` case used to, and the
    // other one took the whole module to the VM instead.
    if matches!(receiver_ty, Ty::Dyn | Ty::MapStrDyn)
        && let Some(arms) = sig.traits.methods.get(name).cloned()
        && !arms.is_empty()
    {
        let mut retry = false;
        for &(_, fidx) in &arms {
            let f = fidx as usize;
            if f >= funcs.len()
                || fidx == entry
                // `self` plus the method's own arguments. Every arm is called
                // through one rendered signature, so an arm of another arity is
                // not a shape this can dispatch.
                || funcs[f].param_count as usize != 1 + argc
                || funcs[f].capture_count != 0
                || sig.specialized.get(f).copied().unwrap_or(false)
            {
                return Err(Unsupported::TypeMismatch { pc });
            }
            if let Some(flag) = sig.plain_called.get_mut(f) {
                *flag = true;
            }
            // A runtime-dispatched arm receives `self` and every argument
            // boxed, so its parameters are `Dyn` and carry no struct name.
            for slot in 0..=argc {
                sig.observe_param(f, slot, Ty::Dyn, None);
            }
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
        // Read the arguments *before* boxing the receiver, so a failure leaves
        // no half-emitted boxing in the stream.
        let mut raw_args = Vec::with_capacity(argc);
        for i in 0..argc {
            raw_args.push(ssa.read(base.wrapping_add(1).wrapping_add(i as u8), block, pc)?);
        }
        let self_arg = to_dyn_any(ssa, insts, receiver, receiver_ty, pc)?;
        let mut args = Vec::with_capacity(argc);
        for (v, ty) in raw_args {
            args.push(to_dyn_any(ssa, insts, v, ty, pc)?);
        }
        let dst = ssa.new_val();
        insts.push(Inst::TraitDispatch {
            dst,
            self_arg,
            args,
            arms: arms.iter().map(|&(tid, f)| (tid, FuncId(f))).collect(),
        });
        return Ok(Some((dst, Ty::Dyn)));
    }
    Ok(None)
}

/// Emits a devirtualized call to `fidx` with arguments **already in frame
/// order**, refining the callee's signature through the shared parameter
/// lattice.
///
/// The window-order readers (`lower_user_call`) cannot serve a call whose
/// arguments are not laid out in parameter order: trait dispatch puts `self`
/// first, and a named call (`lower_named_call`) permutes by name. `label` is
/// the opcode a rejection should name, since that is the only thing the two
/// callers do not share.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_call_with_args(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    fidx: usize,
    call_args: Vec<(ValueId, Ty)>,
    label: Opcode,
    pc: usize,
) -> Result<(ValueId, Ty), Unsupported> {
    if fidx >= funcs.len()
        || fidx as u32 == entry
        || funcs[fidx].param_count as usize != call_args.len()
        || funcs[fidx].capture_count != 0
    {
        return Err(Unsupported::Opcode { pc, op: label });
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
        let want = sig.observe_param(fidx, i, ty, ssa.struct_types.get(&v).map(String::as_str));
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
    seed_ret_struct(ssa, sig, fidx, dst);
    Ok((dst, ret))
}

/// Records the struct a call's result is known to be (`sig.ret_structs`).
///
/// The one place the callee's returned type name reaches the caller. Without it
/// the name stopped at the function boundary and `make(3, 4).norm()` had an
/// untyped receiver — the same missing-provenance failure as an `impl` method's
/// `self`, one call deeper.
pub(crate) fn seed_ret_struct(ssa: &mut Ssa, sig: &SigInfer, fidx: usize, dst: ValueId) {
    if let Some(Some(name)) = sig.ret_structs.get(&(fidx as u32)) {
        ssa.struct_types.insert(dst, name.clone());
    }
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
        && let Some(&fidx) = sig
            .traits
            .impls
            .get(&(type_name, crate::trait_env::IMPLICIT_METHOD_HOOKS[0].to_string()))
    {
        return emit_call_with_args(
            ssa,
            insts,
            funcs,
            entry,
            sig,
            fidx as usize,
            vec![(v, ty)],
            Opcode::CallMethodK,
            pc,
        );
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
            // Callback parameters seeded from the receiver's element type,
            // which is never a struct carrier here.
            sig.observe_param(fidx, i, ty, None);
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
        // `take` / `skip` over every carrier and both directions. Neither looks
        // at the element, and they were written out per carrier — which is how
        // `f64` and `str` ended up with neither, so `[1.5, 2.5].take(1)` dropped
        // its whole module to the VM.
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, name @ ("take" | "skip"), [(n, Ty::I64)]) => {
            let callee = match (receiver_ty, name) {
                (Ty::ListI64, "take") => "i64_take",
                (Ty::ListI64, _) => "i64_skip",
                (Ty::ListF64, "take") => "f64_take",
                (Ty::ListF64, _) => "f64_skip",
                (Ty::ListStr, "take") => "str_take",
                (Ty::ListStr, _) => "str_skip",
                (_, "take") => "dyn_take",
                (_, _) => "dyn_skip",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", callee),
                args: vec![receiver, *n],
            });
            (dst, receiver_ty)
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
        // `sort` is per carrier because its *order* is per carrier — see
        // `list_sort!` in lkrt, where the `f64` comparator is not a total order
        // once a NaN is present and the answer is therefore an artifact of which
        // sort call is used. The boxed carrier is absent on purpose: its order is
        // `compare_runtime_values` across kinds, which is a mirror worth its own
        // conformance test rather than a copy.
        // `sum` on the two numeric carriers, and `min`/`max` on the three
        // ordered ones — the same orders `sort` uses just above, from the same
        // comparators in lkrt.
        //
        // The boxed carrier is out for the reason `sort` states: its order is
        // `compare_runtime_values` across kinds, a mirror that wants its own
        // conformance test rather than a copy. A `List<str>` has no `sum` for
        // the reason the VM gives — summing strings is a mistake, not a join —
        // and with no row that call falls back and raises there.
        (Ty::ListI64 | Ty::ListF64, "sum", []) => {
            let (callee, ty) = match receiver_ty {
                Ty::ListI64 => ("i64_sum", Ty::I64),
                _ => ("f64_sum", Ty::F64),
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", callee),
                args: vec![receiver],
            });
            (dst, ty)
        }
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, "min" | "max", []) => {
            // An `AbiRef` names a `&'static str`, so the carrier × direction
            // pair is spelled out rather than assembled.
            let callee = match (receiver_ty, name) {
                (Ty::ListI64, "min") => "i64_min",
                (Ty::ListI64, _) => "i64_max",
                (Ty::ListF64, "min") => "f64_min",
                (Ty::ListF64, _) => "f64_max",
                (_, "min") => "str_min",
                _ => "str_max",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", callee),
                args: vec![receiver],
            });
            // Boxed: an empty sequence answers nil, which no unboxed carrier
            // can hold.
            (dst, Ty::Dyn)
        }
        (Ty::Bytes, "sum", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "sum"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        (Ty::Bytes, "min" | "max", []) => {
            let callee = if name == "min" { "min" } else { "max" };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", callee),
                args: vec![receiver],
            });
            (dst, Ty::Dyn)
        }
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, "sort", []) => {
            let callee = match receiver_ty {
                Ty::ListI64 => "i64_sort",
                Ty::ListF64 => "f64_sort",
                _ => "str_sort",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", callee),
                args: vec![receiver],
            });
            (dst, receiver_ty)
        }
        // `reverse` does not look at the element, so it is one arm over the
        // carriers rather than four written one at a time — which is how it came
        // to exist for `Int` and nowhere else, dropping `[1.5, 2.5].reverse()`'s
        // whole module to the VM.
        (Ty::ListI64, "count", [(value, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_count"),
                args: vec![receiver, *value],
            });
            (dst, Ty::I64)
        }
        (Ty::ListF64, "count", [(value, Ty::F64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "f64_count"),
                args: vec![receiver, *value],
            });
            (dst, Ty::I64)
        }
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "reverse", []) => {
            let callee = match receiver_ty {
                Ty::ListI64 => "i64_reverse",
                Ty::ListF64 => "f64_reverse",
                Ty::ListStr => "str_reverse",
                _ => "dyn_reverse",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", callee),
                args: vec![receiver],
            });
            (dst, receiver_ty)
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
        // `.slice(start[, end])` — a **window** over the receiver, not a copy
        // of it (negative aborts as the VM does; `end` clamps). This returned
        // `Ty::ListI64` until the VM's `.slice()` became a view: the two
        // backends then disagreed about whether a write to the source shows
        // through, and about whether `.to_list()` existed at all.
        // `xs.slice(start)` — the one-argument form, whose end defaults to the
        // length. `Ty::Str` had both arities and a list had only the two-arg
        // one, so `xs.slice(1)` dropped the program to the VM.
        (Ty::ListI64, "slice", [(start, Ty::I64)]) => {
            let end = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(end),
                callee: AbiRef::new("list_h", "i64_len"),
                args: vec![receiver],
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_new"),
                args: vec![receiver, *start, end],
            });
            (dst, Ty::SliceI64)
        }
        // `clear()` returns the receiver, which is what the VM's `clear` gives
        // back — the same handle, now empty. Every carrier at once: the
        // operation does not look at the element type.
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "clear", []) => {
            let helper = match receiver_ty {
                Ty::ListI64 => "i64_clear",
                Ty::ListF64 => "f64_clear",
                Ty::ListStr => "str_clear",
                _ => "dyn_clear",
            };
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("list_h", helper),
                args: vec![receiver],
            });
            // The value *is* the receiver — the helper returns nothing on
            // purpose (see `lklist::list_clear!`).
            (receiver, receiver_ty)
        }
        // The other element types slice through `*_slice_from`, which has been
        // in the ABI all along — only the dispatch table stopped at `i64`. Same
        // shape as `chain`: the runtime could do it, nothing asked.
        //
        // `i64` above answers a *window* (`SliceI64`); these answer a fresh
        // list. Both are what `slice` means — the window is an optimisation the
        // other carriers do not have, not a different result.
        (Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "slice", [(start, Ty::I64)]) => {
            let helper = match receiver_ty {
                Ty::ListF64 => "f64_slice_from",
                Ty::ListStr => "str_slice_from",
                _ => "dyn_slice_from",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", helper),
                args: vec![receiver, *start],
            });
            (dst, receiver_ty)
        }
        // `contains` likewise: the helpers exist for every carrier.
        (Ty::ListF64, "contains", [(needle, Ty::F64 | Ty::I64)]) => {
            let needle = coerce_to_f64(ssa, insts, *needle, args[0].1);
            let found = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(found),
                callee: AbiRef::new("list_h", "f64_contains"),
                args: vec![receiver, needle],
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
                lhs: found,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        (Ty::ListDyn, "contains", [(needle, nty)]) => {
            let boxed = to_dyn(ssa, insts, *needle, *nty, pc)?;
            let found = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(found),
                callee: AbiRef::new("list_h", "dyn_contains"),
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
                lhs: found,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        (Ty::Bytes, "slice", [(from, Ty::I64)]) => {
            let end = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(end),
                callee: AbiRef::new("bytes_h", "len"),
                args: vec![receiver],
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "slice"),
                args: vec![receiver, *from, end],
            });
            (dst, Ty::Bytes)
        }
        (Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "slice", [(start, Ty::I64), (end, Ty::I64)]) => {
            let helper = match receiver_ty {
                Ty::ListF64 => "f64_slice",
                Ty::ListStr => "str_slice",
                _ => "dyn_slice",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", helper),
                args: vec![receiver, *start, *end],
            });
            (dst, receiver_ty)
        }
        (Ty::ListI64, "slice", [(start, Ty::I64), (end, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_new"),
                args: vec![receiver, *start, *end],
            });
            (dst, Ty::SliceI64)
        }
        // A window on a window resolves against the original source rather
        // than nesting, matching `dispatch_slice_builtin_method`.
        (Ty::SliceI64, "slice", [(start, Ty::I64), (end, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_sub"),
                args: vec![receiver, *start, *end],
            });
            (dst, Ty::SliceI64)
        }
        (Ty::SliceI64, "len", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_len"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        (Ty::SliceI64, "is_empty", []) => {
            let flag = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(flag),
                callee: AbiRef::new("slice_h", "i64_is_empty"),
                args: vec![receiver],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Cmp {
                dst,
                op: CmpOp::Ne,
                float: false,
                lhs: flag,
                rhs: zero,
            });
            (dst, Ty::Bool)
        }
        // The copy, asked for by name — the operation `.slice()` used to
        // perform silently.
        (Ty::SliceI64, "to_list", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_to_list"),
                args: vec![receiver],
            });
            (dst, Ty::ListI64)
        }
        // The read half of the list surface, *through* the window: a window
        // exists so that asking it for a sum does not build a list first, and
        // these nine used to drop the whole module to the VM — the same
        // "almost native receiver" shape `Bytes` had. `take`/`skip` are
        // sub-windows for the same reason, and keep the count guard: a count is
        // not a position, so a negative one is a refusal rather than a window
        // measured from the end.
        (Ty::SliceI64, "sum", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_sum"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        (Ty::SliceI64, "min" | "max", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", if name == "min" { "i64_min" } else { "i64_max" }),
                args: vec![receiver],
            });
            (dst, Ty::Dyn)
        }
        // The ABI's `I64` 0/1 becomes a `Bool` by comparing it, exactly as the
        // `Bytes` arm does — a `Bool`-typed value that is really an i64 makes
        // codegen emit `uextend` on something already 64 bits wide, and the
        // Cranelift verifier rejects the function.
        (Ty::SliceI64, "contains", [(value, Ty::I64)]) => {
            let raw = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(raw),
                callee: AbiRef::new("slice_h", "i64_contains"),
                args: vec![receiver, *value],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Cmp {
                dst,
                op: CmpOp::Ne,
                float: false,
                lhs: raw,
                rhs: zero,
            });
            (dst, Ty::Bool)
        }
        (Ty::SliceI64, "count", [(value, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_count"),
                args: vec![receiver, *value],
            });
            (dst, Ty::I64)
        }
        // A reversed window is not a window of the source, so it materializes
        // — the same rule `map` follows here. Composed from the two symbols
        // that already exist rather than a third that would answer the same.
        (Ty::SliceI64, "reverse" | "sort" | "unique", []) => {
            let list = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(list),
                callee: AbiRef::new("slice_h", "i64_to_list"),
                args: vec![receiver],
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new(
                    "list_h",
                    match name {
                        "sort" => "i64_sort",
                        "unique" => "i64_unique",
                        _ => "i64_reverse",
                    },
                ),
                args: vec![list],
            });
            (dst, Ty::ListI64)
        }
        (Ty::SliceI64, "index_of", [(value, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_index_of"),
                args: vec![receiver, *value],
            });
            (dst, Ty::Dyn)
        }
        (Ty::SliceI64, "take" | "skip", [(count, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", if name == "take" { "i64_take" } else { "i64_skip" }),
                args: vec![receiver, *count],
            });
            (dst, Ty::SliceI64)
        }
        // `first`/`last` are `[0]` and `[-1]`, which the window's own indexed
        // read already is — including the nil an empty window answers.
        (Ty::SliceI64, "first" | "last", []) => {
            let index = ssa.new_val();
            insts.push(Inst::Const {
                dst: index,
                value: Const::I64(if name == "first" { 0 } else { -1 }),
            });
            let dst = ssa.new_val();
            insts.push(Inst::SliceGetMaybe {
                dst,
                handle: receiver,
                index,
            });
            (dst, Ty::MaybeI64)
        }
        // `w.get(i)` — the same read as `w[i]`, answering nil instead of
        // failing, which is what `.get()` means on a list too.
        (Ty::SliceI64, "get", [(index, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::SliceGetMaybe {
                dst,
                handle: receiver,
                index: *index,
            });
            (dst, Ty::MaybeI64)
        }
        // Map iteration family (order = the VM's, layout mirror): keys/
        // values snapshots (Mixed → dyn lists), delete-with-removed-value.
        // A **boxed** map receiver: the tag decides the carrier at run time, so
        // these dispatch inside the runtime instead of unboxing first. They
        // used to go through `dyn.as_map`, which hands back a `str_dyn` handle
        // — fine for a boxed `Map<str, Dyn>` and a `runtime type error` for
        // every typed carrier, on programs the VM answers.
        (Ty::Dyn, "keys" | "values", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("dyn", if name == "keys" { "map_keys" } else { "map_values" }),
                args: vec![receiver],
            });
            (dst, Ty::ListDyn)
        }
        (Ty::Dyn, "has", [(k, Ty::Str)]) => {
            let wide = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(wide),
                callee: AbiRef::new("dyn", "map_has"),
                args: vec![receiver, *k],
            });
            // The ABI answers a machine-width flag; `Ty::Bool` is one bit, and
            // handing the wide value over as-is makes codegen extend an `i64`
            // to `i64` and the verifier reject the function.
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let present = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: present,
                op: CmpOp::Ne,
                float: false,
                lhs: wide,
                rhs: zero,
            });
            (present, Ty::Bool)
        }
        // `delete` writes, which is why the dispatch is per operation: an
        // `as_map` that materialized a copy would answer `keys`/`values`/`has`
        // and silently drop this one.
        (Ty::Dyn, "delete" | "remove", [(k, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("dyn", "map_delete"),
                args: vec![receiver, *k],
            });
            (dst, Ty::Dyn)
        }
        (Ty::MapI64I64 | Ty::MapI64F64, "keys" | "values", []) => {
            let abi_name: &'static str = match (receiver_ty, name) {
                (Ty::MapI64I64, "keys") => "i64_i64_keys",
                (Ty::MapI64I64, _) => "i64_i64_values",
                (_, "keys") => "i64_f64_keys",
                _ => "i64_f64_values",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", abi_name),
                args: vec![receiver],
            });
            (dst, Ty::ListDyn)
        }
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
        // `m.clear()`, the one container method the map did not lower.
        (
            Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64,
            "clear",
            [],
        ) => {
            let abi_name = match receiver_ty {
                // `Map<str, bool>` rides the `str_i64` carrier.
                Ty::MapStrI64 | Ty::MapStrBool => "str_i64_clear",
                Ty::MapStrF64 => "str_f64_clear",
                Ty::MapI64I64 => "i64_i64_clear",
                Ty::MapI64F64 => "i64_f64_clear",
                _ => "str_dyn_clear",
            };
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("map_h", abi_name),
                args: vec![receiver],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
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
        // Only the spellings the language actually has. This accepted `has` and
        // `remove` too, and the type checker rejects both — so those two names
        // could never reach a lowering, while a reader here would conclude
        // `st.has(x)` works. The membership rule is `contains` wherever it is
        // unambiguous (list, set, string) and `has` on a map, where "contains
        // what — a key or a value?" is a real question; `in` works on all of
        // them. See `docs/semantics.md`.
        (Ty::Set, "contains" | "add" | "delete", [(v, vty)]) => {
            let boxed = to_dyn_any(ssa, insts, *v, *vty, pc)?;
            let abi_name = match name {
                "contains" => "has",
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
        // `s.values()` is the members in iteration order — the same list `for x
        // in s` walks, which `set.iter` already builds. It was the one Set method
        // with no arm, so a function using it dropped to the VM while the `for`
        // loop over the same set stayed native.
        //
        // The order is a hash order, so this rides the mirror discipline that
        // makes set iteration lowerable at all (`set_iteration_order_matches_the_vm`,
        // and the single `RtKey` behind it).
        // The set operations. The `kind` operand picks which; the numbering is
        // `lkset::SET_OP_*` / `SET_REL_*`, and a second copy of it here would be
        // a silent mismatch rather than an error — so it is one `match` beside
        // the name that produced it.
        (Ty::Set, "union" | "intersection" | "difference" | "symmetric_difference", [(other, Ty::Set)]) => {
            let kind = ssa.new_val();
            insts.push(Inst::Const {
                dst: kind,
                value: Const::I64(match name {
                    "union" => 0,
                    "intersection" => 1,
                    "difference" => 2,
                    _ => 3,
                }),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("set", "combine"),
                args: vec![receiver, *other, kind],
            });
            (dst, Ty::Set)
        }
        (Ty::Set, "is_subset" | "is_superset" | "is_disjoint", [(other, Ty::Set)]) => {
            let kind = ssa.new_val();
            insts.push(Inst::Const {
                dst: kind,
                value: Const::I64(match name {
                    "is_subset" => 0,
                    "is_superset" => 1,
                    _ => 2,
                }),
            });
            let wide = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(wide),
                callee: AbiRef::new("set", "relate"),
                args: vec![receiver, *other, kind],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Cmp {
                dst,
                op: CmpOp::Ne,
                float: false,
                lhs: wide,
                rhs: zero,
            });
            (dst, Ty::Bool)
        }
        (Ty::Set, "values", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("set", "iter"),
                args: vec![receiver],
            });
            (dst, Ty::ListDyn)
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
        // `s.byte_at(i)` — one byte as a number, the only string read that
        // allocates nothing. `Pure`, so the optimizer may hoist it out of a loop
        // that reads the same index twice; `char_at` next to it cannot be,
        // because it builds a string.
        (Ty::Str, "byte_at", [(index, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::StrByteAtMaybe {
                dst,
                handle: receiver,
                index: *index,
            });
            (dst, Ty::MaybeI64)
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
        // `xs.first()` / `xs.last()` / `xs.pop()` — nil when empty: exactly the
        // dynamic-index `Maybe` model (an OOB/absent `get_pair` is `present = 0`),
        // so all three reuse the existing ListGetMaybe machinery, no new read ABI.
        //
        // One arm for the three because they differ only in *which* index and
        // whether the element is then dropped. Written apart, `first`/`last`
        // covered three carriers and left the boxed one out, and `pop` existed
        // nowhere at all — so a single `xs.pop()` dropped its module to the VM.
        //
        // The boxed carrier reads through `dyn_at`, whose out-of-range answer is
        // already nil, so its `Maybe` is the `Dyn` itself. That also means an
        // empty `pop` and a stored nil are the same answer — which is what the VM
        // says too.
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, name @ ("first" | "last" | "pop"), []) => {
            let idx = ssa.new_val();
            if name == "first" {
                insts.push(Inst::Const {
                    dst: idx,
                    value: Const::I64(0),
                });
            } else {
                // `len - 1`, which is -1 for an empty list — and every carrier's
                // read answers nil for that, so emptiness needs no branch.
                let len_fn = match receiver_ty {
                    Ty::ListI64 => "i64_len",
                    Ty::ListF64 => "f64_len",
                    Ty::ListStr => "str_len",
                    _ => "dyn_len",
                };
                let len = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(len),
                    callee: AbiRef::new("list_h", len_fn),
                    args: vec![receiver],
                });
                let one = ssa.new_val();
                insts.push(Inst::Const {
                    dst: one,
                    value: Const::I64(1),
                });
                insts.push(Inst::IntBin {
                    dst: idx,
                    op: IntBinOp::Sub,
                    lhs: len,
                    rhs: one,
                });
            }
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
                Ty::ListStr => {
                    insts.push(Inst::ListGetMaybeStr {
                        dst,
                        handle: receiver,
                        index: idx,
                    });
                    Ty::MaybeStr
                }
                _ => {
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("list_h", "dyn_at"),
                        args: vec![receiver, idx],
                    });
                    Ty::Dyn
                }
            };
            // `pop` is that read plus the drop. Read first: the value has to come
            // out before the element it names is gone.
            if name == "pop" {
                let drop_fn = match receiver_ty {
                    Ty::ListI64 => "i64_drop_last",
                    Ty::ListF64 => "f64_drop_last",
                    Ty::ListStr => "str_drop_last",
                    _ => "dyn_drop_last",
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("list_h", drop_fn),
                    args: vec![receiver],
                });
            }
            (dst, maybe_ty)
        }
        // `xs.insert(i, v)` answers the receiver (the VM mutates in place and
        // evaluates to the list); `xs.remove_at(i)` answers the element it took
        // out, and raises rather than answering nil when the index is out of
        // range — so unlike `pop` its result is the element type, not a `Maybe`.
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "insert", [(at, _), (value, vty)]) => {
            let (callee, value) = match receiver_ty {
                Ty::ListI64 => ("i64_insert", *value),
                Ty::ListF64 => ("f64_insert", coerce_to_f64(ssa, insts, *value, *vty)),
                Ty::ListStr => ("str_insert", *value),
                _ => ("dyn_insert", to_dyn(ssa, insts, *value, *vty, pc)?),
            };
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("list_h", callee),
                args: vec![receiver, *at, value],
            });
            (receiver, receiver_ty)
        }
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn, "remove_at", [(at, Ty::I64)]) => {
            let (callee, out) = match receiver_ty {
                Ty::ListI64 => ("i64_remove_at", Ty::I64),
                Ty::ListF64 => ("f64_remove_at", Ty::F64),
                Ty::ListStr => ("str_remove_at", Ty::Str),
                _ => ("dyn_remove_at", Ty::Dyn),
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", callee),
                args: vec![receiver, *at],
            });
            (dst, out)
        }
        // `xs.join(sep)` → one string, on every carrier that has one.
        //
        // The numeric arms were absent on purpose: the VM raised "list must
        // contain only strings", so lowering them would have made native answer
        // where the VM refused. That rule is gone — the VM writes each element
        // the way it writes it everywhere else — and the helpers here render the
        // same way the `*_display` ones do, which is what keeps the two ends
        // agreeing about `1.0` and `-0.0`.
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
        // `xs.chain(ys)` is `xs + ys`, and the operator path has always covered
        // every list pairing: same-typed keeps its carrier, cross-typed chains
        // boxed (the VM's result there is a Mixed list, which is what
        // `dyn_chain` builds). The method path had one arm — `ListI64` twice —
        // so `line.chain([byte])` with a boxed element did not lower, and in the
        // x86 kernel that one shape was eleven of the eighteen blockers.
        //
        // One operation, one rule: this mirrors `inst::scalar`'s `list_chain`.
        //
        // `concat` is the same operation under a second name, and it used to have
        // its own two narrower arms — one for `ListI64 ++ ListI64`, one for
        // "either side is boxed". So `xs.chain(ys)` lowered on all four carriers
        // while `xs.concat(ys)` lowered on two, and which spelling a program used
        // decided whether it stayed native. Both arms were subsumed by this one;
        // deleting them is the fix, not adding two more.
        (
            Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn,
            "chain" | "concat",
            [(other, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn | Ty::Dyn)],
        ) => {
            // A boxed argument is ordinary here: a callee's return type is
            // *observed*, and a long `a.chain(b).chain(c)…` chain can see one of
            // its operands as `Dyn` before the fixpoint has settled. Unboxing
            // through the tag guard is the same loud failure the VM gives for a
            // non-list, so nothing is guessed.
            let mut other_ty = args[0].1;
            let other = &if other_ty == Ty::Dyn {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "as_list"),
                    args: vec![*other],
                });
                other_ty = Ty::ListDyn;
                dst
            } else {
                *other
            };
            let (helper, out_ty) = match (receiver_ty, other_ty) {
                (Ty::ListI64, Ty::ListI64) => ("i64_chain", Ty::ListI64),
                (Ty::ListF64, Ty::ListF64) => ("f64_chain", Ty::ListF64),
                (Ty::ListStr, Ty::ListStr) => ("str_chain", Ty::ListStr),
                _ => ("dyn_chain", Ty::ListDyn),
            };
            let (lhs, rhs) = if out_ty == Ty::ListDyn {
                (
                    to_dyn_list_handle(ssa, insts, receiver, receiver_ty, pc)?,
                    to_dyn_list_handle(ssa, insts, *other, other_ty, pc)?,
                )
            } else {
                (receiver, *other)
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", helper),
                args: vec![lhs, rhs],
            });
            (dst, out_ty)
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
        // `flatten` on a carrier that cannot hold a list is a copy, and that is
        // exactly what `slice_from(0)` is. A typed list has no nesting to undo by
        // construction, so this needs no helper of its own — and without it a
        // program calling `.flatten()` generically fell off a cliff depending on
        // which carrier the list happened to have, which is not a distinction any
        // program can see.
        (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, "flatten", []) => {
            let helper = match receiver_ty {
                Ty::ListI64 => "i64_slice_from",
                Ty::ListF64 => "f64_slice_from",
                _ => "str_slice_from",
            };
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", helper),
                args: vec![receiver, zero],
            });
            (dst, receiver_ty)
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
        // `split` is an intrinsic in the bytecode compiler, so the *method*
        // spelling becomes `Opcode::StringSplit` and never arrives here. The
        // module spelling does arrive, now that `string.f(s, …)` forwards like
        // `iter.f(xs, …)` always has — same helper as the opcode lowering, so
        // the two spellings cannot drift.
        (Ty::Str, "split", [(sep, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "split"),
                args: vec![receiver, *sep],
            });
            (dst, Ty::ListStr)
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
        // The read surface every sequence shares, in *characters* — the unit
        // `len()` counts and `[i]` indexes.
        //
        // Only `substring(start, length)` and `find` used to lower, and both
        // called byte-indexed helpers while the VM counted characters, so the
        // two backends disagreed on any text with a multi-byte character in it.
        // Those two methods are gone; these are what replaced them, and
        // `str.slice_chars` has had the VM's exact semantics all along.
        (Ty::Str, "index_of", [(needle, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "index_of"),
                args: vec![receiver, *needle],
            });
            (dst, Ty::Dyn)
        }
        (Ty::Str, "slice", [(start, Ty::I64)]) => {
            let end = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(end),
                callee: AbiRef::new("str", "char_len"),
                args: vec![receiver],
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "slice_chars"),
                args: vec![receiver, *start, end],
            });
            (dst, Ty::Str)
        }
        (Ty::Str, "slice", [(start, Ty::I64), (end, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "slice_chars"),
                args: vec![receiver, *start, *end],
            });
            (dst, Ty::Str)
        }
        // Not `slice_chars(s, 0, n)`: a count is not a position, so a negative
        // one is a refusal rather than a window measured from the tail. Written
        // that way, `"abc".take(-1)` answered `"ab"` compiled and raised
        // interpreted — the List and Bytes carriers had guarded helpers all
        // along, and String is the one that reused the window.
        (Ty::Str, "take" | "skip", [(count, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", if name == "take" { "take" } else { "skip" }),
                args: vec![receiver, *count],
            });
            (dst, Ty::Str)
        }
        // `first`/`last` are `[0]` and `[-1]`, which `char_at` already is —
        // including the nil an empty string answers.
        (Ty::Str, "first" | "last", []) => {
            let index = ssa.new_val();
            insts.push(Inst::Const {
                dst: index,
                value: Const::I64(if name == "first" { 0 } else { -1 }),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "char_at"),
                args: vec![receiver, index],
            });
            (dst, Ty::Dyn)
        }
        // `"a {} b".format(x, …)` — the receiver is the template, which makes
        // this the same compile-time expansion `println("a {} b", x)` already
        // does. Both go through `format_parts`, so the placeholder rules
        // (leftover `{}` stay literal, leftover arguments append space
        // separated) cannot drift between the two spellings.
        //
        // Variadic, and its arguments' types vary — the reason it was the last
        // `string` member lowering on neither spelling. Neither matters once
        // the expansion is static: each argument is display-converted at its
        // own type, exactly as a `println` argument is.
        (Ty::Str, "format", _) => {
            // The template has to be a constant, for the same reason `println`'s
            // does: the pieces are decided at compile time. A computed template
            // falls back.
            let Some(fmt) = ssa.const_strs.get(&receiver).cloned() else {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "format needs a constant template to expand at compile time",
                });
            };
            let parts = crate::lower_module::format_parts(&fmt, args, pc)?;
            let (value, _fresh) = crate::lower_module::fold_parts_to_str(ssa, insts, globals, parts, pc)?;
            (value, Ty::Str)
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
        // The transforms that used to have only a module spelling. Each calls
        // the same `str` symbol the `string.…` row calls, so the two spellings
        // are one implementation here as well as in the VM.
        (Ty::Str, "capitalize" | "title", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", if name == "capitalize" { "capitalize" } else { "title" }),
                args: vec![receiver],
            });
            (dst, Ty::Str)
        }
        (Ty::Str, "strip", [(chars, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "strip"),
                args: vec![receiver, *chars],
            });
            (dst, Ty::Str)
        }
        // The fill is optional, and its default is a space — materialized here
        // rather than given a second ABI symbol, so both arities reach one
        // helper. (A row per arity is how `string.replace` ended up lowering
        // only when `all` was left out.)
        (Ty::Str, "pad_left" | "pad_right", [(width, Ty::I64)] | [(width, Ty::I64), (_, Ty::Str)]) => {
            let fill = match args {
                [_, (fill, _)] => *fill,
                _ => {
                    let space = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: space,
                        value: Const::Str(GlobalId(crate::prescan::intern_global(globals, " "))),
                    });
                    space
                }
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", if name == "pad_left" { "pad_left" } else { "pad_right" }),
                args: vec![receiver, *width, fill],
            });
            (dst, Ty::Str)
        }
        (Ty::Str, "count", [(needle, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "count"),
                args: vec![receiver, *needle],
            });
            (dst, Ty::I64)
        }
        // `String?`, so the carrier is Dyn — nil when the affix was not there.
        (Ty::Str, "strip_prefix" | "strip_suffix", [(affix, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new(
                    "str",
                    if name == "strip_prefix" {
                        "strip_prefix"
                    } else {
                        "strip_suffix"
                    },
                ),
                args: vec![receiver, *affix],
            });
            (dst, Ty::Dyn)
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
        // `s.chars()` — a dyn list, whose display quotes its strings exactly as
        // the VM's `TypedList::String` does. (The VM built a *Mixed* list when
        // this was written, which printed `[a,b]` against the module spelling's
        // `["a","b"]`; both sides say `["a","b"]` now.)
        (Ty::Str, "chars", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "chars"),
                args: vec![receiver],
            });
            (dst, Ty::ListDyn)
        }
        // `s.bytes()` — the string's UTF-8 bytes as a `Bytes` handle.
        (Ty::Str, "bytes", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "from_str"),
                args: vec![receiver],
            });
            (dst, Ty::Bytes)
        }
        // `xs.to_bytes()` — the inverse of `b.to_list()`, and the body behind
        // the `bytes.from_list(xs)` spelling.
        (Ty::ListI64, "to_bytes", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "from_i64_list"),
                args: vec![receiver],
            });
            (dst, Ty::Bytes)
        }
        (Ty::Bytes, "to_string_utf8" | "to_string_lossy", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", if name == "to_string_utf8" { "utf8" } else { "utf8_lossy" }),
                args: vec![receiver],
            });
            (dst, Ty::Str)
        }
        (Ty::Bytes, "concat", [(other, Ty::Bytes)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "concat"),
                args: vec![receiver, *other],
            });
            (dst, Ty::Bytes)
        }
        // The `bytes` module's members are also reachable as methods.
        (Ty::Bytes, "len", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "len"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        (Ty::Bytes, "is_empty", []) => {
            // Through `len == 0`, like `Set::is_empty`: the ABI answers an `i64`
            // and a `Bool` operand has to be an i1.
            let len = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(len),
                callee: AbiRef::new("bytes_h", "len"),
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
        (Ty::Bytes, "get", [(index, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "get"),
                args: vec![receiver, *index],
            });
            (dst, Ty::Dyn)
        }
        (Ty::Bytes, "slice", [(from, Ty::I64), (to, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "slice"),
                args: vec![receiver, *from, *to],
            });
            (dst, Ty::Bytes)
        }
        // The rest of `Bytes`. It had a carrier and four methods, so ten of its
        // fourteen dropped the whole module to the VM — a receiver kind that is
        // *almost* native is the shape a coverage number cannot show.
        //
        // `first`/`last` are `get(0)` / `get(-1)`: the read rule already counts a
        // negative position from the end, so they need no helper of their own.
        (Ty::Bytes, "first" | "last", []) => {
            let index = ssa.new_val();
            insts.push(Inst::Const {
                dst: index,
                value: Const::I64(if name == "first" { 0 } else { -1 }),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "get"),
                args: vec![receiver, index],
            });
            (dst, Ty::Dyn)
        }
        (Ty::Bytes, "take" | "skip", [(n, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", if name == "take" { "take" } else { "skip" }),
                args: vec![receiver, *n],
            });
            (dst, Ty::Bytes)
        }
        (Ty::Bytes, "to_list", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "to_i64_list"),
                args: vec![receiver],
            });
            (dst, Ty::ListI64)
        }
        // The two pure sequence operations `Bytes` was missing while it had
        // every other read of the list surface. `reverse` answers a `Bytes` —
        // shape-preserving and element-type-independent, like `take`/`slice`.
        (Ty::Bytes, "count", [(needle, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "count"),
                args: vec![receiver, *needle],
            });
            (dst, Ty::I64)
        }
        (Ty::Bytes, "reverse" | "sort" | "unique", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new(
                    "bytes_h",
                    match name {
                        "sort" => "sort",
                        "unique" => "unique",
                        _ => "reverse",
                    },
                ),
                args: vec![receiver],
            });
            (dst, Ty::Bytes)
        }
        (Ty::Bytes, "index_of", [(needle, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "index_of"),
                args: vec![receiver, *needle],
            });
            (dst, Ty::Dyn)
        }
        (Ty::Bytes, "contains", [(needle, Ty::I64)]) => {
            let raw = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(raw),
                callee: AbiRef::new("bytes_h", "contains"),
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
                lhs: raw,
                rhs: zero,
            });
            (b, Ty::Bool)
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
        // `index_of` on an int list. The VM has it on every sequence; here it
        // existed only on `Str`, so `[1,2,3].index_of(2)` dropped its module to
        // the VM — same answer, just slower, which is the kind of gap neither
        // the differential corpus nor the coverage gate can see.
        //
        // One arm per carrier, and each takes exactly the needle its `contains`
        // takes — in the VM both answer through one `typed_list_position`, so a
        // carrier that accepts a needle for `contains` and refuses it here would
        // make `xs.contains(v)` and `xs.index_of(v) != nil` disagree about which
        // programs lower.
        (Ty::ListI64, "index_of", [(v, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_index_of"),
                args: vec![receiver, *v],
            });
            (dst, Ty::Dyn)
        }
        (Ty::ListF64, "index_of", [(needle, Ty::F64 | Ty::I64)]) => {
            let needle = coerce_to_f64(ssa, insts, *needle, args[0].1);
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "f64_index_of"),
                args: vec![receiver, needle],
            });
            (dst, Ty::Dyn)
        }
        (Ty::ListStr, "index_of", [(needle, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "str_index_of"),
                args: vec![receiver, *needle],
            });
            (dst, Ty::Dyn)
        }
        (Ty::ListDyn, "index_of", [(needle, nty)]) => {
            let boxed = to_dyn(ssa, insts, *needle, *nty, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "dyn_index_of"),
                args: vec![receiver, boxed],
            });
            (dst, Ty::Dyn)
        }
        // `xs.contains(s)` on a string list. `str_contains` was declared in the
        // ABI and reached only from the `in` operator, so `"a" in xs` lowered and
        // `xs.contains("a")` did not — two spellings of one question, and the
        // comment above these arms states the invariant that breaks: a carrier
        // whose `index_of` lowers and whose `contains` does not makes
        // `xs.contains(v)` and `xs.index_of(v) != nil` disagree about which
        // programs stay native.
        (Ty::ListStr, "contains", [(v, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "str_contains"),
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
        // No method of that name on that receiver — but a map's entry or a
        // struct's field may *hold* a callable, which the interpreter calls
        // (`CallMethodK`'s callable-property path). `m["inc"](3)` and `h.f(2)`
        // are that, and they used to be the one spelling of a closure value
        // that did not lower: `let f = m["inc"]; f(3);` did, so one meaning had
        // a fast form and a slow one.
        //
        // Only after every real method arm has declined, so nothing here can
        // shadow a method.
        (Ty::MapStrDyn | Ty::Dyn, _, _) => {
            let key = materialize_key(ssa, insts, globals, name);
            let property = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(property),
                callee: AbiRef::new(
                    if receiver_ty == Ty::Dyn { "dyn" } else { "map_h" },
                    if receiver_ty == Ty::Dyn {
                        "map_get"
                    } else {
                        "str_dyn_get"
                    },
                ),
                args: vec![receiver, key],
            });
            let block_v = if args.is_empty() {
                let null = ssa.new_val();
                insts.push(Inst::Const {
                    dst: null,
                    value: Const::I64(0),
                });
                null
            } else {
                let b = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(b),
                    callee: AbiRef::new("rt", "spawn_args_new"),
                    args: Vec::new(),
                });
                for &(v, ty) in args {
                    let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("rt", "spawn_args_push"),
                        args: vec![b, boxed],
                    });
                }
                b
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("rt", "closure_call_property"),
                args: vec![property, block_v, key],
            });
            (dst, Ty::Dyn)
        }
        _ => {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this method on this receiver type",
            });
        }
    };
    let _ = globals;
    let _ = block;
    Ok(result)
}
