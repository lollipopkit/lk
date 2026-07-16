use super::*;

/// Lowers a call to a recognized runtime builtin (`println` / `print` /
/// `assert`). The builtin's nil return is written to the call-window base
/// register, matching the VM's return-value placement.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_builtin_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    builtin: Builtin,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    match builtin {
        Builtin::Println | Builtin::Print => {
            let parts = print_parts(ssa, base, argc, block, pc)?;
            emit_print(ssa, insts, globals, parts, builtin == Builtin::Println, pc)?;
        }
        Builtin::CallMethod => {
            // Dispatched by the caller before reaching here.
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        Builtin::TryCall => {
            // Dispatched by the caller before reaching here (it needs the
            // function table and the signature lattice).
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        Builtin::ErrorRaise => {
            // `error(v)`: raise the boxed value to the nearest `try` frame
            // (`raise_dyn` diverges: longjmp with a handler, abort without —
            // the VM's uncaught behaviour). The statement's result register
            // is never observed on the raise path; nil keeps SSA total.
            if argc != 1 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let (v, ty) = ssa.read(base.wrapping_add(1), block, pc)?;
            let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "raise_dyn"),
                args: vec![boxed],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            ssa.write(base, block, (nil, Ty::Nil));
            return Ok(());
        }
        Builtin::BitAnd | Builtin::BitOr => {
            if argc != 2 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let lhs = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: if matches!(builtin, Builtin::BitAnd) {
                    IntBinOp::And
                } else {
                    IntBinOp::Or
                },
                lhs,
                rhs,
            });
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::BitNot => {
            // `~x` = `x xor -1` (two's complement bitwise not).
            if argc != 1 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let v = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let minus_one = ssa.new_val();
            insts.push(Inst::Const {
                dst: minus_one,
                value: Const::I64(-1),
            });
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: IntBinOp::Xor,
                lhs: v,
                rhs: minus_one,
            });
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::ChanNew => {
            // `chan(capacity[, type])` — the type string is a VM checker
            // hint, dropped natively. The channel value is its i64 id.
            if !(1..=2).contains(&argc) {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let cap = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("chan", "new"),
                args: vec![cap],
            });
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::ChanSend => {
            if argc != 2 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let ch = read_channel_id(ssa, insts, base.wrapping_add(1), block, pc)?;
            let (v, ty) = ssa.read(base.wrapping_add(2), block, pc)?;
            let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("chan", "send"),
                args: vec![ch, boxed],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            ssa.write(base, block, (nil, Ty::Nil));
            return Ok(());
        }
        Builtin::ChanRecv => {
            if argc != 1 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let ch = read_channel_id(ssa, insts, base.wrapping_add(1), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("chan", "recv"),
                args: vec![ch],
            });
            ssa.write(base, block, (dst, Ty::Dyn));
            return Ok(());
        }
        Builtin::Spawn => {
            // Dispatched by the caller (needs the function table/signatures).
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        Builtin::MergeFields | Builtin::MakeStruct => {
            // Dispatched by the caller (struct provenance needs `sig`).
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        Builtin::SelectBlock => {
            // Four parallel lists + the default flag; every list normalizes
            // to a dyn list, the result is the VM's exact
            // `[is_default, index, payload]` shape.
            if argc != 5 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let mut lists = Vec::with_capacity(4);
            for i in 0..4 {
                let (v, ty) = ssa.read(base.wrapping_add(1 + i), block, pc)?;
                lists.push(to_dyn_list_handle(ssa, insts, v, ty, pc)?);
            }
            let has_default = {
                let (v, ty) = ssa.read(base.wrapping_add(5), block, pc)?;
                if ty != Ty::Bool {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                let wide = ssa.new_val();
                insts.push(Inst::ZextBool { dst: wide, src: v });
                wide
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("chan", "select"),
                args: vec![lists[0], lists[1], lists[2], lists[3], has_default],
            });
            ssa.write(base, block, (dst, Ty::ListDyn));
            return Ok(());
        }
        Builtin::SetCtor => {
            // `Set()` / `Set(list)` — a fresh native set handle. `Set(set)`
            // (copy) and mixed/Dyn element lists stay out (heap-handle keys).
            let result = match argc {
                0 => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("set", "new"),
                        args: Vec::new(),
                    });
                    dst
                }
                1 => {
                    let (list, list_ty) = ssa.read(base.wrapping_add(1), block, pc)?;
                    let from = match list_ty {
                        Ty::ListStr => "from_str_list",
                        Ty::ListI64 => "from_i64_list",
                        _ => return Err(Unsupported::TypeMismatch { pc }),
                    };
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("set", from),
                        args: vec![list],
                    });
                    dst
                }
                _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
            };
            ssa.write(base, block, (result, Ty::Set));
            return Ok(());
        }
        Builtin::Panic => {
            // `panic(args…)`: the message is the space-joined display of the
            // arguments (`join_runtime_display`), or the literal `panic` with
            // no arguments; always fatal (the VM's loud panic halt).
            let msg = if argc == 0 {
                materialize_key(ssa, insts, globals, "panic")
            } else {
                let (v0, ty0) = ssa.read(base.wrapping_add(1), block, pc)?;
                let (mut acc, mut acc_fresh) = to_display_str(ssa, insts, globals, v0, ty0, true, pc)?;
                for i in 1..argc {
                    let sep = materialize_key(ssa, insts, globals, " ");
                    let with_sep = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(with_sep),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![acc, sep],
                    });
                    if acc_fresh {
                        free_owned_str(insts, acc);
                    }
                    let (v, ty) = ssa.read(base.wrapping_add(1 + i as u8), block, pc)?;
                    acc = concat_display(ssa, insts, globals, with_sep, v, ty, true, pc)?;
                    free_owned_str(insts, with_sep);
                    acc_fresh = true;
                }
                acc
            };
            // The call aborts and never returns; the message is intentionally
            // not freed.
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "panic"),
                args: vec![msg],
            });
        }
        Builtin::AssertEq | Builtin::AssertNe => {
            // `assert_eq(a, b [, extra])` / `assert_ne`: scalar equality with
            // the VM's `runtime_values_equal` semantics (same-type scalars,
            // Int/Float coercion, byte-equal strings). The failure message is
            // built eagerly (dead on the success path) so no extra control
            // flow is needed.
            if !(2..=3).contains(&argc) {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let negated = builtin == Builtin::AssertNe;
            let (lv, lty) = ssa.read(base.wrapping_add(1), block, pc)?;
            let (rv, rty) = ssa.read(base.wrapping_add(2), block, pc)?;
            let op = if negated { CmpOp::Ne } else { CmpOp::Eq };
            let ok = match (lty, rty) {
                (Ty::I64, Ty::I64) | (Ty::Bool, Ty::Bool) => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst,
                        op,
                        float: false,
                        lhs: lv,
                        rhs: rv,
                    });
                    dst
                }
                (Ty::F64, Ty::F64) | (Ty::I64, Ty::F64) | (Ty::F64, Ty::I64) => {
                    let widen = |ssa: &mut Ssa, insts: &mut Vec<Inst>, v: ValueId, ty: Ty| {
                        if ty == Ty::I64 {
                            let f = ssa.new_val();
                            insts.push(Inst::IntToFloat { dst: f, src: v });
                            f
                        } else {
                            v
                        }
                    };
                    let lf = widen(ssa, insts, lv, lty);
                    let rf = widen(ssa, insts, rv, rty);
                    let dst = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst,
                        op,
                        float: true,
                        lhs: lf,
                        rhs: rf,
                    });
                    dst
                }
                (Ty::Str, Ty::Str) => {
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
                    let dst = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst,
                        op,
                        float: false,
                        lhs: cmp,
                        rhs: zero,
                    });
                    dst
                }
                // Anything else boxable (lists, Maybe carriers, Dyn) compares
                // through `dyn.eq` — deep structural equality with numeric
                // coercion, exactly the VM's `runtime_values_equal` (an absent
                // Maybe is nil: `assert_eq(m.get(missing), 3)` fails loud on
                // both sides).
                _ if dyn_boxable_ty(lty) && dyn_boxable_ty(rty) => {
                    let lb = to_dyn_any(ssa, insts, lv, lty, pc)?;
                    let rb = to_dyn_any(ssa, insts, rv, rty, pc)?;
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("dyn", "eq"),
                        args: vec![lb, rb],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    let dst = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst,
                        op,
                        float: false,
                        lhs: eq,
                        rhs: one,
                    });
                    dst
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            // `(msg, fresh)`: a bare const message must not be freed.
            let (msg, msg_fresh) = if negated {
                // "values should not be equal" — no operand displays.
                if argc == 3 {
                    let (ev, ety) = ssa.read(base.wrapping_add(3), block, pc)?;
                    let sep = materialize_key(ssa, insts, globals, "values should not be equal - ");
                    (concat_display(ssa, insts, globals, sep, ev, ety, true, pc)?, true)
                } else {
                    (
                        materialize_key(ssa, insts, globals, "values should not be equal"),
                        false,
                    )
                }
            } else {
                // "expected {b}, got {a}" (+ " - {extra}").
                let head = materialize_key(ssa, insts, globals, "expected ");
                let with_expected = concat_display(ssa, insts, globals, head, rv, rty, true, pc)?;
                let comma = materialize_key(ssa, insts, globals, ", got ");
                let joined = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(joined),
                    callee: AbiRef::new("str", "concat"),
                    args: vec![with_expected, comma],
                });
                free_owned_str(insts, with_expected);
                let full = concat_display(ssa, insts, globals, joined, lv, lty, true, pc)?;
                free_owned_str(insts, joined);
                if argc == 3 {
                    let (ev, ety) = ssa.read(base.wrapping_add(3), block, pc)?;
                    let dash = materialize_key(ssa, insts, globals, " - ");
                    let with_dash = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(with_dash),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![full, dash],
                    });
                    free_owned_str(insts, full);
                    let all = concat_display(ssa, insts, globals, with_dash, ev, ety, true, pc)?;
                    free_owned_str(insts, with_dash);
                    (all, true)
                } else {
                    (full, true)
                }
            };
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: ok });
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "assert_msg"),
                args: vec![wide, msg],
            });
            if msg_fresh {
                free_owned_str(insts, msg);
            }
        }
        Builtin::Typeof => {
            // `typeof(x)` — the VM's type name from the statically proven MIR
            // type. Maybe carriers select between the scalar name and `Nil` at
            // runtime (a missing map key is `Nil` in the VM).
            if argc != 1 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let (v, ty) = ssa.read(base.wrapping_add(1), block, pc)?;
            let scalar_name = |ty: Ty| match ty {
                Ty::I64 => Some("Int"),
                Ty::F64 => Some("Float"),
                Ty::Bool => Some("Bool"),
                Ty::Str => Some("String"),
                Ty::Nil => Some("Nil"),
                _ => None,
            };
            let result = match ty {
                Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                    let value_name = match ty {
                        Ty::MaybeI64 => "Int",
                        Ty::MaybeF64 => "Float",
                        Ty::MaybeBool => "Bool",
                        _ => "String",
                    };
                    let present = ssa.new_val();
                    insts.push(Inst::MaybePresent {
                        dst: present,
                        src: v,
                        maybe_ty: ty,
                    });
                    let then_v = materialize_key(ssa, insts, globals, value_name);
                    let else_v = materialize_key(ssa, insts, globals, "Nil");
                    let dst = ssa.new_val();
                    insts.push(Inst::Select {
                        dst,
                        cond: present,
                        then_v,
                        else_v,
                        ty: Ty::Str,
                    });
                    dst
                }
                ty => match scalar_name(ty) {
                    Some(name) => materialize_key(ssa, insts, globals, name),
                    None => return Err(Unsupported::TypeMismatch { pc }),
                },
            };
            ssa.write(base, block, (result, Ty::Str));
            return Ok(());
        }
        Builtin::Assert => {
            // `assert(cond)` / `assert(cond, message)`: a false condition is a
            // fatal error, matching the VM's loud halt. A `Bool` condition
            // widens directly; a boxed condition evaluates the VM's
            // truthiness (`assert_truthy` = `!(Nil | Bool(false))`).
            if argc == 0 || argc > 2 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let wide = match ssa.read(base.wrapping_add(1), block, pc)? {
                (v, Ty::Dyn) => {
                    let t = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(t),
                        callee: AbiRef::new("dyn", "truthy"),
                        args: vec![v],
                    });
                    t
                }
                _ => {
                    let cond = ssa.read_typed(base.wrapping_add(1), block, Ty::Bool, pc)?;
                    let wide = ssa.new_val();
                    insts.push(Inst::ZextBool { dst: wide, src: cond });
                    wide
                }
            };
            if argc == 1 {
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "assert"),
                    args: vec![wide],
                });
            } else {
                let (mv, mty) = ssa.read(base.wrapping_add(2), block, pc)?;
                let (msg, fresh) = to_display_str(ssa, insts, globals, mv, mty, true, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "assert_msg"),
                    args: vec![wide, msg],
                });
                // On failure the call aborts and never returns; on success the
                // display temporary is dead.
                if fresh {
                    free_owned_str(insts, msg);
                }
            }
        }
    }
    let nil = ssa.new_val();
    insts.push(Inst::Const {
        dst: nil,
        value: Const::Nil,
    });
    ssa.write(base, block, (nil, Ty::Nil));
    Ok(())
}
