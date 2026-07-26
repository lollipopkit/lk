use super::*;

/// Emits a widening cast if needed so `v` is an `f64` (no-op if already `f64`).
pub(crate) fn coerce_to_f64(ssa: &mut Ssa, insts: &mut Vec<Inst>, v: ValueId, ty: Ty) -> ValueId {
    if ty == Ty::F64 {
        return v;
    }
    let f = ssa.new_val();
    insts.push(Inst::IntToFloat { dst: f, src: v });
    f
}

/// Reads a register for a **scalar** (arithmetic/comparison/call/store) context,
/// narrowing a [`Ty::MaybeI64`] to `I64` via a present-asserting unwrap
/// ([`Inst::UnwrapMaybeI64`], which aborts if absent — matching the VM's halt on
/// `nil` arithmetic). Every other type passes through unchanged. This is the
/// scalar-consumer counterpart of a bare `ssa.read` (which a `return` uses instead,
/// to keep the `Maybe` and print `nil`).
pub(crate) fn read_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    pc: usize,
) -> Result<Reg, Unsupported> {
    let (v, ty) = ssa.read(reg, block, pc)?;
    match ty {
        Ty::MaybeI64 => {
            let dst = ssa.new_val();
            insts.push(Inst::UnwrapMaybeI64 { dst, src: v });
            Ok((dst, Ty::I64))
        }
        Ty::MaybeF64 => {
            let dst = ssa.new_val();
            insts.push(Inst::UnwrapMaybeF64 { dst, src: v });
            Ok((dst, Ty::F64))
        }
        Ty::MaybeStr => {
            let dst = ssa.new_val();
            insts.push(Inst::UnwrapMaybeStr { dst, src: v });
            Ok((dst, Ty::Str))
        }
        Ty::MaybeBool => {
            // Same abort-on-absent narrowing as MaybeI64, then re-typed to Bool.
            let wide = ssa.new_val();
            insts.push(Inst::UnwrapMaybeI64 { dst: wide, src: v });
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
            Ok((dst, Ty::Bool))
        }
        _ => Ok((v, ty)),
    }
}

/// [`read_scalar`] that also requires a specific type (the unwrap-aware counterpart
/// of `Ssa::read_typed`).
pub(crate) fn read_typed_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    want: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let (v, ty) = read_scalar(ssa, insts, reg, block, pc)?;
    if ty == want {
        Ok(v)
    } else {
        Err(Unsupported::TypeMismatch { pc })
    }
}

/// Converts a scalar to its display `Str` (the VM's `ToString`/interpolation
/// conversion): a `Str` passes through; `I64`/`F64`/`Bool` go through the display
/// helpers (which use the exact Rust formatting the VM uses, so output matches
/// byte-for-byte). Containers/`Maybe` reject (fall back).
/// Display-converts a value to a `Str`. The returned flag is `true` when the
/// string is a *fresh* runtime allocation created here (a `*_to_str` result) whose
/// only consumer is the caller — such temporaries may be freed once consumed
/// (`free_owned_str`), realizing the RFC §3.4 ownership model for known-dead
/// intermediates. A pre-existing `Str` (interned global or register value) is
/// returned as-is with `false`.
/// `containers` mirrors the VM's two display paths: the stdlib
/// `runtime_display` (print/println/panic/assert messages) renders containers,
/// while the executor's `runtime_value_display_string` (`ToString`, template
/// interpolation, `+` concatenation) is scalar-only and errors loudly on a
/// container — so container display must reject in those contexts.
pub(crate) fn to_display_str(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    v: ValueId,
    ty: Ty,
    containers: bool,
    pc: usize,
) -> Result<(ValueId, bool), Unsupported> {
    match ty {
        Ty::Str => Ok((v, false)),
        // A `Maybe` displays its value when present and `nil` when absent
        // (matching the VM's display of a missing-key read). The value-side
        // conversion runs unconditionally (its result is arena-owned and
        // simply unused on the absent path), then a select picks the text.
        Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
            let raw = ssa.new_val();
            insts.push(Inst::MaybeValue {
                dst: raw,
                src: v,
                maybe_ty: ty,
            });
            let scalar_ty = match ty {
                Ty::MaybeI64 | Ty::MaybeBool => Ty::I64,
                Ty::MaybeF64 => Ty::F64,
                _ => Ty::Str,
            };
            // Bool display goes through from_bool, not the i64 decimal text.
            let raw = if ty == Ty::MaybeBool {
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
                b
            } else {
                raw
            };
            let scalar_ty = if ty == Ty::MaybeBool { Ty::Bool } else { scalar_ty };
            let (value_str, _) = to_display_str(ssa, insts, globals, raw, scalar_ty, false, pc)?;
            let present = ssa.new_val();
            insts.push(Inst::MaybePresent {
                dst: present,
                src: v,
                maybe_ty: ty,
            });
            let nil_gid = intern_global(globals, "nil");
            let nil_str = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil_str,
                value: Const::Str(GlobalId(nil_gid)),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Select {
                dst,
                cond: present,
                then_v: value_str,
                else_v: nil_str,
                ty: Ty::Str,
            });
            // Not marked fresh: the value-side temporary stays arena-owned
            // (freeing it eagerly would dangle when the select picked it).
            Ok((dst, false))
        }
        Ty::I64 => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "from_i64"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        Ty::F64 => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "from_f64"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        Ty::Bool => {
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: v });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "from_bool"),
                args: vec![wide],
            });
            Ok((dst, true))
        }
        // List display (`[1,2,3]` / `["a","b c"]`) renders inside lkrt with
        // the VM's exact separators/quoting. Map display stays out of the
        // subset: its order is the underlying hash iteration order, which is
        // not portable across the two runtimes (see docs/semantics.md).
        Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn => {
            if !containers {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let display_fn = match ty {
                Ty::ListI64 => "i64_display",
                Ty::ListF64 => "f64_display",
                Ty::ListDyn => "dyn_display",
                _ => "str_display",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", display_fn),
                args: vec![v],
            });
            Ok((dst, true))
        }
        // A boxed Dyn from a mixed-list read: at runtime it is a scalar in
        // D2 (nested containers never box — see LoadHeapConst's scalar_only
        // guard), so the bare display mode is exact for both display paths.
        Ty::Dyn => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("dyn", "display"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        _ => Err(Unsupported::TypeMismatch { pc }),
    }
}

