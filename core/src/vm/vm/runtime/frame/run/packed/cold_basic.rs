use super::super::raw_boundary::region_allocator;
use super::*;

#[inline(always)]
fn text_index_value(text: &str, index: i64) -> Val {
    let len = if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    };
    let Some(index) = (if index < 0 {
        len.checked_sub(index.unsigned_abs() as usize)
    } else {
        Some(index as usize)
    }) else {
        return Val::Nil;
    };
    if text.is_ascii() {
        text.as_bytes()
            .get(index)
            .copied()
            .map_or(Val::Nil, Val::ascii_char_value)
    } else {
        text.chars()
            .nth(index)
            .map(|character| Val::from_str(&character.to_string()))
            .unwrap_or(Val::Nil)
    }
}

pub(super) fn handles_basic_op(op: &Op) -> bool {
    matches!(
        op,
        Op::Index { .. }
            | Op::Jmp(_)
            | Op::JmpFalse(_, _)
            | Op::BoolBranch(_, _)
            | Op::JmpIfNil(_, _)
            | Op::JmpIfNotNil(_, _)
            | Op::CmpLtImmJmp { .. }
            | Op::CmpLeImmJmp { .. }
            | Op::CmpGtImmJmp { .. }
            | Op::CmpGeImmJmp { .. }
            | Op::AddIntImmJmp { .. }
            | Op::ToBool(_, _)
            | Op::ToIter { .. }
            | Op::Not(_, _)
            | Op::NullishPick { .. }
            | Op::Ret { .. }
            | Op::Break(_)
            | Op::Continue(_)
            | Op::LoadGlobal(_, _)
            | Op::DefineGlobal(_, _)
            | Op::Access(_, _, _)
            | Op::AccessK(_, _, _)
            | Op::IndexK(_, _, _)
            | Op::ListIndex(_, _, _)
            | Op::ListIndexI(_, _, _)
            | Op::ListSetI { .. }
            | Op::StrIndex(_, _, _)
            | Op::StrIndexI(_, _, _)
            | Op::MapGetInterned(_, _, _)
            | Op::MapGetDynamic(_, _, _)
            | Op::MapSetInterned(_, _, _)
            | Op::MapSetInternedMove(_, _, _)
            | Op::MapHasK(_, _, _)
            | Op::ForRangePrep { .. }
            | Op::ForRangeLoop { .. }
            | Op::RangeLoopI { .. }
            | Op::ForRangeStep { .. }
            | Op::PatternMatch { .. }
            | Op::PatternMatchOrFail { .. }
            | Op::Raise { .. }
            | Op::BuildList { .. }
            | Op::BuildMap { .. }
            | Op::MakeClosure { .. }
            | Op::LoadLocal(_, _)
            | Op::StoreLocal(_, _)
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn exec_basic_op(
    op: Op,
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    f: &Function,
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    access_ic: &mut [Option<AccessIc>],
    index_ic: &mut [Option<IndexIc>],
    global_ic: &mut [Option<GlobalEntry>],
    for_range_ic: &mut [Option<ForRangeState>],
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let mut pc = *pc_ref;
    match op {
        Op::Index { dst, base, idx } => {
            let res = match (&regs[base as usize], &regs[idx as usize]) {
                (Val::List(l), Val::Int(i)) => {
                    if *i < 0 {
                        l.len()
                            .checked_sub(i.unsigned_abs() as usize)
                            .and_then(|idx| {
                                l.get(idx)
                                    .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics))
                            })
                            .unwrap_or(Val::Nil)
                    } else {
                        let lptr = Arc::as_ptr(l) as *const Val as usize;
                        let hit = match index_ic[pc].as_mut() {
                            Some(IndexIc::List(slots)) => Vm::lookup_promote(slots, |e| {
                                e.base_ptr == lptr && e.idx == *i
                            })
                            .map(|entry| copy_container_value_for_register_with_metrics(&entry.value, collect_metrics)),
                            _ => None,
                        };
                        if let Some(v) = hit {
                            v
                        } else {
                            let v = l
                                .get(*i as usize)
                                .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics))
                                .unwrap_or(Val::Nil);
                            Vm::update_list_ic(index_ic, pc, lptr, *i, &v, collect_metrics);
                            v
                        }
                    }
                }
                (base_val, Val::Int(i)) if base_val.as_str().is_some() => {
                    let s_str = base_val.as_str().unwrap();
                    if *i < 0 {
                        text_index_value(s_str, *i)
                    } else {
                        let sptr = s_str.as_ptr() as usize;
                        let hit = match index_ic[pc].as_mut() {
                            Some(IndexIc::Str(slots)) => Vm::lookup_promote(slots, |e| {
                                e.base_ptr == sptr && e.idx == *i
                            })
                            .map(|entry| copy_container_value_for_register_with_metrics(&entry.value, collect_metrics)),
                            _ => None,
                        };
                        if let Some(v) = hit {
                            v
                        } else {
                            let v = if s_str.is_ascii() {
                                let bi = *i as usize;
                                let bs = s_str.as_bytes();
                                if bi < bs.len() {
                                    Val::ascii_char_value(bs[bi])
                                } else {
                                    Val::Nil
                                }
                            } else {
                                s_str
                                    .chars()
                                    .nth(*i as usize)
                                    .map(|c| Val::from_str(&c.to_string()))
                                    .unwrap_or(Val::Nil)
                            };
                            Vm::update_str_ic(index_ic, pc, sptr, *i, &v, collect_metrics);
                            v
                        }
                    }
                }
                (base_val, key) => base_val.access_with_metrics(key, collect_metrics).unwrap_or(Val::Nil),
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::Jmp(ofs) => {
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::JmpFalse(r, ofs) | Op::BoolBranch(r, ofs) => {
            let cond_falsey = matches!(regs[r as usize], Val::Nil | Val::Bool(false));
            if cond_falsey {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::JmpIfNil(r, ofs) => {
            if matches!(regs[r as usize], Val::Nil) {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::JmpIfNotNil(r, ofs) => {
            if !matches!(regs[r as usize], Val::Nil) {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::CmpLtImmJmp { r, imm, ofs } => {
            // Fused CmpLtImm + JmpFalse: if r < imm, fall through; else jump.
            let skip = match &regs[r as usize] {
                Val::Int(x) => *x >= (imm as i64),
                _ => true,
            };
            if skip {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::CmpLeImmJmp { r, imm, ofs } => {
            // Fused CmpLeImm + JmpFalse: if r <= imm, fall through; else jump.
            let skip = match &regs[r as usize] {
                Val::Int(x) => *x > (imm as i64),
                _ => true,
            };
            if skip {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::CmpGtImmJmp { r, imm, ofs } => {
            // Fused CmpGtImm + JmpFalse: if r > imm, fall through; else jump.
            let skip = match &regs[r as usize] {
                Val::Int(x) => *x <= (imm as i64),
                _ => true,
            };
            if skip {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::CmpGeImmJmp { r, imm, ofs } => {
            // Fused CmpGeImm + JmpFalse: if r >= imm, fall through; else jump.
            let skip = match &regs[r as usize] {
                Val::Int(x) => *x < (imm as i64),
                _ => true,
            };
            if skip {
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::AddIntImmJmp { r, imm, ofs } => {
            // Fused: r += imm, then jump by ofs.
            if let Val::Int(x) = regs[r as usize] {
                let result = x.wrapping_add(imm as i64);
                assign_reg_with_metrics(regs, r as usize, Val::Int(result), collect_metrics);
            }
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::ToBool(dst, src) => {
            let truthy = !matches!(regs[src as usize], Val::Nil | Val::Bool(false));
            assign_reg_with_metrics(regs, dst as usize, Val::Bool(truthy), collect_metrics);
            pc = next_pc_default;
        }
        Op::ToIter { dst, src } => {
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            let out = match &regs[src as usize] {
                v if matches!(v, Val::List(_)) || v.as_str().is_some() => {
                    copy_value_for_register_with_metrics(v, collect_metrics)
                }
                Val::Map(m) => {
                    let mut entries: Vec<_> = m.iter().collect();
                    entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
                    if use_thread_local && !entries.is_empty() {
                        let allocator = region_allocator(region_allocator_ptr);
                        allocator.with_val_buffer(entries.len(), |scratch| {
                            for (key, value) in entries.iter() {
                                let pair = Val::List(
                                    vec![
                                        Val::from_str(key.as_str()),
                                        copy_value_for_register_with_metrics(value, collect_metrics),
                                    ]
                                    .into(),
                                );
                                scratch.push(pair);
                            }
                            let data = scratch.split_off(0);
                            Val::List(data.into())
                        })
                    } else {
                        let mut pairs = Vec::with_capacity(entries.len());
                        for (key, value) in entries {
                            let pair = Val::List(
                                vec![
                                    Val::from_str(key.as_str()),
                                    copy_value_for_register_with_metrics(value, collect_metrics),
                                ]
                                .into(),
                            );
                            pairs.push(pair);
                        }
                        Val::List(pairs.into())
                    }
                }
                _ => Val::List(Vec::<Val>::new().into()),
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::Not(dst, src) => {
            match &regs[src as usize] {
                Val::Bool(b) => assign_reg_with_metrics(regs, dst as usize, Val::Bool(!b), collect_metrics),
                Val::Nil => assign_reg_with_metrics(regs, dst as usize, Val::Bool(true), collect_metrics),
                other => {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("Invalid operand: !{:?}", other))).map(Some);
                }
            }
            pc = next_pc_default;
        }
        Op::NullishPick { l, dst, ofs } => {
            if !matches!(regs[l as usize], Val::Nil) {
                assign_reg_from_reg_with_metrics(regs, dst as usize, l as usize, collect_metrics);
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::Ret { base, retc } => {
            let retc = retc as usize;
            let base_idx = base as usize;
            let ret_val = if retc > 0 {
                take_register_value(regs, base_idx)
            } else {
                Val::Nil
            };
            return handle_return_common(frame_raw, regs, pc, base_idx, retc, ret_val, self_ptr, collect_metrics)
                .map(Some);
        }
        Op::Break(ofs) => {
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::Continue(ofs) => {
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::LoadGlobal(dst, name_k) => {
            let name_val = &f.consts[name_k as usize];
            let out = load_global_for_register(ctx, global_ic, pc, name_val, collect_metrics);
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::DefineGlobal(name_k, src) => {
            let name_val = &f.consts[name_k as usize];
            if let Some(s) = name_val.as_str() {
                ctx.set(
                    s.to_string(),
                    copy_value_for_register_with_metrics(&regs[src as usize], collect_metrics),
                );
            }
            pc = next_pc_default;
        }
        Op::Access(dst, base, field) => {
            let hit_val = match (&regs[base as usize], &regs[field as usize]) {
                (Val::List(l), Val::Int(i)) => {
                    if *i < 0 {
                        Some(Val::Nil)
                    } else {
                        Some(
                            l.get(*i as usize)
                                .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                                .unwrap_or(Val::Nil),
                        )
                    }
                }
                (base_val, Val::Int(i)) if base_val.as_str().is_some() => {
                    let s_str = base_val.as_str().unwrap();
                    if *i < 0 {
                        Some(Val::Nil)
                    } else if s_str.is_ascii() {
                        let idx = *i as usize;
                        let bs = s_str.as_bytes();
                        if idx < bs.len() {
                            Some(Val::ascii_char_value(bs[idx]))
                        } else {
                            Some(Val::Nil)
                        }
                    } else {
                        Some(
                            s_str
                                .chars()
                                .nth(*i as usize)
                                .map(|ch| Val::from_str(&ch.to_string()))
                                .unwrap_or(Val::Nil),
                        )
                    }
                }
                (Val::Map(m), Val::Str(s)) => Val::map_get_str(m, s.as_str())
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics)),
                (Val::Map(m), Val::ShortStr(s)) => Val::map_get_str(m, s.as_str())
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics)),
                (Val::Object(object), Val::Str(s)) => {
                    let fields = &object.fields;
                    let optr = Arc::as_ptr(fields) as usize;
                    let kstr = s.as_str();
                    match access_ic[pc].as_mut() {
                        Some(AccessIc::ObjectStr(slots)) => {
                            Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == kstr)
                                .map(|entry| copy_value_for_register_with_metrics(&entry.value, collect_metrics))
                        }
                        _ => None,
                    }
                }
                (Val::Object(object), Val::ShortStr(s)) => {
                    let fields = &object.fields;
                    let optr = Arc::as_ptr(fields) as usize;
                    let kstr = s.as_str();
                    match access_ic[pc].as_mut() {
                        Some(AccessIc::ObjectStr(slots)) => {
                            Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == kstr)
                                .map(|entry| copy_value_for_register_with_metrics(&entry.value, collect_metrics))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            let res = if let Some(v) = hit_val {
                v
            } else {
                let v = regs[base as usize]
                    .access_with_metrics(&regs[field as usize], collect_metrics)
                    .unwrap_or(Val::Nil);
                match (&regs[base as usize], &regs[field as usize]) {
                    (Val::Object(object), field_val) if field_val.as_str().is_some() => {
                        let s = field_val.as_str().unwrap();
                        let fields = &object.fields;
                        let optr = Arc::as_ptr(fields) as usize;
                        Vm::update_object_ic(access_ic, pc, optr, s, &v, collect_metrics);
                    }
                    _ => {}
                }
                v
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::AccessK(dst, base, kidx) => {
            let key = &f.consts[kidx as usize];
            let res = if let Some(s) = key.as_str() {
                let (hit_val, obj_ptr) = match &regs[base as usize] {
                    Val::Map(m) => {
                        let out = Val::map_get_str(m, s)
                            .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                            .unwrap_or(Val::Nil);
                        (Some(out), None)
                    }
                    Val::Object(object) => {
                        let fields = &object.fields;
                        let optr = Arc::as_ptr(fields) as usize;
                        let out = match access_ic[pc].as_mut() {
                            Some(AccessIc::ObjectStr(slots)) => {
                                Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == s)
                                    .map(|entry| copy_value_for_register_with_metrics(&entry.value, collect_metrics))
                            }
                            _ => None,
                        };
                        (out, Some(optr))
                    }
                    _ => (None, None),
                };
                if let Some(v) = hit_val {
                    v
                } else {
                    let v = regs[base as usize]
                        .access_with_metrics(key, collect_metrics)
                        .unwrap_or(Val::Nil);
                    if let Some(optr) = obj_ptr {
                        Vm::update_object_ic(access_ic, pc, optr, s, &v, collect_metrics);
                    }
                    v
                }
            } else {
                Val::Nil
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::IndexK(dst, base, kidx) => {
            let key = &f.consts[kidx as usize];
            let res = if let Val::Int(i) = key {
                match &regs[base as usize] {
                    Val::List(_) | Val::Str(_) | Val::ShortStr(_) => regs[base as usize]
                        .access_with_metrics(&Val::Int(*i), collect_metrics)
                        .unwrap_or(Val::Nil),
                    _ => Val::Nil,
                }
            } else {
                Val::Nil
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::ListIndexI(dst, base, index) => {
            let res = match &regs[base as usize] {
                Val::List(_) => regs[base as usize]
                    .access_with_metrics(&Val::Int(index as i64), collect_metrics)
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::ListIndex(dst, base, index) => {
            let res = match (&regs[base as usize], &regs[index as usize]) {
                (Val::List(_), Val::Int(index)) if *index >= 0 => regs[base as usize]
                    .access_with_metrics(&Val::Int(*index), collect_metrics)
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::StrIndex(dst, base, index) => {
            let res = match (&regs[base as usize], &regs[index as usize]) {
                (value, Val::Int(index)) if *index >= 0 && value.as_str().is_some() => value
                    .access_with_metrics(&Val::Int(*index), collect_metrics)
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::StrIndexI(dst, base, index) => {
            let res = match &regs[base as usize] {
                value if value.as_str().is_some() => value
                    .access_with_metrics(&Val::Int(index as i64), collect_metrics)
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, dst as usize, res, collect_metrics);
            pc = next_pc_default;
        }
        Op::MapHasK(dst, map, kidx) => {
            let key = f.consts[kidx as usize].as_str().unwrap_or("");
            let out = match &regs[map as usize] {
                Val::Map(map) => Val::Bool(Val::map_contains_str(map, key)),
                _ => {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("has() first argument must be a map")))
                        .map(Some);
                }
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::MapGetInterned(dst, map, kidx) => {
            let key = f.consts[kidx as usize].as_str().unwrap_or("");
            let out = match &regs[map as usize] {
                Val::Map(map) => Val::map_get_str(map, key)
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::MapGetDynamic(dst, map, key) => {
            let out = match (&regs[map as usize], regs[key as usize].as_str()) {
                (Val::Map(map), Some(key)) => Val::map_get_str(map, key)
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::ListSetI { dst, list, index, val } => {
            let out = if index < 0 {
                return frame_return_common(frame_raw, pc, Err(anyhow!("set() index must be non-negative"))).map(Some);
            } else if let Val::List(items) = &regs[list as usize] {
                let index = index as usize;
                let Some(old) = items
                    .get(index)
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                else {
                    return frame_return_common(
                        frame_raw,
                        pc,
                        Err(anyhow!("index {} out of bounds for len {}", index, items.len())),
                    )
                    .map(Some);
                };
                let mut updated = Vec::with_capacity(items.len());
                updated.extend(
                    items
                        .iter()
                        .map(|value| copy_value_for_register_with_metrics(value, collect_metrics)),
                );
                updated[index] = copy_value_for_register_with_metrics(&regs[val as usize], collect_metrics);
                Val::List(vec![Val::List(Arc::new(updated)), old].into())
            } else {
                return frame_return_common(frame_raw, pc, Err(anyhow!("set() first argument must be a list")))
                    .map(Some);
            };
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            pc = next_pc_default;
        }
        Op::MapSetInterned(map, kidx, val) => {
            let key = f.consts[kidx as usize]
                .string_key_arcstr()
                .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
            let pushed_val = copy_value_for_register_with_metrics(&regs[val as usize], collect_metrics);
            match &mut regs[map as usize] {
                Val::Map(arc) => insert_map_entry(arc, key, pushed_val),
                _ => {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet target is not a Map"))).map(Some);
                }
            }
            pc = next_pc_default;
        }
        Op::MapSetInternedMove(map, kidx, val) => {
            let map_idx = map as usize;
            let val_idx = val as usize;
            if map_idx == val_idx {
                let key = f.consts[kidx as usize]
                    .string_key_arcstr()
                    .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
                let pushed_val = copy_value_for_register_with_metrics(&regs[val_idx], collect_metrics);
                match &mut regs[map_idx] {
                    Val::Map(arc) => insert_map_entry(arc, key, pushed_val),
                    _ => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet target is not a Map")))
                            .map(Some);
                    }
                }
            } else {
                let key = f.consts[kidx as usize]
                    .string_key_arcstr()
                    .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
                if !matches!(regs[map_idx], Val::Map(_)) {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("MapSet target is not a Map"))).map(Some);
                }
                let pushed_val = take_register_value(regs, val_idx);
                match &mut regs[map_idx] {
                    Val::Map(arc) => insert_map_entry(arc, key, pushed_val),
                    _ => unreachable!("MapSet target was checked before moving value"),
                }
            }
            pc = next_pc_default;
        }
        Op::ForRangePrep {
            idx,
            limit,
            step,
            inclusive,
            explicit,
        } => {
            let idx_reg = idx as usize;
            let limit_reg = limit as usize;
            let step_reg = step as usize;
            let (i0, ilim) = match (&regs[idx_reg], &regs[limit_reg]) {
                (Val::Int(a0), Val::Int(b0)) => (*a0, *b0),
                _ => {
                    return frame_return_common(
                        frame_raw,
                        pc,
                        Err(anyhow!(
                            "For-range requires integer bounds, got idx={:?}, limit={:?}",
                            regs[idx_reg],
                            regs[limit_reg]
                        )),
                    )
                    .map(Some);
                }
            };
            let step_val = if !explicit {
                let step_val = if i0 <= ilim { 1 } else { -1 };
                assign_reg_with_metrics(regs, step_reg, Val::Int(step_val), collect_metrics);
                step_val
            } else {
                match &regs[step_reg] {
                    Val::Int(0) => {
                        return frame_return_common(frame_raw, pc, Err(anyhow!("For-range step cannot be zero")))
                            .map(Some);
                    }
                    Val::Int(v) => *v,
                    other => {
                        return frame_return_common(
                            frame_raw,
                            pc,
                            Err(anyhow!("For-range step must be Int when explicit, got {:?}", other)),
                        )
                        .map(Some);
                    }
                }
            };
            if step_val == 0 {
                return frame_return_common(frame_raw, pc, Err(anyhow!("For-range step cannot be zero"))).map(Some);
            }
            if let Some(slot) = for_range_ic.get_mut(next_pc_default) {
                *slot = Some(ForRangeState::new(i0, ilim, step_val, inclusive));
            }
            pc = next_pc_default;
        }
        Op::ForRangeLoop {
            idx, write_idx, ofs, ..
        }
        | Op::RangeLoopI {
            idx, write_idx, ofs, ..
        } => {
            let idx_reg = idx as usize;
            let state_entry = match fetch_for_range_state(for_range_ic, pc) {
                Ok(state) => state,
                Err(err) => {
                    return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                }
            };
            if state_entry.should_continue() {
                if write_idx {
                    assign_reg_with_metrics(regs, idx_reg, Val::Int(state_entry.current), collect_metrics);
                }
                state_entry.current += state_entry.step;
                pc = next_pc_default;
            } else {
                for_range_ic[pc] = None;
                pc = ((pc as isize) + (ofs as isize)) as usize;
            }
        }
        // ForRangeLoop already pre-increments; ForRangeStep only jumps back.
        Op::ForRangeStep { back_ofs, .. } => {
            let guard_pc = ((pc as isize) + (back_ofs as isize)) as usize;
            pc = guard_pc;
        }
        Op::PatternMatch { dst, src, plan } => {
            let plan = &f.pattern_plans[plan as usize];
            let value = &regs[src as usize];
            match plan.pattern.matches(value, Some(&*ctx))? {
                Some(bound) => {
                    assign_pattern_bindings_with_metrics(regs, &plan.bindings, &bound, collect_metrics);
                    assign_reg_with_metrics(regs, dst as usize, Val::Bool(true), collect_metrics);
                }
                None => {
                    clear_pattern_bindings_with_metrics(regs, &plan.bindings, collect_metrics);
                    assign_reg_with_metrics(regs, dst as usize, Val::Bool(false), collect_metrics);
                }
            }
            pc = next_pc_default;
        }
        Op::PatternMatchOrFail {
            src,
            plan,
            err_kidx,
            is_const,
        } => {
            let plan = &f.pattern_plans[plan as usize];
            let value = &regs[src as usize];
            match plan.pattern.matches(value, Some(&*ctx))? {
                Some(bound) => {
                    let mut assigned = Vec::with_capacity(plan.bindings.len());
                    assign_pattern_bindings_for_context_with_metrics(
                        regs,
                        &plan.bindings,
                        &bound,
                        &mut assigned,
                        collect_metrics,
                    );
                    for (name, val) in assigned {
                        if is_const {
                            ctx.define_const(name, val);
                        } else {
                            ctx.set(name, val);
                        }
                    }
                    pc = next_pc_default;
                }
                None => {
                    let msg_val = &f.consts[err_kidx as usize];
                    let msg = match msg_val {
                        val if val.as_str().is_some() => val.as_str().unwrap().to_string(),
                        other => other.to_string(),
                    };
                    return frame_return_common(frame_raw, pc, Err(anyhow!(msg))).map(Some);
                }
            }
        }
        Op::Raise { err_kidx } => {
            let msg_val = &f.consts[err_kidx as usize];
            let msg = match msg_val {
                val if val.as_str().is_some() => val.as_str().unwrap().to_string(),
                other => other.to_string(),
            };
            return frame_return_common(frame_raw, pc, Err(anyhow!(msg))).map(Some);
        }
        Op::BuildList { dst, base, len } => {
            let start = base as usize;
            let n = len as usize;
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            if use_thread_local {
                let allocator = region_allocator(region_allocator_ptr);
                let list_val = allocator.with_val_buffer(n, |scratch| {
                    scratch.extend(
                        (0..n).map(|i| copy_value_for_register_with_metrics(&regs[start + i], collect_metrics)),
                    );
                    let data = scratch.split_off(0);
                    Val::List(data.into())
                });
                assign_reg_with_metrics(regs, dst as usize, list_val, collect_metrics);
            } else {
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    v.push(copy_value_for_register_with_metrics(&regs[start + i], collect_metrics));
                }
                assign_reg_with_metrics(regs, dst as usize, Val::List(v.into()), collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::BuildMap { dst, base, len } => {
            let start = base as usize;
            let n = len as usize;
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            if use_thread_local {
                let allocator = region_allocator(region_allocator_ptr);
                let map_val = allocator.with_map_entries(n, |entries| {
                    for i in 0..n {
                        let key_arc;
                        let value;
                        {
                            let key_val = &regs[start + 2 * i];
                            value = copy_value_for_register_with_metrics(&regs[start + 2 * i + 1], collect_metrics);
                            key_arc = key_val
                                .primitive_key_arcstr()
                                .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", key_val))?;
                        }
                        entries.push((key_arc, value));
                    }
                    let mut map = fast_hash_map_with_capacity(entries.len());
                    for (k, v) in entries.drain(..) {
                        Val::map_insert_arcstr(&mut map, k, v);
                    }
                    Ok(Val::Map(Arc::new(map)))
                });
                match map_val {
                    Ok(val) => assign_reg_with_metrics(regs, dst as usize, val, collect_metrics),
                    Err(err) => {
                        return frame_return_common(frame_raw, pc, Err(err)).map(Some);
                    }
                }
            } else {
                let mut map: FastHashMap<ArcStr, Val> = fast_hash_map_with_capacity(n);
                for i in 0..n {
                    let key_arc;
                    let value;
                    {
                        let key_val = &regs[start + 2 * i];
                        value = copy_value_for_register_with_metrics(&regs[start + 2 * i + 1], collect_metrics);
                        key_arc = match key_val.primitive_key_arcstr() {
                            Some(key_arc) => key_arc,
                            None => {
                                return frame_return_common(
                                    frame_raw,
                                    pc,
                                    Err(anyhow!("Map key must be a primitive type, got: {:?}", key_val)),
                                )
                                .map(Some);
                            }
                        };
                    }
                    Val::map_insert_arcstr(&mut map, key_arc, value);
                }
                assign_reg_with_metrics(regs, dst as usize, Val::Map(Arc::new(map)), collect_metrics);
            }
            pc = next_pc_default;
        }
        Op::MakeClosure { dst, proto } => {
            let clo = make_closure_value(f, proto, ctx, regs, frame_base, collect_metrics)?;
            assign_reg_with_metrics(regs, dst as usize, clo, collect_metrics);
            pc = next_pc_default;
        }
        Op::LoadLocal(dst, idx) => {
            let instr_pc = packed_instr_pc(f, pc);
            let may_take = local_load_may_take_source(f, instr_pc);
            assign_reg_from_local_load_or_take_with_metrics(
                regs,
                dst as usize,
                idx as usize,
                may_take,
                collect_metrics,
            );
            pc = next_pc_default;
        }
        Op::StoreLocal(idx, src) => {
            let may_take = local_store_may_take_source(f, pc);
            assign_local_from_reg_or_take_with_metrics(regs, idx as usize, src as usize, may_take, collect_metrics);
            pc = next_pc_default;
        }
        _ => unreachable!("basic packed op predicate drifted"),
    }
    *pc_ref = pc;
    Ok(None)
}
