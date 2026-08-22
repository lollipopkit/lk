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

/// The language's name for a value of `ty` — what the VM's error messages say.
///
/// Not `lk_aot_mir::ty_name`, which answers this backend's carrier names
/// (`list<i64>`, `maybe<i64>`). A program never wrote those; it wrote `List`.
pub(crate) fn language_type_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Nil => "Nil",
        Ty::Bool | Ty::MaybeBool => "Bool",
        Ty::I64 | Ty::MaybeI64 => "Int",
        Ty::F64 | Ty::MaybeF64 => "Float",
        Ty::Str | Ty::MaybeStr => "String",
        Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn | Ty::SliceI64 => "List",
        Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64 => "Map",
        Ty::Set => "Set",
        Ty::Bytes => "Bytes",
        Ty::Cell | Ty::Dyn => "Object",
    }
}

/// Reads a register in a scalar context, raising `message` — verbatim — if the
/// value turns out to be nil.
///
/// The difference from [`read_scalar`] is the sentence. That one narrows a
/// carrier through `lkrt_maybe_*_unwrap`, which is handed a value and a bit and
/// so can only say `"runtime error"`; the interpreter, at the same point, names
/// the operator and both operand types. So `try { xs[9] + 1 } catch e { e }`
/// read two different strings depending on which backend ran it — a difference a
/// program can see, not just a reader.
///
/// The sentence is built by the caller, where the operator and the other
/// operand's type are still known, and interned as a constant. Nothing about the
/// present path changes: the guard is a compare and a cold call, and the value
/// comes out of the carrier exactly as before.
pub(crate) fn read_scalar_saying(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    reg: u8,
    block: usize,
    pc: usize,
    message: &str,
) -> Result<Reg, Unsupported> {
    let (v, ty) = ssa.read(reg, block, pc)?;
    let payload = match ty {
        Ty::MaybeI64 => Ty::I64,
        Ty::MaybeF64 => Ty::F64,
        Ty::MaybeStr => Ty::Str,
        Ty::MaybeBool => Ty::Bool,
        // Not nullable: the guard would have nothing to check.
        _ => return read_scalar(ssa, insts, reg, block, pc),
    };
    let present = ssa.new_val();
    insts.push(Inst::MaybePresent {
        dst: present,
        src: v,
        maybe_ty: ty,
    });
    let wide = ssa.new_val();
    insts.push(Inst::ZextBool {
        dst: wide,
        src: present,
    });
    let text = ssa.new_val();
    insts.push(Inst::Const {
        dst: text,
        value: Const::Str(GlobalId(crate::prescan::intern_global(globals, message))),
    });
    insts.push(Inst::Call {
        dst: None,
        callee: AbiRef::new("rt", "maybe_guard"),
        args: vec![wide, text],
    });
    let value = ssa.new_val();
    insts.push(Inst::MaybeValue {
        dst: value,
        src: v,
        maybe_ty: ty,
    });
    // A `MaybeBool` payload is the 0/1 word; re-typed the way `read_scalar` does.
    if payload == Ty::Bool {
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
            lhs: value,
            rhs: zero,
        });
        return Ok((dst, Ty::Bool));
    }
    Ok((value, payload))
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

/// Reads an operand that must be an `I64`, unboxing a `Dyn` through the
/// runtime's tag check.
///
/// A `Dyn` here is ordinary: iterating a list yields a `Maybe` carrier, and
/// passing that as an argument boxes it, so `fn at(xs, i) { return xs[i]; }`
/// called from `for i in idx` sees one. `dyn.as_i64` raises for a non-integer
/// tag, which is what the VM does for `xs["a"]` or `xs[1.0]` — error for error.
///
/// Named for the index case it was written for, but the rule is the same
/// wherever an Int is *required* rather than merely expected: the bitwise
/// operators and the shifts read their operands through this, so
/// `font[i] >> 3` lowers natively instead of stopping at the boxed element.
pub(crate) fn read_index_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let (v, ty) = read_scalar(ssa, insts, reg, block, pc)?;
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
        other => Err(Unsupported::OperandType {
            pc,
            want: "i64",
            got: lk_aot_mir::ty_name(other),
        }),
    }
}

