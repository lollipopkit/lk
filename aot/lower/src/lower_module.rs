use super::*;

/// Lowers a `module.method(args)` call whose member [`module_call_abi_rows`] maps to
/// a typed lkrt ABI entry. Arity and argument types must match the schema
/// exactly; the result (or nil) is written to the call-window base register.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_module_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    module: &str,
    name: &str,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    // `iter.range([start,] end[, step])` — an exclusive integer range,
    // materialized eagerly like the VM (reuses the `NewRange` helper; zero
    // step aborts inside it). The one-arg form counts from 0.
    // Streams over finite sources with pure lambdas are observationally an
    // eager list pipeline (the corpus is differential-gated on stdout, and
    // laziness has no side channel there): `from_list`/`collect` pass
    // through, `range` materializes.
    if module == "stream" {
        match name {
            "from_list" | "collect" => {
                if argc != 1 {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "no native lowering for this stdlib module function",
                    });
                }
                let (v, ty) = ssa.read(base.wrapping_add(1), block, pc)?;
                if name == "collect" {
                    // The list behind the stream. Its own value, so the stream
                    // it came from keeps its identity.
                    if ty != Ty::Dyn {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let list = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(list),
                        callee: AbiRef::new("dyn", "stream_list"),
                        args: vec![v],
                    });
                    ssa.write(base, block, (list, Ty::ListDyn));
                    return Ok(());
                }
                if !matches!(ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                // Boxed under `DYN_STREAM`: a stream is not the list it is
                // materialized into, and a box is a value of its own — which is
                // what lets `from_list` answer a stream without the caller's
                // `xs` becoming one.
                let list = to_dyn_list_handle(ssa, insts, v, ty, pc)?;
                let boxed = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(boxed),
                    callee: AbiRef::new("dyn", "from_stream"),
                    args: vec![list],
                });
                ssa.write(base, block, (boxed, Ty::Dyn));
                return Ok(());
            }
            // The same three arities `iter.range` takes: the one-argument form
            // counts from 0 and the third argument is the step. Only the
            // two-argument form was accepted, so `stream.range(n)` — the
            // shortest way to write it — dropped its module to the VM.
            "range" if (1..=3).contains(&argc) => {
                let (start, end) = if argc == 1 {
                    let zero = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: zero,
                        value: Const::I64(0),
                    });
                    let end = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                    (zero, end)
                } else {
                    let start = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                    let end = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
                    (start, end)
                };
                let one = if argc == 3 {
                    read_typed_scalar(ssa, insts, base.wrapping_add(3), block, Ty::I64, pc)?
                } else {
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    one
                };
                let exclusive = ssa.new_val();
                insts.push(Inst::Const {
                    dst: exclusive,
                    value: Const::I64(0),
                });
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", "i64_from_range"),
                    args: vec![start, end, one, exclusive],
                });
                let list = to_dyn_list_handle(ssa, insts, handle, Ty::ListI64, pc)?;
                let boxed = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(boxed),
                    callee: AbiRef::new("dyn", "from_stream"),
                    args: vec![list],
                });
                ssa.write(base, block, (boxed, Ty::Dyn));
                return Ok(());
            }
            _ => {}
        }
    }
    if module == "iter" && name == "range" {
        if !(1..=3).contains(&argc) {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let (start, end) = if argc == 1 {
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let end = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            (zero, end)
        } else {
            let start = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            let end = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
            (start, end)
        };
        let step = if argc == 3 {
            read_typed_scalar(ssa, insts, base.wrapping_add(3), block, Ty::I64, pc)?
        } else {
            let one = ssa.new_val();
            insts.push(Inst::Const {
                dst: one,
                value: Const::I64(1),
            });
            one
        };
        let exclusive = ssa.new_val();
        insts.push(Inst::Const {
            dst: exclusive,
            value: Const::I64(0),
        });
        let handle = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(handle),
            callee: AbiRef::new("list_h", "i64_from_range"),
            args: vec![start, end, step, exclusive],
        });
        ssa.write(base, block, (handle, Ty::ListI64));
        return Ok(());
    }
    // `string.to_int(text[, base])` — the base is optional in the language and
    // not in the ABI, so a missing one is materialized as 10 here rather than
    // duplicating the entry. Only the String arm lowers: `to_int(3.99)` is a
    // Float and takes the generic path (it has no ABI row, so it falls back).
    if module == "string" && name == "to_int" {
        if !(1..=2).contains(&argc) {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let text = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::Str, pc)?;
        let radix = if argc == 2 {
            read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?
        } else {
            let ten = ssa.new_val();
            insts.push(Inst::Const {
                dst: ten,
                value: Const::I64(10),
            });
            ten
        };
        let dst = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(dst),
            callee: AbiRef::new("str", "to_int"),
            args: vec![text, radix],
        });
        ssa.write(base, block, (dst, Ty::Dyn));
        return Ok(());
    }
    // `math.floor`/`ceil`/`round` dispatch on the argument's static type,
    // matching the VM's `integer_round`: an `Int` passes through unchanged, a
    // `Float` rounds via the lkrt helper (`f64::xxx() as i64`).
    if module == "math" && matches!(name, "floor" | "ceil" | "round") {
        if argc != 1 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let (v, ty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        match ty {
            Ty::I64 => ssa.write(base, block, (v, Ty::I64)),
            Ty::F64 => {
                let round_fn = match name {
                    "floor" => "floor",
                    "ceil" => "ceil",
                    _ => "round",
                };
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("math", round_fn),
                    args: vec![v],
                });
                ssa.write(base, block, (dst, Ty::I64));
            }
            _ => return Err(Unsupported::TypeMismatch { pc }),
        }
        return Ok(());
    }
    // `math.clamp(v[, min[, max]])` — the module defaults `min` to 0 and `max`
    // to 100, and a default lives in the export wrapper, which this side cannot
    // read. So the two short arities materialize the same constants the
    // declaration states rather than growing a row each; the full arity takes
    // the row below. Named spellings (`clamp(v, max: 9)`) still go through the
    // row, which is where the permutation is.
    if module == "math" && name == "clamp" && (1..=2).contains(&argc) {
        let value = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
        let min = if argc == 2 {
            read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?
        } else {
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            zero
        };
        let max = ssa.new_val();
        insts.push(Inst::Const {
            dst: max,
            value: Const::I64(100),
        });
        let dst = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(dst),
            callee: AbiRef::new("math", "clamp_i64"),
            args: vec![value, min, max],
        });
        ssa.write(base, block, (dst, Ty::I64));
        return Ok(());
    }
    // Four members that answer differently for an Int than for a Float, and so
    // dispatch on the argument's static type the way `math.floor` and
    // `math.abs` below do rather than taking a promoting ABI row. Each Int arm
    // is the module's own: `trunc` and `to_int` hand an Int back unchanged,
    // `fract` answers `0.0` for one, and `to_float` widens. A Bool is a number
    // to the two converters and to nothing else, matching the module's arms.
    if module == "math" && matches!(name, "trunc" | "fract" | "to_int" | "to_float") {
        if argc != 1 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let (v, ty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        // A Bool crosses as its word, which is the 0/1 the module converts.
        let (v, ty) = if ty == Ty::Bool && matches!(name, "to_int" | "to_float") {
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: v });
            (wide, Ty::I64)
        } else {
            (v, ty)
        };
        let result = match (name, ty) {
            ("trunc", Ty::I64) | ("to_int", Ty::I64) => (v, Ty::I64),
            ("to_float", Ty::F64) => (v, Ty::F64),
            ("to_float", Ty::I64) => {
                let f = ssa.new_val();
                insts.push(Inst::IntToFloat { dst: f, src: v });
                (f, Ty::F64)
            }
            ("fract", Ty::I64) => {
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::F64(0.0),
                });
                (zero, Ty::F64)
            }
            (_, Ty::F64) => {
                let (helper, ret) = match name {
                    "trunc" => ("trunc_f64", Ty::F64),
                    "fract" => ("fract_f64", Ty::F64),
                    _ => ("to_int_f64", Ty::I64),
                };
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("math", helper),
                    args: vec![v],
                });
                (dst, ret)
            }
            _ => return Err(Unsupported::TypeMismatch { pc }),
        };
        ssa.write(base, block, result);
        return Ok(());
    }
    // `math.abs` returns its argument's type: Int → wrapping integer abs
    // (select(x < 0, 0 - x, x), sub wraps like the VM's release build),
    // Float → fabs via select on the float compare.
    if module == "math" && name == "abs" {
        if argc != 1 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let (v, ty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        if !matches!(ty, Ty::I64 | Ty::F64) {
            return Err(Unsupported::TypeMismatch { pc });
        }
        let zero = ssa.new_val();
        insts.push(Inst::Const {
            dst: zero,
            value: if ty == Ty::F64 { Const::F64(0.0) } else { Const::I64(0) },
        });
        let negative = ssa.new_val();
        insts.push(Inst::Cmp {
            dst: negative,
            op: CmpOp::Lt,
            float: ty == Ty::F64,
            lhs: v,
            rhs: zero,
        });
        let negated = ssa.new_val();
        if ty == Ty::F64 {
            insts.push(Inst::FloatBin {
                dst: negated,
                op: FloatBinOp::Sub,
                lhs: zero,
                rhs: v,
            });
        } else {
            insts.push(Inst::IntBin {
                dst: negated,
                op: IntBinOp::Sub,
                lhs: zero,
                rhs: v,
            });
        }
        let dst = ssa.new_val();
        insts.push(Inst::Select {
            dst,
            cond: negative,
            then_v: negated,
            else_v: v,
            ty,
        });
        ssa.write(base, block, (dst, ty));
        return Ok(());
    }
    // `io.std` (bound as the `std` global by `use { std } from io`): the
    // stdio resources are fixed handles (stdin 0 / stdout 1 / stderr 2 — the
    // lkrt convention); `write`/`writeln` return the VM's written byte count,
    // `flush` is always `true` on success (errors abort loudly on both sides).
    if module == "std" {
        match name {
            "stdin" | "stdout" | "stderr" => {
                if argc != 0 {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "no native lowering for this stdlib module function",
                    });
                }
                let handle = match name {
                    "stdin" => 0,
                    "stdout" => 1,
                    _ => 2,
                };
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::I64(handle),
                });
                ssa.write(base, block, (dst, Ty::I64));
                return Ok(());
            }
            "write" | "writeln" => {
                if argc != 2 {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "no native lowering for this stdlib module function",
                    });
                }
                let handle = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                let data = ssa.read_typed(base.wrapping_add(2), block, Ty::Str, pc)?;
                let newline = ssa.new_val();
                insts.push(Inst::Const {
                    dst: newline,
                    value: Const::I64(i64::from(name == "writeln")),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("io.std", "write"),
                    args: vec![handle, data, newline],
                });
                ssa.write(base, block, (dst, Ty::I64));
                return Ok(());
            }
            "flush" => {
                if argc != 1 {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "no native lowering for this stdlib module function",
                    });
                }
                let handle = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("io.std", "flush"),
                    args: vec![handle],
                });
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(true),
                });
                ssa.write(base, block, (dst, Ty::Bool));
                return Ok(());
            }
            "read_to_string" => {
                if argc != 1 {
                    return Err(Unsupported::CallShape {
                        pc,
                        reason: "no native lowering for this stdlib module function",
                    });
                }
                let handle = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("io.std", "read_to_string"),
                    args: vec![handle],
                });
                ssa.write(base, block, (dst, Ty::Str));
                return Ok(());
            }
            _ => {
                return Err(Unsupported::CallShape {
                    pc,
                    reason: "no native lowering for this stdlib module function",
                });
            }
        }
    }
    // `datetime.add`/`sub` are plain Int arithmetic (`timestamp ± seconds`);
    // `is_weekend` returns the helper's 0/1 as a `Bool`.
    if module == "datetime" && matches!(name, "add" | "sub") {
        if argc != 2 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let ts = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
        let secs = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
        let dst = ssa.new_val();
        insts.push(Inst::IntBin {
            dst,
            op: if name == "add" { IntBinOp::Add } else { IntBinOp::Sub },
            lhs: ts,
            rhs: secs,
        });
        ssa.write(base, block, (dst, Ty::I64));
        return Ok(());
    }
    if module == "datetime" && name == "is_weekend" {
        if argc != 1 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let ts = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
        let wide = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(wide),
            callee: AbiRef::new("datetime", "is_weekend"),
            args: vec![ts],
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
        ssa.write(base, block, (dst, Ty::Bool));
        return Ok(());
    }
    // `time.since(start, end)` is `end - start` (the VM's `numeric_millis`
    // subtraction); Int-typed millisecond values only — Float coercion stays
    // out of the subset.
    if module == "time" && name == "since" {
        if argc != 2 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let start = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
        let end = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
        let dst = ssa.new_val();
        insts.push(Inst::IntBin {
            dst,
            op: IntBinOp::Sub,
            lhs: end,
            rhs: start,
        });
        ssa.write(base, block, (dst, Ty::I64));
        return Ok(());
    }
    // `math.min`/`max` return one of the *original* arguments (comparison per
    // the VM's `min_max`); same-type scalar pairs lower to a select.
    if module == "math" && matches!(name, "min" | "max") {
        if argc != 2 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let (l, lty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        let (r, rty) = read_scalar(ssa, insts, base.wrapping_add(2), block, pc)?;
        if lty != rty || !matches!(lty, Ty::I64 | Ty::F64) {
            return Err(Unsupported::TypeMismatch { pc });
        }
        let pick_left = ssa.new_val();
        insts.push(Inst::Cmp {
            dst: pick_left,
            op: if name == "min" { CmpOp::Lt } else { CmpOp::Gt },
            float: lty == Ty::F64,
            lhs: l,
            rhs: r,
        });
        let dst = ssa.new_val();
        insts.push(Inst::Select {
            dst,
            cond: pick_left,
            then_v: l,
            else_v: r,
            ty: lty,
        });
        ssa.write(base, block, (dst, lty));
        return Ok(());
    }
    // `math.sign` keeps its argument's numeric flavor (the module's two arms).
    if module == "math" && name == "sign" {
        if argc != 1 {
            return Err(Unsupported::CallShape {
                pc,
                reason: "no native lowering for this stdlib module function",
            });
        }
        let (v, ty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        let sign_fn = match ty {
            Ty::I64 => "sign_i64",
            Ty::F64 => "sign_f64",
            _ => return Err(Unsupported::TypeMismatch { pc }),
        };
        let dst = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(dst),
            callee: AbiRef::new("math", sign_fn),
            args: vec![v],
        });
        ssa.write(base, block, (dst, ty));
        return Ok(());
    }
    // `task.join_all(a, b, …)` — await each, in order, into a list.
    //
    // Variadic, so no row can describe it: a row has one arity. The three
    // spellings the VM accepts are `join_all(a, b)`, `join_all(a)` (one task)
    // and `join_all([a, b])` (a list of them); the first two are the same loop,
    // and the list form needs the elements out of a handle, which is the
    // `ListI64` case below.
    if module == "task" && name == "join_all" && argc >= 1 {
        let list = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(list),
            callee: AbiRef::new("list_h", "dyn_new"),
            args: Vec::new(),
        });
        let await_into = |ssa: &mut Ssa, insts: &mut Vec<Inst>, handle: ValueId| {
            let value = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(value),
                callee: AbiRef::new("rt", "task_await"),
                args: vec![handle],
            });
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("list_h", "dyn_push"),
                args: vec![list, value],
            });
        };
        // `join_all([a, b])`: a single `List<Int>` of task handles. Its length
        // is only known at run time, so the awaits are a loop in lkrt rather
        // than unrolled here — which this lowering has no way to emit, so the
        // list form stays a fallback and only the handle forms lower.
        let mut handles = Vec::with_capacity(argc);
        for index in 0..argc {
            let reg = base.wrapping_add(1).wrapping_add(index as u8);
            // A task travels boxed under `DYN_TASK`; this reads the id behind
            // it, and a bare `I64` still passes through.
            handles.push(crate::dyn_box::read_channel_id(ssa, insts, reg, block, pc)?);
        }
        for handle in handles {
            await_into(ssa, insts, handle);
        }
        ssa.write(base, block, (list, Ty::ListDyn));
        return Ok(());
    }
    // `string.slice(s, start)` — as above, defaulting to the character length.
    if module == "string" && name == "slice" && argc == 2 {
        let text = ssa.read_typed(base.wrapping_add(1), block, Ty::Str, pc)?;
        let start = ssa.read_typed(base.wrapping_add(2), block, Ty::I64, pc)?;
        let end = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(end),
            callee: AbiRef::new("str", "char_len"),
            args: vec![text],
        });
        let dst = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(dst),
            callee: AbiRef::new("str", "slice_chars"),
            args: vec![text, start, end],
        });
        ssa.write(base, block, (dst, Ty::Str));
        return Ok(());
    }
    // `bytes.slice(b, start)` — the two-argument form, whose `end` defaults to
    // the length. A row has one arity, so the default belongs here.
    if module == "bytes" && name == "slice" && argc == 2 {
        let handle = ssa.read_typed(base.wrapping_add(1), block, Ty::Bytes, pc)?;
        let start = ssa.read_typed(base.wrapping_add(2), block, Ty::I64, pc)?;
        let end = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(end),
            callee: AbiRef::new("bytes_h", "len"),
            args: vec![handle],
        });
        let dst = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(dst),
            callee: AbiRef::new("bytes_h", "slice"),
            args: vec![handle, start, end],
        });
        ssa.write(base, block, (dst, Ty::Bytes));
        return Ok(());
    }
    let arg_regs: Vec<u8> = (0..argc).map(|i| base.wrapping_add(1).wrapping_add(i as u8)).collect();
    lower_module_abi_call(ssa, insts, module, name, base, &arg_regs, block, pc)
}

