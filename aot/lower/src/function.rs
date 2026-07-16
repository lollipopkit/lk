use super::*;

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

    // 1. Classify control-flow exits; a fused `TestXxx`+`Jmp` consumes the `Jmp`.
    let mut consumed = vec![false; code_len];
    let exits: Vec<Option<Exit>> = (0..code_len)
        .map(|pc| exit_of(pc, &instrs, code_len, &mut consumed, &func.performance))
        .collect::<Result<Vec<_>, _>>()?;

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
        fn_params.push((pv, pty));
    }
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
    let mut capture_params: Vec<(ValueId, Ty)> = Vec::with_capacity(capture_count);
    for k in 0..capture_count {
        let cty = sig.param_ty(func_index as usize, param_count + env_total + k);
        let cv = ssa.new_val();
        capture_params.push((cv, cty));
        fn_params.push((cv, cty));
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
    let mut ret_val: Vec<Option<ValueId>> = vec![None; total_blocks];
    let mut cond_val: Vec<Option<ValueId>> = vec![None; total_blocks];

    for (bi, &(start, end)) in block_bounds.iter().enumerate() {
        ssa.seal_ready()?;
        let (body_end, exit) = block_span(&exits, &consumed, start, end);
        if exit.is_none() {
            ssa.single_fallthrough_target[bi] = Some(end);
        }
        let mut insts = Vec::new();
        #[allow(clippy::needless_range_loop)] // `pc` is the semantic bytecode index
        for pc in start..body_end {
            // Trait registration sequences were lifted into `sig.traits` by
            // the prescan; their instructions never lower (plan J1).
            if is_entry && sig.traits.skip_pcs.contains(&pc) {
                continue;
            }
            lower_inst(
                &mut ssa,
                bi,
                &mut insts,
                func,
                funcs,
                entry,
                globals,
                module_globals,
                sig,
                &capture_params,
                &instrs[pc],
                pc,
            )?;
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
    let mut mir_blocks: Vec<Block> = Vec::with_capacity(total_blocks);
    for bi in 0..leader_vec.len() {
        let params: Vec<(ValueId, Ty)> = ssa.phis[bi].iter().map(|p| (p.param, p.ty)).collect();
        let exit = block_exit[bi];
        // Phi-edge conversions land after the block's own instructions,
        // before the terminator.
        let edge_tail = std::mem::take(&mut ssa.edge_insts[bi]);
        let term = build_term(bi, exit, &ssa, &block_id, ret_val[bi], cond_val[bi]);
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

    let ret = ret_ty.unwrap_or(Ty::Nil);
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
    Ok(MirFunction {
        id: FuncId(func_index),
        params: fn_params,
        entry: BlockId(0),
        ret,
        blocks: mir_blocks,
    })
}
