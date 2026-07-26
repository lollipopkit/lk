//! Scalar opcodes: constants, moves, type predicates, arithmetic, comparisons.

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
    let func = ctx.func;
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
                            // `cop` is already restricted to `Eq`/`Ne` above.
                            op: cop,
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
        op => return Err(Unsupported::Opcode { pc, op }),
    }
    Ok(())
}
