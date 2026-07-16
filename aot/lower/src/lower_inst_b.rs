use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_inst_b(
    ssa: &mut Ssa,
    block: usize,
    insts: &mut Vec<Inst>,
    func: &FunctionData,
    funcs: &[FunctionData],
    entry: u32,
    globals: &mut Vec<String>,
    module_globals: &[String],
    sig: &mut SigInfer,
    capture_params: &[(ValueId, Ty)],
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    match instr.opcode() {
        op @ (Opcode::CmpInt
        | Opcode::CmpNeInt
        | Opcode::CmpLtInt
        | Opcode::CmpLeInt
        | Opcode::CmpGtInt
        | Opcode::CmpGeInt) => {
            // Like arithmetic, comparisons dispatch on runtime operand type: two
            // ints → integer compare; any float operand → float compare (coercing);
            // two strings → a `strcmp`-style helper compared to 0. A `Maybe` operand
            // (dynamic index result) unwraps to `I64` here.
            //
            // `== nil` / `!= nil` resolves *before* the scalar read (which would
            // unwrap a Maybe, aborting on absent): a Maybe operand tests its
            // present bit, a concrete-typed operand folds to a constant (values
            // of non-Maybe types are never nil). Ordered nil comparisons are VM
            // errors, so they reject.
            let (lv_raw, lty_raw) = ssa.read(instr.b(), block, pc)?;
            let (rv_raw, rty_raw) = ssa.read(instr.c(), block, pc)?;
            if lty_raw == Ty::Nil || rty_raw == Ty::Nil {
                let cop = cmp_op(op);
                if !matches!(cop, CmpOp::Eq | CmpOp::Ne) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                let (other_v, other_ty) = if lty_raw == Ty::Nil {
                    (rv_raw, rty_raw)
                } else {
                    (lv_raw, lty_raw)
                };
                match other_ty {
                    Ty::Nil => {
                        let dst = ssa.new_val();
                        insts.push(Inst::Const {
                            dst,
                            value: Const::Bool(cop == CmpOp::Eq),
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    }
                    Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                        let present = ssa.new_val();
                        insts.push(Inst::MaybePresent {
                            dst: present,
                            src: other_v,
                            maybe_ty: other_ty,
                        });
                        if cop == CmpOp::Ne {
                            ssa.write(instr.a(), block, (present, Ty::Bool));
                        } else {
                            let dst = ssa.new_val();
                            insts.push(Inst::Not { dst, src: present });
                            ssa.write(instr.a(), block, (dst, Ty::Bool));
                        }
                    }
                    // A boxed Dyn: nil-ness is its tag (`0` = Nil).
                    Ty::Dyn => {
                        let tag = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(tag),
                            callee: AbiRef::new("dyn", "tag"),
                            args: vec![other_v],
                        });
                        let zero = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: zero,
                            value: Const::I64(0),
                        });
                        let dst = ssa.new_val();
                        insts.push(Inst::Cmp {
                            dst,
                            op: if cop == CmpOp::Eq { CmpOp::Eq } else { CmpOp::Ne },
                            float: false,
                            lhs: tag,
                            rhs: zero,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    }
                    _ => {
                        let dst = ssa.new_val();
                        insts.push(Inst::Const {
                            dst,
                            value: Const::Bool(cop == CmpOp::Ne),
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    }
                }
                return Ok(());
            }
            // A Dyn (or mixed-list) operand: box the other side and compare
            // through the `dyn.*` helpers (VM equality semantics live in
            // lkrt; ordered compares are numeric-only there, aborting like
            // the VM — which also errors on ordered list compares).
            if matches!(lty_raw, Ty::Dyn | Ty::ListDyn) || matches!(rty_raw, Ty::Dyn | Ty::ListDyn) {
                let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                let (helper, negate) = match cmp_op(op) {
                    CmpOp::Eq => ("eq", false),
                    CmpOp::Ne => ("eq", true),
                    CmpOp::Lt => ("lt", false),
                    CmpOp::Le => ("le", false),
                    CmpOp::Gt => ("gt", false),
                    CmpOp::Ge => ("ge", false),
                };
                let raw = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(raw),
                    callee: AbiRef::new("dyn", helper),
                    args: vec![lhs, rhs],
                });
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst,
                    op: if negate { CmpOp::Eq } else { CmpOp::Ne },
                    float: false,
                    lhs: raw,
                    rhs: zero,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            let (lv, lty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let (rv, rty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
            let (float, lhs, rhs) = match (lty, rty) {
                (Ty::I64, Ty::I64) => (false, lv, rv),
                // Bool equality (`b == true`): widen to i64 (the integer
                // compare renders `icmp … i64`); ordered comparisons on Bools
                // are VM errors, so they reject.
                (Ty::Bool, Ty::Bool) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let lw = ssa.new_val();
                    insts.push(Inst::ZextBool { dst: lw, src: lv });
                    let rw = ssa.new_val();
                    insts.push(Inst::ZextBool { dst: rw, src: rv });
                    (false, lw, rw)
                }
                (Ty::F64, Ty::F64) | (Ty::I64, Ty::F64) | (Ty::F64, Ty::I64) => (
                    true,
                    coerce_to_f64(ssa, insts, lv, lty),
                    coerce_to_f64(ssa, insts, rv, rty),
                ),
                // List structural equality: same length + element-wise `==` via
                // an lkrt helper returning 1/0, compared against 1 (so `!=`
                // reuses the same op). Int/Float lists compare with numeric
                // coercion (`[1] == [1.0]` is true); other cross-typed pairs
                // reject — folding them to `false` would be wrong for two
                // empty lists, which the VM deems equal regardless of type.
                (Ty::ListI64, Ty::ListI64)
                | (Ty::ListF64, Ty::ListF64)
                | (Ty::ListStr, Ty::ListStr)
                | (Ty::ListI64, Ty::ListF64)
                | (Ty::ListF64, Ty::ListI64) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let (helper, a, b) = match (lty, rty) {
                        (Ty::ListI64, Ty::ListI64) => ("i64_eq", lv, rv),
                        (Ty::ListF64, Ty::ListF64) => ("f64_eq", lv, rv),
                        (Ty::ListStr, Ty::ListStr) => ("str_eq", lv, rv),
                        // The mixed helper takes (ints, floats).
                        (Ty::ListI64, Ty::ListF64) => ("i64_f64_eq", lv, rv),
                        _ => ("i64_f64_eq", rv, lv),
                    };
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("list_h", helper),
                        args: vec![a, b],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    (false, eq, one)
                }
                // A dyn list against any list: both sides normalize to dyn
                // lists and compare structurally (`dyn_eq` recurses with the
                // VM's numeric coercion).
                (Ty::ListDyn, Ty::ListDyn | Ty::ListI64 | Ty::ListF64 | Ty::ListStr)
                | (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, Ty::ListDyn) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let a = to_dyn_list_handle(ssa, insts, lv, lty, pc)?;
                    let b = to_dyn_list_handle(ssa, insts, rv, rty, pc)?;
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("list_h", "dyn_eq"),
                        args: vec![a, b],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    (false, eq, one)
                }
                // Cross-typed list pairs beyond Int/Float can only be equal
                // when *both* are empty (the VM compares structurally
                // regardless of the typed-list representation). With both
                // proven non-empty at materialization (lengths never shrink),
                // the comparison folds; an unproven side could be empty at
                // runtime, so it rejects instead of guessing.
                (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, Ty::ListI64 | Ty::ListF64 | Ty::ListStr) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let lbase = ssa.list_base_len.get(&lv).copied().unwrap_or(0);
                    let rbase = ssa.list_base_len.get(&rv).copied().unwrap_or(0);
                    if lbase < 1 || rbase < 1 {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let dst = ssa.new_val();
                    insts.push(Inst::Const {
                        dst,
                        value: Const::Bool(cmp_op(op) == CmpOp::Ne),
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Bool));
                    return Ok(());
                }
                (Ty::Str, Ty::Str) => {
                    // The VM only supports `==`/`!=` on strings (ordered comparisons
                    // are a runtime error), so reject the rest — falling back rather
                    // than computing an order the VM would refuse.
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    // `str_cmp(a, b)` returns -1/0/1; comparing to 0 realizes `==`/`!=`.
                    let cmp = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(cmp),
                        callee: AbiRef::new("str", "cmp"),
                        args: vec![lv, rv],
                    });
                    let zero = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: zero,
                        value: Const::I64(0),
                    });
                    (false, cmp, zero)
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let dst = ssa.new_val();
            insts.push(Inst::Cmp {
                dst,
                op: cmp_op(op),
                float,
                lhs,
                rhs,
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
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
        Opcode::LoadCapture => {
            // `a` = dst, `bx` = capture index. Captures are cells: the loaded
            // register carries a cell ref whose `LoadCellVal` reads the hidden
            // trailing parameter (the cell's value at the call site). A direct
            // (non-cell) use of the register finds no SSA value and rejects.
            let k = instr.bx() as usize;
            if k >= capture_params.len() {
                return Err(Unsupported::BadConst { pc });
            }
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::CellParam(k));
        }
        Opcode::LoadCellVal => {
            // `a` = dst, `b` = cell register: reads the cell's current content.
            // The cell ref backtracks across blocks like any global ref; the
            // content read goes through the virtual slot (phis on demand).
            match ssa.builtin_ref_at(instr.b(), block) {
                Some(GlobalRef::CellParam(k)) => {
                    let &(v, ty) = capture_params.get(k).ok_or(Unsupported::BadConst { pc })?;
                    // A runtime cell (a `try$call` boundary capture) reads
                    // through the shared slot; a spawned goroutine reads its
                    // thread-private copy; by-value captures stay as-is.
                    if ty == Ty::Cell {
                        let dst = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("rt", "cell_get"),
                            args: vec![v],
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Dyn));
                    } else if ssa.spawned_isolate {
                        let slot = ssa.cellparam_slot(k);
                        let (sv, sty) = ssa.read_slot(slot, block, pc)?;
                        ssa.write(instr.a(), block, (sv, sty));
                    } else {
                        ssa.write(instr.a(), block, (v, ty));
                    }
                }
                Some(GlobalRef::Cell(cid)) => {
                    let slot = ssa.cell_slot(cid);
                    let (v, ty) = ssa.read_slot(slot, block, pc)?;
                    ssa.write(instr.a(), block, (v, ty));
                }
                _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            }
        }
        Opcode::StoreCellVal => {
            // `a` = cell register, `b` = value register: updates the tracked
            // cell content. A `CellParam` backed by a *runtime* cell (the
            // `try$call` boundary) writes through the shared slot; a
            // by-value capture parameter still rejects (no write-back path).
            match ssa.builtin_ref_at(instr.a(), block) {
                Some(GlobalRef::Cell(cid)) => {
                    let (v, ty) = ssa.read(instr.b(), block, pc)?;
                    let slot = ssa.cell_slot(cid);
                    ssa.write_slot(slot, block, (v, ty));
                }
                Some(GlobalRef::CellParam(k)) => {
                    let &(cell, cty) = capture_params.get(k).ok_or(Unsupported::BadConst { pc })?;
                    if cty == Ty::Cell {
                        let (v, ty) = ssa.read(instr.b(), block, pc)?;
                        let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("rt", "cell_set"),
                            args: vec![cell, boxed],
                        });
                    } else if ssa.spawned_isolate {
                        // Isolate: the write lands in the goroutine's private
                        // slot, never visible to the spawner (VM snapshot).
                        let (v, ty) = ssa.read(instr.b(), block, pc)?;
                        let slot = ssa.cellparam_slot(k);
                        ssa.write_slot(slot, block, (v, ty));
                    } else {
                        // A by-value capture parameter has no write-back path.
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                    }
                }
                _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            }
        }
        Opcode::SetGlobal => {
            // Storing a function value into the global table is the compiler's
            // top-level `fn` bookkeeping — a no-op natively.
            if let Some(GlobalRef::UserFn) = ssa.builtin_regs.get(&(block, instr.a())) {
                return Ok(());
            }
            // A top-level `let f = |x| …` stores a lambda ref: a no-op when the
            // prescan proved the slot single-assigned with this exact closure
            // (readers resolve it statically); anything else would let readers
            // observe a stale ref, so it rejects.
            if let Some(GlobalRef::Lambda(fidx)) = ssa.builtin_regs.get(&(block, instr.a())) {
                if sig.lambda_globals.get(instr.bx() as usize).copied().flatten() == Some(*fidx) {
                    return Ok(());
                }
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            // Writing a global whose *name* this lowering recognizes would let
            // later `GetGlobal` reads resolve to the stale builtin/module
            // meaning and miscompile (`println = f; println(x)`), so those
            // writes reject the program.
            let slot = instr.bx();
            let name = module_globals.get(slot as usize).map(String::as_str);
            if let Some(name) = name
                && (matches!(
                    name,
                    "println" | "print" | "assert" | "assert_eq" | "assert_ne" | "panic" | "typeof" | "Set"
                ) || module_global(name))
            {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            // Mutable module global (a top-level `let` shared with functions).
            // Scalar slots stay typed when every write agrees; disagreeing or
            // non-scalar (but boxable) writes join the slot to `Dyn` — each
            // write boxes, reads flow through the Dyn arms (plan M4.2).
            let (v, ty) = ssa.read(instr.a(), block, pc)?;
            let obs = match ty {
                Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str => ty,
                t if dyn_boxable_ty(t) => Ty::Dyn,
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            // A reader discovered this slot can be observed before its first
            // (non-prefix) write: only the Dyn carrier's zeroinit is nil.
            let obs = if sig.force_dyn_globals.contains(&slot) {
                Ty::Dyn
            } else {
                obs
            };
            let slot_ty = match sig.global_tys.get_mut(slot as usize) {
                Some(state @ None) => {
                    *state = Some(obs);
                    obs
                }
                Some(Some(prev)) if *prev != obs => {
                    *prev = Ty::Dyn;
                    Ty::Dyn
                }
                Some(Some(prev)) => *prev,
                None => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            };
            let v = if slot_ty == Ty::Dyn && ty != Ty::Dyn {
                to_dyn_any(ssa, insts, v, ty, pc)?
            } else {
                v
            };
            insts.push(Inst::GlobalSet {
                gvar: sig.gvar(slot),
                src: v,
            });
        }
        Opcode::GetGlobal => {
            // Reads of known runtime builtins / stdlib module objects load a
            // global ref (no SSA value — any unrecognized use finds it
            // undefined and rejects). Reads of mutable scalar globals load the
            // typed value, but only for slots provably initialized in the
            // entry prefix (the VM's pre-write value is nil, native storage is
            // zero — a read that could observe it must reject).
            let slot = instr.bx();
            let name = module_globals.get(slot as usize).map(String::as_str);
            let global_ref = match name {
                Some("println") => Some(GlobalRef::Builtin(Builtin::Println)),
                Some("print") => Some(GlobalRef::Builtin(Builtin::Print)),
                Some("assert") => Some(GlobalRef::Builtin(Builtin::Assert)),
                Some("assert_eq") => Some(GlobalRef::Builtin(Builtin::AssertEq)),
                Some("assert_ne") => Some(GlobalRef::Builtin(Builtin::AssertNe)),
                Some("panic") => Some(GlobalRef::Builtin(Builtin::Panic)),
                Some("typeof") => Some(GlobalRef::Builtin(Builtin::Typeof)),
                Some("__lk_call_method") => Some(GlobalRef::Builtin(Builtin::CallMethod)),
                Some("Set") => Some(GlobalRef::Builtin(Builtin::SetCtor)),
                Some("try$call") => Some(GlobalRef::Builtin(Builtin::TryCall)),
                Some("error") => Some(GlobalRef::Builtin(Builtin::ErrorRaise)),
                Some("__lk_merge_fields") => Some(GlobalRef::Builtin(Builtin::MergeFields)),
                Some("__lk_make_struct") => Some(GlobalRef::Builtin(Builtin::MakeStruct)),
                Some("__lk_bit_and") => Some(GlobalRef::Builtin(Builtin::BitAnd)),
                Some("__lk_bit_or") => Some(GlobalRef::Builtin(Builtin::BitOr)),
                Some("__lk_bit_not") => Some(GlobalRef::Builtin(Builtin::BitNot)),
                Some("chan") => Some(GlobalRef::Builtin(Builtin::ChanNew)),
                Some("send") => Some(GlobalRef::Builtin(Builtin::ChanSend)),
                Some("recv") => Some(GlobalRef::Builtin(Builtin::ChanRecv)),
                Some("spawn") => Some(GlobalRef::Builtin(Builtin::Spawn)),
                Some("select$block") => Some(GlobalRef::Builtin(Builtin::SelectBlock)),
                // Two-level stdlib exports arrive as `module::member` global
                // names (`chan.close(c)` → `GetGlobal "chan::close"`).
                Some(name) if name.contains("::") => {
                    let (module, member) = name.split_once("::").expect("checked");
                    Some(GlobalRef::ModuleFn(module.to_string(), member.to_string()))
                }
                Some(name) if module_global(name) => Some(GlobalRef::Module(name.to_string())),
                _ => None,
            };
            if let Some(global_ref) = global_ref {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
                return Ok(());
            }
            // Import-derived bindings (aliases, `use {..} from`, bundled file
            // modules): only when the slot is never written (a user global of
            // the same name shadows the import, like the VM's environment).
            if sig.global_tys.get(slot as usize).copied().flatten().is_none()
                && let Some(name) = name
            {
                if let Some(module) = sig.imports.module_aliases.get(name) {
                    let global_ref = GlobalRef::Module(module.clone());
                    ssa.builtin_regs.insert((block, instr.a()), global_ref);
                    return Ok(());
                }
                if let Some((module, member)) = sig.imports.module_items.get(name) {
                    // `use { json } from encoding` binds a *submodule* object,
                    // not a function: member reads route through Module.
                    let global_ref = if is_submodule(module, member) {
                        GlobalRef::Module(member.clone())
                    } else {
                        GlobalRef::ModuleFn(module.clone(), member.clone())
                    };
                    ssa.builtin_regs.insert((block, instr.a()), global_ref);
                    return Ok(());
                }
                if let Some(&fidx) = sig.imports.file_items.get(name) {
                    ssa.builtin_regs.insert((block, instr.a()), GlobalRef::Lambda(fidx));
                    return Ok(());
                }
                if let Some(&bundle) = sig.imports.file_namespaces.get(name) {
                    ssa.builtin_regs
                        .insert((block, instr.a()), GlobalRef::UserModule(bundle));
                    return Ok(());
                }
            }
            // A single-assignment top-level lambda slot resolves statically to
            // its function reference (initialization-order safe: the prescan
            // only accepts entry-prefix writes, which precede any user call).
            if let Some(fidx) = sig.lambda_globals.get(slot as usize).copied().flatten() {
                ssa.builtin_regs.insert((block, instr.a()), GlobalRef::Lambda(fidx));
                return Ok(());
            }
            let initialized = sig.initialized_globals.get(slot as usize).copied().unwrap_or(false);
            let ty = sig.global_tys.get(slot as usize).copied().flatten();
            let Some(ty) = ty else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            // A typed slot read before its entry-prefix initialization could
            // observe native zero where the VM has nil. A `Dyn` slot is
            // exempt — its zeroinit `{0, 0}` *is* the nil tag, VM-exact — so
            // force the slot Dyn and rerun (retriable discovery: writes box,
            // an early read observes boxed nil).
            if !initialized && ty != Ty::Dyn {
                sig.force_dyn_globals.insert(slot);
                return Err(Unsupported::TypeMismatch { pc });
            }
            let dst = ssa.new_val();
            insts.push(Inst::GlobalGet {
                dst,
                gvar: sig.gvar(slot),
            });
            ssa.write(instr.a(), block, (dst, ty));
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
        // A string constant materializes an interned module global (a C-string) with
        // type `Str`. String *operations* (concat/compare/…) aren't modelled, so a
        // `Str` flowing into one rejects (falls back); but a returned literal prints,
        // and a constant map key is consumed directly by the map ABI.
        Opcode::LoadString => {
            let s = func
                .consts
                .strings
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let gid = intern_global(globals, s);
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Str(GlobalId(gid)),
            });
            ssa.const_strs.insert(dst, s.clone());
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::ToString => {
            // `a` = dst, `b` = source. Display-convert to a `Str` (Str/Int/Bool
            // supported; float/other fall back).
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            // Auto-Display (plan J1): a single-interpolation template string
            // (`"${point}"`) compiles to a bare `ToString`.
            let (v, ty) = apply_display_show(ssa, insts, funcs, entry, sig, v, ty, pc)?;
            // The result is register-visible, so it stays arena-owned (never
            // freed eagerly, reclaimed by `lkrt_cleanup` at exit).
            let (s, _fresh) = to_display_str(ssa, insts, globals, v, ty, false, pc)?;
            ssa.write(instr.a(), block, (s, Ty::Str));
        }
        Opcode::ConcatString => {
            // `a` = dst, `b` = lhs, `c` = rhs. Concatenate `display(lhs) ++
            // display(rhs)` (each operand display-converted, as the VM does);
            // an int rhs fuses into a single `concat_i64` call.
            let (lv, lty) = ssa.read(instr.b(), block, pc)?;
            let (rv, rty) = ssa.read(instr.c(), block, pc)?;
            // Auto-Display (plan J1): a struct-instance operand with a
            // registered `show` interpolates its result, like the VM.
            let (lv, lty) = apply_display_show(ssa, insts, funcs, entry, sig, lv, lty, pc)?;
            let (rv, rty) = apply_display_show(ssa, insts, funcs, entry, sig, rv, rty, pc)?;
            let (l, l_fresh) = to_display_str(ssa, insts, globals, lv, lty, false, pc)?;
            let dst = concat_display(ssa, insts, globals, l, rv, rty, false, pc)?;
            if l_fresh {
                free_owned_str(insts, l);
            }
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::ConcatN => {
            // `a` = dst, `b` = first element register, `c` = element count. The VM
            // display-converts each element then concatenates; each element is
            // display-converted (`Str`/`Int`/`Bool`) and folded via repeated
            // `str_concat` — int elements fuse into `concat_i64` (no suffix
            // temporary). A float/other element rejects (falls back).
            let start = instr.b();
            let count = instr.c() as usize;
            let result = if count == 0 {
                // Empty concat → the empty string.
                let gid = intern_global(globals, "");
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::Str(GlobalId(gid)),
                });
                dst
            } else {
                let (v0, ty0) = ssa.read(start, block, pc)?;
                let (v0, ty0) = apply_display_show(ssa, insts, funcs, entry, sig, v0, ty0, pc)?;
                let (mut acc, mut acc_fresh) = to_display_str(ssa, insts, globals, v0, ty0, false, pc)?;
                for i in 1..count {
                    let (v, ty) = ssa.read(start.wrapping_add(i as u8), block, pc)?;
                    let (v, ty) = apply_display_show(ssa, insts, funcs, entry, sig, v, ty, pc)?;
                    let dst = concat_display(ssa, insts, globals, acc, v, ty, false, pc)?;
                    // The consumed accumulator is dead; free it if this
                    // lowering allocated it.
                    if acc_fresh {
                        free_owned_str(insts, acc);
                    }
                    acc = dst;
                    acc_fresh = true;
                }
                acc
            };
            ssa.write(instr.a(), block, (result, Ty::Str));
        }
        Opcode::ListJoin => {
            // `a` = dst, `b` = list, `c` = separator. The VM joins a *string* list; we
            // support `List<str>` with a `Str` separator → a fresh `Str`.
            let (handle, list_ty) = ssa.read(instr.b(), block, pc)?;
            if list_ty != Ty::ListStr {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let sep = ssa.read_typed(instr.c(), block, Ty::Str, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "str_join"),
                args: vec![handle, sep],
            });
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        _ => {
            return lower_inst_c(
                ssa,
                block,
                insts,
                func,
                funcs,
                entry,
                globals,
                module_globals,
                sig,
                capture_params,
                instr,
                pc,
            );
        }
    }
    Ok(())
}
