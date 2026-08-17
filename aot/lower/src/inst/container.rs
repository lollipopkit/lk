//! Container opcodes: list/map/object construction, indexing, mutation.

use super::LowerCtx;
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
                let elem_struct = elems.first().and_then(|&(v, _)| ssa.struct_types.get(&v).cloned());
                if let Some(name) = elem_struct
                    && elems.iter().all(|&(v, _)| ssa.struct_types.get(&v) == Some(&name))
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
                let key = ssa.read(key_reg, block, pc)?;
                // Through `read_value`: a lambda written as a map's value
                // becomes a closure here, the same way it does in a list
                // literal. A *key* cannot be one — a callable is not a map key
                // in this language — so that read stays as it was.
                let val = read_value(ssa, insts, sig, funcs, cap_ctx, val_reg, block, pc)?;
                entries.push((key, val));
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
            let all_keys = |t: Ty| entries.iter().all(|&((_, kt), _)| kt == t);
            let all_vals = |t: Ty| entries.iter().all(|&(_, (_, vt))| vt == t);
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
            let lit = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(lit),
                callee: AbiRef::new("map_h", "lit_new"),
                args: Vec::new(),
            });
            for &((k, kt), (v, vt)) in &entries {
                let boxed_key = to_dyn(ssa, insts, k, kt, pc)?;
                let boxed_value = to_dyn(ssa, insts, v, vt, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", "lit_set"),
                    args: vec![lit, boxed_key, boxed_value],
                });
            }
            let handle = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(handle),
                callee: AbiRef::new("map_h", finish_fn),
                args: vec![lit],
            });
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
                    let lit = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(lit),
                        callee: AbiRef::new("map_h", "lit_new"),
                        args: Vec::new(),
                    });
                    for (k, v) in entries {
                        let boxed_key = match k {
                            RuntimeMapKeyData::ShortStr(key) | RuntimeMapKeyData::String(key) => {
                                let raw = materialize_key(ssa, insts, globals, key);
                                let boxed = ssa.new_val();
                                insts.push(Inst::Call {
                                    dst: Some(boxed),
                                    callee: AbiRef::new("dyn", "from_str"),
                                    args: vec![raw],
                                });
                                boxed
                            }
                            RuntimeMapKeyData::Int(ik) => {
                                let raw = ssa.new_val();
                                insts.push(Inst::Const {
                                    dst: raw,
                                    value: Const::I64(*ik),
                                });
                                let boxed = ssa.new_val();
                                insts.push(Inst::Call {
                                    dst: Some(boxed),
                                    callee: AbiRef::new("dyn", "from_i64"),
                                    args: vec![raw],
                                });
                                boxed
                            }
                            _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
                        };
                        let boxed_value = box_const_scalar(ssa, insts, globals, v);
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("map_h", "lit_set"),
                            args: vec![lit, boxed_key, boxed_value],
                        });
                    }
                    let handle = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(handle),
                        callee: AbiRef::new("map_h", finish_fn),
                        args: vec![lit],
                    });
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
            let (module, len_fn) = match ty {
                // Strings count Unicode scalar values (the VM's char length).
                Ty::Str => ("str", "char_len"),
                Ty::ListI64 => ("list_h", "i64_len"),
                // A window's length is its own, not the source's.
                Ty::SliceI64 => ("slice_h", "i64_len"),
                Ty::ListF64 => ("list_h", "f64_len"),
                Ty::ListStr => ("list_h", "str_len"),
                Ty::MapStrI64 => ("map_h", "str_i64_len"),
                // The bool map rides the `str_i64` carrier, so it is the same
                // call — it was simply missing from this table, which cost a
                // program its lowering for `{"a": true}.len()` alone.
                Ty::MapStrBool => ("map_h", "str_i64_len"),
                Ty::MapI64I64 => ("map_h", "i64_i64_len"),
                Ty::MapStrF64 => ("map_h", "str_f64_len"),
                Ty::MapI64F64 => ("map_h", "i64_f64_len"),
                Ty::ListDyn => ("list_h", "dyn_len"),
                Ty::MapStrDyn => ("map_h", "str_dyn_len"),
                Ty::Set => ("set", "len"),
                Ty::Bytes => ("bytes_h", "len"),
                // A boxed Dyn: length dispatches on the runtime tag.
                Ty::Dyn => ("dyn", "len_of"),
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new(module, len_fn),
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
            let map = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(map),
                callee: AbiRef::new("map_h", "str_dyn_new"),
                args: Vec::new(),
            });
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
                let boxed = to_dyn_any(ssa, insts, vv, vty, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", "str_dyn_set"),
                    args: vec![map, key_v, boxed],
                });
            }
            // Struct provenance (plan J1): the type name drives static
            // method devirtualization; a type with registered trait impls
            // also marks the handle for boxed runtime dispatch.
            let type_name = {
                let tv = ssa.read(instr.b(), block, pc).ok().map(|(v, _)| v);
                tv.and_then(|v| ssa.const_strs.get(&v).cloned())
                    .or_else(|| ssa.reg_const_str(instr.b(), block))
            };
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
                ssa.struct_types.insert(map, type_name);
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
                let boxed = to_dyn_any(ssa, insts, value, value_ty, pc)?;
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
                let boxed = to_dyn_any(ssa, insts, value, value_ty, pc)?;
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
                // Mixed list: any boxable value pushes as a Dyn carrier.
                Ty::ListDyn => {
                    let (bv, bty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
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
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", helper),
                    args: vec![handle, key],
                });
                // A member chain (`nodes[i].next`) reaches the field this way
                // rather than through `GetFieldK`, so the declared-type
                // narrowing has to be here too.
                let (dst, result_ty) = match (helper, ssa.const_str_value(key)) {
                    ("field", Some(name)) => unbox_declared_field(ssa, insts, sig, handle, &name, dst, Ty::Dyn, pc)?,
                    _ => (dst, Ty::Dyn),
                };
                ssa.write(instr.a(), block, (dst, result_ty));
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
                    ssa.struct_types.insert(dst, name);
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
                            ssa.struct_types.insert(dst, name);
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
                let key = to_dyn_any(ssa, insts, kv, kty, pc)?;
                let (cv, cty) = ssa.read(instr.c(), block, pc)?;
                let boxed = to_dyn_any(ssa, insts, cv, cty, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("dyn", "index_set"),
                    args: vec![handle, key, boxed],
                });
                return Ok(());
            }
            // String-keyed map stores take a `Str` key (dynamic template keys
            // included); the map ABI copies the key.
            // A boxed map takes any value: box it and store. Without this arm
            // the Dyn carrier existed but nothing could be put into it, so the
            // retry below would have had nowhere to land.
            if list_ty == Ty::MapStrDyn {
                let key = read_map_key(ssa, insts, instr.b(), block, Ty::Str, pc)?;
                let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                let boxed = to_dyn_any(ssa, insts, cv, cty, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", "str_dyn_set"),
                    args: vec![handle, key, boxed],
                });
                return Ok(());
            }
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64) {
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
                    let value = read_typed_scalar(ssa, insts, instr.c(), block, Ty::Str, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "str_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::ListDyn => {
                    let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                    let value = crate::dyn_box::to_dyn_any(ssa, insts, cv, cty, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "dyn_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::MapI64I64 => {
                    let value = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("map_h", "i64_i64_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::MapI64F64 => {
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
            let (handle, map_ty) = ssa.read(instr.b(), block, pc)?;
            let key = func
                .consts
                .strings
                .get(instr.c() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
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
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("map_h", "str_dyn_get"),
                        args: vec![handle, key_v],
                    });
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
            // A field of a *declared* struct has a declared type, and reading
            // it through the string-keyed map gives a boxed `Dyn` that knows
            // nothing about it. Unboxing here is what keeps `p.count + 1` an
            // integer add instead of `dyn.add`, and what lets a loop variable
            // fed from a field (`cur = nodes[cur].next`) stay an `I64` —
            // without it the loop phi joined `I64` with `Dyn` and the whole
            // walk fell back.
            let (dst, result_ty) = unbox_declared_field(ssa, insts, sig, handle, key, dst, result_ty, pc)?;
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
                    let (bv, bty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
                    if !matches!(bty, Ty::I64 | Ty::F64) {
                        return Err(
                            carrier_contradicted_here_or_at_callers(ssa, func, instr.a(), handle, map_ty)
                                .unwrap_or(Unsupported::TypeMismatch { pc }),
                        );
                    }
                    ("str_f64_set", coerce_to_f64(ssa, insts, bv, bty))
                }
                // Struct-instance field stores (`p.x += 9` on a `NewObject`
                // map): any boxable value stores boxed, insert-or-update.
                Ty::MapStrDyn => {
                    let (bv, bty) = ssa.read(instr.b(), block, pc)?;
                    ("str_dyn_set", to_dyn_any(ssa, insts, bv, bty, pc)?)
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
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
                let needle = to_dyn_any(ssa, insts, nv, nty, pc)?;
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

/// A declared struct field's read, narrowed to its declared scalar type.
///
/// Returns the input unchanged whenever the answer is not known: the receiver
/// is not a value this lowering tracked to a declared struct, the field has no
/// annotation, or its type is not one held unboxed. Nothing here guesses — an
/// undeclared field stays the boxed `Dyn` it has always been.
#[allow(clippy::too_many_arguments)]
fn unbox_declared_field(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    sig: &SigInfer,
    handle: ValueId,
    field: &str,
    dst: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<(ValueId, Ty), Unsupported> {
    if ty != Ty::Dyn {
        return Ok((dst, ty));
    }
    let Some(struct_name) = ssa.struct_types.get(&handle).cloned() else {
        return Ok((dst, ty));
    };
    let Some(&want) = sig.traits.struct_field_tys.get(&(struct_name, field.to_string())) else {
        return Ok((dst, ty));
    };
    let unbox = match want {
        Ty::I64 => "as_i64",
        Ty::F64 => "as_f64",
        Ty::Bool => "as_bool",
        Ty::Str => "as_str",
        _ => return Ok((dst, ty)),
    };
    let narrowed = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(narrowed),
        callee: AbiRef::new("dyn", unbox),
        args: vec![dst],
    });
    let _ = pc;
    Ok((narrowed, want))
}
