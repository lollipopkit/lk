use super::*;

/// The `lkmap::KIND_*` number for a typed string-keyed map carrier, or `None`
/// for anything else (a boxed map, a non-map).
///
/// One table, read by everything that tags a typed map handle: boxing
/// (`dyn.from_typed_map`) and the merge overlay both need the same numbering,
/// and a second copy of it would be a silent mismatch rather than an error.
pub(crate) fn typed_map_kind(ty: Ty) -> Option<i64> {
    Some(match ty {
        Ty::MapStrI64 => 0,
        Ty::MapStrF64 => 1,
        Ty::MapStrBool => 2,
        Ty::MapI64I64 => 3,
        Ty::MapI64F64 => 4,
        _ => return None,
    })
}

/// The `lkdyn::TLIST_*` number for a typed list carrier, or `None` for anything
/// else (a boxed list, a non-list).
///
/// The list counterpart of [`typed_map_kind`], and read by the same kinds of
/// call sites for the same reason: one numbering, not two.
pub(crate) fn typed_list_kind(ty: Ty) -> Option<i64> {
    Some(match ty {
        Ty::ListI64 => 0,
        Ty::ListF64 => 1,
        Ty::ListStr => 2,
        _ => return None,
    })
}

/// Normalizes a map operand to the `Map<str, Dyn>` carrier: `MapStrDyn` passes
/// through and `nil` becomes an empty map (the VM accepts a nil merge base).
///
/// A **typed** map rejects, and the reason is worth keeping: it used to convert,
/// with the claim that "iteration order is preserved — the rebuild replays the
/// source order". It does not. Re-inserting a map's entries into a fresh table
/// *in its iteration order* is a different insertion sequence from the one that
/// built it, and once deletions are in the history the two tables iterate
/// differently — the same mistake `DYN_RAW`'s doc warns about and that
/// `a_boxed_typed_map_keeps_its_order` pins for the boxing path.
///
/// Here the copy is unavoidable (the merge helper wants a real `StrDynMap`), so
/// the arm is *gone* rather than fixed: a fallback is correct, a silent reorder
/// is not. Supporting it means a typed-map-aware merge in lkrt, with its own
/// order-conformance test — separate work, not a table entry.
pub(crate) fn to_dyn_map_handle(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    match ty {
        Ty::MapStrDyn => Ok(v),
        Ty::Nil => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", "str_dyn_new"),
                args: Vec::new(),
            });
            Ok(dst)
        }
        _ => Err(Unsupported::TypeMismatch { pc }),
    }
}

/// Materializes a constant map key as a `Str` value (an interned global) for the
/// map ABI, which takes the key as a `*const c_char`.
/// Whether a constant value has a Dyn boxed form (`box_const_scalar`):
/// scalars, long strings, and (recursively) nested constant lists.
pub(crate) fn const_is_dyn_boxable(value: &ConstRuntimeValueData) -> bool {
    match value {
        ConstRuntimeValueData::Nil
        | ConstRuntimeValueData::Bool(_)
        | ConstRuntimeValueData::Int(_)
        | ConstRuntimeValueData::Float(_)
        | ConstRuntimeValueData::ShortStr(_) => true,
        ConstRuntimeValueData::Heap(heap) => match heap.as_ref() {
            ConstHeapValueData::LongString(_) => true,
            ConstHeapValueData::List(elems) => elems.iter().all(const_is_dyn_boxable),
            ConstHeapValueData::Map(entries) => entries.iter().all(|(k, v)| {
                matches!(k, RuntimeMapKeyData::ShortStr(_) | RuntimeMapKeyData::String(_)) && const_is_dyn_boxable(v)
            }),
            _ => false,
        },
    }
}