/// A stdlib module member called with `name: value` arguments.
///
/// The window is the same one [`lower_named_call`] reads — callee at `base`,
/// the positional prefix, then `(name, value)` pairs — and the permutation is
/// the row's `named` list, which is the stdlib export's own `named(...)`
/// declaration. Every name is a constant the compiler emitted, so the ordering
/// is a compile-time fact.
///
/// Rejects rather than guesses: a member with no names, a name that is not a
/// constant, an unknown or duplicated name, or a call that leaves a named
/// parameter out. That last one is not laziness — an omitted parameter takes
/// the *default*, and a default lives in the stdlib export wrapper, not in
/// anything this side can read.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_named_module_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    module: &str,
    name: &str,
    base: u8,
    positional_count: usize,
    named_count: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let reject = || Unsupported::Opcode {
        pc,
        op: Opcode::CallNamed,
    };
    let argc = positional_count + named_count;
    let row = module_call_abi_rows(module, name)
        // The row's `named` list is the member's whole declaration, which can be
        // longer than what this call passes (`string.replace` declares `all`
        // too, and this arity leaves it defaulted) — so it bounds the names,
        // rather than counting them.
        .find(|row| row.args.len() == argc && row.named.len() >= argc - positional_count)
        // No row of this arity, but the member may still be one that forwards
        // to a method, and a method arm is matched on its argument list rather
        // than on a row. `string.replace(s, p, w, all: false)` is that call:
        // the positional spelling of it forwards and lowers, and only the
        // named spelling landed here and fell back. Any row for the member
        // carries the same declaration — the names are the stdlib export's,
        // not the row's — so the permutation below can use it.
        .or_else(|| {
            forwards_to_method(module, name)
                .and_then(|_| module_call_abi_rows(module, name).find(|row| !row.named.is_empty()))
        })
        .ok_or_else(reject)?;
    let mut arg_regs: Vec<Option<u8>> = vec![None; argc];
    for (i, slot) in arg_regs.iter_mut().enumerate().take(positional_count) {
        *slot = Some(base.wrapping_add(1).wrapping_add(i as u8));
    }
    for pair in 0..named_count {
        let name_reg = base
            .wrapping_add(1)
            .wrapping_add(positional_count as u8)
            .wrapping_add((pair * 2) as u8);
        let value_reg = name_reg.wrapping_add(1);
        let arg_name = ssa.const_str_at(name_reg, block, pc).ok_or_else(reject)?;
        // `row.leading`, not the call's positional count: a named-eligible
        // parameter may be written either way, so a call that passes some of
        // them positionally still needs the *declaration's* frame order. Adding
        // the call's count instead pushed `string.slice(s, 1, end: 3)` past the
        // end of a 3-argument frame — the mixed spelling fell back while both
        // pure spellings lowered.
        let slot = row
            .named
            .iter()
            .position(|param| *param == arg_name.as_str())
            .ok_or_else(reject)?
            + row.leading;
        // A member may declare more names than this row's arity covers
        // (`string.replace` declares `all` too, and the row is the arity that
        // leaves it defaulted), so a name can land past the end. That is a call
        // this row cannot serve, not an index to trust. The same check catches
        // a name written for a slot an earlier *positional* argument already
        // filled — `string.slice(s, 1, start: 2)` is that call, and the VM
        // refuses it too.
        if slot >= argc || arg_regs[slot].is_some() {
            return Err(reject());
        }
        arg_regs[slot] = Some(value_reg);
    }
    let arg_regs = arg_regs.into_iter().collect::<Option<Vec<_>>>().ok_or_else(reject)?;
    // Only when no row serves this arity: a member that both forwards and has
    // a row of the right shape keeps taking the row, so nothing that lowered
    // before now takes a different path.
    if row.args.len() != argc
        && let Some(method) = forwards_to_method(module, name)
        && let Some((&receiver_reg, rest)) = arg_regs.split_first()
    {
        let (receiver, receiver_ty) = ssa.read(receiver_reg, block, pc)?;
        let args = rest
            .iter()
            .map(|reg| ssa.read(*reg, block, pc))
            .collect::<Result<Vec<_>, _>>()?;
        let result = lower_method_dispatch(ssa, insts, globals, receiver, receiver_ty, method, &args, block, pc)?;
        ssa.write(base, block, result);
        return Ok(());
    }
    lower_module_abi_call(ssa, insts, module, name, base, &arg_regs, block, pc)
}

