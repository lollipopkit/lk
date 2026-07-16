use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_inst(
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
        Opcode::LoadInt => {
            let value = *func
                .consts
                .ints
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::I64(value),
            });
            ssa.const_int.insert(dst, value);
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        Opcode::LoadFloat => {
            let value = *func
                .consts
                .floats
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::F64(value),
            });
            ssa.write(instr.a(), block, (dst, Ty::F64));
        }
        Opcode::LoadBool => {
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Bool(instr.b() != 0),
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::LoadNil => {
            let dst = ssa.new_val();
            insts.push(Inst::Const { dst, value: Const::Nil });
            ssa.write(instr.a(), block, (dst, Ty::Nil));
        }
        Opcode::Move => {
            // A register holding a global ref (no SSA value) propagates the ref
            // instead of a value: the compiler moves the callee into the
            // call-window base before the `Call`. Cross-block backtracking
            // covers refs inherited from predecessors (an SSA definition in
            // this block shadows; conflicting paths resolve to None).
            if let Some(global_ref) = ssa.builtin_ref_at(instr.b(), block) {
                // An `ArgList` view coexists with a materialized SSA handle
                // ("both views", see `NewList`): propagate the SSA half too,
                // so index/display through the moved register keep working.
                // Only for ArgList — for every other ref kind an SSA write
                // would shadow the ref at its consumers (e.g. a recycled
                // register's stale definition burying a `println` ref).
                let dual_view = matches!(global_ref, GlobalRef::ArgList(_));
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
                if dual_view && let Some(src) = ssa.current_def[block][instr.b() as usize] {
                    ssa.write(instr.a(), block, src);
                }
                return Ok(());
            }
            let src = ssa.read(instr.b(), block, pc)?;
            ssa.write(instr.a(), block, src);
        }
        Opcode::Move2 => {
            // Fused adjacent moves: `a ← b`, then `b ← c`. The VM reads `b`
            // before overwriting it; SSA reads naturally see the old value.
            if let Some(global_ref) = ssa.builtin_ref_at(instr.b(), block) {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
            } else {
                let first = ssa.read(instr.b(), block, pc)?;
                ssa.write(instr.a(), block, first);
            }
            if let Some(global_ref) = ssa.builtin_ref_at(instr.c(), block) {
                ssa.builtin_regs.insert((block, instr.b()), global_ref);
            } else {
                let second = ssa.read(instr.c(), block, pc)?;
                ssa.write(instr.b(), block, second);
            }
        }
        Opcode::IsNil => {
            // `a` = dst, `b` = src. The statically-typed subset resolves most nil
            // tests at lower time: concrete scalars are never nil, `Nil` always
            // is; a Maybe carrier (dynamic map/list read) tests its present bit.
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            let dst = ssa.new_val();
            match ty {
                Ty::Nil => insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(true),
                }),
                Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str => insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(false),
                }),
                Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                    let present = ssa.new_val();
                    insts.push(Inst::MaybePresent {
                        dst: present,
                        src: v,
                        maybe_ty: ty,
                    });
                    insts.push(Inst::Not { dst, src: present });
                }
                // A boxed Dyn (struct field / mixed-container read): nil-ness
                // is its tag (`0` = Nil), same as the Cmp `== nil` arm.
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
                    insts.push(Inst::Cmp {
                        dst,
                        op: CmpOp::Eq,
                        float: false,
                        lhs: tag,
                        rhs: zero,
                    });
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::IsList => {
            // `a` = dst, `b` = src. In the statically-typed subset the list-ness
            // of a register is known at lower time: a typed list handle is a
            // list; every other lowerable type (scalars, maps, maybe-carriers,
            // nil) is not. Const-folds to a `Bool`, mirroring the VM's
            // `runtime_value_is_list`.
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            // A boxed Dyn is list-ness only at runtime: test its tag (5 =
            // DYN_LIST). Everything else const-folds.
            if ty == Ty::Dyn {
                let tag = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(tag),
                    callee: AbiRef::new("dyn", "tag"),
                    args: vec![v],
                });
                let want = ssa.new_val();
                insts.push(Inst::Const {
                    dst: want,
                    value: Const::I64(5),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst,
                    op: CmpOp::Eq,
                    float: false,
                    lhs: tag,
                    rhs: want,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            let is_list = matches!(ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn);
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Bool(is_list),
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::IsMap => {
            // `a` = dst, `b` = src. Analogous to `IsList`: a typed map handle is
            // a map at lower time; every other lowerable type is not. Const-folds
            // to a `Bool`, mirroring the VM's `runtime_value_is_map`.
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            if ty == Ty::Dyn {
                let tag = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(tag),
                    callee: AbiRef::new("dyn", "tag"),
                    args: vec![v],
                });
                let want = ssa.new_val();
                insts.push(Inst::Const {
                    dst: want,
                    value: Const::I64(6),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst,
                    op: CmpOp::Eq,
                    float: false,
                    lhs: tag,
                    rhs: want,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            let is_map = matches!(
                ty,
                Ty::MapStrI64 | Ty::MapI64I64 | Ty::MapStrF64 | Ty::MapI64F64 | Ty::MapStrBool | Ty::MapStrDyn
            );
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Bool(is_map),
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::Not => {
            // `!x`: `a` = dst, `b` = src. The VM negates a `Bool` and treats `Nil` as
            // `true`; a non-bool/non-nil operand is a VM error, so reject (fall back).
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            let dst = ssa.new_val();
            match ty {
                Ty::Bool => insts.push(Inst::Not { dst, src: v }),
                Ty::Nil => insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(true),
                }),
                // A boxed value dispatches at runtime (Bool/Nil legal,
                // anything else the VM's loud error): `dyn.not` → 0/1.
                Ty::Dyn => {
                    let wide = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(wide),
                        callee: AbiRef::new("dyn", "not"),
                        args: vec![v],
                    });
                    let zero = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: zero,
                        value: Const::I64(0),
                    });
                    insts.push(Inst::Cmp {
                        dst,
                        op: CmpOp::Ne,
                        float: false,
                        lhs: wide,
                        rhs: zero,
                    });
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        op @ (Opcode::AddInt | Opcode::SubInt | Opcode::MulInt | Opcode::DivInt | Opcode::ModInt) => {
            // These opcodes dispatch on runtime operand type in the VM: two ints →
            // integer op; any float operand → coerce ints to float and use the float
            // op (matching `dynamic_add`/etc.). We resolve that dispatch statically.
            // A `Maybe` operand (dynamic index result) unwraps to `I64` here.
            //
            // A Dyn operand routes both sides through the `dyn.*` helpers,
            // which carry the same promotion rules at runtime (`/` always
            // Float, type errors abort like the VM). Result stays `Ty::Dyn`.
            {
                let (lv_raw, lty_raw) = ssa.read(instr.b(), block, pc)?;
                let (rv_raw, rty_raw) = ssa.read(instr.c(), block, pc)?;
                // `Str + Dyn`: the VM only accepts Str + Str here (anything
                // else is a loud error), so unbox the Dyn side through the
                // `as_str` tag guard (same loud failure) and emit a *typed*
                // concat — the result stays `Str`, keeping a loop
                // accumulator (`acc += s[i]`) same-typed through its phi.
                if op == Opcode::AddInt && matches!((lty_raw, rty_raw), (Ty::Str, Ty::Dyn) | (Ty::Dyn, Ty::Str)) {
                    let unbox = |ssa: &mut Ssa, insts: &mut Vec<Inst>, v: ValueId, ty: Ty| {
                        if ty == Ty::Dyn {
                            let dst = ssa.new_val();
                            insts.push(Inst::Call {
                                dst: Some(dst),
                                callee: AbiRef::new("dyn", "as_str"),
                                args: vec![v],
                            });
                            dst
                        } else {
                            v
                        }
                    };
                    let lhs = unbox(ssa, insts, lv_raw, lty_raw);
                    let rhs = unbox(ssa, insts, rv_raw, rty_raw);
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![lhs, rhs],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Str));
                    return Ok(());
                }
                // `list + list` concatenates into a fresh list (the VM's
                // AddInt dispatch; the `[a, ..spread, b]` literal desugars to
                // an `+` chain). Same-typed operands keep the typed carrier —
                // display stays typed-exact (a `List<str>` result still
                // quotes) — while a Dyn/mixed side chains boxed (the VM's
                // Mixed result displays bare, matching `dyn_chain`).
                let is_list = |t: Ty| matches!(t, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn);
                let list_chain = |lty: Ty, rty: Ty| match (lty, rty) {
                    _ if op != Opcode::AddInt => None,
                    (Ty::ListI64, Ty::ListI64) => Some(("i64_chain", Ty::ListI64)),
                    (Ty::ListF64, Ty::ListF64) => Some(("f64_chain", Ty::ListF64)),
                    (Ty::ListStr, Ty::ListStr) => Some(("str_chain", Ty::ListStr)),
                    // Cross-typed operands chain boxed — the VM's result is a
                    // Mixed list (bare-text display), exactly `dyn_chain`.
                    (l, r) if is_list(l) && is_list(r) => Some(("dyn_chain", Ty::ListDyn)),
                    _ => None,
                };
                if let Some((helper, out_ty)) = list_chain(lty_raw, rty_raw) {
                    let (lhs, rhs) = if out_ty == Ty::ListDyn {
                        (
                            to_dyn_list_handle(ssa, insts, lv_raw, lty_raw, pc)?,
                            to_dyn_list_handle(ssa, insts, rv_raw, rty_raw, pc)?,
                        )
                    } else {
                        (lv_raw, rv_raw)
                    };
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("list_h", helper),
                        args: vec![lhs, rhs],
                    });
                    ssa.write(instr.a(), block, (dst, out_ty));
                    return Ok(());
                }
                if lty_raw == Ty::Dyn || rty_raw == Ty::Dyn {
                    let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                    let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                    let helper = match op {
                        Opcode::AddInt => "add",
                        Opcode::SubInt => "sub",
                        Opcode::MulInt => "mul",
                        Opcode::DivInt => "div",
                        _ => "mod",
                    };
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("dyn", helper),
                        args: vec![lhs, rhs],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Dyn));
                    return Ok(());
                }
            }
            let (lv, lty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let (rv, rty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
            match (lty, rty) {
                (Ty::I64, Ty::I64) => {
                    let dst = ssa.new_val();
                    insts.push(Inst::IntBin {
                        dst,
                        op: int_bin_op(op),
                        lhs: lv,
                        rhs: rv,
                    });
                    ssa.write(instr.a(), block, (dst, Ty::I64));
                }
                (Ty::F64, Ty::F64) | (Ty::I64, Ty::F64) | (Ty::F64, Ty::I64) => {
                    let lhs = coerce_to_f64(ssa, insts, lv, lty);
                    let rhs = coerce_to_f64(ssa, insts, rv, rty);
                    let dst = ssa.new_val();
                    insts.push(Inst::FloatBin {
                        dst,
                        op: int_to_float_bin_op(op),
                        lhs,
                        rhs,
                    });
                    ssa.write(instr.a(), block, (dst, Ty::F64));
                }
                // `str + str` is concatenation (the VM's `AddInt` dispatches to it);
                // only `+` is defined on strings — `-`/`*`/… are VM errors, so reject.
                (Ty::Str, Ty::Str) if matches!(op, Opcode::AddInt) => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![lv, rv],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Str));
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
        }
        Opcode::NewList => {
            // `a` = dst, `b` = base, `c` = count: a register-window list. The
            // compiler also uses this to box method-call arguments, so the raw
            // elements are always recorded as an ArgList ref; a homogeneous
            // scalar window additionally materializes a real list handle.
            let count = instr.c() as usize;
            let mut elems = Vec::with_capacity(count);
            for i in 0..count {
                let reg = instr.b().wrapping_add(i as u8);
                elems.push(ssa.read(reg, block, pc)?);
            }
            let all = |t: Ty| elems.iter().all(|&(_, ty)| ty == t);
            let materialized = if !elems.is_empty() && all(Ty::I64) {
                Some(("i64_new", "i64_push", Ty::ListI64))
            } else if !elems.is_empty() && all(Ty::F64) {
                Some(("f64_new", "f64_push", Ty::ListF64))
            } else if !elems.is_empty() && all(Ty::Str) {
                Some(("str_new", "str_push", Ty::ListStr))
            } else {
                None
            };
            if let Some((new_fn, push_fn, list_ty)) = materialized {
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", new_fn),
                    args: Vec::new(),
                });
                for &(v, _) in &elems {
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", push_fn),
                        args: vec![handle, v],
                    });
                }
                ssa.list_len.insert(handle, elems.len() as i64);
                ssa.list_base_len.insert(handle, elems.len() as i64);
                ssa.write(instr.a(), block, (handle, list_ty));
            } else if !elems.is_empty()
                && elems.iter().all(|&(_, ty)| {
                    matches!(
                        ty,
                        Ty::I64
                            | Ty::F64
                            | Ty::Str
                            | Ty::Bool
                            | Ty::Nil
                            | Ty::Dyn
                            | Ty::ListI64
                            | Ty::ListF64
                            | Ty::ListStr
                            | Ty::ListDyn
                            | Ty::MapStrDyn
                    )
                })
            {
                // Mixed (or Dyn-carrying) elements: materialize a boxed-dynamic
                // list (plan M4.2), same as the constant mixed-list path but
                // boxing runtime values via `to_dyn`.
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", "dyn_new"),
                    args: Vec::new(),
                });
                // A repeated element boxes once: the VM pushes the same heap
                // handle twice (`[l, l]` dedups under `unique()`), so the
                // boxed views must share pointer identity too.
                let mut boxed_memo: std::collections::HashMap<ValueId, ValueId> = std::collections::HashMap::new();
                for &(v, ty) in &elems {
                    let boxed = match boxed_memo.get(&v) {
                        Some(&cached) => cached,
                        None => {
                            let boxed = to_dyn(ssa, insts, v, ty, pc)?;
                            boxed_memo.insert(v, boxed);
                            boxed
                        }
                    };
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "dyn_push"),
                        args: vec![handle, boxed],
                    });
                }
                ssa.list_len.insert(handle, elems.len() as i64);
                ssa.list_base_len.insert(handle, elems.len() as i64);
                ssa.write(instr.a(), block, (handle, Ty::ListDyn));
            } else if elems.is_empty() {
                // An empty literal (`let flat = [];`) materializes as an
                // empty dyn list: later pushes box their elements, and the
                // cross-typed Cmp arms cover `[] == [1, 2]`-style compares.
                // (Call-window `NewList 0` also lands here; the dead handle
                // is one no-arg call.) 旧留档顾虑(typed eq lowering)已被
                // typed↔Dyn 跨型比较解除。
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", "dyn_new"),
                    args: Vec::new(),
                });
                ssa.list_len.insert(handle, 0);
                ssa.list_base_len.insert(handle, 0);
                ssa.write(instr.a(), block, (handle, Ty::ListDyn));
            }
            // Recorded after the write (which clears the slot) so both views
            // coexist: SSA reads see the handle, method dispatch sees elements.
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::ArgList(elems));
        }
        Opcode::GetIndexStrI | Opcode::SetIndexStrI => {
            // Composite string-int key access (`m["n${i}"]`): the key is the
            // compiler-proven constant prefix plus the decimal suffix register.
            // A store passes (prefix, suffix) straight to the `set_ik` ABI (key
            // built on the stack inside lkrt, nothing to free); a load builds
            // the key via `concat_i64` in one allocation, and the fresh
            // temporary frees right after the map call.
            let Some(key_fact) = func.performance.known_key(pc).and_then(|fact| fact.string_int) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let prefix = func
                .consts
                .strings
                .get(key_fact.prefix_key as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let prefix_v = materialize_key(ssa, insts, globals, prefix);
            let is_set = instr.opcode() == Opcode::SetIndexStrI;
            let (map_reg, suffix_reg) = if is_set {
                (instr.a(), instr.b())
            } else {
                (instr.b(), instr.c())
            };
            let (handle, map_ty) = ssa.read(map_reg, block, pc)?;
            let suffix = ssa.read_typed(suffix_reg, block, Ty::I64, pc)?;
            if is_set {
                let (value, value_ty) = ssa.read(instr.c(), block, pc)?;
                let set_fn = match (map_ty, value_ty) {
                    (Ty::MapStrI64, Ty::I64) => "str_i64_set_ik",
                    (Ty::MapStrF64, Ty::F64) => "str_f64_set_ik",
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", set_fn),
                    args: vec![handle, prefix_v, suffix, value],
                });
            } else {
                let key = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(key),
                    callee: AbiRef::new("str", "concat_i64"),
                    args: vec![prefix_v, suffix],
                });
                let dst = ssa.new_val();
                let maybe_ty = match map_ty {
                    Ty::MapStrI64 => {
                        insts.push(Inst::MapGetMaybe { dst, handle, key });
                        Ty::MaybeI64
                    }
                    Ty::MapStrF64 => {
                        insts.push(Inst::MapGetMaybeStrF64 { dst, handle, key });
                        Ty::MaybeF64
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                free_owned_str(insts, key);
                ssa.write(instr.a(), block, (dst, maybe_ty));
            }
        }
        Opcode::MidInt => {
            // `a = (b + c) / 2` — wrapping add then truncated division
            // (`wrapping_add / 2` in the VM; the guarded div helper's
            // `wrapping_div` by the constant 2 is identical).
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let sum = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: sum,
                op: IntBinOp::Add,
                lhs,
                rhs,
            });
            let two = ssa.new_val();
            insts.push(Inst::Const {
                dst: two,
                value: Const::I64(2),
            });
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: IntBinOp::Div,
                lhs: sum,
                rhs: two,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        Opcode::MinInt | Opcode::MaxInt => {
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let op = if instr.opcode() == Opcode::MinInt {
                IntBinOp::Min
            } else {
                IntBinOp::Max
            };
            let dst = ssa.new_val();
            insts.push(Inst::IntBin { dst, op, lhs, rhs });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddMulInt | Opcode::Add2Int) => {
            // Fused accumulator updates: `a += b * c` / `a += b + c` (all Int,
            // wrapping — matching the VM's typed fast path, which bails on
            // non-Int operands, so the static Int requirement is exact).
            let acc = read_typed_scalar(ssa, insts, instr.a(), block, Ty::I64, pc)?;
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let term = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: term,
                op: if op == Opcode::AddMulInt {
                    IntBinOp::Mul
                } else {
                    IntBinOp::Add
                },
                lhs,
                rhs,
            });
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: IntBinOp::Add,
                lhs: acc,
                rhs: term,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddListInt | Opcode::SubListInt) => {
            // `a ±= list[key]` — the element read has the VM's exact
            // negative-index/halt semantics via the scalar list read.
            let acc = read_typed_scalar(ssa, insts, instr.a(), block, Ty::I64, pc)?;
            let item = list_i64_element_scalar(ssa, insts, instr.b(), instr.c(), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: if op == Opcode::AddListInt {
                    IntBinOp::Add
                } else {
                    IntBinOp::Sub
                },
                lhs: acc,
                rhs: item,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddIntI | Opcode::MulIntI | Opcode::ModIntI) => {
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let imm = ssa.new_val();
            insts.push(Inst::Const {
                dst: imm,
                value: Const::I64(instr.sc() as i64),
            });
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: imm_int_bin_op(op),
                lhs,
                rhs: imm,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddFloat | Opcode::SubFloat | Opcode::MulFloat | Opcode::DivFloat | Opcode::ModFloat) => {
            // The compiler emits these when it expects float arithmetic, but an
            // operand may still be an `I64` (e.g. an `I64` parameter in `x / 2.0`) —
            // the VM coerces it, so we widen `I64`/`Maybe` operands to `F64` here.
            // A Dyn operand (a typed struct field read back as Dyn) routes
            // through the `dyn.*` helpers, same as the Int family above.
            let (lv, lty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let (rv, rty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
            if lty == Ty::Dyn || rty == Ty::Dyn {
                let lhs = to_dyn(ssa, insts, lv, lty, pc)?;
                let rhs = to_dyn(ssa, insts, rv, rty, pc)?;
                let helper = match op {
                    Opcode::AddFloat => "add",
                    Opcode::SubFloat => "sub",
                    Opcode::MulFloat => "mul",
                    Opcode::DivFloat => "div",
                    _ => "mod",
                };
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", helper),
                    args: vec![lhs, rhs],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            if !matches!(lty, Ty::I64 | Ty::F64) || !matches!(rty, Ty::I64 | Ty::F64) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let lhs = coerce_to_f64(ssa, insts, lv, lty);
            let rhs = coerce_to_f64(ssa, insts, rv, rty);
            let dst = ssa.new_val();
            insts.push(Inst::FloatBin {
                dst,
                op: float_bin_op(op),
                lhs,
                rhs,
            });
            ssa.write(instr.a(), block, (dst, Ty::F64));
        }
        _ => {
            return lower_inst_b(
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