/// Boxes a typed runtime value into a `Dyn` carrier (plan M4.2): identity
/// for `Ty::Dyn`, a `dyn.from_*` call for scalars/strings/mixed lists.
/// Types without a boxed form (Maybe carriers, typed containers) reject —
/// their typed paths stay typed.
/// Coerces a list-typed value to a dyn-list *handle* (not a boxed carrier):
/// ListDyn passes through, typed lists convert element-wise (cold path —
/// only emitted for methods whose VM result is a mixed list anyway).
pub(crate) fn to_dyn_list_handle(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    // A boxed value is a list handle one tag guard away, and every caller that
    // wanted one wrote that guard itself — `chain` did, inline, and `zip` did
    // not, which is the whole of why `xs.zip(ys)` refused when `ys` was a
    // parameter. `dyn.as_list` aborts on a non-list tag, the loud error the VM
    // raises for the same call.
    if ty == Ty::Dyn {
        let unboxed = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(unboxed),
            callee: AbiRef::new("dyn", "as_list"),
            args: vec![v],
        });
        return Ok(unboxed);
    }
    let converter = match ty {
        Ty::ListDyn => return Ok(v),
        Ty::ListI64 => "i64_to_dyn",
        Ty::ListF64 => "f64_to_dyn",
        Ty::ListStr => "str_to_dyn",
        _ => return Err(Unsupported::TypeMismatch { pc }),
    };
    let converted = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(converted),
        callee: AbiRef::new("list_h", converter),
        args: vec![v],
    });
    Ok(converted)
}

/// Types [`to_dyn_any`] can box into a `Dyn` carrier.
pub(crate) fn dyn_boxable_ty(ty: Ty) -> bool {
    matches!(
        ty,
        Ty::Dyn
            | Ty::Nil
            | Ty::Bool
            | Ty::I64
            | Ty::F64
            | Ty::Str
            | Ty::ListDyn
            | Ty::ListI64
            | Ty::ListF64
            | Ty::ListStr
            | Ty::MapStrDyn
            | Ty::MapStrI64
            | Ty::MapStrF64
            | Ty::MapStrBool
            | Ty::MapI64I64
            | Ty::MapI64F64
            | Ty::Set
            | Ty::Bytes
            | Ty::MaybeI64
            | Ty::MaybeF64
            | Ty::MaybeStr
            | Ty::MaybeBool
    )
}

/// Reads a channel/task id operand: a plain `I64`, or a boxed value
/// unwrapped through the `as_i64` guard (a channel captured into a spawn
/// closure arrives boxed).
pub(crate) fn read_channel_id(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let (v, ty) = ssa.read(reg, block, pc)?;
    match ty {
        Ty::I64 => Ok(v),
        Ty::Dyn => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("dyn", "as_i64"),
                args: vec![v],
            });
            Ok(dst)
        }
        _ => Err(Unsupported::TypeMismatch { pc }),
    }
}

/// Marshals one argument for a user call: a `Dyn` parameter takes any
/// boxable value (nullable carriers included), a typed parameter takes
/// exactly its type. A residual mismatch is a stale observation from an
/// earlier fixpoint pass — tolerated there, fatal only on the final pass.
pub(crate) fn coerce_arg(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    v: ValueId,
    ty: Ty,
    want: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    if want == Ty::Dyn && ty != Ty::Dyn {
        return to_dyn(ssa, insts, v, ty, pc);
    }
    if ty != want {
        return Err(Unsupported::TypeMismatch { pc });
    }
    // A materialized stream passed to a *typed* parameter crosses without a
    // box, and the mark does not cross with it — the callee sees a plain list
    // and would answer as one. Carrying the fact across the boundary is what
    // `try_body_closure_inputs` does for a closure; a stream is rare enough
    // that declining is the better trade. See `Ssa::disguised_values`.
    if ssa.escape_is_visible.contains(&v) {
        return Err(Unsupported::TypeMismatch { pc });
    }
    Ok(v)
}

