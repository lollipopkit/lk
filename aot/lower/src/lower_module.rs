use super::*;

/// Lowers a `module.method(args)` call whose member [`module_call_abi`] maps to
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
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                }
                let (v, ty) = ssa.read(base.wrapping_add(1), block, pc)?;
                if !matches!(ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                ssa.write(base, block, (v, ty));
                return Ok(());
            }
            "range" => {
                if argc != 2 {
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                }
                let start = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                let end = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
                let one = ssa.new_val();
                insts.push(Inst::Const {
                    dst: one,
                    value: Const::I64(1),
                });
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
                ssa.write(base, block, (handle, Ty::ListI64));
                return Ok(());
            }
            _ => {}
        }
    }
    if module == "iter" && name == "range" {
        if !(1..=3).contains(&argc) {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
    // `math.floor`/`ceil`/`round` dispatch on the argument's static type,
    // matching the VM's `integer_round`: an `Int` passes through unchanged, a
    // `Float` rounds via the lkrt helper (`f64::xxx() as i64`).
    if module == "math" && matches!(name, "floor" | "ceil" | "round") {
        if argc != 1 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
    // `math.abs` returns its argument's type: Int → wrapping integer abs
    // (select(x < 0, 0 - x, x), sub wraps like the VM's release build),
    // Float → fabs via select on the float compare.
    if module == "math" && name == "abs" {
        if argc != 1 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
            _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
        }
    }
    // `datetime.add`/`sub` are plain Int arithmetic (`timestamp ± seconds`);
    // `is_weekend` returns the helper's 0/1 as a `Bool`.
    if module == "datetime" && matches!(name, "add" | "sub") {
        if argc != 2 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
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
    let Some((callee, param_tys, ret_ty)) = module_call_abi(module, name) else {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    };
    if argc != param_tys.len() {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    }
    let mut args = Vec::with_capacity(argc);
    for (i, want) in param_tys.iter().enumerate() {
        let arg_reg = base.wrapping_add(1).wrapping_add(i as u8);
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
    let rest = &args[1..];
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
    insts.push(Inst::PrintStr { value, newline });
    if fresh {
        free_owned_str(insts, value);
    }
    Ok(())
}