/// The table-row half of [`lower_module_call`], with the argument registers
/// given explicitly rather than assumed consecutive.
///
/// A named call (`regex.replace(s, pattern: p, replacement: r)`) supplies its
/// arguments out of frame order, so it permutes the registers and lands here.
/// Everything above this point — the shapes with defaults, dispatch on argument
/// type, or a variadic tail — stays positional-only: those read fixed register
/// offsets, and a permuted window would need each of them to agree separately.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_module_abi_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    module: &str,
    name: &str,
    base: u8,
    arg_regs: &[u8],
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let argc = arg_regs.len();
    // A member may have one row per carrier (`hash.sha256` takes `Bytes |
    // String`), so the arity filter comes first and the argument types choose
    // among what is left. The peek is `ssa.read`, which is the same read the
    // materialisation below performs — it adds no instruction, so a candidate
    // that loses leaves nothing behind in the stream.
    let candidates: Vec<&ModuleAbiRow> = module_call_abi_rows(module, name)
        .filter(|row| row.args.len() == argc)
        .collect();
    let Some(&first) = candidates.first() else {
        return Err(Unsupported::CallShape {
            pc,
            reason: "no native lowering for this stdlib module function",
        });
    };
    let row = if candidates.len() == 1 {
        first
    } else {
        candidates
            .iter()
            .copied()
            .find(|row| {
                row.args.iter().enumerate().all(|(i, want)| {
                    ssa.read(arg_regs[i], block, pc)
                        .is_ok_and(|(_, got)| abi_param_accepts(*want, got))
                })
            })
            // No row matches: take the first and let the materialisation below
            // report the mismatch, so the failure reads the same as it does for
            // a single-row member.
            .unwrap_or(first)
    };
    let (callee, param_tys, ret_ty) = (row.abi, row.args, row.ret);
    let mut args = Vec::with_capacity(argc);
    for (i, want) in param_tys.iter().enumerate() {
        let arg_reg = arg_regs[i];
        // `Number` parameters (schema type F64) accept an Int by promotion,
        // matching the stdlib module's `number_arg` coercion.
        if *want == Ty::F64 {
            let (v, ty) = read_scalar(ssa, insts, arg_reg, block, pc)?;
            match ty {
                Ty::F64 => args.push(v),
                Ty::I64 => {
                    let f = ssa.new_val();
                    insts.push(Inst::IntToFloat { dst: f, src: v });
                    args.push(f);
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            continue;
        }
        // A `Dyn` parameter is the schema's "any value", so it boxes whatever
        // the register holds rather than demanding the caller already produced a
        // `Dyn` — the strict read refused `chan.try_send(c, 7)` for no reason
        // other than 7 being an unboxed Int.
        if *want == Ty::Dyn {
            let (v, ty) = ssa.read(arg_reg, block, pc)?;
            args.push(to_dyn(ssa, insts, v, ty, pc)?);
            continue;
        }
        // A channel or a task travels boxed (`DYN_CHAN` / `DYN_TASK`), and
        // these members take the id behind it. The generic read would unbox
        // through `dyn.as_i64`, which refuses a handle *on purpose* — a channel
        // must not be usable wherever an `Int` is required.
        // Every `I64` of theirs, not just the first: `task.join_all(a, b)`
        // takes several. A capacity or a count passes through unchanged — an
        // unboxed `I64` is returned as-is, and a boxed one unboxes either way.
        if *want == Ty::I64 && matches!(module, "chan" | "task") {
            args.push(crate::dyn_box::read_channel_id(ssa, insts, arg_reg, block, pc)?);
            continue;
        }
        args.push(ssa.read_typed(arg_reg, block, *want, pc)?);
    }
    let dst = match ret_ty {
        Ty::Nil => {
            insts.push(Inst::Call {
                dst: None,
                callee,
                args,
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
        }
        // The ABI vocabulary has no Bool: a Bool-typed member returns 0/1 as
        // I64 and narrows here.
        Ty::Bool => {
            let wide = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(wide),
                callee,
                args,
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
        _ => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee,
                args,
            });
            (dst, ret_ty)
        }
    };
    ssa.write(base, block, dst);
    Ok(())
}

/// Assembles the output pieces of a `print`/`println` call, mirroring the VM's
/// `format_variadic_runtime` exactly:
///  - no args → empty output;
///  - a *constant* string first arg is the format: each `{}` consumes the next
///    arg's display, leftover `{}` stay literal, and leftover args append
///    space-separated with a leading space iff the rendered format part is
///    non-empty (decided statically; the one runtime-dependent case — only
///    `Str` placeholders and no literal text — rejects);
///  - a dynamic (non-constant) `Str` first arg lowers only as the sole
///    argument (the output is then the string itself, `{}` included);
///  - a non-string first arg joins all args' displays with single spaces.
pub(crate) fn print_parts(
    ssa: &mut Ssa,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<Vec<PrintPart>, Unsupported> {
    let mut args = Vec::with_capacity(argc);
    for i in 0..argc {
        args.push(ssa.read(base.wrapping_add(1).wrapping_add(i as u8), block, pc)?);
    }
    let Some((&(first_v, first_ty), _)) = args.split_first() else {
        return Ok(Vec::new());
    };
    if first_ty != Ty::Str {
        let mut parts = Vec::new();
        for (i, &(v, ty)) in args.iter().enumerate() {
            if i > 0 {
                parts.push(PrintPart::Lit(" ".to_string()));
            }
            parts.push(PrintPart::Val(v, ty));
        }
        return Ok(parts);
    }
    let const_fmt = ssa
        .const_strs
        .get(&first_v)
        .cloned()
        // Loop bodies read the format through a loop-header phi (the compiler
        // hoists loop literals); recover the constant via reaching definitions.
        .or_else(|| ssa.reg_const_str(base.wrapping_add(1), block));
    let Some(fmt) = const_fmt else {
        if argc == 1 {
            return Ok(vec![PrintPart::Val(first_v, Ty::Str)]);
        }
        return Err(Unsupported::TypeMismatch { pc });
    };
    format_parts(&fmt, &args[1..], pc)
}

/// Expand one *constant* format template against its arguments.
///
/// This is the half of `println`'s lowering that `"{} and {}".format(a, b)`
/// needs too: `format`'s receiver *is* the template, so the two spellings are
/// the same expansion producing the same pieces. `println` hands the pieces to
/// [`emit_print`]; `format` folds them into a value with [`fold_parts_to_str`].
/// Sharing this is what keeps the two from drifting — the leftover-argument
/// rule below is subtle enough that a copy would.
pub(crate) fn format_parts(fmt: &str, rest: &[(ValueId, Ty)], pc: usize) -> Result<Vec<PrintPart>, Unsupported> {
    let mut parts: Vec<PrintPart> = Vec::new();
    let mut lit = String::new();
    let mut chars = fmt.chars().peekable();
    let mut next_arg = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if let Some(&(v, ty)) = rest.get(next_arg) {
                if !lit.is_empty() {
                    parts.push(PrintPart::Lit(std::mem::take(&mut lit)));
                }
                parts.push(PrintPart::Val(v, ty));
                next_arg += 1;
            } else {
                lit.push_str("{}");
            }
        } else {
            lit.push(ch);
        }
    }
    if !lit.is_empty() {
        parts.push(PrintPart::Lit(std::mem::take(&mut lit)));
    }
    let extras = &rest[next_arg..];
    if !extras.is_empty() {
        // The VM inserts one space iff the rendered format part is non-empty.
        // Statically: literal pieces are non-empty by construction and
        // i64/f64/bool displays are never empty; only a `Str` placeholder can
        // render empty, which makes the space runtime-dependent → reject.
        if !parts.is_empty() {
            let has_lit = parts.iter().any(|p| matches!(p, PrintPart::Lit(_)));
            let str_placeholder = parts.iter().any(|p| matches!(p, PrintPart::Val(_, Ty::Str)));
            if !has_lit && str_placeholder {
                return Err(Unsupported::TypeMismatch { pc });
            }
            parts.push(PrintPart::Lit(" ".to_string()));
        }
        for (i, &(v, ty)) in extras.iter().enumerate() {
            if i > 0 {
                parts.push(PrintPart::Lit(" ".to_string()));
            }
            parts.push(PrintPart::Val(v, ty));
        }
    }
    Ok(parts)
}