/// [`read_scalar`] that also requires a specific type (the unwrap-aware counterpart
/// of `Ssa::read_typed`).
///
/// A `Dyn` is unboxed through the runtime's tag check rather than rejected —
/// the same rule, and the same `dyn.as_*` calls, that [`read_index_scalar`]
/// documents: wherever a type is *required* rather than merely expected, a
/// boxed value of that type is one, and a box holding something else raises
/// exactly where the VM raises.
///
/// What made this matter: a parameter observes as `Dyn` the moment *any* call
/// site passes a nullable carrier (see `Sig::observe_param`), and that widens
/// it for every other call site too. `s.byte_at(i)` began answering a `Maybe` —
/// honestly, since an index past the end is nil — so `put_char(base, code)`
/// widened `put_char`'s `ascii` to `Dyn`, and the `ascii == 8` inside it then
/// had a boxed operand where an `I64` was wanted. The whole bare-metal kernel
/// stopped lowering, on a function that never touches a string.
pub(crate) fn read_typed_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    want: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    read_typed_scalar_as(ssa, insts, reg, block, want, KeyUse::Value, pc)
}

/// What a [`read_typed_scalar_as`] unbox is *for*, which decides what it says
/// when the box holds the wrong thing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyUse {
    /// Any ordinary use: the generic runtime type error.
    Value,
    /// A map key or set member: a type no map can key is refused by name, the
    /// wording the interpreter and the boxed-map path both use.
    MapKey,
}

/// [`read_typed_scalar`] that also unboxes a *map key* into the carrier's key
/// type, which a typed carrier needs because it stores the key unboxed and so
/// never reaches `vm_mirror::key_from_dyn`.
pub(crate) fn read_map_key(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    want: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    read_typed_scalar_as(ssa, insts, reg, block, want, KeyUse::MapKey, pc)
}

