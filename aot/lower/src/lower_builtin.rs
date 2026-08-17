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
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this builtin in this argument shape",
            });
        }
        Builtin::ErrorRaise => {
            // `error(v)`: raise the boxed value to the nearest `try` frame
            // (`raise_dyn` diverges: longjmp with a handler, abort without —
            // the VM's uncaught behaviour). The statement's result register
            // is never observed on the raise path; nil keeps SSA total.
            if argc != 1 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
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
        Builtin::U64ToFloat => {
            if argc != 1 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let value = read_index_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("arith", "u64_to_f64"),
                args: vec![value],
            });
            ssa.write(base, block, (dst, Ty::F64));
            return Ok(());
        }
        Builtin::U64Str => {
            if argc != 1 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let value = read_index_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "from_u64"),
                args: vec![value],
            });
            ssa.write(base, block, (dst, Ty::Str));
            return Ok(());
        }
        Builtin::LtU | Builtin::DivU | Builtin::ModU => {
            if argc != 2 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let lhs = read_index_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
            let rhs = read_index_scalar(ssa, insts, base.wrapping_add(2), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new(
                    "arith",
                    match builtin {
                        Builtin::LtU => "u64_lt",
                        Builtin::DivU => "u64_div",
                        _ => "u64_rem",
                    },
                ),
                args: vec![lhs, rhs],
            });
            // `u64_lt` answers 1 or 0; the comparison's result is a Bool
            // everywhere else, so it is one here too.
            let ty = if matches!(builtin, Builtin::LtU) {
                Ty::Bool
            } else {
                Ty::I64
            };
            if matches!(builtin, Builtin::LtU) {
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                let b = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: b,
                    op: CmpOp::Ne,
                    lhs: dst,
                    rhs: zero,
                    float: false,
                });
                ssa.write(base, block, (b, ty));
                return Ok(());
            }
            ssa.write(base, block, (dst, ty));
            return Ok(());
        }
        Builtin::Shl | Builtin::Shr | Builtin::ShrU => {
            if argc != 2 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let lhs = read_index_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
            let rhs = read_index_scalar(ssa, insts, base.wrapping_add(2), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new(
                    "arith",
                    match builtin {
                        Builtin::Shl => "i64_shl",
                        // Logical, because the compiler only picks this name
                        // when the left operand is a `u64` — where bit 63 is
                        // part of the value and not its sign.
                        Builtin::ShrU => "u64_shr",
                        _ => "i64_shr",
                    },
                ),
                args: vec![lhs, rhs],
            });
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::BitAnd | Builtin::BitOr | Builtin::BitXor => {
            if argc != 2 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let lhs = read_index_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
            let rhs = read_index_scalar(ssa, insts, base.wrapping_add(2), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: match builtin {
                    Builtin::BitAnd => IntBinOp::And,
                    Builtin::BitOr => IntBinOp::Or,
                    _ => IntBinOp::Xor,
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
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let v = read_index_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
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
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
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
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
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
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
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
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this builtin in this argument shape",
            });
        }
        Builtin::MergeFields | Builtin::MakeStruct => {
            // Dispatched by the caller (struct provenance needs `sig`).
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this builtin in this argument shape",
            });
        }
        Builtin::SelectBlock => {
            // Four parallel lists + the default flag; every list normalizes
            // to a dyn list, the result is the VM's exact
            // `[is_default, index, payload]` shape.
            if argc != 5 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
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
                        // A constant list is `List<Dyn>` as soon as its
                        // elements are not one uniform type — and strings split
                        // by length, so `["ab", "aaaaaaaaaa"]` is not uniform.
                        Ty::ListDyn => "from_dyn_list",
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
                _ => {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "no native lowering for this builtin in this argument shape",
                    });
                }
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
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
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
        Builtin::Cpu(entry) => {
            // Arity and result come from the ABI table, which is the schema
            // these calls are emitted against. Spelling either out here would
            // be a second copy of a signature — and the failure of a copy that
            // disagrees is not a build error but a call with the wrong number
            // of arguments, or a result quietly overwritten with nil below.
            let Some(abi) = lk_aot_abi::find("cpu", entry) else {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            };
            if argc != abi.params.len() {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let mut call_args = Vec::with_capacity(argc);
            for index in 0..argc {
                let slot = base.wrapping_add(1 + index as u8);
                let (value, ty) = ssa.read(slot, block, pc)?;
                if !matches!(ty, Ty::I64) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                call_args.push(value);
            }
            if abi.result != lk_aot_abi::AbiType::Nil {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("cpu", entry),
                    args: call_args,
                });
                ssa.write(base, block, (dst, Ty::I64));
                // Early return: the tail writes nil to base, which would
                // clobber this.
                return Ok(());
            }
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("cpu", entry),
                args: call_args,
            });
        }
        Builtin::SymbolAddress => {
            // The name has to be a literal: a relocation is a name resolved at
            // link time, and a kernel has no symbol table to look one up in.
            if argc != 1 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let (name_value, _) = ssa.read(base.wrapping_add(1), block, pc)?;
            let Some(symbol) = ssa.const_str_value(name_value) else {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            };
            let dst = ssa.new_val();
            insts.push(Inst::SymbolAddr { dst, symbol });
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::CallAddress2 => {
            if argc != 3 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let callee = read_index_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
            let first = read_index_scalar(ssa, insts, base.wrapping_add(2), block, pc)?;
            let second = read_index_scalar(ssa, insts, base.wrapping_add(3), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::CallIndirect {
                dst: Some(dst),
                callee,
                args: vec![first, second],
            });
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::VolatileRead(bits) => {
            // `volatile_read_uN(ptr)`. The address is an `I64` — a pointer is
            // just an address, and the type checker has already established
            // that this argument is a pointer of the matching width.
            if argc != 1 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            // `read_scalar`, not a bare read: an address that came out of a
            // container arrives as a `Maybe` carrier, and MMIO is a scalar
            // consumer — absent aborts, exactly as `nil` arithmetic does in
            // the VM.
            let addr = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let dst = ssa.new_val();
            // A real machine load. This used to be an opaque `lkrt` call, on
            // the grounds that Cranelift has no volatile flag and its alias
            // analysis collapses two loads of one address into one — which was
            // measured, and true of a plain load. The way out is not a flag:
            // `Inst::VolatileLoad` emits a `sequence_point` first, which costs
            // no machine code and defeats the collapse. See its doc comment.
            insts.push(Inst::VolatileLoad { dst, addr, bits });
            // The VM writes a builtin's result to the call-window base.
            //
            // `return`, not `break`: this function ends by writing `nil` to
            // base for the builtins that produce nothing, which would clobber
            // the value just stored there. `Typeof` returns early for the same
            // reason.
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::VolatileWrite(bits) => {
            if argc != 2 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            // Both operands through `read_scalar` — see the read arm above.
            // Iterating a list and writing each element is the ordinary shape
            // of a driver's output loop, and its elements are `Maybe` carriers.
            let addr = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let value = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
            insts.push(Inst::VolatileStore { addr, value, bits });
            // A write produces nothing, so the shared nil-return tail below is
            // exactly right — fall through to it rather than duplicating it.
        }
        Builtin::PortIn(bits) => {
            // `port_in_uN(port)`. Same shape as the MMIO read: one opaque call,
            // whose result the VM leaves at the call-window base.
            if argc != 1 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let port = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("port", port_in_name(bits)),
                args: vec![port],
            });
            ssa.write(base, block, (dst, Ty::I64));
            return Ok(());
        }
        Builtin::PortOut(bits) => {
            if argc != 2 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let port = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let value = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("port", port_out_name(bits)),
                args: vec![port, value],
            });
            // A write produces nothing — the shared nil-return tail applies.
        }
        Builtin::Typeof => {
            // `typeof(x)` — the VM's type name from the statically proven MIR
            // type. Maybe carriers select between the scalar name and `Nil` at
            // runtime (a missing map key is `Nil` in the VM).
            if argc != 1 {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
            }
            let (v, ty) = ssa.read(base.wrapping_add(1), block, pc)?;
            // Every proven type, not just the scalars: `typeof` asks what the
            // value *is*, and a container is as proven as an `Int` here. With
            // only the five scalars, `typeof([1, 2])` — and every other
            // container — dropped the whole program to the VM.
            //
            // The names are the VM's (`RuntimeVal::type_name_in`), which is
            // what `every_proven_type_has_a_typeof_name` compares them against.
            let scalar_name = |ty: Ty| match ty {
                Ty::I64 => Some("Int"),
                Ty::F64 => Some("Float"),
                Ty::Bool => Some("Bool"),
                Ty::Str => Some("String"),
                Ty::Nil => Some("Nil"),
                Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn => Some("List"),
                // `MapStrDyn` is deliberately absent: it is also the struct
                // carrier, so it has no static answer (see the arms below).
                Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapI64I64 | Ty::MapI64F64 => Some("Map"),
                Ty::Set => Some("Set"),
                Ty::Bytes => Some("Bytes"),
                Ty::SliceI64 => Some("Slice"),
                // A `Dyn` is whatever it is at run time, so its name is a
                // runtime question — `dyn.type_name` answers it, and this
                // static table cannot.
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
                // A struct instance the lowering can name: answer the declared
                // name, statically. Its carrier is `MapStrDyn`, and the static
                // table said `Map` — so `typeof(p)` read `Map` compiled and
                // `P` interpreted, a divergence no example happened to cover.
                _ if ssa.struct_types.contains_key(&v) => {
                    let name = ssa.struct_types[&v].clone();
                    materialize_key(ssa, insts, globals, &name)
                }
                // A carrier that *may* be a struct at run time but is not
                // proven one — a plain map and a struct instance share
                // `MapStrDyn`, and a `Dyn` is anything. The runtime reads the
                // type mark; guessing `Map` here would be a wrong answer half
                // the time.
                Ty::MapStrDyn | Ty::Dyn => {
                    let boxed = to_dyn_any(ssa, insts, v, ty, pc)?;
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("dyn", "type_name"),
                        args: vec![boxed],
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
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this builtin in this argument shape",
                });
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

/// ABI entry names for port I/O, keyed by width.
fn port_in_name(bits: u8) -> &'static str {
    match bits {
        8 => "in_u8",
        16 => "in_u16",
        _ => "in_u32",
    }
}

fn port_out_name(bits: u8) -> &'static str {
    match bits {
        8 => "out_u8",
        16 => "out_u16",
        _ => "out_u32",
    }
}