/// Renders assembled [`PrintPart`]s: adjacent literals merge into one interned
/// global, value parts display-convert, everything folds into a single string
/// via `str.concat` (freeing consumed temporaries), and one [`Inst::PrintStr`]
/// emits it.
pub(crate) fn emit_print(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    parts: Vec<PrintPart>,
    newline: bool,
    pc: usize,
) -> Result<(), Unsupported> {
    let (value, fresh) = fold_parts_to_str(ssa, insts, globals, parts, pc)?;
    insts.push(Inst::PrintStr { value, newline });
    if fresh {
        free_owned_str(insts, value);
    }
    Ok(())
}

/// Fold [`PrintPart`]s into one string value, answering whether the result is a
/// fresh allocation the caller now owns (as opposed to an interned constant).
///
/// `println` frees it after printing; `format` keeps it as the method's result.
pub(crate) fn fold_parts_to_str(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    parts: Vec<PrintPart>,
    pc: usize,
) -> Result<(ValueId, bool), Unsupported> {
    pub(crate) fn lit_value(ssa: &mut Ssa, insts: &mut Vec<Inst>, globals: &mut Vec<String>, text: &str) -> ValueId {
        let gid = intern_global(globals, text);
        let dst = ssa.new_val();
        insts.push(Inst::Const {
            dst,
            value: Const::Str(GlobalId(gid)),
        });
        dst
    }

    let mut pieces: Vec<(ValueId, bool)> = Vec::new();
    let mut pending = String::new();
    for part in parts {
        match part {
            PrintPart::Lit(s) => pending.push_str(&s),
            PrintPart::Val(v, ty) => {
                if !pending.is_empty() {
                    let lit = lit_value(ssa, insts, globals, &pending);
                    pieces.push((lit, false));
                    pending.clear();
                }
                pieces.push(to_display_str(ssa, insts, globals, v, ty, true, pc)?);
            }
        }
    }
    if !pending.is_empty() {
        let lit = lit_value(ssa, insts, globals, &pending);
        pieces.push((lit, false));
    }

    let (value, fresh) = match pieces.split_first() {
        None => (lit_value(ssa, insts, globals, ""), false),
        Some((&(first, first_fresh), rest)) => {
            let mut acc = first;
            let mut acc_fresh = first_fresh;
            for &(v, v_fresh) in rest {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("str", "concat"),
                    args: vec![acc, v],
                });
                if acc_fresh {
                    free_owned_str(insts, acc);
                }
                if v_fresh {
                    free_owned_str(insts, v);
                }
                acc = dst;
                acc_fresh = true;
            }
            (acc, acc_fresh)
        }
    };
    Ok((value, fresh))
}