/// Emits `acc ++ display(v)`. An `I64` operand fuses into a single
/// `str.concat_i64` call (no intermediate suffix string); every other display
/// type goes through [`to_display_str`] + `str.concat`, eagerly freeing the
/// fresh display temporary.
#[allow(clippy::too_many_arguments)]
pub(crate) fn concat_display(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    acc: ValueId,
    v: ValueId,
    ty: Ty,
    containers: bool,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    if ty == Ty::I64 {
        let dst = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(dst),
            callee: AbiRef::new("str", "concat_i64"),
            args: vec![acc, v],
        });
        return Ok(dst);
    }
    let (s, fresh) = to_display_str(ssa, insts, globals, v, ty, containers, pc)?;
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("str", "concat"),
        args: vec![acc, s],
    });
    if fresh {
        free_owned_str(insts, s);
    }
    Ok(dst)
}

/// Frees a fresh, lower-created string temporary that has been fully consumed.
/// Sound only for values invisible to user code (display temporaries and
/// intermediate concat accumulators).
pub(crate) fn free_owned_str(insts: &mut Vec<Inst>, v: ValueId) {
    insts.push(Inst::Call {
        dst: None,
        callee: AbiRef::new("lkrt", "string_free"),
        args: vec![v],
    });
}

pub(crate) fn materialize_key(ssa: &mut Ssa, insts: &mut Vec<Inst>, globals: &mut Vec<String>, key: &str) -> ValueId {
    let gid = intern_global(globals, key);
    let dst = ssa.new_val();
    insts.push(Inst::Const {
        dst,
        value: Const::Str(GlobalId(gid)),
    });
    dst
}

/// Reads `list[index]` as an `i64` **scalar** (for fused list-arithmetic opcodes): a
/// provably in-range constant index folds to a clean `at`; otherwise it goes through
/// the Maybe read + present-asserting unwrap, which aborts on an out-of-range or
/// too-negative index — exactly matching the VM's `read_known_int_list_index`
/// (negative counts from the end, else the access is a fatal halt).
pub(crate) fn list_i64_element_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    list_reg: u8,
    index_reg: u8,
    block: usize,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let (handle, list_ty) = ssa.read(list_reg, block, pc)?;
    if list_ty != Ty::ListI64 {
        return Err(Unsupported::TypeMismatch { pc });
    }
    let index = read_typed_scalar(ssa, insts, index_reg, block, Ty::I64, pc)?;
    let const_in_range = match (ssa.list_len.get(&handle), ssa.const_int.get(&index)) {
        (Some(&len), Some(&idx)) if idx >= 0 && idx < len => Some(idx),
        _ => None,
    };
    if let Some(idx) = const_in_range {
        let idx_v = ssa.new_val();
        insts.push(Inst::Const {
            dst: idx_v,
            value: Const::I64(idx),
        });
        let d = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(d),
            callee: AbiRef::new("list_h", "i64_at"),
            args: vec![handle, idx_v],
        });
        Ok(d)
    } else {
        let m = ssa.new_val();
        insts.push(Inst::ListGetMaybe { dst: m, handle, index });
        let d = ssa.new_val();
        insts.push(Inst::UnwrapMaybeI64 { dst: d, src: m });
        Ok(d)
    }
}
