//! Scalar opcodes: constants, moves, type predicates, arithmetic, comparisons.

use super::LowerCtx;
use crate::*;
use lk_core::vm::CastTarget;

pub(super) fn lower(
    ctx: &mut LowerCtx<'_>,
    block: usize,
    insts: &mut Vec<Inst>,
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    let ssa = &mut *ctx.ssa;
    let globals = &mut *ctx.globals;
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
                ssa.bind_ref(block, instr.a(), global_ref);
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
                ssa.bind_ref(block, instr.a(), global_ref);
            } else {
                let first = ssa.read(instr.b(), block, pc)?;
                ssa.write(instr.a(), block, first);
            }
            if let Some(global_ref) = ssa.builtin_ref_at(instr.c(), block) {
                ssa.bind_ref(block, instr.b(), global_ref);
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
            // A boxed Dyn is list-ness only at runtime, and it is not one tag:
            // `rt.is_list` answers for every representation, including the
            // `String` the interpreter also calls list-like.
            if ty == Ty::Dyn {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "is_list"),
                    args: vec![v],
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            // `Str` included for the same reason: `runtime_value_is_list` says
            // true for one, and a `let [a, b] = "ab"` relies on it.
            let is_list = matches!(ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn | Ty::Str);
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
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "is_map"),
                    args: vec![v],
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
        Opcode::CastTo => {
            // `a = b as <c>`. The VM normalises machine ints inside an `i64`
            // carrier, and this mirrors that exactly — the differential tests
            // compare the two backends' output, so the bit patterns have to
            // agree, not merely the intent.
            // `read_scalar`: a cast is a scalar consumer, so a value that came
            // out of a container (a `Maybe` carrier) narrows here rather than
            // refusing to lower. Without this, `for b in bytes { p(b as u32) }`
            // — an ordinary driver loop — falls off the native path.
            let (v, ty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let Some(target) = CastTarget::from_u8(instr.c()) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let dst = ssa.new_val();

            match target {
                // Float and Bool targets need the source's runtime type to pick
                // a conversion; only the statically-known cases lower natively,
                // the rest falls back rather than guessing.
                CastTarget::Float => match ty {
                    Ty::I64 => {
                        insts.push(Inst::IntToFloat { dst, src: v });
                        ssa.write(instr.a(), block, (dst, Ty::F64));
                    }
                    Ty::F64 => {
                        ssa.write(instr.a(), block, (v, Ty::F64));
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                },
                CastTarget::Bool => match ty {
                    Ty::Bool => ssa.write(instr.a(), block, (v, Ty::Bool)),
                    Ty::I64 => {
                        let zero = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: zero,
                            value: Const::I64(0),
                        });
                        insts.push(Inst::Cmp {
                            dst,
                            op: CmpOp::Ne,
                            float: false,
                            lhs: v,
                            rhs: zero,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                },
                integer => {
                    // A boxed source goes through the runtime's cast helper,
                    // which performs exactly the VM's `cast_source_to_i64`
                    // (Int through, Float truncated toward zero, Bool 0/1,
                    // anything else a raise). Doing it here rather than
                    // refusing keeps `for b in bytes { p(b as u32) }` native.
                    let v = match ty {
                        Ty::I64 => v,
                        // A float truncates toward zero and a bool is 0/1 —
                        // the same source conversion the VM's
                        // `cast_source_to_i64` performs.
                        Ty::F64 => {
                            let truncated = ssa.new_val();
                            insts.push(Inst::FloatToInt { dst: truncated, src: v });
                            truncated
                        }
                        Ty::Bool => {
                            let widened = ssa.new_val();
                            insts.push(Inst::ZextBool { dst: widened, src: v });
                            widened
                        }
                        Ty::Dyn => {
                            let unboxed = ssa.new_val();
                            insts.push(Inst::Call {
                                dst: Some(unboxed),
                                callee: AbiRef::new("dyn", "cast_to_i64"),
                                args: vec![v],
                            });
                            unboxed
                        }
                        _ => return Err(Unsupported::TypeMismatch { pc }),
                    };
                    match integer.int_kind().and_then(|kind| kind.bits()) {
                        // Full width, or pointer width on a 64-bit target:
                        // the carrier already holds exactly these bits.
                        None | Some(64) => ssa.write(instr.a(), block, (v, Ty::I64)),
                        Some(bits) => {
                            let signed = integer.int_kind().expect("machine target").is_signed();
                            insts.push(Inst::IntTruncate {
                                dst,
                                src: v,
                                bits: bits as u8,
                                signed,
                            });
                            ssa.write(instr.a(), block, (dst, Ty::I64));
                        }
                    }
                }
            }
        }
        // `A = floor(B / C)` on two `Int`s — the fused `math.floor(a / b)`.
        //
        // Floor, not truncation, so the truncating quotient is corrected by one
        // when the operands' signs differ and the division was not exact.
        // `(a ^ b) < 0` is that sign test.
        Opcode::FloorDivInt => {
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let quotient = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: quotient,
                op: IntBinOp::Div,
                lhs,
                rhs,
            });
            let remainder = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: remainder,
                op: IntBinOp::Mod,
                lhs,
                rhs,
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let inexact = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: inexact,
                op: CmpOp::Ne,
                float: false,
                lhs: remainder,
                rhs: zero,
            });
            let signs = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: signs,
                op: IntBinOp::Xor,
                lhs,
                rhs,
            });
            let opposite = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: opposite,
                op: CmpOp::Lt,
                float: false,
                lhs: signs,
                rhs: zero,
            });
            let adjust = ssa.new_val();
            insts.push(Inst::BoolAnd {
                dst: adjust,
                lhs: inexact,
                rhs: opposite,
            });
            let one = ssa.new_val();
            insts.push(Inst::Const {
                dst: one,
                value: Const::I64(1),
            });
            let lowered = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: lowered,
                op: IntBinOp::Sub,
                lhs: quotient,
                rhs: one,
            });
            let dst = ssa.new_val();
            insts.push(Inst::Select {
                dst,
                cond: adjust,
                then_v: lowered,
                else_v: quotient,
                ty: Ty::I64,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        Opcode::Neg => {
            // `-x`: `a` = dst, `b` = src. Integers negate as `0 - x` (exact,
            // and it wraps at `i64::MIN` exactly as the VM's `wrapping_neg`
            // does); floats need a real `fneg`, because `0.0 - 0.0` is `+0.0`
            // where `-(0.0)` is `-0.0`. A boxed operand falls back — there is
            // no `dyn.neg` in the ABI yet.
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            let dst = ssa.new_val();
            match ty {
                Ty::I64 => {
                    let zero = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: zero,
                        value: Const::I64(0),
                    });
                    insts.push(Inst::IntBin {
                        dst,
                        op: IntBinOp::Sub,
                        lhs: zero,
                        rhs: v,
                    });
                    ssa.write(instr.a(), block, (dst, Ty::I64));
                }
                Ty::F64 => {
                    insts.push(Inst::FloatNeg { dst, src: v });
                    ssa.write(instr.a(), block, (dst, Ty::F64));
                }
                // A boxed operand dispatches at runtime, the same way `Not`
                // does: Int and Float negate, anything else raises.
                Ty::Dyn => {
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("dyn", "neg"),
                        args: vec![v],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Dyn));
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
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
            //
            // The raw reads stay in scope past the arms below: the nullable
            // handling further down needs both operands' *declared* types to
            // build the sentence the interpreter would have raised.
            let (lv_raw, lty_raw) = ssa.read(instr.b(), block, pc)?;
            let (rv_raw, rty_raw) = ssa.read(instr.c(), block, pc)?;
            // `Str + Dyn`: the VM only accepts Str + Str here (anything
            // else is a loud error), so unbox the Dyn side through the
            // `as_str` tag guard (same loud failure) and emit a *typed*
            // concat — the result stays `Str`, keeping a loop
            // accumulator (`acc += s[i]`) same-typed through its phi.
            // `Str + Dyn`: ask the runtime, which is where the VM's rule
            // lives (`dyn.add` mirrors `Executor::dynamic_add`). This used
            // to unbox the Dyn side with `as_str` — a *raise* unless it
            // happened to hold a string — on the belief that the VM "only
            // accepts Str + Str here". It does not: `"v=" + x` with a boxed
            // Int is `v=1`, and `"p=" + xs` with a boxed list is the list
            // `["p=", 1, 2]`, because a list operand outranks a string one.
            // The old arm aborted both.
            //
            // The result is `Dyn` rather than `Str` for the same reason: a
            // list operand makes it a list. A loop accumulator stays
            // same-typed through its phi either way, since both sides of
            // the phi come out of this arm.
            // A nullable operand joins this arm for the same reason it
            // joins the equality one: absent *is* nil, and the VM renders
            // nil as `nil` here rather than refusing. `"[" + xs[9] + "]"`
            // is `[nil]` on the interpreter and raised compiled.
            let nullable_operand = |ty| matches!(ty, Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool);
            if op == Opcode::AddInt
                && (matches!((lty_raw, rty_raw), (Ty::Str, Ty::Dyn) | (Ty::Dyn, Ty::Str))
                    || (lty_raw == Ty::Str && nullable_operand(rty_raw))
                    || (nullable_operand(lty_raw) && rty_raw == Ty::Str))
            {
                let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "add"),
                    args: vec![lhs, rhs],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            // `Str + scalar` / `scalar + Str`: display-concatenate, the
            // VM's fourth `dynamic_add` case. Statically known on both
            // sides, so it needs no runtime dispatch — and it had no arm at
            // all, which took `println(1 + "ab")` down with it.
            if op == Opcode::AddInt
                && matches!((lty_raw, rty_raw), (Ty::Str, _) | (_, Ty::Str))
                && matches!(lty_raw, Ty::Str | Ty::I64 | Ty::F64 | Ty::Bool | Ty::Nil)
                && matches!(rty_raw, Ty::Str | Ty::I64 | Ty::F64 | Ty::Bool | Ty::Nil)
                && (lty_raw, rty_raw) != (Ty::Str, Ty::Str)
            {
                let (l, l_fresh) = to_display_str(ssa, insts, globals, lv_raw, lty_raw, false, pc)?;
                let dst = concat_display(ssa, insts, globals, l, rv_raw, rty_raw, false, pc)?;
                if l_fresh {
                    free_owned_str(insts, l);
                }
                ssa.write(instr.a(), block, (dst, Ty::Str));
                return Ok(());
            }
            // `map + map` merges, the right side winning. Both operands
            // box and the runtime does it, because the answer's key and
            // value types are the two operands' widened — there is no
            // typed carrier for "either of these" — and because the fill
            // *sequence* is the contract (`lkrt_dyn_add` replays the VM's).
            //
            // Only string-keyed maps: the boxed map carrier is
            // string-keyed, so an int-keyed merge has nowhere to land and
            // keeps falling back rather than answering `{"3": 1}` where the
            // VM answers `{3: 1}`.
            let is_str_map = |t: Ty| matches!(t, Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn);
            // `xs - ys` / `m - n` removes, and both go through the runtime
            // for the reason the merge below does: the answer is built by
            // filtering, in the left's own order.
            let is_list = |t: Ty| matches!(t, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn);
            if op == Opcode::SubInt
                && ((is_list(lty_raw) && is_list(rty_raw)) || (is_str_map(lty_raw) && is_str_map(rty_raw)))
            {
                let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                let boxed = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(boxed),
                    callee: AbiRef::new("dyn", "sub"),
                    args: vec![lhs, rhs],
                });
                let (unbox, out_ty) = if is_list(lty_raw) {
                    ("as_list", Ty::ListDyn)
                } else {
                    ("as_map", Ty::MapStrDyn)
                };
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", unbox),
                    args: vec![boxed],
                });
                ssa.write(instr.a(), block, (dst, out_ty));
                return Ok(());
            }
            if op == Opcode::AddInt && is_str_map(lty_raw) && is_str_map(rty_raw) {
                let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                let boxed = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(boxed),
                    callee: AbiRef::new("dyn", "add"),
                    args: vec![lhs, rhs],
                });
                // The answer is always a `str -> Dyn` map, so unbox to the
                // typed handle rather than leaving it `Dyn` — every later
                // read then stays on the typed path.
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "as_map"),
                    args: vec![boxed],
                });
                ssa.write(instr.a(), block, (dst, Ty::MapStrDyn));
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
            // A nullable operand reads through the guard that carries the
            // interpreter's own sentence, so `try { xs[9] + 1 } catch e { e }`
            // is the same string on both backends. Both sides nullable is the
            // one case a *static* sentence cannot get right — the VM names both
            // operands, and whether the second one is absent is only known at
            // run time — so that one boxes and asks `dyn.*`, which formats it
            // from the values.
            let nullable = |ty| matches!(ty, Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool);
            if nullable(lty_raw) && nullable(rty_raw) {
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
            let say = |absent_left: bool| {
                let (l, r) = if absent_left {
                    ("Nil", language_type_name(rty_raw))
                } else {
                    (language_type_name(lty_raw), "Nil")
                };
                arith_operand_message(op, l, r)
            };
            let (lv, lty) = if nullable(lty_raw) {
                read_scalar_saying(ssa, insts, globals, instr.b(), block, pc, &say(true))?
            } else {
                read_scalar(ssa, insts, instr.b(), block, pc)?
            };
            let (rv, rty) = if nullable(rty_raw) {
                read_scalar_saying(ssa, insts, globals, instr.c(), block, pc, &say(false))?
            } else {
                read_scalar(ssa, insts, instr.c(), block, pc)?
            };
            match (lty, rty) {
                // `/` yields a `Float` even for two `Int`s — the rule the
                // checker, the constant folder and the `dyn` helpers above all
                // state, and the one place that used to ignore it. Lowering it
                // as `IntBin::Div` made a *native* `7 / 2` answer `3` where the
                // VM answers `3.5`, and `1 / 0` abort where the VM says `inf`.
                (Ty::I64, Ty::I64) if op == Opcode::DivInt => {
                    let lhs = ssa.new_val();
                    insts.push(Inst::IntToFloat { dst: lhs, src: lv });
                    let rhs = ssa.new_val();
                    insts.push(Inst::IntToFloat { dst: rhs, src: rv });
                    let dst = ssa.new_val();
                    insts.push(Inst::FloatBin {
                        dst,
                        op: FloatBinOp::Div,
                        lhs,
                        rhs,
                    });
                    ssa.write(instr.a(), block, (dst, Ty::F64));
                }
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
            // A nullable operand against a *non-nil* one. An absent carrier is
            // nil, and nil equals nothing — so `xs[oob] == 4` is `false` and
            // `!= 4` is `true`, which is what the VM answers. The scalar read
            // below would instead assert the carrier present and raise, on a
            // program the interpreter runs to completion.
            //
            // Only the equalities. An *ordered* compare against nil is an error
            // in the VM too (`< expected Int, Float, or String, got Nil and
            // Int`), so asserting presence there fails on the same programs.
            let nullable = |ty| matches!(ty, Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool);
            if (nullable(lty_raw) || nullable(rty_raw)) && matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                let cop = cmp_op(op);
                // The common shape — an integer element against an integer —
                // stays typed: present *and* equal, one extra `and` rather than
                // two boxes and a call. `xs[i] == k` in a search loop is this.
                if let (Ty::MaybeI64, Ty::I64) | (Ty::I64, Ty::MaybeI64) = (lty_raw, rty_raw) {
                    let (carrier, carrier_ty, plain) = if nullable(lty_raw) {
                        (lv_raw, lty_raw, rv_raw)
                    } else {
                        (rv_raw, rty_raw, lv_raw)
                    };
                    let present = ssa.new_val();
                    insts.push(Inst::MaybePresent {
                        dst: present,
                        src: carrier,
                        maybe_ty: carrier_ty,
                    });
                    let value = ssa.new_val();
                    insts.push(Inst::MaybeValue {
                        dst: value,
                        src: carrier,
                        maybe_ty: carrier_ty,
                    });
                    let same = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst: same,
                        op: CmpOp::Eq,
                        float: false,
                        lhs: value,
                        rhs: plain,
                    });
                    let equal = ssa.new_val();
                    insts.push(Inst::BoolAnd {
                        dst: equal,
                        lhs: present,
                        rhs: same,
                    });
                    if cop == CmpOp::Ne {
                        let dst = ssa.new_val();
                        insts.push(Inst::Not { dst, src: equal });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    } else {
                        ssa.write(instr.a(), block, (equal, Ty::Bool));
                    }
                    return Ok(());
                }
                // Anything else boxes and asks the runtime, which is where the
                // VM's equality rules live.
                let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                let raw = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(raw),
                    callee: AbiRef::new("dyn", "eq"),
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
                    op: if cop == CmpOp::Ne { CmpOp::Eq } else { CmpOp::Ne },
                    float: false,
                    lhs: raw,
                    rhs: zero,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
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
            // Only the *ordered* compares reach here with a nullable operand —
            // the equalities were answered above — and an ordered compare
            // against nil is an error in the interpreter too. Same treatment as
            // arithmetic: carry the interpreter's own sentence, and let the
            // both-nullable case be formatted from the values by `dyn.*`.
            if nullable_cmp(lty_raw) && nullable_cmp(rty_raw) {
                let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                let helper = match cmp_op(op) {
                    CmpOp::Lt => "lt",
                    CmpOp::Le => "le",
                    CmpOp::Gt => "gt",
                    _ => "ge",
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
                    op: CmpOp::Ne,
                    float: false,
                    lhs: raw,
                    rhs: zero,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            let ordered_say = |absent_left: bool| {
                let (l, r) = if absent_left {
                    ("Nil", language_type_name(rty_raw))
                } else {
                    (language_type_name(lty_raw), "Nil")
                };
                format!("{} expected Int, Float, or String, got {l} and {r}", compare_symbol(op))
            };
            let (lv, lty) = if nullable_cmp(lty_raw) {
                read_scalar_saying(ssa, insts, globals, instr.b(), block, pc, &ordered_say(true))?
            } else {
                read_scalar(ssa, insts, instr.b(), block, pc)?
            };
            let (rv, rty) = if nullable_cmp(rty_raw) {
                read_scalar_saying(ssa, insts, globals, instr.c(), block, pc, &ordered_say(false))?
            } else {
                read_scalar(ssa, insts, instr.c(), block, pc)?
            };
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
                // A `Set` is its member set: same size, every member present.
                // Order-free, so nothing here depends on iteration order.
                (Ty::Set, Ty::Set) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("set", "eq"),
                        args: vec![lv, rv],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    (false, eq, one)
                }
                // `Bytes` compares by content, the VM's rule (unlike a struct,
                // which compared by handle until that was fixed).
                (Ty::Bytes, Ty::Bytes) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("bytes_h", "eq"),
                        args: vec![lv, rv],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    (false, eq, one)
                }
                // Two string-keyed maps — and therefore two structs, which are
                // marked maps. Both sides box to a `Dyn` map and `dyn.eq`
                // decides: order-free, key-by-key, recursing with the VM's
                // numeric coercion (`{"a": 1} == {"a": 1.0}`) and refusing
                // across struct type marks (`P{x:1} != Q{x:1} != {"x":1}`).
                //
                // No map comparison lowered at all before this — not even
                // `{"a": 1} == {"a": 1}` — while every list pairing did.
                //
                // Int-keyed maps stay out: there is no int-keyed `Dyn` map to
                // normalize to, so they would need their own helper family
                // rather than this one line. A fallback, not a wrong answer.
                (
                    Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64,
                    Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64,
                ) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    // `to_dyn` of a `MapStrDyn` tags the handle in place, so a
                    // struct keeps its mark; a *typed* map's conversion
                    // rebuilds, and a typed map is never a struct.
                    let a = to_dyn(ssa, insts, lv, lty, pc)?;
                    let b = to_dyn(ssa, insts, rv, rty, pc)?;
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("dyn", "eq"),
                        args: vec![a, b],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    (false, eq, one)
                }
                // A window against a window or against a list: `xs.slice(0, 2)
                // == [3, 1]` is true in the VM, because a window is a *range of
                // a list* and not a distinct kind of value. Both sides box —
                // `dyn.eq` knows the window tag and compares element-wise
                // across it — rather than materializing the window, which would
                // allocate a list to answer a question about one.
                (Ty::SliceI64, Ty::SliceI64 | Ty::ListI64 | Ty::ListDyn)
                | (Ty::ListI64 | Ty::ListDyn, Ty::SliceI64) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let a = to_dyn(ssa, insts, lv, lty, pc)?;
                    let b = to_dyn(ssa, insts, rv, rty, pc)?;
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("dyn", "eq"),
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
                    // `str_cmp(a, b)` returns -1/0/1, so comparing it to 0 with the
                    // *same* operator realizes all six — `==`, `!=` and the four
                    // orderings alike. Only `==`/`!=` used to get here: the comment
                    // said "the VM only supports those on strings", which was never
                    // true (`Executor::number_compare` has always had a string arm)
                    // — it was the type checker that refused, and it no longer does.
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
                // Two values of *different kinds* are never equal, and both
                // kinds are known here — so the answer is a constant.
                //
                // Every arm above pairs a kind with itself (or Int with Float,
                // which coerce). What was left was `1 == "a"`, `true == [1]`,
                // `nil == 2.5` and the hundred-odd other cross-kind pairings —
                // each a `false` the VM computes and the lowering refused,
                // taking the whole program down with it.
                //
                // `eq_kind` returns `None` for anything whose kind is not
                // static (`Dyn`, a `Maybe` carrier), and those must not fold: a
                // `Maybe<Int>` is an Int *or* nil, which is two kinds.
                (lk, rk)
                    if matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne)
                        && matches!((eq_kind(lk), eq_kind(rk)), (Some(a), Some(b)) if a != b) =>
                {
                    let dst = ssa.new_val();
                    insts.push(Inst::Const {
                        dst,
                        value: Const::Bool(cmp_op(op) == CmpOp::Ne),
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Bool));
                    return Ok(());
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

/// The *kind* a value belongs to for equality: two values of different kinds
/// are never equal, whatever their contents.
///
/// `Int` and `Float` share a kind because the VM coerces them (`1 == 1.0`),
/// and the four list representations share one because a list's element typing
/// is a storage detail, not part of its value — the same for the five map
/// carriers. `None` means the kind is not decidable at lower time: a `Dyn` is
/// whatever it is at runtime, and a `Maybe<Int>` is an Int *or* nil, which is
/// two kinds in one static type.
fn eq_kind(ty: Ty) -> Option<u8> {
    Some(match ty {
        Ty::Nil => 0,
        Ty::Bool => 1,
        Ty::I64 | Ty::F64 => 2,
        Ty::Str => 3,
        Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn | Ty::SliceI64 => 4,
        Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64 => 5,
        Ty::Set => 6,
        Ty::Bytes => 7,
        _ => return None,
    })
}

/// The sentence the interpreter raises when an arithmetic operand is the wrong
/// kind, for the operator `op` and the two operand type names.
///
/// Three shapes, and they are the interpreter's own: `+` and `-` each name what
/// they accept (a list or map may be added or subtracted), and the rest share
/// the generic one. Probed against a running interpreter rather than read off
/// its source, operator by operator and side by side.
fn arith_operand_message(op: Opcode, lhs: &str, rhs: &str) -> String {
    match op {
        Opcode::AddInt => format!("Add expected numbers or strings, got {lhs} and {rhs}"),
        Opcode::SubInt => format!("Sub expected numbers or list/map lhs, got {lhs} and {rhs}"),
        Opcode::MulInt => format!("* expects Int or Float, got {lhs} and {rhs}"),
        Opcode::DivInt => format!("/ expects Int or Float, got {lhs} and {rhs}"),
        _ => format!("% expects Int or Float, got {lhs} and {rhs}"),
    }
}

/// Whether `ty` is a nullable carrier — the shape a bounds-checked read has.
fn nullable_cmp(ty: Ty) -> bool {
    matches!(ty, Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool)
}

/// The symbol a comparison opcode was written as. The interpreter names the
/// operator a program wrote, not the typed opcode the compiler chose.
fn compare_symbol(op: Opcode) -> &'static str {
    match op {
        Opcode::CmpLtInt => "<",
        Opcode::CmpLeInt => "<=",
        Opcode::CmpGtInt => ">",
        Opcode::CmpGeInt => ">=",
        Opcode::CmpNeInt => "!=",
        _ => "==",
    }
}
