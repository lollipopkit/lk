//! Container opcodes: list/map/object construction, indexing, mutation.

use super::LowerCtx;
use crate::trait_env::DECLARED_ANY;
use crate::*;

pub(super) fn lower(
    ctx: &mut LowerCtx<'_>,
    block: usize,
    insts: &mut Vec<Inst>,
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    let ssa = &mut *ctx.ssa;
    let globals = &mut *ctx.globals;
    let sig = &mut *ctx.sig;
    let func = ctx.func;
    let funcs = ctx.funcs;
    // Where a lambda used as a value becomes a closure (`read_value`).
    let cap_ctx = CaptureCtx {
        params: ctx.capture_params,
        index: ctx.func_index,
        param_count: ctx.func.param_count as usize,
    };
    match instr.opcode() {
        Opcode::NewList => {
            // `a` = dst, `b` = base, `c` = count: a register-window list. The
            // compiler also uses this to box method-call arguments, so the raw
            // elements are always recorded as an ArgList ref; a homogeneous
            // scalar window additionally materializes a real list handle.
            let count = instr.c() as usize;
            let mut elems = Vec::with_capacity(count);
            for i in 0..count {
                let reg = instr.b().wrapping_add(i as u8);
                // Through `read_value`, so a lambda in the literal becomes a
                // closure value here rather than reporting that it is a
                // reference: `[|x| x + 1, |x| x * 2]` is the shape.
                elems.push(read_value(ssa, insts, sig, funcs, cap_ctx, reg, block, pc)?);
            }
            let all = |t: Ty| elems.iter().all(|&(_, ty)| ty == t);
            // Same retry channel as the constant-list path above: a push of a
            // wider element contradicted this literal's element type.
            let contradicted = ssa.dyn_literal_pcs.contains(&pc);
            let materialized = if contradicted {
                None
            } else if !elems.is_empty() && all(Ty::I64) {
                Some(("i64_new", "i64_push", Ty::ListI64))
            } else if !elems.is_empty() && all(Ty::F64) {
                Some(("f64_new", "f64_push", Ty::ListF64))
            } else if !elems.is_empty() && all(Ty::Str) {
                Some(("str_new", "str_push", Ty::ListStr))
            } else {
                None
            };
            if let Some((new_fn, push_fn, list_ty)) = materialized {
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", new_fn),
                    args: Vec::new(),
                });
                for &(v, _) in &elems {
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", push_fn),
                        args: vec![handle, v],
                    });
                }
                ssa.list_len.insert(handle, elems.len() as i64);
                ssa.list_base_len.insert(handle, elems.len() as i64);
                ssa.literal_carrier.insert(handle, (pc, list_ty));
                ssa.write(instr.a(), block, (handle, list_ty));
            } else if !elems.is_empty()
                && elems.iter().all(|&(_, ty)| {
                    matches!(
                        ty,
                        Ty::I64
                            | Ty::F64
                            | Ty::Str
                            | Ty::Bool
                            | Ty::Nil
                            | Ty::Dyn
                            | Ty::ListI64
                            | Ty::ListF64
                            | Ty::ListStr
                            | Ty::ListDyn
                            | Ty::MapStrDyn
                            // The typed maps box through the same `to_dyn`
                            // family. Leaving them out did not make `[m]`
                            // reject — no arm fired, so the destination kept
                            // only the ArgList view, and the call that read it
                            // printed `{"a":1}` where the VM printed
                            // `[{"a":1}]`. A missing element type is a wrong
                            // answer here, not a fallback.
                            | Ty::MapStrI64
                            | Ty::MapStrF64
                            | Ty::MapStrBool
                            | Ty::MapI64I64
                            | Ty::MapI64F64
                            | Ty::Set
                            | Ty::Bytes
                            // A window boxes in place too (`DYN_SLICE`), so
                            // `[w]` holds something that still tracks the list
                            // it windows — which is what the VM's
                            // `HeapValue::Slice` does.
                            | Ty::SliceI64
                            // A nullable element boxes to nil when absent,
                            // which is the element the VM puts there:
                            // `[xs[9], 1]` is `[nil, 1]`. Their absence from
                            // this list is the same mistake the typed maps
                            // above were, one family later.
                            | Ty::MaybeI64
                            | Ty::MaybeF64
                            | Ty::MaybeStr
                            | Ty::MaybeBool
                    )
                })
            {
                // Mixed (or Dyn-carrying) elements: materialize a boxed-dynamic
                // list (plan M4.2), same as the constant mixed-list path but
                // boxing runtime values via `to_dyn`.
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", "dyn_new"),
                    args: Vec::new(),
                });
                // A repeated element boxes once: the VM pushes the same heap
                // handle twice (`[l, l]` dedups under `unique()`), so the
                // boxed views must share pointer identity too.
                let mut boxed_memo: std::collections::HashMap<ValueId, ValueId> = std::collections::HashMap::new();
                for &(v, ty) in &elems {
                    let boxed = match boxed_memo.get(&v) {
                        Some(&cached) => cached,
                        None => {
                            let boxed = to_dyn(ssa, insts, v, ty, pc)?;
                            boxed_memo.insert(v, boxed);
                            boxed
                        }
                    };
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "dyn_push"),
                        args: vec![handle, boxed],
                    });
                }
                ssa.list_len.insert(handle, elems.len() as i64);
                ssa.list_base_len.insert(handle, elems.len() as i64);
                // Every element the same declared struct: the list remembers
                // which, so an element read out of it is still that struct.
                let elem_struct = elems.first().and_then(|&(v, _)| ssa.struct_name(v).map(str::to_string));
                if let Some(name) = elem_struct
                    && elems.iter().all(|&(v, _)| ssa.struct_name(v) == Some(name.as_str()))
                {
                    ssa.list_elem_struct.insert(handle, name);
                }
                ssa.write(instr.a(), block, (handle, Ty::ListDyn));
            } else if elems.is_empty() {
                // An empty literal (`let flat = [];`) materializes as an
                // empty dyn list: later pushes box their elements, and the
                // cross-typed Cmp arms cover `[] == [1, 2]`-style compares.
                // (Call-window `NewList 0` also lands here; the dead handle
                // is one no-arg call.) The old concern on file — that this
                // broke typed eq lowering — was resolved by the typed↔Dyn
                // cross-type comparison arms.
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", "dyn_new"),
                    args: Vec::new(),
                });
                ssa.list_len.insert(handle, 0);
                ssa.list_base_len.insert(handle, 0);
                ssa.write(instr.a(), block, (handle, Ty::ListDyn));
            } else {
                // No arm materialized a handle. Falling through here left the
                // destination with *only* the ArgList view, and a consumer that
                // reads that view — a call window, which is the other thing
                // this opcode spells — saw the elements rather than the list.
                // `println([m])` printed `{"a":1}`. A list whose elements have
                // no boxing is a fallback, not a silent unpack.
                return Err(Unsupported::TypeMismatch { pc });
            }
            // Recorded after the write (which clears the slot) so both views
            // coexist: SSA reads see the handle, method dispatch sees elements.
            ssa.bind_ref(block, instr.a(), GlobalRef::ArgList(elems));
        }
        Opcode::NewMap => {
            // `a` = dst, `b` = base, `c` = entry count: a register window of
            // interleaved key/value pairs (`read_map_entries`).
            //
            // A map literal whose values are all constants is folded into a
            // heap constant and lowered by `LoadHeapConst` above. This is the
            // other half — `{"k": a}`, `{"a": f(3)}` — and it was missing
            // entirely, so a program that built a record from anything it had
            // computed fell back whole. The list spelling (`[a, a + 1]`) has
            // always lowered, which is what made the hole invisible: the two
            // literals read alike and only one of them compiled.
            //
            // The same builder as the constant path, deliberately: `lit_new` /
            // `lit_set` accumulate boxed pairs in literal order and
            // `lit_finish_<shape>` converts to the typed representation. The
            // shape choice below therefore only has to mirror the arms there —
            // and through them the VM's `typed_map_from_entries` — rather than
            // being a second, independently-drifting classification.
            let count = instr.c() as usize;
            let mut entries = Vec::with_capacity(count);
            for i in 0..count {
                let key_reg = instr.b().wrapping_add((i * 2) as u8);
                let val_reg = key_reg.wrapping_add(1);
                // The written text of a constant key, so the map can borrow it
                // out of the program image instead of copying it per instance.
                // A computed key (`{name: 1}`) has none, and its string may be
                // released while the map lives — that one is copied.
                let const_key = ssa.const_str_at(key_reg, block, pc);
                let key = ssa.read(key_reg, block, pc)?;
                // Through `read_value`: a lambda written as a map's value
                // becomes a closure here, the same way it does in a list
                // literal. A *key* cannot be one — a callable is not a map key
                // in this language — so that read stays as it was.
                let val = read_value(ssa, insts, sig, funcs, cap_ctx, val_reg, block, pc)?;
                entries.push((key, val, const_key));
            }
            if entries.is_empty() {
                // `{}` written as a window rather than a constant. The constant
                // path types the key by lookahead; there is nothing to look at
                // here, and a wrong guess only costs a fallback.
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("map_h", "str_i64_new"),
                    args: Vec::new(),
                });
                ssa.write(instr.a(), block, (handle, Ty::MapStrI64));
                return Ok(());
            }
            let all_keys = |t: Ty| entries.iter().all(|((_, kt), _, _)| *kt == t);
            let all_vals = |t: Ty| entries.iter().all(|(_, (_, vt), _)| *vt == t);
            let (finish_fn, map_ty) = if all_keys(Ty::Str) && all_vals(Ty::Bool) {
                ("lit_finish_str_bool", Ty::MapStrBool)
            } else if all_keys(Ty::Str) && all_vals(Ty::I64) {
                ("lit_finish_str_i64", Ty::MapStrI64)
            } else if all_keys(Ty::Str) && all_vals(Ty::F64) {
                ("lit_finish_str_f64", Ty::MapStrF64)
            } else if all_keys(Ty::I64) && all_vals(Ty::I64) {
                ("lit_finish_i64_i64", Ty::MapI64I64)
            } else if all_keys(Ty::I64) && all_vals(Ty::F64) {
                ("lit_finish_i64_f64", Ty::MapI64F64)
            } else if all_keys(Ty::Str) {
                // Heterogeneous values under string keys: the boxed map. Each
                // value has to survive boxing, which `to_dyn` decides — an
                // unboxable one rejects there rather than here.
                ("lit_finish_str_dyn", Ty::MapStrDyn)
            } else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            // Straight into the carrier the shape above already chose.
            //
            // This used to go through the two-stage literal builder
            // (`lit_new`/`lit_set`/`lit_finish_*`): every key and value boxed,
            // inserted into a `RtKey`-keyed map, then that map iterated and
            // re-inserted into the typed one. Twice the hash inserts and twice
            // the key allocations, plus a box per entry — a 24-entry map
            // literal built 100k times took 1.8s where the same-sized list
            // literal took 0.08s.
            //
            // The second stage existed to replay the VM's stage-1 *hash* order
            // into stage 2. Since the VM's maps became insertion-ordered there
            // is no such order to replay: inserting in written order is what
            // both sides do. The builder stays for the shapes chosen at run
            // time (`lit_finish_str_dyn` from a `MapRest`, the decoders).
            let (new_fn, set_fn) = match map_ty {
                Ty::MapStrBool | Ty::MapStrI64 => ("str_i64_new", "str_i64_set"),
                Ty::MapStrF64 => ("str_f64_new", "str_f64_set"),
                Ty::MapI64I64 => ("i64_i64_new", "i64_i64_set"),
                Ty::MapI64F64 => ("i64_f64_new", "i64_f64_set"),
                _ => ("str_dyn_new", "str_dyn_set"),
            };
            let handle = ssa.new_val();
            // The entry count is known here, so the map is built at its final
            // size rather than rehashing as it fills.
            if new_fn == "str_dyn_new" {
                let capacity = ssa.new_val();
                insts.push(Inst::Const {
                    dst: capacity,
                    value: Const::I64(entries.len() as i64),
                });
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("map_h", "str_dyn_new_sized"),
                    args: vec![capacity],
                });
            } else {
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("map_h", new_fn),
                    args: Vec::new(),
                });
            }
            // A literal is an ordinary map. Only `NewObject` builds a struct,
            // and the two share the `MapStrDyn` carrier, so the collection
            // operations need this said out loud to answer at all.
            ssa.set_plain_map(handle);
            for ((k, kt), (v, vt), const_key) in entries.clone() {
                let value = match map_ty {
                    Ty::MapStrDyn => to_dyn(ssa, insts, v, vt, pc)?,
                    // A `bool` carrier stores its members as `i64` (it shares
                    // the `str_i64` ABI), and a MIR `Bool` is one bit.
                    Ty::MapStrBool => {
                        let wide = ssa.new_val();
                        insts.push(Inst::ZextBool { dst: wide, src: v });
                        wide
                    }
                    _ => v,
                };
                let _ = (kt, vt);
                // A constant key is re-materialised as the interned global and
                // borrowed; anything else keeps the copying setter.
                let (set_fn, k) = match (set_fn, const_key.as_deref()) {
                    ("str_dyn_set", Some(text)) => ("str_dyn_set_const", materialize_key(ssa, insts, globals, text)),
                    _ => (set_fn, k),
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", set_fn),
                    args: vec![handle, k, value],
                });
            }
            let _ = finish_fn;
            ssa.write(instr.a(), block, (handle, map_ty));
        }
        Opcode::GetIndexStrI | Opcode::SetIndexStrI => {
            // Composite string-int key access (`m["n${i}"]`): the key is the
            // compiler-proven constant prefix plus the decimal suffix register.
            // A store passes (prefix, suffix) straight to the `set_ik` ABI (key
            // built on the stack inside lkrt, nothing to free); a load builds
            // the key via `concat_i64` in one allocation, and the fresh
            // temporary frees right after the map call.
            let Some(key_fact) = func.performance.known_key(pc).and_then(|fact| fact.string_int) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let prefix = func
                .consts
                .strings
                .get(key_fact.prefix_key as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let prefix_v = materialize_key(ssa, insts, globals, prefix);
            let is_set = instr.opcode() == Opcode::SetIndexStrI;
            let (map_reg, suffix_reg) = if is_set {
                (instr.a(), instr.b())
            } else {
                (instr.b(), instr.c())
            };
            let (handle, map_ty) = ssa.read(map_reg, block, pc)?;
            let suffix = ssa.read_typed(suffix_reg, block, Ty::I64, pc)?;
            if is_set {
                let (value, value_ty) = ssa.read(instr.c(), block, pc)?;
                let set_fn = match (map_ty, value_ty) {
                    (Ty::MapStrI64, Ty::I64) => "str_i64_set_ik",
                    (Ty::MapStrF64, Ty::F64) => "str_f64_set_ik",
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", set_fn),
                    args: vec![handle, prefix_v, suffix, value],
                });
            } else {
                let key = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(key),
                    callee: AbiRef::new("str", "concat_i64"),
                    args: vec![prefix_v, suffix],
                });
                let dst = ssa.new_val();
                let maybe_ty = match map_ty {
                    Ty::MapStrI64 => {
                        insts.push(Inst::MapGetMaybe { dst, handle, key });
                        Ty::MaybeI64
                    }
                    Ty::MapStrF64 => {
                        insts.push(Inst::MapGetMaybeStrF64 { dst, handle, key });
                        Ty::MaybeF64
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                free_owned_str(insts, key);
                ssa.write(instr.a(), block, (dst, maybe_ty));
            }
        }
        Opcode::LoadHeapConst => {
            // Constant container literals: materialize a growable `lkrt` handle.
            //  - `List<i64>` / `List<f64>` → new + push per element.
            //  - `Map<str, i64>` → new + set per (const-string key, int value) entry.
            // Other heap constants (nested/mixed, other key/elem types, long strings)
            // fall back.
            let hv = func
                .consts
                .heap_values
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            match hv {
                ConstHeapValueData::List(elems) => {
                    // An empty `[]` is ambiguous — a lookahead types it from the
                    // first value pushed (a wrong guess only costs a fallback).
                    if elems.is_empty() {
                        let (new_fn, list_ty) = if ssa.dyn_literal_pcs.contains(&pc) {
                            // A consumer contradicted an earlier guess — the
                            // fixpoint retry forces the Dyn materialization.
                            ("dyn_new", Ty::ListDyn)
                        } else {
                            match empty_list_elem_guess(func, pc, instr.a()) {
                                EmptyListGuess::Str => ("str_new", Ty::ListStr),
                                EmptyListGuess::Dyn => ("dyn_new", Ty::ListDyn),
                                EmptyListGuess::Default => ("i64_new", Ty::ListI64),
                            }
                        };
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("list_h", new_fn),
                            args: Vec::new(),
                        });
                        if list_ty != Ty::ListDyn {
                            ssa.literal_carrier.insert(handle, (pc, list_ty));
                        }
                        ssa.list_len.insert(handle, 0);
                        ssa.list_base_len.insert(handle, 0);
                        ssa.write(instr.a(), block, (handle, list_ty));
                        return Ok(());
                    }
                    let all_int = elems.iter().all(|e| matches!(e, ConstRuntimeValueData::Int(_)));
                    let all_float = elems.iter().all(|e| matches!(e, ConstRuntimeValueData::Float(_)));
                    let all_str = elems.iter().all(|e| matches!(e, ConstRuntimeValueData::ShortStr(_)));
                    // A push of a wider element contradicted this literal's
                    // element type, so the fixpoint asked for it as a Dyn list.
                    // A homogeneous literal is as contradictable as an empty
                    // one: `let xs: List<Any> = [1, 2]; xs.push("a");` is the
                    // shape the VM answers by widening the carrier in place,
                    // and the only reason it was refused here is that a
                    // `Vec<i64>` cannot become a `Vec<LkDyn>` after the fact.
                    // Building it Dyn from the start is the same answer.
                    if ssa.dyn_literal_pcs.contains(&pc) && elems.iter().all(const_is_dyn_boxable) {
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("list_h", "dyn_new"),
                            args: Vec::new(),
                        });
                        for e in elems {
                            let boxed = box_const_scalar(ssa, insts, globals, e);
                            insts.push(Inst::Call {
                                dst: None,
                                callee: AbiRef::new("list_h", "dyn_push"),
                                args: vec![handle, boxed],
                            });
                        }
                        ssa.list_len.insert(handle, elems.len() as i64);
                        ssa.list_base_len.insert(handle, elems.len() as i64);
                        ssa.write(instr.a(), block, (handle, Ty::ListDyn));
                        return Ok(());
                    }
                    let (new_fn, push_fn, list_ty) = if all_int {
                        ("i64_new", "i64_push", Ty::ListI64)
                    } else if all_float {
                        ("f64_new", "f64_push", Ty::ListF64)
                    } else if all_str {
                        ("str_new", "str_push", Ty::ListStr)
                    } else {
                        // Mixed scalar elements: a boxed-dynamic list (plan
                        // M4.2 Dyn). Nested containers still fall back.
                        if !elems.iter().all(const_is_dyn_boxable) {
                            return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                        }
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("list_h", "dyn_new"),
                            args: Vec::new(),
                        });
                        for e in elems {
                            let boxed = box_const_scalar(ssa, insts, globals, e);
                            insts.push(Inst::Call {
                                dst: None,
                                callee: AbiRef::new("list_h", "dyn_push"),
                                args: vec![handle, boxed],
                            });
                        }
                        ssa.list_len.insert(handle, elems.len() as i64);
                        ssa.list_base_len.insert(handle, elems.len() as i64);
                        ssa.write(instr.a(), block, (handle, Ty::ListDyn));
                        return Ok(());
                    };
                    let handle = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(handle),
                        callee: AbiRef::new("list_h", new_fn),
                        args: Vec::new(),
                    });
                    for e in elems {
                        let v = ssa.new_val();
                        let value = match e {
                            ConstRuntimeValueData::Int(n) => Const::I64(*n),
                            ConstRuntimeValueData::Float(x) => Const::F64(*x),
                            ConstRuntimeValueData::ShortStr(s) => Const::Str(GlobalId(intern_global(globals, s))),
                            _ => unreachable!("filtered to a single element type above"),
                        };
                        insts.push(Inst::Const { dst: v, value });
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("list_h", push_fn),
                            args: vec![handle, v],
                        });
                    }
                    ssa.list_len.insert(handle, elems.len() as i64);
                    ssa.list_base_len.insert(handle, elems.len() as i64);
                    // Recorded like an empty literal's guess: a later push of a
                    // wider element names this pc, and the fixpoint rebuilds it
                    // above as a Dyn list.
                    ssa.literal_carrier.insert(handle, (pc, list_ty));
                    ssa.write(instr.a(), block, (handle, list_ty));
                }
                ConstHeapValueData::Map(entries) => {
                    // Map literals build through the lit protocol (plan D1):
                    // stage-1 inserts in serialized order, the finisher
                    // replays the VM's two-stage construction so iteration
                    // order (`for k in m`, `.keys()`) is VM-exact. Shapes:
                    // uniform str/int keys with int/float/bool values take a
                    // typed carrier; mixed boxable values a `Map<str, Dyn>`.
                    let all_str_keys = entries
                        .iter()
                        .all(|(k, _)| matches!(k, RuntimeMapKeyData::ShortStr(_) | RuntimeMapKeyData::String(_)));
                    let all_int_keys = entries.iter().all(|(k, _)| matches!(k, RuntimeMapKeyData::Int(_)));
                    let all_bool_vals =
                        !entries.is_empty() && entries.iter().all(|(_, v)| matches!(v, ConstRuntimeValueData::Bool(_)));
                    let all_int_vals = entries.iter().all(|(_, v)| matches!(v, ConstRuntimeValueData::Int(_)));
                    let all_f64_vals = entries
                        .iter()
                        .all(|(_, v)| matches!(v, ConstRuntimeValueData::Float(_)));
                    // An empty `{}` is ambiguous: a lookahead types the key (a
                    // wrong guess only costs a fallback), the value defaults
                    // to `i64`; no entries means no order to mirror.
                    if entries.is_empty() {
                        let new_fn = if ssa.dyn_literal_pcs.contains(&pc) {
                            // A store of a wider value contradicted the guess.
                            ("str_dyn_new", Ty::MapStrDyn)
                        } else if empty_map_is_int_keyed(func, pc, instr.a()) {
                            ("i64_i64_new", Ty::MapI64I64)
                        } else {
                            ("str_i64_new", Ty::MapStrI64)
                        };
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("map_h", new_fn.0),
                            args: Vec::new(),
                        });
                        if new_fn.1 != Ty::MapStrDyn {
                            ssa.literal_carrier.insert(handle, (pc, new_fn.1));
                        }
                        ssa.set_plain_map(handle);
                        ssa.write(instr.a(), block, (handle, new_fn.1));
                        return Ok(());
                    }
                    let contradicted = ssa.dyn_literal_pcs.contains(&pc)
                        && all_str_keys
                        && entries.iter().all(|(_, v)| const_is_dyn_boxable(v));
                    let (finish_fn, map_ty) = if contradicted {
                        ("lit_finish_str_dyn", Ty::MapStrDyn)
                    } else if all_str_keys && all_bool_vals {
                        ("lit_finish_str_bool", Ty::MapStrBool)
                    } else if all_str_keys && all_int_vals {
                        ("lit_finish_str_i64", Ty::MapStrI64)
                    } else if all_str_keys && all_f64_vals {
                        ("lit_finish_str_f64", Ty::MapStrF64)
                    } else if all_int_keys && all_int_vals {
                        ("lit_finish_i64_i64", Ty::MapI64I64)
                    } else if all_int_keys && all_f64_vals {
                        ("lit_finish_i64_f64", Ty::MapI64F64)
                    } else if all_str_keys && entries.iter().all(|(_, v)| const_is_dyn_boxable(v)) {
                        ("lit_finish_str_dyn", Ty::MapStrDyn)
                    } else {
                        // Non-scalar values / mixed key kinds fall back.
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                    };
                    // Straight into the carrier, as the register-window path
                    // above does and for the same reason: the two-stage
                    // builder's second stage only existed to replay stage 1's
                    // *hash* order, and the VM's maps are insertion-ordered now.
                    let (new_fn, set_fn) = match map_ty {
                        Ty::MapStrBool | Ty::MapStrI64 => ("str_i64_new", "str_i64_set"),
                        Ty::MapStrF64 => ("str_f64_new", "str_f64_set"),
                        Ty::MapI64I64 => ("i64_i64_new", "i64_i64_set"),
                        Ty::MapI64F64 => ("i64_f64_new", "i64_f64_set"),
                        _ => ("str_dyn_new", "str_dyn_set"),
                    };
                    let handle = ssa.new_val();
                    // A literal knows how many entries it has, so the map is
                    // built at its final size instead of rehashing on the way.
                    if new_fn == "str_dyn_new" {
                        let capacity = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: capacity,
                            value: Const::I64(entries.len() as i64),
                        });
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("map_h", "str_dyn_new_sized"),
                            args: vec![capacity],
                        });
                    } else {
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("map_h", new_fn),
                            args: Vec::new(),
                        });
                    }
                    ssa.set_plain_map(handle);
                    for (k, v) in entries {
                        let key = match k {
                            RuntimeMapKeyData::ShortStr(key) | RuntimeMapKeyData::String(key) => {
                                materialize_key(ssa, insts, globals, key)
                            }
                            RuntimeMapKeyData::Int(ik) => {
                                let raw = ssa.new_val();
                                insts.push(Inst::Const {
                                    dst: raw,
                                    value: Const::I64(*ik),
                                });
                                raw
                            }
                            _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
                        };
                        let value = match map_ty {
                            Ty::MapStrDyn => box_const_scalar(ssa, insts, globals, v),
                            _ => unboxed_const_scalar(ssa, insts, globals, v)
                                .ok_or(Unsupported::Opcode { pc, op: instr.opcode() })?,
                        };
                        // A literal's string key is an interned global, so the
                        // map borrows it rather than copying it per instance.
                        let set_fn = if set_fn == "str_dyn_set" && !matches!(k, RuntimeMapKeyData::Int(_)) {
                            "str_dyn_set_const"
                        } else {
                            set_fn
                        };
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("map_h", set_fn),
                            args: vec![handle, key, value],
                        });
                    }
                    let _ = finish_fn;
                    if map_ty != Ty::MapStrDyn {
                        ssa.literal_carrier.insert(handle, (pc, map_ty));
                    }
                    ssa.write(instr.a(), block, (handle, map_ty));
                }
                ConstHeapValueData::LongString(s) => {
                    // A string literal too long for the inline `ShortStr` encoding:
                    // same lowering as `LoadString` (an interned C-string global).
                    let gid = intern_global(globals, s);
                    let dst = ssa.new_val();
                    insts.push(Inst::Const {
                        dst,
                        value: Const::Str(GlobalId(gid)),
                    });
                    ssa.const_strs.insert(dst, s.clone());
                    ssa.write(instr.a(), block, (dst, Ty::Str));
                }
                ConstHeapValueData::UpvalCell(initial) => {
                    // The compiler's shared mutable box for a captured local.
                    // The cell never materializes: its content lives in a
                    // virtual SSA slot (`reg_count + cid`) under the same
                    // Braun construction as registers, so cross-block state
                    // (mutation in a branch, reads after a merge, loop-carried
                    // updates) gets phis. Cells start nil (any pre-store read
                    // is a `Nil` value, exactly the VM's fresh-cell content);
                    // re-executing this site (a loop-created cell) re-
                    // initializes the slot, matching the VM's fresh cell.
                    if !matches!(initial.as_ref(), ConstRuntimeValueData::Nil) {
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                    }
                    let cid = ssa.next_cell;
                    ssa.next_cell += 1;
                    let nil = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: nil,
                        value: Const::Nil,
                    });
                    let slot = ssa.cell_slot(cid);
                    ssa.write_slot(slot, block, (nil, Ty::Nil));
                    ssa.bind_ref(block, instr.a(), GlobalRef::Cell(cid));
                }
            }
        }
        Opcode::Len => {
            // `a` = dst, `b` = container register; the length is always a plain `i64`,
            // regardless of element type (lists) or key/value type (maps).
            //
            // Read through `read_scalar`, so a `Maybe` receiver unwraps first —
            // the VM raises on `nil.len()`, and so does the unwrap. A string
            // list's loop variable is a `Maybe` (the element read is
            // bounds-checked), so without this `for s in ["ab", "cde"] {
            // s.len() }` dropped the whole program to the interpreter.
            let (handle, ty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            // A struct instance rides the map carrier and has no length: the
            // interpreter answers "`len()` has no answer for P". Declining is
            // what the method spelling does too (`lower_method_dispatch`).
            if !ssa.is_plain_map(handle) && matches!(ty, Ty::MapStrDyn) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            // Strings count Unicode scalar values (the VM's char length), which
            // is a string call rather than a container one; every other carrier
            // shares its row with `is_empty`.
            let callee = if ty == Ty::Str {
                AbiRef::new("str", "char_len")
            } else {
                container_len_abi(ty).ok_or(Unsupported::TypeMismatch { pc })?
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee,
                args: vec![handle],
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        Opcode::SliceFrom => {
            // `a` = dst, `b` = target (list) register, `c` = start register. Only
            // typed lists lower natively — the runtime returns a fresh handle
            // with the elements from `start` on (negative `start` aborts, like
            // the VM). String slicing and other element types fall back for now.
            let (handle, ty) = ssa.read(instr.b(), block, pc)?;
            // A boxed Dyn target (`for [head, ..tail] in matrix` slices the
            // iterated row) unwraps through the as_list guard first.
            let (handle, ty) = if ty == Ty::Dyn {
                let unboxed = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(unboxed),
                    callee: AbiRef::new("dyn", "as_list"),
                    args: vec![handle],
                });
                (unboxed, Ty::ListDyn)
            } else {
                (handle, ty)
            };
            let slice_fn = match ty {
                Ty::ListI64 => "i64_slice_from",
                Ty::ListF64 => "f64_slice_from",
                Ty::ListStr => "str_slice_from",
                Ty::ListDyn => "dyn_slice_from",
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let start = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", slice_fn),
                args: vec![handle, start],
            });
            ssa.write(instr.a(), block, (dst, ty));
        }
        Opcode::NewRange => {
            // `a` = dst; `b`..`b+2` = start/end/step registers; `c` != 0 =
            // inclusive. The VM materializes the range eagerly as a
            // `List<Int>` (`build_int_range`) — same here via one lkrt call
            // (zero step / stepping overflow abort inside the helper).
            let start = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let end = read_typed_scalar(ssa, insts, instr.b().wrapping_add(1), block, Ty::I64, pc)?;
            let step = read_typed_scalar(ssa, insts, instr.b().wrapping_add(2), block, Ty::I64, pc)?;
            let inclusive = ssa.new_val();
            insts.push(Inst::Const {
                dst: inclusive,
                value: Const::I64(i64::from(instr.c() != 0)),
            });
            let handle = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(handle),
                callee: AbiRef::new("list_h", "i64_from_range"),
                args: vec![start, end, step, inclusive],
            });
            // A fully-constant unit-step range keeps its slice meaning
            // alongside the materialized list (`s[1..3]` indexes by range).
            if let (Some(&s0), Some(&e0), Some(&st)) = (
                ssa.const_int.get(&start),
                ssa.const_int.get(&end),
                ssa.const_int.get(&step),
            ) && st == 1
            {
                let end_excl = if instr.c() != 0 { e0.saturating_add(1) } else { e0 };
                ssa.range_def.insert(handle, (s0, end_excl));
            }
            ssa.write(instr.a(), block, (handle, Ty::ListI64));
        }
        Opcode::ToIter => {
            // `a` = dst, `b` = source. The VM normalizes the iterated value
            // to a list (`to_iter`): lists pass through, a string iterates
            // per char (a Mixed list there → dyn list here), a boxed Dyn
            // unwraps through the as_list guard (iterating a non-container
            // is the VM's loud error). A map snapshots to `[key, value]`
            // pair lists — in the VM's exact order, by the layout mirror
            // (`lkrt vm_mirror.rs`, plan D1/D2).
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            match ty {
                // A window iterates as itself — `len` and indexing on it are
                // window-relative, which is exactly what the loop needs. The VM
                // does the same (`to_iter` hands back the slice handle rather
                // than materializing it).
                Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn | Ty::SliceI64 => {
                    ssa.write(instr.a(), block, (v, ty));
                }
                Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64 => {
                    // A struct instance rides the `MapStrDyn` carrier and is
                    // not iterable — the VM raises `ToIter target object is not
                    // iterable`. Iterating one handed the loop its fields as
                    // pairs.
                    if ty == Ty::MapStrDyn && !ssa.is_plain_map(v) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let iter_fn = match ty {
                        Ty::MapStrI64 => "str_i64_iter_pairs",
                        Ty::MapStrF64 => "str_f64_iter_pairs",
                        Ty::MapStrBool => "str_bool_iter_pairs",
                        Ty::MapI64I64 => "i64_i64_iter_pairs",
                        Ty::MapI64F64 => "i64_f64_iter_pairs",
                        _ => "str_dyn_iter_pairs",
                    };
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("map_h", iter_fn),
                        args: vec![v],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::ListDyn));
                }
                Ty::Str => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("str", "chars"),
                        args: vec![v],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::ListDyn));
                }
                // A set snapshots to its members, in the VM's order — both
                // sides key by the same `RtKey` and fill by the same sequence,
                // and a set has no second stage for anything else to enter.
                Ty::Set => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("set", "iter"),
                        args: vec![v],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::ListDyn));
                }
                // `Bytes` iterates its byte values, in order — no hash
                // anywhere, so nothing to mirror.
                Ty::Bytes => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("bytes_h", "to_i64_list"),
                        args: vec![v],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::ListI64));
                }
                // A boxed value normalizes at run time, by tag, exactly as the
                // arms above do by static type. This used to be `dyn.as_list`
                // — a *list* guard — so a boxed map, set, bytes or string
                // raised `runtime type error` in a loop the VM runs.
                Ty::Dyn => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("dyn", "to_iter"),
                        args: vec![v],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::ListDyn));
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
        }
        Opcode::NewObject => {
            // `a` = dst, `b` = base, `c` = field count: `base` holds the type
            // name, fields at `base+1+2k` (constant-string key) / `base+2+2k`
            // (value). A struct instance is carried as a string-keyed Dyn map
            // (plan M4.2, decision D4): `GetFieldK` reads work unchanged, an
            // absent optional field is `str_dyn_get`'s Nil — matching the
            // VM's absent-Object-field nil. The type name is dropped: whole-
            // object display/`typeof` are not in the native subset.
            // Sized: a struct literal knows its field count, and growing a map
            // rehashes everything already in it.
            let capacity = ssa.new_val();
            insts.push(Inst::Const {
                dst: capacity,
                value: Const::I64(i64::from(instr.c())),
            });
            let map = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(map),
                callee: AbiRef::new("map_h", "str_dyn_new_sized"),
                args: vec![capacity],
            });
            let type_name = ssa.const_str_at(instr.b(), block, pc);
            for i in 0..instr.c() as usize {
                let key_reg = instr.b().wrapping_add(1).wrapping_add((i * 2) as u8);
                let value_reg = key_reg.wrapping_add(1);
                let key = ssa
                    .const_str_at(key_reg, block, pc)
                    .ok_or(Unsupported::Opcode { pc, op: instr.opcode() })?;
                let key_v = materialize_key(ssa, insts, globals, &key);
                // Through `read_value`: a struct field written as a lambda
                // becomes a closure here, like a list element or a map value.
                let (vv, vty) = read_value(ssa, insts, sig, funcs, cap_ctx, value_reg, block, pc)?;
                // `to_dyn_any`: a dynamically indexed field value arrives as
                // a `Maybe` carrier and boxes through `from_maybe_*` (nil
                // stays nil, like the VM's absent-element field value).
                let boxed = to_dyn(ssa, insts, vv, vty, pc)?;
                // `A { v: x }` with an untyped `x` is a store the type checker
                // cannot see, exactly like `p["v"] = x` is — so it is measured
                // against the declaration. Only when the value's own type does
                // not already settle it: a literal `Int` into an `Int` field
                // needs nothing, which is the common case and pays nothing.
                if let Some(name) = type_name.as_deref() {
                    emit_declared_field_check(ssa, insts, globals, sig, name, &key, vty, boxed);
                }
                insts.push(Inst::Call {
                    dst: None,
                    // The field name is an interned global, so the map borrows
                    // it instead of copying it into every instance.
                    callee: AbiRef::new("map_h", "str_dyn_set_const"),
                    args: vec![map, key_v, boxed],
                });
            }
            // Struct provenance (plan J1): the type name drives static method
            // devirtualization; a type with registered trait impls also marks
            // the handle for boxed runtime dispatch.
            if let Some(type_name) = type_name {
                if let Some(&tid) = sig.traits.type_ids.get(&type_name) {
                    let tid_v = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: tid_v,
                        value: Const::I64(tid),
                    });
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("map_h", "obj_mark"),
                        args: vec![map, tid_v],
                    });
                }
                ssa.set_struct(map, type_name);
            }
            ssa.write(instr.a(), block, (map, Ty::MapStrDyn));
        }
        Opcode::ListPush => {
            // `a` = list register (mutated in place), `b` = value register. The list
            // handle is a reference (matching the VM), so the push is visible through
            // aliases; no new SSA value is produced for the list.
            let (handle, list_ty) = ssa.read(instr.a(), block, pc)?;
            // A boxed receiver pushes through `dyn.list_push`, which reaches
            // the carrier behind the tag. It used to unwrap through
            // `dyn.as_list` — correct while every boxed list was a
            // `Vec<LkDyn>`, and a lost write once typed carriers box in place,
            // because that guard has to materialize one.
            if list_ty == Ty::Dyn {
                let (value, value_ty) = ssa.read(instr.b(), block, pc)?;
                let boxed = to_dyn(ssa, insts, value, value_ty, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("dyn", "list_push"),
                    args: vec![handle, boxed],
                });
                return Ok(());
            }
            // A lambda pushed into a list becomes a closure value, and the
            // carrier has to be one that can hold it.
            if let Some(GlobalRef::Lambda(_) | GlobalRef::Closure(..) | GlobalRef::UserFn(_)) =
                ssa.builtin_ref_at(instr.b(), block)
            {
                let (value, value_ty) = read_value(ssa, insts, sig, funcs, cap_ctx, instr.b(), block, pc)?;
                if list_ty != Ty::ListDyn && list_ty != Ty::Dyn {
                    return Err(
                        carrier_contradicted_here_or_at_callers(ssa, func, instr.a(), handle, list_ty)
                            .unwrap_or(Unsupported::TypeMismatch { pc }),
                    );
                }
                let boxed = to_dyn(ssa, insts, value, value_ty, pc)?;
                let (module, name) = if list_ty == Ty::Dyn {
                    ("dyn", "list_push")
                } else {
                    ("list_h", "dyn_push")
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new(module, name),
                    args: vec![handle, boxed],
                });
                return Ok(());
            }
            // Values read through `read_scalar` so a `Maybe` (a dynamic list
            // read like `xs[i]` in `flat.push(xs[i])`) unwraps first.
            // A push whose value type contradicts a guessed empty-`[]`
            // element type retries the literal as a Dyn list (fixpoint).
            let guess_wrong =
                |ssa: &Ssa| carrier_contradicted_here_or_at_callers(ssa, func, instr.a(), handle, list_ty);
            match list_ty {
                Ty::ListI64 => {
                    if let Some(e) =
                        nullable_into_typed_carrier(ssa, func, instr.b(), block, instr.a(), handle, list_ty)
                    {
                        return Err(e);
                    }
                    let value = match read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc) {
                        Ok(v) => v,
                        Err(e) => return Err(keep_discovery(e, guess_wrong(ssa))),
                    };
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "i64_push"),
                        args: vec![handle, value],
                    });
                }
                Ty::ListF64 => {
                    if let Some(e) =
                        nullable_into_typed_carrier(ssa, func, instr.b(), block, instr.a(), handle, list_ty)
                    {
                        return Err(e);
                    }
                    let (bv, bty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
                    if !matches!(bty, Ty::I64 | Ty::F64) {
                        return Err(guess_wrong(ssa).unwrap_or(Unsupported::TypeMismatch { pc }));
                    }
                    let value = coerce_to_f64(ssa, insts, bv, bty);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "f64_push"),
                        args: vec![handle, value],
                    });
                }
                Ty::ListStr => {
                    // Stored strings are arena-owned (interned constants or
                    // register-visible arena strings), alive until exit, so the
                    // pointer push involves no ownership transfer.
                    if let Some(e) =
                        nullable_into_typed_carrier(ssa, func, instr.b(), block, instr.a(), handle, list_ty)
                    {
                        return Err(e);
                    }
                    let value = match read_typed_scalar(ssa, insts, instr.b(), block, Ty::Str, pc) {
                        Ok(v) => v,
                        Err(e) => return Err(keep_discovery(e, guess_wrong(ssa))),
                    };
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "str_push"),
                        args: vec![handle, value],
                    });
                }
                // Mixed list: any boxable value pushes as a Dyn carrier —
                // including a nullable one, which pushes **nil**.
                //
                // Read raw rather than through `read_scalar`: that narrows a
                // carrier by asserting it is present, which is right where a
                // number is required and wrong here. A Dyn list holds nil, and
                // the VM puts nil in it, so `out.push(xs[i])` past the end of
                // `xs` appended nil on the interpreter and *raised* compiled —
                // a program that ran one way and died the other.
                Ty::ListDyn => {
                    let (bv, bty) = ssa.read(instr.b(), block, pc)?;
                    let boxed = to_dyn(ssa, insts, bv, bty, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "dyn_push"),
                        args: vec![handle, boxed],
                    });
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            // Keep the known length in sync so subsequent constant-index bounds
            // checks stay accurate (only meaningful for a still-tracked handle).
            if let Some(len) = ssa.list_len.get_mut(&handle) {
                *len += 1;
            }
        }
        // `GetList` is the list-typed index; `GetIndex` is the generic index the
        // compiler emits when it hasn't proven the container is a list (e.g. inside a
        // `for x in xs` loop body). For a list operand both have identical semantics,
        // so they share this arm; a non-list operand rejects (falls back).
        Opcode::GetList | Opcode::GetIndex => {
            // A constant-name member read on a module object resolves to a
            // module function ref (`os.clock` → `GetIndex` on the module with a
            // constant string key) — or, for constant members (`math.pi`), to
            // the literal value itself.
            // A constant-name member read on a bundled file module resolves
            // to the merged function (`fib.iterative` → direct call target).
            if let Some(GlobalRef::UserModule(bundle)) = ssa.builtin_regs.get(&(block, instr.b())).cloned() {
                let name = ssa.const_str_at(instr.c(), block, pc);
                let fidx = name.and_then(|n| sig.imports.bundles.get(bundle).and_then(|b| b.fns.get(&n)).copied());
                let Some(fidx) = fidx else {
                    return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                };
                ssa.bind_ref(block, instr.a(), GlobalRef::Lambda(fidx));
                return Ok(());
            }
            // `use chan;` binds the module over the `chan()` global, and both
            // are the *same name* — so `builtin_for_name` claims it first and
            // the module case never got a chance: `chan.new(1)` dropped its
            // module to the VM while `chan(1)` lowered.
            //
            // The bytecode does *not* tell them apart: `chan.new(1)` compiles to
            // the same `GetGlobal chan` + `GetIndex "new"` whether or not the
            // file wrote `use chan;`. What differs is at run time — the import
            // replaces the global with the module object, and the VM's
            // `GetIndex` only succeeds against that. Without the import the
            // global still holds the constructor function and the VM answers
            // `index target object is not indexable: "Function"`.
            //
            // So the import is what licenses the module spelling, and it is
            // recorded: `sig.imports`. Reading it here is the difference
            // between the two ends agreeing and a program that runs natively
            // and fails under the VM.
            let module_ref = match ssa.builtin_regs.get(&(block, instr.b())).cloned() {
                Some(GlobalRef::Module(module)) => Some(module),
                Some(GlobalRef::Builtin(Builtin::ChanNew))
                    if sig.imports.module_aliases.get("chan").is_some_and(|m| m == "chan") =>
                {
                    Some("chan".to_string())
                }
                _ => None,
            };
            if let Some(module) = module_ref {
                let name = ssa.const_str_at(instr.c(), block, pc);
                let Some(name) = name else {
                    return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                };
                if let Some((value, ty)) = module_const(&module, &name) {
                    let dst = ssa.new_val();
                    insts.push(Inst::Const { dst, value });
                    ssa.write(instr.a(), block, (dst, ty));
                    return Ok(());
                }
                // `encoding.json`, `io.std`, `net.tcp`: reading a *submodule*
                // off its parent gives another module object, not a function of
                // the parent. Without this the chain stopped at the first dot,
                // so `encoding.json.parse(s)` dropped the program to the VM
                // while `use { json } from encoding;` lowered — the same rule
                // the import path already applies (`is_submodule`).
                let global_ref = if is_submodule(&module, &name) {
                    GlobalRef::Module(name)
                } else {
                    GlobalRef::ModuleFn(module, name)
                };
                ssa.bind_ref(block, instr.a(), global_ref);
                return Ok(());
            }
            // `a` = dst, `b` = container register, `c` = key register.
            let (handle, list_ty) = ssa.read(instr.b(), block, pc)?;
            // A range key (`s[1..3]`, `xs[1..5]`): the compiler lowers the
            // range to a materialized list; the recorded constant bounds
            // recover the slice. Clamping (negative/OOB) lives in lkrt,
            // matching the VM's `get_index_slice`.
            if let Ok((kv, _)) = ssa.read(instr.c(), block, pc)
                && let Some(&(r_start, r_end)) = ssa.range_def.get(&kv)
            {
                let start = ssa.new_val();
                insts.push(Inst::Const {
                    dst: start,
                    value: Const::I64(r_start),
                });
                let end = ssa.new_val();
                insts.push(Inst::Const {
                    dst: end,
                    value: Const::I64(r_end),
                });
                // Every carrier that has a slice symbol, not the two this
                // listed. `xs[1..3]` lowered for `List<Int>` and fell back for
                // `List<Float>`, `List<str>`, a mixed list, a `Bytes` and a
                // window — the same operation, decided by which carrier the
                // list happened to have.
                let (module, name, out_ty) = match list_ty {
                    Ty::Str => ("str", "slice_chars", Ty::Str),
                    Ty::ListI64 => ("list_h", "i64_slice", Ty::ListI64),
                    Ty::ListF64 => ("list_h", "f64_slice", Ty::ListF64),
                    Ty::ListStr => ("list_h", "str_slice", Ty::ListStr),
                    Ty::ListDyn => ("list_h", "dyn_slice", Ty::ListDyn),
                    Ty::Bytes => ("bytes_h", "slice", Ty::Bytes),
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new(module, name),
                    args: vec![handle, start, end],
                });
                ssa.write(instr.a(), block, (dst, out_ty));
                return Ok(());
            }
            // A boxed Dyn container (e.g. a nested list read out of a mixed
            // list): index through the runtime tag check — a non-list tag is
            // the VM's loud failure, OOB is nil (the Dyn's own Nil tag).
            if list_ty == Ty::Dyn {
                // Key type picks the accessor: an integer indexes a boxed
                // list, a string reads a boxed map's field (the compiler
                // emits GetIndex for nested member chains). Runtime tag
                // checks live in the dyn helpers.
                let (kv, kty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                let (helper, key) = match kty {
                    Ty::I64 => ("index", kv),
                    Ty::Str => ("field", kv),
                    // Both boxed: nothing static says whether this indexes a
                    // list or reads a map's field, so the tag decides at run
                    // time — which is what the VM does.
                    Ty::Dyn => ("get", kv),
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                let dst = ssa.new_val();
                let field_name = (helper == "field").then(|| ssa.const_str_value(key)).flatten();
                match field_name
                    .as_deref()
                    .and_then(|name| struct_field_position(ssa, sig, handle, name).map(|i| (name, i)))
                {
                    // A declared struct's field, by position — the boxed twin
                    // of the `MapStrDyn` read above.
                    Some((name, index)) => {
                        let index_v = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: index_v,
                            value: Const::I64(index as i64),
                        });
                        let len_v = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: len_v,
                            value: Const::I64(name.len() as i64),
                        });
                        insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("dyn", "field_at"),
                            args: vec![handle, index_v, key, len_v],
                        });
                    }
                    None => insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("dyn", helper),
                        args: vec![handle, key],
                    }),
                }
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            // `s[i]` — single-char read, char-indexed, OOB = nil (the VM's
            // `index_string_at`); the Dyn carrier holds the nil itself.
            // (`for ch in "abc"` desugars to exactly this indexed read.)
            if list_ty == Ty::Str {
                let key = read_map_key(ssa, insts, instr.c(), block, Ty::I64, pc)?;
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("str", "char_at"),
                    args: vec![handle, key],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            // Mixed-value map indexed by string key: same accessor as
            // `GetFieldK` (missing key = Nil-tag Dyn).
            if list_ty == Ty::MapStrDyn {
                let key = read_map_key(ssa, insts, instr.c(), block, Ty::Str, pc)?;
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("map_h", "str_dyn_get"),
                    args: vec![handle, key],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            // A key of a type the map cannot hold is a *miss*, and the answer
            // is nil for every value of that type — so it is a constant rather
            // than a call. The interpreter answers the same way, and the
            // checker stopped refusing the shape, so `{"k": 1}[0]` reaches here
            // now instead of being turned back at check time.
            // A constant string key is not in a register to be read, so it is
            // asked for by name — which is also how an int-keyed map sees the
            // only key type it can be handed wrongly.
            let key_ty = match ssa.const_str_at(instr.c(), block, pc) {
                Some(_) => Ty::Str,
                None => ssa.read(instr.c(), block, pc).map(|(_, t)| t).unwrap_or(Ty::Dyn),
            };
            //
            // A `Float` is not one of them, and it read like one for as long as
            // this fold has existed. `nil`, `true` and an Int are all *keys* —
            // a map simply does not have that one, so the read is a miss. A
            // Float is not a key at all, and the interpreter says so out loud
            // for a read exactly as it does for a store ("Float cannot be a map
            // key or set member"). Folding it to nil answered where the
            // interpreter raised, on both map key kinds.
            let map_ty = matches!(
                list_ty,
                Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64
            );
            if map_ty && key_ty == Ty::F64 {
                let msg = materialize_key(ssa, insts, globals, "Float cannot be a map key or set member");
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "raise_msg"),
                    args: vec![msg],
                });
                // A read has a destination and the store path does not, so the
                // raise alone would leave this register undefined for whatever
                // reads it next. `raise_msg` does not return, so the value is
                // never observed — it only has to exist.
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "from_nil"),
                    args: vec![],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            let map_key_mismatch = match list_ty {
                Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn => {
                    matches!(key_ty, Ty::I64 | Ty::Bool | Ty::Nil)
                }
                Ty::MapI64I64 | Ty::MapI64F64 => matches!(key_ty, Ty::Str | Ty::Bool | Ty::Nil),
                _ => false,
            };
            if map_key_mismatch {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "from_nil"),
                    args: vec![],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            // A key the lowering cannot type, against a map whose key type it
            // can: unboxing the key to the map's type raises for anything else,
            // and a *read* has an answer — nil, the way a key that is simply
            // absent does. So the map boxes and `dyn.get` dispatches on the
            // key's tag at run time, which is what the interpreter does.
            //
            // Reads only. `m[k] = v` builds a key and stays where it was.
            if key_ty == Ty::Dyn
                && matches!(
                    list_ty,
                    Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64
                )
            {
                let boxed_map = to_dyn(ssa, insts, handle, list_ty, pc)?;
                let (key_v, key_v_ty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                let boxed_key = to_dyn(ssa, insts, key_v, key_v_ty, pc)?;
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "get"),
                    args: vec![boxed_map, boxed_key],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            // String-keyed map reads take a `Str` key (dynamic template keys
            // included); a missing key is the `Maybe` nil model.
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool) {
                // A `Maybe` key (`freq[xs[i]]`) unwraps first (absent aborts —
                // the scalar-context rule).
                let key = read_map_key(ssa, insts, instr.c(), block, Ty::Str, pc)?;
                let dst = ssa.new_val();
                let maybe_ty = match list_ty {
                    Ty::MapStrF64 => {
                        insts.push(Inst::MapGetMaybeStrF64 { dst, handle, key });
                        Ty::MaybeF64
                    }
                    Ty::MapStrBool => {
                        insts.push(Inst::MapGetMaybe { dst, handle, key });
                        Ty::MaybeBool
                    }
                    _ => {
                        insts.push(Inst::MapGetMaybe { dst, handle, key });
                        Ty::MaybeI64
                    }
                };
                ssa.write(instr.a(), block, (dst, maybe_ty));
                return Ok(());
            }
            // Lists / int-keyed maps index with an `I64` (a `Maybe` index —
            // `xs[ys[j]]` — unwraps first, a boxed one goes through the tag
            // check).
            // An int-keyed map's index is a *key*, and a key of a type no map
            // can hold is refused by name rather than with the generic type
            // error a list's index gives.
            let index_val = if matches!(list_ty, Ty::MapI64I64 | Ty::MapI64F64) {
                read_map_key(ssa, insts, instr.c(), block, Ty::I64, pc)?
            } else {
                read_index_scalar(ssa, insts, instr.c(), block, pc)?
            };
            // Fast path: a **provably in-range** access (constant list of known
            // length indexed by a constant in `[0, len)`) is a clean scalar `at`.
            let const_in_range = match (ssa.list_len.get(&handle), ssa.const_int.get(&index_val)) {
                (Some(&len), Some(&idx)) if idx >= 0 && idx < len => Some(idx),
                _ => None,
            };
            if let Some(idx) = const_in_range {
                let (at_fn, elem_ty) = match list_ty {
                    // A `Bytes` element is a `Dyn` whether or not the index is
                    // provably in range: one helper, one rule.
                    Ty::Bytes => {
                        let idx_v = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: idx_v,
                            value: Const::I64(idx),
                        });
                        let dst = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("bytes_h", "get"),
                            args: vec![handle, idx_v],
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Dyn));
                        return Ok(());
                    }
                    Ty::ListI64 => ("i64_at", Ty::I64),
                    Ty::ListF64 => ("f64_at", Ty::F64),
                    Ty::ListStr => ("str_at", Ty::Str),
                    // Mixed list: the element is a boxed Dyn either way
                    // (`dyn_at` handles negative/OOB as a Nil-tag Dyn).
                    Ty::ListDyn => ("dyn_at", Ty::Dyn),
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                let idx_v = ssa.new_val();
                insts.push(Inst::Const {
                    dst: idx_v,
                    value: Const::I64(idx),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("list_h", at_fn),
                    args: vec![handle, idx_v],
                });
                if elem_ty == Ty::Dyn
                    && let Some(name) = ssa.list_elem_struct.get(&handle).cloned()
                {
                    ssa.set_struct(dst, name);
                }
                ssa.write(instr.a(), block, (dst, elem_ty));
            } else {
                // Dynamic / not-provably-in-range: the result is `Maybe<Int>` (VM:
                // out-of-range or negative → nil). Model it explicitly as
                // [`Ty::MaybeI64`]; its only supported consumer is a function return
                // (which prints the element or `nil`, matching the VM byte-for-byte).
                // A scalar consumer unwraps this via `read_scalar` (present-assert,
                // matching the VM's halt on `nil` arithmetic); a `return` keeps it
                // and prints `nil`. Either way there is no eager-abort shortcut that
                // would diverge from `return xs[oob]` printing `nil`.
                match list_ty {
                    // `b[i]` on a `Bytes`, which is also what `b.get(i)`
                    // compiles to. Like a dyn list, the Dyn's Nil tag is the
                    // absent case — negative counts from the end, out of range
                    // is nil.
                    Ty::Bytes => {
                        let dst = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("bytes_h", "get"),
                            args: vec![handle, index_val],
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Dyn));
                    }
                    // Mixed list: no Maybe carrier needed — the Dyn's Nil tag
                    // *is* the absent case (`dyn_at` maps OOB/negative-beyond
                    // to Nil, matching the VM's nil-on-out-of-range).
                    Ty::ListDyn => {
                        let dst = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("list_h", "dyn_at"),
                            args: vec![handle, index_val],
                        });
                        // An element of a list of one declared struct is that
                        // struct, so `nodes[i].next` reads a declared field.
                        if let Some(name) = ssa.list_elem_struct.get(&handle).cloned() {
                            ssa.set_struct(dst, name);
                        }
                        ssa.write(instr.a(), block, (dst, Ty::Dyn));
                    }
                    Ty::ListI64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::ListGetMaybe {
                            dst,
                            handle,
                            index: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeI64));
                    }
                    // `w[i]` on a window: resolved against the window (negative
                    // counts from *its* end), then read through to the source —
                    // the VM's `slice_element`.
                    Ty::SliceI64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::SliceGetMaybe {
                            dst,
                            handle,
                            index: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeI64));
                    }
                    Ty::ListF64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::ListGetMaybeF64 {
                            dst,
                            handle,
                            index: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeF64));
                    }
                    Ty::ListStr => {
                        let dst = ssa.new_val();
                        insts.push(Inst::ListGetMaybeStr {
                            dst,
                            handle,
                            index: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeStr));
                    }
                    // Int-keyed map lookup (`m[k]`): the key is the read index; a
                    // missing key is `nil`, i.e. the same `Maybe` model.
                    Ty::MapI64I64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::MapGetMaybeI64Key {
                            dst,
                            handle,
                            key: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeI64));
                    }
                    Ty::MapI64F64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::MapGetMaybeI64F64 {
                            dst,
                            handle,
                            key: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeF64));
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                }
            }
        }
        Opcode::SetIndex => {
            // `a` = container register, `b` = index/key register, `c` = value register.
            // For a **list**, the store is bounds-checked in the runtime helper (aborts
            // on an out-of-range/negative index — the VM's fatal store error, a halt).
            // For a **map**, the store always inserts-or-updates. An unsupported
            // container/key/value combination rejects (falls back).
            let (handle, list_ty) = ssa.read(instr.a(), block, pc)?;
            // A `Float` key is the VM's loud "cannot be used as a key" error,
            // and it is known *here*: no map carrier accepts one, so the store
            // can only raise. Emitting the raise keeps the rest of the program
            // native — refusing sent the whole thing back to the VM to produce
            // the same error.
            let map_ty = matches!(
                list_ty,
                Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64
            );
            if map_ty && ssa.read(instr.b(), block, pc).map(|(_, t)| t) == Ok(Ty::F64) {
                let msg = materialize_key(ssa, insts, globals, "Float cannot be a map key or set member");
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "raise_msg"),
                    args: vec![msg],
                });
                return Ok(());
            }
            // A boxed receiver stores through `dyn.index_set`, which reaches
            // the carrier behind the tag. Both the key and the value travel
            // boxed: which key shape a carrier accepts is the callee's rule
            // (an integer key on a map is a key, not a position), and this
            // side has no carrier to check it against.
            if list_ty == Ty::Dyn {
                let (kv, kty) = ssa.read(instr.b(), block, pc)?;
                // `nil` and a Bool are keys the interpreter *stores* — `m[nil]
                // = 1` gives `{nil:1}` — and no native map representation holds
                // one: the boxed carrier is keyed by `String` and the typed ones
                // by `String` or `i64`. So the store has no native form, and
                // emitting one raised "runtime type error" on a program the
                // interpreter answers. Falling back is the whole program on the
                // VM, which is slower and right.
                //
                // Only these two, and only where the key's type says so. A `Str`
                // or `I64` key stores natively as before, and a key this side
                // cannot type still goes through — a fallback for every erased
                // key would cost far more coverage than the shape is worth.
                // Reading such a key is a different question and is answered:
                // `lkrt_dyn_get` looks it up and misses.
                if matches!(kty, Ty::Nil | Ty::Bool) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                let key = to_dyn(ssa, insts, kv, kty, pc)?;
                let (cv, cty) = ssa.read(instr.c(), block, pc)?;
                let boxed = to_dyn(ssa, insts, cv, cty, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("dyn", "index_set"),
                    args: vec![handle, key, boxed],
                });
                return Ok(());
            }
            // A key this side cannot type, stored into a map carrier that holds
            // *one* key kind. The runtime unbox (`dyn.as_key_str` /
            // `dyn.as_key_i64`) refuses every other kind — and the interpreter
            // does not: an LK map takes nil, a Bool, an Int and a String alike,
            // and which carrier holds it is a native representation choice no
            // program asked for. So `fn put(m, k) { m[k] = 1; }` called once
            // with a string and once with an integer stored the first and
            // raised "runtime type error" on the second, where the interpreter
            // answered `{"a":1,7:1}`. An explicit `{"a": 1}` literal reaches it
            // too, so this is not about the empty-literal guess.
            //
            // Except a **closure**, which is provably not a key at all: there
            // the runtime refusal is the interpreter's own sentence, word for
            // word, and lowering it keeps the rest of the module native.
            // `examples/syntax/closure_value.lk` writes that on purpose, inside
            // a `try` — which is why the fact has to cross the region boundary
            // (`SigInfer::try_body_closure_inputs`) rather than be re-derived.
            //
            // Measured: the generative fuzzer's fully-native count is unchanged
            // at 300 cases, and no example loses its lowering.
            if matches!(
                list_ty,
                Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool | Ty::MapStrDyn | Ty::MapI64I64 | Ty::MapI64F64
            ) && let Ok((kv, Ty::Dyn)) = ssa.read(instr.b(), block, pc)
                && !ssa.closure_values.contains(&kv)
            {
                return Err(Unsupported::TypeMismatch { pc });
            }
            // String-keyed map stores take a `Str` key (dynamic template keys
            // included); the map ABI copies the key.
            // A boxed map takes any value: box it and store. Without this arm
            // the Dyn carrier existed but nothing could be put into it, so the
            // retry below would have had nowhere to land.
            if list_ty == Ty::MapStrDyn {
                let key = read_map_key(ssa, insts, instr.b(), block, Ty::Str, pc)?;
                // Raw, not `read_scalar`: a boxed map holds nil, so a nullable
                // value stores as nil rather than asserting it is present. Same
                // divergence the `ListDyn` push had — `m[k] = xs[i]` past the
                // end of `xs` stored nil on the interpreter and raised compiled.
                let (cv, cty) = ssa.read(instr.c(), block, pc)?;
                let boxed = to_dyn(ssa, insts, cv, cty, pc)?;
                // As in `SetFieldK`: a store into a declared field is measured
                // against the declaration. The key here may be computed, so
                // the constant-code form only applies when it is not.
                match ssa.const_str_at(instr.b(), block, pc) {
                    Some(field) => emit_field_store_check(ssa, insts, globals, sig, handle, &field, cty, boxed),
                    // A computed key cannot be filtered by name, so this is
                    // the one store shape that asks at run time — and only in
                    // a module that declares a typed field at all.
                    None if sig.traits.struct_field_codes.values().any(|&code| code != DECLARED_ANY) => {
                        let key_dyn = to_dyn(ssa, insts, key, Ty::Str, pc)?;
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("obj_ty", "check_marked_dyn"),
                            args: vec![handle, key_dyn, boxed],
                        });
                    }
                    None => {}
                }
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", "str_dyn_set"),
                    args: vec![handle, key, boxed],
                });
                return Ok(());
            }
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64) {
                if let Some(e) = nullable_into_typed_carrier(ssa, func, instr.c(), block, instr.a(), handle, list_ty) {
                    return Err(e);
                }
                let key = read_map_key(ssa, insts, instr.b(), block, Ty::Str, pc)?;
                let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                let (set_fn, value) = match (list_ty, cty) {
                    (Ty::MapStrI64, Ty::I64) => ("str_i64_set", cv),
                    (Ty::MapStrF64, Ty::F64) => ("str_f64_set", cv),
                    (Ty::MapStrF64, Ty::I64) => ("str_f64_set", coerce_to_f64(ssa, insts, cv, cty)),
                    // The value contradicts what this map was built to hold —
                    // the same situation a push contradicting a list literal is,
                    // and the same answer: name the literal and let the fixpoint
                    // rebuild it with a Dyn carrier.
                    _ => {
                        return Err(
                            carrier_contradicted_here_or_at_callers(ssa, func, instr.a(), handle, list_ty)
                                .unwrap_or(Unsupported::TypeMismatch { pc }),
                        );
                    }
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", set_fn),
                    args: vec![handle, key, value],
                });
                return Ok(());
            }
            // A boxed index unboxes through the tag check, as it does on the
            // read side: a store from a loop over a list has exactly the same
            // shape as a load.
            let index = if matches!(list_ty, Ty::MapI64I64 | Ty::MapI64F64) {
                read_map_key(ssa, insts, instr.b(), block, Ty::I64, pc)?
            } else {
                read_index_scalar(ssa, insts, instr.b(), block, pc)?
            };
            match list_ty {
                Ty::ListI64 => {
                    if let Some(e) =
                        nullable_into_typed_carrier(ssa, func, instr.c(), block, instr.a(), handle, list_ty)
                    {
                        return Err(e);
                    }
                    let value = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "i64_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::ListF64 => {
                    let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                    if !matches!(cty, Ty::I64 | Ty::F64) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let value = coerce_to_f64(ssa, insts, cv, cty);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "f64_set"),
                        args: vec![handle, index, value],
                    });
                }
                // The other two carriers, so that `xs[i] = v` does not depend
                // on the list's internal representation: `Int` and `Float` had
                // arms here, `Str` and the boxed carrier did not, and the same
                // two lines therefore stayed native or did not for a reason no
                // program can observe.
                Ty::ListStr => {
                    if let Some(e) =
                        nullable_into_typed_carrier(ssa, func, instr.c(), block, instr.a(), handle, list_ty)
                    {
                        return Err(e);
                    }
                    let value = read_typed_scalar(ssa, insts, instr.c(), block, Ty::Str, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "str_set"),
                        args: vec![handle, index, value],
                    });
                }
                // Raw, not `read_scalar`: a Dyn list holds nil, and the VM
                // puts nil in it (`xs[0] = ys[oob]`).
                Ty::ListDyn => {
                    let (cv, cty) = ssa.read(instr.c(), block, pc)?;
                    let value = crate::dyn_box::to_dyn(ssa, insts, cv, cty, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "dyn_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::MapI64I64 => {
                    if let Some(e) =
                        nullable_into_typed_carrier(ssa, func, instr.c(), block, instr.a(), handle, list_ty)
                    {
                        return Err(e);
                    }
                    let value = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("map_h", "i64_i64_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::MapI64F64 => {
                    if let Some(e) =
                        nullable_into_typed_carrier(ssa, func, instr.c(), block, instr.a(), handle, list_ty)
                    {
                        return Err(e);
                    }
                    let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                    if !matches!(cty, Ty::I64 | Ty::F64) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let value = coerce_to_f64(ssa, insts, cv, cty);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("map_h", "i64_f64_set"),
                        args: vec![handle, index, value],
                    });
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
        }
        Opcode::GetFieldK => {
            // `a` = dst, `b` = map register, `c` = key string-constant index. A
            // missing key is `nil` → the `Maybe` model (i64- or f64-valued map).
            //
            // Except when the "map" is a **module object**: `m.get(k)` with one
            // argument compiles to a map read whatever `m` is, so `env.get(k)`
            // arrives here rather than as a call, with `env` where the map
            // belongs and `k` as the key. The VM dispatches that at run time;
            // this side read the module as a value and reported it as a
            // compile-time reference, so `env.get(k)` fell back while
            // `env.get_or(k, d)` — an ordinary call — lowered.
            let (handle, map_ty) = ssa.read(instr.b(), block, pc)?;
            let key = func
                .consts
                .strings
                .get(instr.c() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            // A constant *string* key against an int-keyed map is a miss, and
            // nil for every such key — the same fold `GetIndex` takes for the
            // other direction. Only reachable since the checker stopped
            // refusing the shape.
            if matches!(map_ty, Ty::MapI64I64 | Ty::MapI64F64) {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "from_nil"),
                    args: vec![],
                });
                ssa.write(instr.a(), block, (dst, Ty::Dyn));
                return Ok(());
            }
            let key_v = materialize_key(ssa, insts, globals, key);
            let dst = ssa.new_val();
            let result_ty = match map_ty {
                Ty::MapStrBool => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle,
                        key: key_v,
                    });
                    Ty::MaybeBool
                }
                Ty::MapStrI64 => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle,
                        key: key_v,
                    });
                    Ty::MaybeI64
                }
                Ty::MapStrF64 => {
                    insts.push(Inst::MapGetMaybeStrF64 {
                        dst,
                        handle,
                        key: key_v,
                    });
                    Ty::MaybeF64
                }
                // Mixed-value map: the Dyn carrier's Nil tag *is* the
                // missing-key case — no Maybe wrapper needed.
                Ty::MapStrDyn => {
                    // A declared struct's field sits at a known position, so
                    // this is an index rather than a hash of the key — see
                    // `lkrt_lkmap_str_dyn_get_at` for why the key travels
                    // along anyway.
                    match struct_field_position(ssa, sig, handle, key) {
                        Some(index) => {
                            let index_v = ssa.new_val();
                            insts.push(Inst::Const {
                                dst: index_v,
                                value: Const::I64(index as i64),
                            });
                            let len_v = ssa.new_val();
                            insts.push(Inst::Const {
                                dst: len_v,
                                value: Const::I64(key.len() as i64),
                            });
                            insts.push(Inst::Call {
                                dst: Some(dst),
                                callee: AbiRef::new("map_h", "str_dyn_get_at"),
                                args: vec![handle, index_v, key_v, len_v],
                            });
                        }
                        None => insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("map_h", "str_dyn_get"),
                            args: vec![handle, key_v],
                        }),
                    }
                    Ty::Dyn
                }
                // A boxed Dyn (e.g. a nested map read out of a MapStrDyn):
                // the runtime tag check lives in `dyn.field` (non-map = the
                // VM's loud failure on member access).
                Ty::Dyn => {
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("dyn", "field"),
                        args: vec![handle, key_v],
                    });
                    Ty::Dyn
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            ssa.write(instr.a(), block, (dst, result_ty));
        }
        Opcode::SetFieldK => {
            // `a` = map register, `b` = value register, `c` = key string-constant
            // index. A store always inserts-or-updates (never an error).
            let (handle, map_ty) = ssa.read(instr.a(), block, pc)?;
            let key = func
                .consts
                .strings
                .get(instr.c() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let key_v = materialize_key(ssa, insts, globals, key);
            let (set_fn, value) = match map_ty {
                // A value the carrier cannot hold contradicts the literal this
                // map was built from — the same situation a push contradicting
                // a list literal is, and the same answer: name the literal so
                // the fixpoint rebuilds it with a Dyn carrier.
                Ty::MapStrI64
                    if nullable_into_typed_carrier(ssa, func, instr.b(), block, instr.a(), handle, map_ty)
                        .is_some() =>
                {
                    return Err(
                        nullable_into_typed_carrier(ssa, func, instr.b(), block, instr.a(), handle, map_ty)
                            .expect("just checked"),
                    );
                }
                Ty::MapStrI64 => match read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc) {
                    Ok(v) => ("str_i64_set", v),
                    Err(e) => {
                        return Err(keep_discovery(
                            e,
                            carrier_contradicted_here_or_at_callers(ssa, func, instr.a(), handle, map_ty),
                        ));
                    }
                },
                Ty::MapStrF64 => {
                    if let Some(e) = nullable_into_typed_carrier(ssa, func, instr.b(), block, instr.a(), handle, map_ty)
                    {
                        return Err(e);
                    }
                    let (bv, bty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
                    if !matches!(bty, Ty::I64 | Ty::F64) {
                        return Err(
                            carrier_contradicted_here_or_at_callers(ssa, func, instr.a(), handle, map_ty)
                                .unwrap_or(Unsupported::TypeMismatch { pc }),
                        );
                    }
                    ("str_f64_set", coerce_to_f64(ssa, insts, bv, bty))
                }
                // A bool map rides the `str_i64` carrier, and its value crosses
                // as that carrier's word — so a `Bool` is widened here, the way
                // it is everywhere a `Bool` meets an `I64` ABI parameter. The
                // carrier had no arm at all, which is why `m[k] = true` on a
                // `Map<String, Bool>` dropped the module to the VM: reading and
                // deleting lowered, writing did not.
                Ty::MapStrBool => {
                    let (bv, bty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
                    let word = match bty {
                        Ty::I64 => bv,
                        Ty::Bool => {
                            let wide = ssa.new_val();
                            insts.push(Inst::ZextBool { dst: wide, src: bv });
                            wide
                        }
                        _ => {
                            return Err(
                                carrier_contradicted_here_or_at_callers(ssa, func, instr.a(), handle, map_ty)
                                    .unwrap_or(Unsupported::TypeMismatch { pc }),
                            );
                        }
                    };
                    ("str_i64_set", word)
                }
                // Struct-instance field stores (`p.x += 9` on a `NewObject`
                // map): any boxable value stores boxed, insert-or-update.
                Ty::MapStrDyn => {
                    let (bv, bty) = ssa.read(instr.b(), block, pc)?;
                    ("str_dyn_set", to_dyn(ssa, insts, bv, bty, pc)?)
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            // A store into a declared field is measured against the
            // declaration. The struct type is usually known here, which makes
            // the code a constant and the check a tag compare; when it is not,
            // the mark answers at run time.
            if map_ty == Ty::MapStrDyn {
                let value_ty = ssa.peek(instr.b(), block).map(|(_, ty)| ty).unwrap_or(Ty::Dyn);
                emit_field_store_check(ssa, insts, globals, sig, handle, key, value_ty, value);
            }
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("map_h", set_fn),
                args: vec![handle, key_v, value],
            });
        }
        Opcode::Contains => {
            // `a` = dst (bool), `b` = needle, `c` = haystack. List and string-keyed
            // map haystacks are lowered; other haystacks fall back.
            let (handle, list_ty) = ssa.read(instr.c(), block, pc)?;
            // Dyn containers: list membership boxes the needle and defers to
            // the structural `dyn_contains`; map membership is a dedicated
            // `has` (a stored-nil value still counts, unlike get+tag).
            // A **boxed** haystack: what membership means is the tag's answer,
            // not the static type's, so the runtime picks. A map tests its
            // keys, every other container its elements.
            if list_ty == Ty::Dyn {
                let (nv, nty) = ssa.read(instr.b(), block, pc)?;
                let needle = to_dyn(ssa, insts, nv, nty, pc)?;
                let raw = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(raw),
                    callee: AbiRef::new("dyn", "contains"),
                    args: vec![handle, needle],
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
            // `needle in text` is `text.contains(needle)` — the same
            // operation, and the method spelling already lowered while the
            // operator sent the whole program back to the VM.
            if list_ty == Ty::Str {
                let needle = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
                let raw = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(raw),
                    callee: AbiRef::new("str", "contains"),
                    args: vec![handle, needle],
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
            if list_ty == Ty::ListDyn || list_ty == Ty::MapStrDyn {
                let raw = ssa.new_val();
                if list_ty == Ty::ListDyn {
                    let (nv, nty) = ssa.read(instr.b(), block, pc)?;
                    let needle = to_dyn(ssa, insts, nv, nty, pc)?;
                    insts.push(Inst::Call {
                        dst: Some(raw),
                        callee: AbiRef::new("list_h", "dyn_contains"),
                        args: vec![handle, needle],
                    });
                } else {
                    let key = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
                    insts.push(Inst::Call {
                        dst: Some(raw),
                        callee: AbiRef::new("map_h", "str_dyn_has"),
                        args: vec![handle, key],
                    });
                }
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
            // `Bytes` and a window: both already have a `contains` symbol —
            // the operator was the one place they were not containers, in the
            // checker, in the VM and here. A needle the carrier cannot hold is
            // the VM's `false`, not an error, which the `I64` needle read
            // gives for free only when the needle *is* an `Int`; anything else
            // keeps rejecting rather than guessing.
            if matches!(list_ty, Ty::Bytes | Ty::SliceI64) {
                let needle = ssa.read_typed(instr.b(), block, Ty::I64, pc)?;
                let raw = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(raw),
                    callee: match list_ty {
                        Ty::Bytes => AbiRef::new("bytes_h", "contains"),
                        _ => AbiRef::new("slice_h", "i64_contains"),
                    },
                    args: vec![handle, needle],
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
            // `key in map` tests key membership (VM `map_contains`): read the
            // map's `Maybe` for the key and take its present bit — no value
            // materialization needed. Mirrors the map `GetIndex` path.
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool) {
                let key = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
                let maybe = ssa.new_val();
                let maybe_ty = match list_ty {
                    Ty::MapStrF64 => {
                        insts.push(Inst::MapGetMaybeStrF64 {
                            dst: maybe,
                            handle,
                            key,
                        });
                        Ty::MaybeF64
                    }
                    Ty::MapStrBool => {
                        insts.push(Inst::MapGetMaybe {
                            dst: maybe,
                            handle,
                            key,
                        });
                        Ty::MaybeBool
                    }
                    _ => {
                        insts.push(Inst::MapGetMaybe {
                            dst: maybe,
                            handle,
                            key,
                        });
                        Ty::MaybeI64
                    }
                };
                let dst = ssa.new_val();
                insts.push(Inst::MaybePresent {
                    dst,
                    src: maybe,
                    maybe_ty,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            // Int-keyed maps: same present-bit test with an `I64` key.
            if matches!(list_ty, Ty::MapI64I64 | Ty::MapI64F64) {
                let key = read_map_key(ssa, insts, instr.b(), block, Ty::I64, pc)?;
                let maybe = ssa.new_val();
                let maybe_ty = if list_ty == Ty::MapI64F64 {
                    insts.push(Inst::MapGetMaybeI64F64 {
                        dst: maybe,
                        handle,
                        key,
                    });
                    Ty::MaybeF64
                } else {
                    insts.push(Inst::MapGetMaybeI64Key {
                        dst: maybe,
                        handle,
                        key,
                    });
                    Ty::MaybeI64
                };
                let dst = ssa.new_val();
                insts.push(Inst::MaybePresent {
                    dst,
                    src: maybe,
                    maybe_ty,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            // Typed-list `in` compares numerically across `Int`/`Float`, the
            // same rule `==` uses. It used to demand the *same* type here and
            // in the VM, so `a == b` was true and `a in [b]` false for the
            // same pair — and the answer depended on the list's internal
            // representation, which no program can see. A needle whose proven
            // type cannot match any element (a string against a number list)
            // still folds to constant false; a Dyn needle still rejects.
            let (fn_name, needle) = match list_ty {
                Ty::ListI64 | Ty::ListF64 | Ty::ListStr => {
                    let (nv, nty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
                    match (list_ty, nty) {
                        (Ty::ListI64, Ty::I64) => ("i64_contains", nv),
                        (Ty::ListF64, Ty::F64) => ("f64_contains", nv),
                        (Ty::ListStr, Ty::Str) => ("str_contains", nv),
                        (Ty::ListI64, Ty::F64) => ("i64_contains_f64", nv),
                        (Ty::ListF64, Ty::I64) => ("f64_contains_i64", nv),
                        (_, Ty::Dyn) => return Err(Unsupported::TypeMismatch { pc }),
                        _ => {
                            let dst = ssa.new_val();
                            insts.push(Inst::Const {
                                dst,
                                value: Const::Bool(false),
                            });
                            ssa.write(instr.a(), block, (dst, Ty::Bool));
                            return Ok(());
                        }
                    }
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let raw = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(raw),
                callee: AbiRef::new("list_h", fn_name),
                args: vec![handle, needle],
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
        }
        Opcode::MapRest => {
            // `a` = dst, `b` = base (source map), `c` = key_count. The result is
            // the map with the `key_count` string keys in registers
            // base+1..=base+key_count removed — one `without` call chained per
            // key (matching the VM's `map_rest`). Only string-keyed maps lower.
            let base = instr.b();
            let key_count = instr.c();
            let (map_handle, map_ty) = ssa.read(base, block, pc)?;
            let without_fn = match map_ty {
                Ty::MapStrI64 | Ty::MapStrBool => "str_i64_without",
                Ty::MapStrF64 => "str_f64_without",
                Ty::MapStrDyn => "str_dyn_without",
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let mut current = map_handle;
            for offset in 0..key_count {
                let key_reg = base
                    .checked_add(1)
                    .and_then(|r| r.checked_add(offset))
                    .ok_or(Unsupported::TypeMismatch { pc })?;
                let key = ssa.read_typed(key_reg, block, Ty::Str, pc)?;
                let next = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(next),
                    callee: AbiRef::new("map_h", without_fn),
                    args: vec![current, key],
                });
                current = next;
            }
            // `{ k: v, ..rest }` builds `rest` fresh, so it is an ordinary map
            // whatever the source was — which is also why a map pattern's
            // refusal to match a struct is the only thing keeping a struct out.
            if map_ty == Ty::MapStrDyn {
                ssa.set_plain_map(current);
            }
            ssa.write(instr.a(), block, (current, map_ty));
        }
        op => return Err(Unsupported::Opcode { pc, op }),
    }
    Ok(())
}

/// The container literal whose carrier a store into `handle` contradicts.
///
/// `None` when this function built no literal whose carrier is a judgement —
/// then the store is simply unsupported and the caller says so.
///
/// The handle itself when its provenance is known; otherwise a handle read
/// through an unsealed loop phi has none yet, so the pending literals *of the
/// receiver's own shape* are marked (only those can be the contradicted one; a
/// correctly typed `ListStr` elsewhere in the function must keep its typed
/// lowering — `join` and friends have no Dyn arm). If shape-filtering leaves
/// nothing, over-mark all: that costs typed-ness, never correctness.
/// The same demand, for a store whose receiver may be a bare *parameter*.
///
/// The literal to rebuild is in this function when the receiver traces to one.
/// A parameter has none: the container belongs to the caller, and the caller's
/// other aliases read the same allocation by its static type, so the carrier
/// has to be decided at the caller's literal (`SigInfer::dyn_params`).
/// Which rejection to report when a container write's value could not be read
/// *and* the container's carrier looks contradicted.
///
/// The carrier answer stands in for a **type** failure only. An
/// `UndefinedOperand` is not one: it is a *discovery*, and the fixpoint keys a
/// `try` region's write-back cells on that exact variant
/// (`try_body_extra_cells`). Substituting the carrier rejection for it is how a
/// region's write reached nobody —
///
/// ```lk
/// try { try { b = clo(); } catch c1 { } } catch c2 { }
/// acc.push(b);
/// ```
///
/// printed `b`'s value from *before* the region, natively, with no fallback and
/// no warning, while `let t = b; acc.push(t);` — the same program with a `Move`
/// in the way — was correct. A `ReferenceAsValue` is the same kind of thing: it
/// names a register the caller can be asked about, not a type that is wrong.
/// A nullable value on its way *into* a typed container, which cannot hold one.
///
/// The store paths read their value through `read_scalar`/`read_typed_scalar`,
/// which narrows a carrier by asserting it is present. That is right where a
/// number is required and wrong here: the VM's list and map hold nil, so
/// `out.push(xs[i])` past the end of `xs` appends nil there — while the
/// compiled program asserted, found the value absent, and raised. Nothing
/// static caught it, because a `Maybe<Int>` narrows to `Int` and `Int` is
/// exactly what the carrier wants.
///
/// So the *carrier* is what is wrong: a container that receives a nullable
/// value has to be a Dyn one. Reported as a contradiction of the literal it was
/// built from, which is the fixpoint's existing way of rebuilding it — the same
/// answer a push of a genuinely unboxable type already gets.
fn nullable_into_typed_carrier(
    ssa: &Ssa,
    func: &FunctionData,
    value_reg: u8,
    block: usize,
    receiver_reg: u8,
    handle: ValueId,
    carrier: Ty,
) -> Option<Unsupported> {
    let ty = ssa.peek(value_reg, block).map(|(_, ty)| ty)?;
    // A *boxed* value is the same situation as a nullable one and was not
    // treated as it: unboxing it into the carrier is a guess that raises at run
    // time, where widening the carrier answers. An empty `{}` guesses
    // `str -> i64`, so
    //
    //     fn s(v: Any) -> Int { let m = {}; m["k"] = v; return m.len(); }
    //
    // stored an `Int` and raised "runtime type error" for every other kind,
    // while the interpreter stored all of them. The guess is meant to cost a
    // widening — that is what this function is for — and the unbox spent it on
    // a raise instead.
    if !matches!(ty, Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool | Ty::Dyn) {
        return None;
    }
    if ty == Ty::Dyn && carrier == Ty::Dyn {
        return None;
    }
    carrier_contradicted_here_or_at_callers(ssa, func, receiver_reg, handle, carrier).or(Some(
        Unsupported::OperandType {
            pc: 0,
            want: "a Dyn container, which is the only kind that holds this",
            got: lk_aot_mir::ty_name(ty),
        },
    ))
}

fn keep_discovery(original: Unsupported, carrier: Option<Unsupported>) -> Unsupported {
    match original {
        Unsupported::UndefinedOperand { .. } | Unsupported::ReferenceAsValue { .. } => original,
        _ => carrier.unwrap_or(original),
    }
}

pub(crate) fn carrier_contradicted_here_or_at_callers(
    ssa: &Ssa,
    func: &FunctionData,
    receiver_reg: u8,
    handle: ValueId,
    carrier: Ty,
) -> Option<Unsupported> {
    carrier_contradicted(ssa, handle, carrier).or_else(|| {
        (u16::from(receiver_reg) < func.param_count)
            .then_some(Unsupported::ParamCarrierContradicted { param: receiver_reg })
    })
}

pub(crate) fn carrier_contradicted(ssa: &Ssa, handle: ValueId, carrier: Ty) -> Option<Unsupported> {
    if ssa.literal_carrier.is_empty() {
        return None;
    }
    let pcs = match ssa.literal_carrier.get(&handle) {
        Some(&(pc0, _)) => vec![pc0],
        None => {
            let same_shape: Vec<usize> = ssa
                .literal_carrier
                .values()
                .filter(|&&(_, gty)| gty == carrier)
                .map(|&(p0, _)| p0)
                .collect();
            if same_shape.is_empty() {
                ssa.literal_carrier.values().map(|&(p0, _)| p0).collect()
            } else {
                same_shape
            }
        }
    };
    Some(Unsupported::LiteralElemTypeContradicted { pcs })
}

/// Where a declared struct keeps this field, when the receiver is one.
fn struct_field_position(ssa: &Ssa, sig: &SigInfer, handle: ValueId, field: &str) -> Option<usize> {
    let name = ssa.struct_name(handle)?;
    sig.traits
        .struct_field_index
        .get(&(name.to_string(), field.to_string()))
        .copied()
}

/// Emits the declared-field check for a store the value's own type does not
/// already settle.
///
/// The declared code is a compile-time constant, so the runtime side is a tag
/// compare (`lkrt_check_declared_field`) with no table lookup. A statically
/// satisfying store emits nothing at all — which is every field of an ordinary
/// `P { x: 1, y: 2 }`.
#[allow(clippy::too_many_arguments)]
fn emit_declared_field_check(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    sig: &SigInfer,
    type_name: &str,
    field: &str,
    value_ty: Ty,
    boxed: ValueId,
) {
    let Some(&declared) = sig
        .traits
        .struct_field_codes
        .get(&(type_name.to_string(), field.to_string()))
    else {
        return;
    };
    if declared == DECLARED_ANY || statically_satisfies(declared, value_ty) {
        return;
    }
    let type_v = materialize_key(ssa, insts, globals, type_name);
    let field_v = materialize_key(ssa, insts, globals, field);
    let declared_v = ssa.new_val();
    insts.push(Inst::Const {
        dst: declared_v,
        value: Const::I64(declared),
    });
    insts.push(Inst::Call {
        dst: None,
        callee: AbiRef::new("obj_ty", "check"),
        args: vec![type_v, field_v, declared_v, boxed],
    });
}

/// Whether a value of this MIR type always satisfies the declared code.
fn statically_satisfies(declared: i64, ty: Ty) -> bool {
    use crate::trait_env::{DECLARED_BOOL, DECLARED_FLOAT, DECLARED_INT, DECLARED_NULLABLE, DECLARED_STR};
    match declared & !DECLARED_NULLABLE {
        DECLARED_INT => ty == Ty::I64,
        DECLARED_FLOAT => matches!(ty, Ty::I64 | Ty::F64),
        DECLARED_BOOL => ty == Ty::Bool,
        DECLARED_STR => ty == Ty::Str,
        _ => true,
    }
}

/// The declared-field check for a *store* into a map that may be a struct
/// instance.
///
/// Statically decided when the receiver's struct type is known — the common
/// case, and then a satisfying value emits nothing at all. Otherwise the mark
/// decides at run time, which is one table lookup on a path that had none of
/// this before and no guarantee either.
#[allow(clippy::too_many_arguments)]
fn emit_field_store_check(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    sig: &SigInfer,
    handle: ValueId,
    field: &str,
    value_ty: Ty,
    boxed: ValueId,
) {
    if let Some(type_name) = ssa.struct_name(handle).map(str::to_string) {
        emit_declared_field_check(ssa, insts, globals, sig, &type_name, field, value_ty, boxed);
        return;
    }
    // A handle this function watched a *map literal* produce is not a struct
    // instance — those come from `NewObject`, which records a struct type
    // above. So there is nothing to check, and an ordinary `m[k] = v` loop
    // pays nothing.
    if ssa.literal_carrier.contains_key(&handle) {
        return;
    }
    // Nor is there anything to check when *no declared struct has a field of
    // this name with a type*: whatever this map is, this key cannot name a
    // field a store could violate. A program with no structs emits none of
    // this, and `m["count"] = v` only pays where some struct really declares
    // `count`.
    let constrained = sig
        .traits
        .struct_field_codes
        .iter()
        .any(|((_, name), code)| name == field && *code != DECLARED_ANY);
    if !constrained {
        return;
    }
    let field_v = materialize_key(ssa, insts, globals, field);
    insts.push(Inst::Call {
        dst: None,
        callee: AbiRef::new("obj_ty", "check_marked"),
        args: vec![handle, field_v, boxed],
    });
}

/// A constant scalar as the carrier stores it, unboxed. `None` for a constant
/// no typed carrier holds.
fn unboxed_const_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    value: &ConstRuntimeValueData,
) -> Option<ValueId> {
    let dst = ssa.new_val();
    match value {
        ConstRuntimeValueData::Int(v) => insts.push(Inst::Const {
            dst,
            value: Const::I64(*v),
        }),
        // A `bool` carrier stores its members as `i64`, which is why the
        // `MapStrBool` shape shares the `str_i64` ABI.
        ConstRuntimeValueData::Bool(v) => insts.push(Inst::Const {
            dst,
            value: Const::I64(i64::from(*v)),
        }),
        ConstRuntimeValueData::Float(v) => insts.push(Inst::Const {
            dst,
            value: Const::F64(*v),
        }),
        ConstRuntimeValueData::ShortStr(v) => return Some(materialize_key(ssa, insts, globals, v)),
        ConstRuntimeValueData::Heap(heap) => match &**heap {
            lk_core::vm::ConstHeapValueData::LongString(v) => {
                return Some(materialize_key(ssa, insts, globals, v));
            }
            _ => return None,
        },
        _ => return None,
    }
    Some(dst)
}