/// Boxes a typed value into a `Dyn`.
///
/// A nullable carrier boxes to its payload's tag when present and to **nil**
/// when absent (`dyn.from_maybe_*`), because that is what the value *is*: the
/// VM has no `Maybe`, it has nil, and a carrier is this backend's way of
/// carrying "the VM would have nil here". Boxing is the point at which that
/// distinction stops mattering.
///
/// This used to be two functions — one that refused a carrier and one that did
/// not — and every site except the call-argument marshaller reached for the
/// refusing one. So `xs[i] + 1` with a bounds-checked element, which is what
/// indexing *is*, dropped a whole module to the VM rather than lowering; the
/// refusal was never a semantic choice, only an unfinished match. A scalar
/// context still aborts on an absent value, but it reaches that through
/// `convert`'s unwrap, not through here.
pub(crate) fn to_dyn(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    // Boxing is where a materialized stream *escapes*: into a list element, a
    // map value, a struct field, an argument, a trait-typed return. The mark
    // does not survive the box — the value on the other side is a plain list —
    // and everything that could see the difference is on the other side. So the
    // program declines to lower rather than answering as a list. See
    // `Ssa::disguised_values`.
    if ssa.escape_is_visible.contains(&v) {
        return Err(Unsupported::TypeMismatch { pc });
    }
    let from = match ty {
        Ty::MaybeI64 => "from_maybe_i64",
        Ty::MaybeF64 => "from_maybe_f64",
        Ty::MaybeStr => "from_maybe_str",
        Ty::MaybeBool => "from_maybe_bool",
        _ => return to_dyn_plain(ssa, insts, v, ty, pc),
    };
    let value_narrow = ssa.new_val();
    insts.push(Inst::MaybeValue {
        dst: value_narrow,
        src: v,
        maybe_ty: ty,
    });
    // `MaybeValue` hands back a `MaybeBool`'s half as the `Bool` it is, and
    // `from_maybe_bool` takes the word — the same widening the present half
    // gets just below. Without it the call is not well-typed IR, so `"" +
    // m.get(k)` on a `Map<String, Bool>` failed Cranelift verification.
    let value = if ty == Ty::MaybeBool {
        let wide = ssa.new_val();
        insts.push(Inst::ZextBool {
            dst: wide,
            src: value_narrow,
        });
        wide
    } else {
        value_narrow
    };
    let present_b = ssa.new_val();
    insts.push(Inst::MaybePresent {
        dst: present_b,
        src: v,
        maybe_ty: ty,
    });
    let present = ssa.new_val();
    insts.push(Inst::ZextBool {
        dst: present,
        src: present_b,
    });
    let boxed = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(boxed),
        callee: AbiRef::new("dyn", from),
        args: vec![value, present],
    });
    Ok(boxed)
}

/// [`to_dyn`] for everything that is not a nullable carrier. Only [`to_dyn`]
/// calls it; the split exists so the carrier arms have somewhere to fall
/// through to.
fn to_dyn_plain(ssa: &mut Ssa, insts: &mut Vec<Inst>, v: ValueId, ty: Ty, pc: usize) -> Result<ValueId, Unsupported> {
    let from = match ty {
        Ty::Dyn => return Ok(v),
        Ty::I64 => "from_i64",
        Ty::F64 => "from_f64",
        Ty::Str => "from_str",
        Ty::Nil => "from_nil",
        Ty::ListDyn => "from_list",
        Ty::MapStrDyn => "from_map",
        // Both box by tagging the handle in place — no rebuild, so identity and
        // any mutation ride along.
        Ty::Set => "from_set",
        Ty::Bytes => "from_bytes",
        // A window too — in place, so the box keeps tracking the list it
        // windows. Without a box it could not enter a list, a map, a struct
        // field or a `try` value at all, which is why every one of those
        // dropped the whole program to the VM.
        Ty::SliceI64 => "from_slice",
        // A typed map boxes **in place**, under a tag naming its carrier.
        //
        // It used to convert — `str_i64_to_dyn` rebuilds the map into a
        // `str -> Dyn` one by re-inserting in iteration order. That is a
        // re-representation, and the copy's layout is not the original's once
        // deletions are in the history, so `println([m])` listed its entries in
        // an order the VM never produces. `DYN_RAW`'s doc already said boxing
        // must not re-represent a container; this is the same rule, applied
        // where it had been missed.
        Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapI64I64 | Ty::MapI64F64 => {
            let kind = typed_map_kind(ty).expect("checked by the arm");
            let kind_v = ssa.new_val();
            insts.push(Inst::Const {
                dst: kind_v,
                value: Const::I64(kind),
            });
            let boxed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_typed_map"),
                args: vec![v, kind_v],
            });
            return Ok(boxed);
        }
        // A typed list boxes **in place** too, for the same reason the typed
        // maps above do — and here the rebuild was losing more than an order.
        // `let xs = [1]; let c = [xs]; xs.push(2); c[0].len()` answered 1 where
        // the VM answers 2, and `c[0].push(9)` appended to the copy: both
        // directions of aliasing, on programs that compiled fully native.
        Ty::ListI64 | Ty::ListF64 | Ty::ListStr => {
            let kind = typed_list_kind(ty).expect("checked by the arm");
            let kind_v = ssa.new_val();
            insts.push(Inst::Const {
                dst: kind_v,
                value: Const::I64(kind),
            });
            let boxed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_typed_list"),
                args: vec![v, kind_v],
            });
            return Ok(boxed);
        }
        Ty::Bool => {
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: v });
            let boxed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_bool"),
                args: vec![wide],
            });
            return Ok(boxed);
        }
        _ => return Err(Unsupported::TypeMismatch { pc }),
    };
    let boxed = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(boxed),
        callee: AbiRef::new("dyn", from),
        args: if ty == Ty::Nil { Vec::new() } else { vec![v] },
    });
    Ok(boxed)
}

