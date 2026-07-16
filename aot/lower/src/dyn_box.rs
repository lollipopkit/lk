use super::*;

/// Normalizes a map operand to the `Map<str, Dyn>` carrier: `MapStrDyn`
/// passes through, a typed string-keyed map converts (iteration order is
/// preserved — the rebuild replays the source order, `vm_mirror`'s
/// argument), `nil` becomes an empty map (the VM accepts a nil merge base).
pub(crate) fn to_dyn_map_handle(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let helper = match ty {
        Ty::MapStrDyn => return Ok(v),
        Ty::MapStrI64 => "str_i64_to_dyn",
        Ty::MapStrF64 => "str_f64_to_dyn",
        Ty::MapStrBool => "str_bool_to_dyn",
        Ty::Nil => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", "str_dyn_new"),
                args: Vec::new(),
            });
            return Ok(dst);
        }
        _ => return Err(Unsupported::TypeMismatch { pc }),
    };
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("map_h", helper),
        args: vec![v],
    });
    Ok(dst)
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
        return to_dyn_any(ssa, insts, v, ty, pc);
    }
    if ty != want {
        return Err(Unsupported::TypeMismatch { pc });
    }
    Ok(v)
}

/// [`to_dyn`] extended to the nullable carriers: a `Maybe` boxes to its
/// payload's tag when present and to nil when absent (`dyn.from_maybe_*`),
/// preserving VM call semantics — a nil argument arrives as nil instead of
/// hitting the scalar-context unwrap abort.
pub(crate) fn to_dyn_any(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let from = match ty {
        Ty::MaybeI64 => "from_maybe_i64",
        Ty::MaybeF64 => "from_maybe_f64",
        Ty::MaybeStr => "from_maybe_str",
        Ty::MaybeBool => "from_maybe_bool",
        _ => return to_dyn(ssa, insts, v, ty, pc),
    };
    let value = ssa.new_val();
    insts.push(Inst::MaybeValue {
        dst: value,
        src: v,
        maybe_ty: ty,
    });
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

pub(crate) fn to_dyn(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let from = match ty {
        Ty::Dyn => return Ok(v),
        Ty::I64 => "from_i64",
        Ty::F64 => "from_f64",
        Ty::Str => "from_str",
        Ty::Nil => "from_nil",
        Ty::ListDyn => "from_list",
        Ty::MapStrDyn => "from_map",
        // Typed string maps box via a value-boxing conversion (cold path:
        // a typed map crossing a `try$call` cell boundary).
        Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool => {
            let converter = match ty {
                Ty::MapStrI64 => "str_i64_to_dyn",
                Ty::MapStrF64 => "str_f64_to_dyn",
                _ => "str_bool_to_dyn",
            };
            let converted = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(converted),
                callee: AbiRef::new("map_h", converter),
                args: vec![v],
            });
            let boxed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_map"),
                args: vec![converted],
            });
            return Ok(boxed);
        }
        // Typed lists box via an element-wise conversion (cold path: only
        // emitted where a typed list actually meets a Dyn).
        Ty::ListI64 | Ty::ListF64 | Ty::ListStr => {
            let converter = match ty {
                Ty::ListI64 => "i64_to_dyn",
                Ty::ListF64 => "f64_to_dyn",
                _ => "str_to_dyn",
            };
            let converted = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(converted),
                callee: AbiRef::new("list_h", converter),
                args: vec![v],
            });
            let boxed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_list"),
                args: vec![converted],
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