fn read_typed_scalar_as(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    want: Ty,
    key_use: KeyUse,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let (v, ty) = read_scalar(ssa, insts, reg, block, pc)?;
    if ty == want {
        return Ok(v);
    }
    // A closure is never a scalar, so unboxing one is not a lowering — it is a
    // guess that raises at run time. Refused here so the caller can widen
    // whatever it was going to store the closure in (`inst::container`'s
    // `keep_discovery`). A map *key* is left alone: there the runtime answers,
    // and it answers with the interpreter's own sentence.
    if key_use == KeyUse::Value && ssa.closure_values.contains(&v) {
        return Err(Unsupported::TypeMismatch { pc });
    }
    let unbox = match (ty, want, key_use) {
        (Ty::Dyn, Ty::I64, KeyUse::MapKey) => "as_key_i64",
        (Ty::Dyn, Ty::Str, KeyUse::MapKey) => "as_key_str",
        (Ty::Dyn, Ty::I64, _) => "as_i64",
        (Ty::Dyn, Ty::F64, _) => "as_f64",
        (Ty::Dyn, Ty::Bool, _) => "as_bool",
        (Ty::Dyn, Ty::Str, _) => "as_str",
        _ => {
            return Err(Unsupported::OperandType {
                pc,
                want: lk_aot_mir::ty_name(want),
                got: lk_aot_mir::ty_name(ty),
            });
        }
    };
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("dyn", unbox),
        args: vec![v],
    });
    Ok(dst)
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
        // `nil` renders as the word, in every display context the VM has. It
        // had no arm at all, so `"x" + nil` and `"${nil}"` fell back.
        Ty::Nil => {
            let gid = intern_global(globals, "nil");
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Str(GlobalId(gid)),
            });
            Ok((dst, false))
        }
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
            // Bool display goes through `from_bool`, not the i64 decimal text.
            // `MaybeValue` narrows a `MaybeBool`'s word to a `Bool` itself, so
            // the extracted value is already the scalar in every case. It used
            // to be re-derived here with a `!= 0` against an `i64` zero, which
            // is not well-typed IR: `println(m.get(k))` on a `Map<String,
            // Bool>` failed Cranelift verification rather than lowering, and
            // with fallback on (the default) that reads as a program that
            // merely declines to lower.
            let scalar_ty = match ty {
                Ty::MaybeI64 => Ty::I64,
                Ty::MaybeBool => Ty::Bool,
                Ty::MaybeF64 => Ty::F64,
                _ => Ty::Str,
            };
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
        // `Set([1,2,3])`, sorted by member.
        //
        // This is the one container display that needs no mirror discipline:
        // a set's *display* order is not its hash order, it is imposed — and
        // imposed on the members' values, so both sides just compare content.
        // (`RuntimeMapKey::display_order` is the rule; it used to sort the
        // rendered text, which is why `Set([1, 2, 10])` printed `1,10,2`.)
        Ty::Set => {
            if !containers {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("set", "display"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        // `Bytes([104,105])` — rendered inside lkrt with the VM's exact
        // separators. A container, so the scalar-only display contexts reject it
        // like they reject a list.
        Ty::Bytes => {
            if !containers {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("bytes_h", "to_str"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        // A window prints as the list it windows — the VM renders a
        // `HeapValue::Slice` through the same list formatter.
        Ty::SliceI64 => {
            if !containers {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("slice_h", "i64_display"),
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
        // A struct instance. `NewObject` marked it with its type id and the
        // entry described that type to the runtime (name + field order), so the
        // renderer produces the VM's `Name{f:v,…}` — including for a field that
        // holds another struct, which is why this cannot be spelled out at the
        // display site (see `docs/aot/aot-gaps-and-lkrt.md`).
        //
        // A statically typed map renders inside lkrt from the carrier's own
        // iteration order.
        //
        // This used to say "a plain map stays out of the subset: its order is
        // the underlying hash iteration order, which the two runtimes do not
        // share" — a ruling that predates `lkrt/src/vm_mirror.rs`, whose whole
        // job is to make them share it, and which
        // `lit_protocol_matches_vm_iteration_order` checks against `lk-core`
        // directly. The arm right below already displayed a `MapStrDyn`, so the
        // ruling had been retired for one map type and left standing for the
        // rest: `println({"a": 1})` cost a program its lowering while
        // `println({"a": 1, "b": "x"})` did not.
        // An int-keyed map is included too, but it took a carrier fix first:
        // the VM runs *no* stage 2 for a non-string key
        // (`typed_map_from_entries` returns `Mixed`, which is the stage-1
        // table), while `lit_finish_i64_*` used to rehash into an
        // `FxMap<i64, _>` — a different hash and a second insertion sequence.
        // `{1: 1.5, 2: 2.5}` iterated `2,1` in the VM and `1,2` natively. The
        // carrier is now keyed by `vm_mirror::IntKey`, which hashes as
        // `RtKey::Int`, and the finisher replays the literal order.
        Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapI64I64 | Ty::MapI64F64 => {
            if !containers {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let display_fn = match ty {
                Ty::MapStrI64 => "str_i64_display",
                Ty::MapStrF64 => "str_f64_display",
                Ty::MapStrBool => "str_bool_display",
                Ty::MapI64I64 => "i64_i64_display",
                _ => "i64_f64_display",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("map_h", display_fn),
                args: vec![v],
            });
            Ok((dst, true))
        }
        Ty::MapStrDyn => {
            let boxed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_map"),
                args: vec![v],
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("dyn", if containers { "display_quoted" } else { "display" }),
                args: vec![boxed],
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