/// Boxes one constant scalar into a `Dyn` carrier value (plan M4.2): emits
/// the scalar `Const` plus the matching `dyn.from_*` call. Callers filtered
/// to scalar variants.
pub(crate) fn box_const_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    value: &ConstRuntimeValueData,
) -> ValueId {
    let boxed = ssa.new_val();
    match value {
        ConstRuntimeValueData::Nil => {
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_nil"),
                args: Vec::new(),
            });
        }
        ConstRuntimeValueData::Bool(b) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::I64(i64::from(*b)),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_bool"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::Int(n) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::I64(*n),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_i64"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::Float(x) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::F64(*x),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_f64"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::ShortStr(s) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::Str(GlobalId(intern_global(globals, s))),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_str"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::Heap(heap) => match heap.as_ref() {
            // A long string literal boxes like a short one (interned global).
            ConstHeapValueData::LongString(s) => {
                let raw = ssa.new_val();
                insts.push(Inst::Const {
                    dst: raw,
                    value: Const::Str(GlobalId(intern_global(globals, s))),
                });
                insts.push(Inst::Call {
                    dst: Some(boxed),
                    callee: AbiRef::new("dyn", "from_str"),
                    args: vec![raw],
                });
            }
            // A nested constant list: build its own dyn list recursively and
            // box the handle (`[[1,"a"],[2,"b"]]`-shaped constants).
            ConstHeapValueData::List(elems) => {
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", "dyn_new"),
                    args: Vec::new(),
                });
                for e in elems {
                    let inner = box_const_scalar(ssa, insts, globals, e);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "dyn_push"),
                        args: vec![handle, inner],
                    });
                }
                insts.push(Inst::Call {
                    dst: Some(boxed),
                    callee: AbiRef::new("dyn", "from_list"),
                    args: vec![handle],
                });
            }
            // A nested constant map (string keys): build its own str_dyn map
            // and box the handle.
            ConstHeapValueData::Map(entries) => {
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("map_h", "str_dyn_new"),
                    args: Vec::new(),
                });
                for (k, v) in entries {
                    let key_v = match k {
                        RuntimeMapKeyData::ShortStr(key) | RuntimeMapKeyData::String(key) => {
                            materialize_key(ssa, insts, globals, key)
                        }
                        _ => unreachable!("const_is_dyn_boxable filters to string keys"),
                    };
                    let inner = box_const_scalar(ssa, insts, globals, v);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("map_h", "str_dyn_set"),
                        args: vec![handle, key_v, inner],
                    });
                }
                insts.push(Inst::Call {
                    dst: Some(boxed),
                    callee: AbiRef::new("dyn", "from_map"),
                    args: vec![handle],
                });
            }
            _ => unreachable!("callers filter to boxable variants"),
        },
    }
    boxed
}
