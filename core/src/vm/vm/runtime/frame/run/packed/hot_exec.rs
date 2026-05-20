use super::*;
use crate::vm::{VmCallMetric, VmContainerMetric, record_branch_op, record_call_op, record_container_op};

#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(super) fn exec_hot_slot(
    entry: &PackedHotSlot,
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    ctx: &mut VmContext,
    frame_captures: &Option<Arc<ClosureCapture>>,
    frame_capture_specs: &Option<Arc<Vec<CaptureSpec>>>,
    access_ic: &mut [Option<AccessIc>],
    index_ic: &mut [Option<IndexIc>],
    global_ic: &mut [Option<GlobalEntry>],
    call_ic: &mut [Option<CallIc>],
    for_range_ic: &mut [Option<ForRangeState>],
    pc: usize,
    frame_base: usize,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
    collect_metrics: bool,
) -> Result<Option<usize>> {
    let record_branch = |typed| {
        if collect_metrics {
            record_branch_op(typed);
        }
    };
    let record_call = |kind| {
        if collect_metrics {
            record_call_op(kind);
        }
    };
    let record_container = |kind| {
        if collect_metrics {
            record_container_op(kind);
        }
    };

    let result = match &entry.kind {
        PackedHotKind::Move { dst, src } => {
            assign_reg(frame_raw, regs, *dst as usize, regs[*src as usize].clone());
            None
        }
        PackedHotKind::LoadK { dst, kidx } => {
            assign_reg(frame_raw, regs, *dst as usize, func.consts[*kidx as usize].clone());
            None
        }
        PackedHotKind::LoadLocal { dst, idx } => {
            assign_reg(frame_raw, regs, *dst as usize, regs[*idx as usize].clone());
            None
        }
        PackedHotKind::StoreLocal { idx, src } => {
            let value = regs[*src as usize].clone();
            assign_reg(frame_raw, regs, *idx as usize, value);
            None
        }
        PackedHotKind::LoadGlobal { dst, name_k } => {
            let name_val = &func.consts[*name_k as usize];
            let mut out = Val::Nil;
            if let Some(s) = name_val.as_str() {
                let key_ptr = s.as_ptr() as usize;
                let cur_gen = ctx.generation();
                let local_shadowed = ctx.is_local_name(s);
                if !local_shadowed {
                    if let Some(GlobalEntry(ptr, v, generation)) = &global_ic[pc]
                        && *ptr == key_ptr
                        && *generation == cur_gen
                    {
                        out = v.clone();
                    } else if let Some(v) = ctx.get(s) {
                        out = v.clone();
                        global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                    }
                }
                if matches!(out, Val::Nil)
                    && let Some(v) = ctx.get_value(s)
                {
                    out = v;
                    if !local_shadowed {
                        global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                    }
                }
                if matches!(out, Val::Nil)
                    && let Some(builtin) = ctx.resolver().get_builtin(s)
                {
                    out = builtin.clone();
                    if !local_shadowed {
                        global_ic[pc] = Some(GlobalEntry(key_ptr, out.clone(), cur_gen));
                    }
                }
            } else {
                let fallback_name = format!("{}", name_val);
                if let Some(v) = ctx.get(fallback_name.as_str()) {
                    out = v.clone();
                }
            }
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::DefineGlobal { name_k, src } => {
            if let Some(s) = func.consts[*name_k as usize].as_str() {
                ctx.set(s.to_string(), regs[*src as usize].clone());
            }
            None
        }
        PackedHotKind::LoadCapture { dst, idx } => {
            closure::run_load_capture(frame_raw, regs, ctx, frame_captures, frame_capture_specs, *dst, *idx)?;
            None
        }
        PackedHotKind::Access { dst, base, field } => {
            let hit_val = match (&regs[*base as usize], &regs[*field as usize]) {
                (Val::List(list), Val::Int(index)) => {
                    if *index < 0 {
                        Some(Val::Nil)
                    } else {
                        Some(list.get(*index as usize).cloned().unwrap_or(Val::Nil))
                    }
                }
                (base_val, Val::Int(index)) if base_val.as_str().is_some() => {
                    let text = base_val.as_str().unwrap();
                    if *index < 0 {
                        Some(Val::Nil)
                    } else if text.is_ascii() {
                        let idx = *index as usize;
                        text.as_bytes()
                            .get(idx)
                            .copied()
                            .map_or(Some(Val::Nil), |byte| Some(Val::ascii_char_value(byte)))
                    } else {
                        Some(
                            text.chars()
                                .nth(*index as usize)
                                .map(|ch| Val::from_str(&ch.to_string()))
                                .unwrap_or(Val::Nil),
                        )
                    }
                }
                (Val::Map(map), key) if key.as_str().is_some() => Val::map_get_str(map, key.as_str().unwrap()).cloned(),
                (Val::Object(object), key) if key.as_str().is_some() => {
                    let fields = &object.fields;
                    let object_ptr = Arc::as_ptr(fields) as usize;
                    let key = key.as_str().unwrap();
                    match access_ic[pc].as_mut() {
                        Some(AccessIc::ObjectStr(slots)) => {
                            Vm::lookup_promote(slots, |entry| entry.obj_ptr == object_ptr && entry.key.as_str() == key)
                                .map(|entry| entry.value.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            let result = if let Some(value) = hit_val {
                value
            } else {
                let value = regs[*base as usize].access(&regs[*field as usize]).unwrap_or(Val::Nil);
                if let (Val::Object(object), field_val) = (&regs[*base as usize], &regs[*field as usize])
                    && let Some(key) = field_val.as_str()
                {
                    let fields = &object.fields;
                    let object_ptr = Arc::as_ptr(fields) as usize;
                    Vm::update_object_ic(access_ic, pc, object_ptr, key, &value);
                }
                value
            };
            assign_reg(frame_raw, regs, *dst as usize, result);
            None
        }
        PackedHotKind::AccessK { dst, base, key } => {
            let key_val = &func.consts[*key as usize];
            let result = if let Some(key_str) = key_val.as_str() {
                let (hit_value, object_ptr) = match &regs[*base as usize] {
                    Val::Map(map) => (Some(Val::map_get_str(map, key_str).cloned().unwrap_or(Val::Nil)), None),
                    Val::Object(object) => {
                        let fields = &object.fields;
                        let object_ptr = Arc::as_ptr(fields) as usize;
                        let hit = match access_ic[pc].as_mut() {
                            Some(AccessIc::ObjectStr(slots)) => Vm::lookup_promote(slots, |entry| {
                                entry.obj_ptr == object_ptr && entry.key.as_str() == key_str
                            })
                            .map(|entry| entry.value.clone()),
                            _ => None,
                        };
                        (hit, Some(object_ptr))
                    }
                    _ => (None, None),
                };
                if let Some(value) = hit_value {
                    value
                } else {
                    let value = regs[*base as usize].access(key_val).unwrap_or(Val::Nil);
                    if let Some(object_ptr) = object_ptr {
                        Vm::update_object_ic(access_ic, pc, object_ptr, key_str, &value);
                    }
                    value
                }
            } else {
                Val::Nil
            };
            assign_reg(frame_raw, regs, *dst as usize, result);
            None
        }
        PackedHotKind::ListLen { dst, src } => {
            record_container(VmContainerMetric::List);
            let out = match &regs[*src as usize] {
                Val::List(list) => Val::Int(list.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::MapLen { dst, src } => {
            record_container(VmContainerMetric::Map);
            let out = match &regs[*src as usize] {
                Val::Map(map) => Val::Int(map.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::StrLen { dst, src } => {
            record_container(VmContainerMetric::String);
            let out = match &regs[*src as usize] {
                Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
                Val::Str(value) => Val::Int(value.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::Len { dst, src } => {
            record_container(VmContainerMetric::Generic);
            exec_len(frame_raw, regs, *dst, *src);
            None
        }
        PackedHotKind::Index { dst, base, idx } => {
            record_container(VmContainerMetric::Generic);
            exec_index(frame_raw, regs, index_ic, pc, *dst, *base, *idx);
            None
        }
        PackedHotKind::MapGetInterned { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let key = func.consts[*key as usize].as_str().unwrap_or("");
            let out = match &regs[*map as usize] {
                Val::Map(map) => Val::map_get_str(map, key).cloned().unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::MapGetInternedCmpJmp {
            dst,
            map,
            key,
            op,
            rhs,
            jump_pc,
        } => {
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let key = func.consts[*key as usize].as_str().unwrap_or("");
            let out = match &regs[*map as usize] {
                Val::Map(map) => Val::map_get_str(map, key).cloned().unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            let lhs = &regs[*dst as usize];
            let rhs = rk_read(regs, &func.consts, *rhs);
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                _ => unreachable!("map-get compare fusion only supports equality"),
            };
            if cmp { None } else { Some(*jump_pc) }
        }
        PackedHotKind::MapGetDynamic { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let out = match (&regs[*map as usize], regs[*key as usize].as_str()) {
                (Val::Map(map), Some(key)) => Val::map_get_str(map, key).cloned().unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::MapGetDynamicCmpJmp {
            dst,
            map,
            key,
            op,
            rhs,
            jump_pc,
        } => {
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let out = match (&regs[*map as usize], regs[*key as usize].as_str()) {
                (Val::Map(map), Some(key)) => Val::map_get_str(map, key).cloned().unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            let lhs = &regs[*dst as usize];
            let rhs = rk_read(regs, &func.consts, *rhs);
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                _ => unreachable!("map-get compare fusion only supports equality"),
            };
            if cmp { None } else { Some(*jump_pc) }
        }
        PackedHotKind::MapHas { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let out = match (&regs[*map as usize], regs[*key as usize].as_str()) {
                (Val::Map(map), Some(key)) => Val::Bool(Val::map_contains_str(map, key)),
                (Val::Map(_), None) => Val::Bool(false),
                _ => return Err(anyhow!("has() first argument must be a map")),
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::MapHasK { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let key = func.consts[*key as usize].as_str().unwrap_or("");
            let out = match &regs[*map as usize] {
                Val::Map(map) => Val::Bool(Val::map_contains_str(map, key)),
                _ => return Err(anyhow!("has() first argument must be a map")),
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::MapSetInterned { map, key, val } => {
            record_container(VmContainerMetric::Map);
            exec_map_set_interned(func, regs, *map, *key, *val)?;
            None
        }
        PackedHotKind::MapSetInternedMove { map, key, val } => {
            record_container(VmContainerMetric::Map);
            exec_map_set_interned_move(func, regs, *map, *key, *val)?;
            None
        }
        PackedHotKind::StrConcatKnownCap { dst, a, b } => {
            record_container(VmContainerMetric::String);
            let a_val = &regs[*a as usize];
            let b_val = &regs[*b as usize];
            let out = match (a_val.as_str(), b_val.as_str()) {
                (Some(a_str), Some(b_str)) => Val::concat_strings(a_str, b_str),
                _ => BinOp::Add.eval_vals(a_val, b_val)?,
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::StrConcatToStr { dst, lhs, src } => {
            record_container(VmContainerMetric::String);
            let lhs_val = &regs[*lhs as usize];
            let out = if let Some(lhs_str) = lhs_val.as_str()
                && let Some(value) = Val::concat_str_tostr_rhs(lhs_str, &regs[*src as usize])
            {
                value
            } else {
                let rhs = Val::to_str_value(&regs[*src as usize]);
                BinOp::Add.eval_vals(lhs_val, &rhs)?
            };
            assign_reg(frame_raw, regs, *dst as usize, out);
            None
        }
        PackedHotKind::IntArith { op, dst, a, b } => {
            exec_int_arith(frame_raw, regs, func, *op, *dst, *a, *b)?;
            None
        }
        PackedHotKind::AddIntFloorDivImm {
            add_dst,
            a,
            b,
            div_dst,
            imm,
        } => {
            exec_int_arith(frame_raw, regs, func, PackedArithOp::Add, *add_dst, *a, *b)?;
            exec_floor_div_imm(frame_raw, regs, *div_dst, *add_dst, *imm);
            None
        }
        PackedHotKind::FloatArith { op, dst, a, b } => {
            exec_float_arith(frame_raw, regs, func, *op, *dst, *a, *b)?;
            None
        }
        PackedHotKind::Floor { dst, src } => {
            exec_floor(frame_raw, regs, *dst, *src);
            None
        }
        PackedHotKind::FloorDivImm { dst, src, imm } => {
            exec_floor_div_imm(frame_raw, regs, *dst, *src, *imm);
            None
        }
        PackedHotKind::ToBool { dst, src } => {
            let truthy = !matches!(regs[*src as usize], Val::Nil | Val::Bool(false));
            assign_reg(frame_raw, regs, *dst as usize, Val::Bool(truthy));
            None
        }
        PackedHotKind::StartsWithK { dst, src, key } => {
            record_container(VmContainerMetric::String);
            exec_starts_with_k(frame_raw, regs, func, *dst, *src, *key);
            None
        }
        PackedHotKind::ContainsK { dst, src, key } => {
            record_container(VmContainerMetric::String);
            exec_contains_k(frame_raw, regs, func, *dst, *src, *key);
            None
        }
        PackedHotKind::ToIter { dst, src } => {
            record_container(VmContainerMetric::Generic);
            exec_to_iter(frame_raw, regs, *dst, *src, region_plan, region_allocator_ptr);
            None
        }
        PackedHotKind::BuildList { dst, base, len } => {
            record_container(VmContainerMetric::List);
            let start = *base as usize;
            let len = *len as usize;
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            if use_thread_local {
                let allocator = region_allocator(region_allocator_ptr);
                let list_val = allocator.with_val_buffer(len, |scratch| {
                    scratch.extend((0..len).map(|i| regs[start + i].clone()));
                    let data = scratch.split_off(0);
                    Val::List(data.into())
                });
                assign_reg(frame_raw, regs, *dst as usize, list_val);
            } else {
                let mut values = Vec::with_capacity(len);
                for i in 0..len {
                    values.push(regs[start + i].clone());
                }
                assign_reg(frame_raw, regs, *dst as usize, Val::List(values.into()));
            }
            None
        }
        PackedHotKind::BuildMap { dst, base, len } => {
            record_container(VmContainerMetric::Map);
            let start = *base as usize;
            let len = *len as usize;
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            if use_thread_local {
                let allocator = region_allocator(region_allocator_ptr);
                let map_val = allocator.with_map_entries(len, |entries| {
                    for i in 0..len {
                        let key_val = &regs[start + 2 * i];
                        let value = regs[start + 2 * i + 1].clone();
                        let key_arc = key_val
                            .primitive_key_arcstr()
                            .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", key_val))?;
                        entries.push((key_arc, value));
                    }
                    let mut map = fast_hash_map_with_capacity(entries.len());
                    for (key, value) in entries.drain(..) {
                        Val::map_insert_arcstr(&mut map, key, value);
                    }
                    Ok::<Val, anyhow::Error>(Val::Map(Arc::new(map)))
                })?;
                assign_reg(frame_raw, regs, *dst as usize, map_val);
            } else {
                let mut map: FastHashMap<ArcStr, Val> = fast_hash_map_with_capacity(len);
                for i in 0..len {
                    let key_val = &regs[start + 2 * i];
                    let value = regs[start + 2 * i + 1].clone();
                    let key_arc = key_val
                        .primitive_key_arcstr()
                        .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", key_val))?;
                    Val::map_insert_arcstr(&mut map, key_arc, value);
                }
                assign_reg(frame_raw, regs, *dst as usize, Val::Map(Arc::new(map)));
            }
            None
        }
        PackedHotKind::ForRangePrep {
            idx,
            limit,
            step,
            inclusive,
            explicit,
        } => {
            let idx_reg = *idx as usize;
            let limit_reg = *limit as usize;
            let step_reg = *step as usize;
            let (i0, ilim) = match (&regs[idx_reg], &regs[limit_reg]) {
                (Val::Int(a0), Val::Int(b0)) => (*a0, *b0),
                _ => {
                    return Err(anyhow!(
                        "For-range requires integer bounds, got idx={:?}, limit={:?}",
                        regs[idx_reg],
                        regs[limit_reg]
                    ));
                }
            };
            let step_val = if !*explicit {
                let step_val = if i0 <= ilim { 1 } else { -1 };
                assign_reg(frame_raw, regs, step_reg, Val::Int(step_val));
                step_val
            } else {
                match &regs[step_reg] {
                    Val::Int(0) => return Err(anyhow!("For-range step cannot be zero")),
                    Val::Int(v) => *v,
                    other => return Err(anyhow!("For-range step must be Int when explicit, got {:?}", other)),
                }
            };
            if step_val == 0 {
                return Err(anyhow!("For-range step cannot be zero"));
            }
            if let Some(slot) = for_range_ic.get_mut(entry.next_pc) {
                *slot = Some(ForRangeState::new(i0, ilim, step_val, *inclusive));
            }
            None
        }
        PackedHotKind::ForRangeLoop { idx, write_idx, ofs } => {
            let state_entry = match for_range_ic.get_mut(pc).and_then(Option::as_mut) {
                Some(state) => state,
                None => return Err(anyhow!("For-range state missing at pc {}", pc)),
            };
            let keep_going = if state_entry.positive {
                if state_entry.inclusive {
                    state_entry.current <= state_entry.limit
                } else {
                    state_entry.current < state_entry.limit
                }
            } else if state_entry.inclusive {
                state_entry.current >= state_entry.limit
            } else {
                state_entry.current > state_entry.limit
            };
            if keep_going {
                let current = state_entry.current;
                if *write_idx {
                    assign_reg(frame_raw, regs, *idx as usize, Val::Int(current));
                }
                state_entry.current += state_entry.step;
                // ForRange fusion: peek at next BC32 word. If it's ForRangeStep,
                // jump directly back to the loop guard — saves one PackedHotSlot
                // dispatch per for-range iteration.
                if let Some(code32) = func.code32.as_ref() {
                    let step_pc = entry.next_pc;
                    if let Some(&step_w) = code32.get(step_pc)
                        && bc32::tag_of(step_w) == bc32::TAG_FOR_RANGE_STEP
                    {
                        let ext_idx = step_pc + 1;
                        let mut ext = code32.get(ext_idx).copied();
                        if ext.is_some() && bc32::tag_of(ext.unwrap()) == bc32::TAG_REG_EXT {
                            ext = code32.get(ext_idx + 1).copied();
                        }
                        if let Some(e) = ext {
                            let back = (((((e >> 8) & 0xFF) as u16) << 8) | ((e & 0xFF) as u16)) as i16;
                            return Ok(Some(((step_pc as isize) + (back as isize)) as usize));
                        }
                    }
                }
                None
            } else {
                // Write final counter value on exit for correct post-loop counter value.
                if *write_idx {
                    assign_reg(frame_raw, regs, *idx as usize, Val::Int(state_entry.current));
                }
                for_range_ic[pc] = None;
                Some(((pc as isize) + (*ofs as isize)) as usize)
            }
        }
        PackedHotKind::ForRangeStep { back_ofs, tail } => {
            let guard_pc = ((pc as isize) + (*back_ofs as isize)) as usize;
            match tail {
                Some(tail) => Some(advance_for_range_tail(
                    frame_raw,
                    regs,
                    for_range_ic,
                    tail.guard_pc,
                    tail.body_pc,
                    tail.exit_pc,
                    tail.idx,
                    tail.write_idx,
                )?),
                None => Some(guard_pc),
            }
        }
        PackedHotKind::ToStr { dst, src } => {
            let s = Val::to_str_value(&regs[*src as usize]);
            assign_reg(frame_raw, regs, *dst as usize, s);
            None
        }
        PackedHotKind::ToStrAddRhs {
            tmp,
            src,
            out,
            lhs,
            add_pc,
        } => {
            let lhs_val = rk_read(regs, &func.consts, *lhs);
            if let Some(lhs_str) = lhs_val.as_str()
                && let Some(value) = Val::concat_str_tostr_rhs(lhs_str, &regs[*src as usize])
            {
                assign_reg(frame_raw, regs, *out as usize, value);
                None
            } else {
                let s = Val::to_str_value(&regs[*src as usize]);
                assign_reg(frame_raw, regs, *tmp as usize, s);
                Some(*add_pc)
            }
        }
        PackedHotKind::MakeClosure { dst, proto } => {
            let clo = make_closure_value(func, *proto, ctx, regs, frame_base)?;
            assign_reg(frame_raw, regs, *dst as usize, clo);
            None
        }
        PackedHotKind::Arith { op, dst, a, b } => {
            if let (Val::Int(x), Val::Int(y)) = (rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b)) {
                match op {
                    PackedArithOp::Add => {
                        assign_reg(frame_raw, regs, *dst as usize, Val::Int(*x + *y));
                    }
                    PackedArithOp::Sub => {
                        assign_reg(frame_raw, regs, *dst as usize, Val::Int(*x - *y));
                    }
                    PackedArithOp::Mul => {
                        assign_reg(frame_raw, regs, *dst as usize, Val::Int(*x * *y));
                    }
                    PackedArithOp::Div => {
                        // Consistent with eval_vals: Int/Int returns Int when divisible, Float otherwise
                        let res = *x as f64 / *y as f64;
                        if res.fract() == 0.0 {
                            assign_reg(frame_raw, regs, *dst as usize, Val::Int(res as i64));
                        } else {
                            assign_reg(frame_raw, regs, *dst as usize, Val::Float(res));
                        }
                    }
                    PackedArithOp::Mod => {
                        assign_reg(frame_raw, regs, *dst as usize, Val::Int(*x % *y));
                    }
                }
            } else {
                match op {
                    PackedArithOp::Add => {
                        let a_val = rk_read(regs, &func.consts, *a);
                        let b_val = rk_read(regs, &func.consts, *b);
                        if let Some(a_str) = a_val.as_str()
                            && let Some(out) = Val::concat_str_add_rhs(a_str, b_val)
                        {
                            assign_reg(frame_raw, regs, *dst as usize, out);
                        } else if let Some(b_str) = b_val.as_str()
                            && let Some(out) = Val::concat_add_lhs_str(a_val, b_str)
                        {
                            assign_reg(frame_raw, regs, *dst as usize, out);
                        } else if !Vm::arith2_try_numeric(
                            frame_raw,
                            regs,
                            &func.consts,
                            *dst,
                            *a,
                            *b,
                            "add",
                            |x, y| x + y,
                            |x, y| x + y,
                        ) {
                            let out = BinOp::Add
                                .eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                            assign_reg(frame_raw, regs, *dst as usize, out);
                        }
                    }
                    PackedArithOp::Sub => {
                        if !Vm::arith2_try_numeric(
                            frame_raw,
                            regs,
                            &func.consts,
                            *dst,
                            *a,
                            *b,
                            "sub",
                            |x, y| x - y,
                            |x, y| x - y,
                        ) {
                            let out = BinOp::Sub
                                .eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                            assign_reg(frame_raw, regs, *dst as usize, out);
                        }
                    }
                    PackedArithOp::Mul => {
                        if !Vm::arith2_try_numeric(
                            frame_raw,
                            regs,
                            &func.consts,
                            *dst,
                            *a,
                            *b,
                            "mul",
                            |x, y| x * y,
                            |x, y| x * y,
                        ) {
                            let out = BinOp::Mul
                                .eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                            assign_reg(frame_raw, regs, *dst as usize, out);
                        }
                    }
                    PackedArithOp::Div => {
                        let ar = rk_read(regs, &func.consts, *a);
                        let br = rk_read(regs, &func.consts, *b);
                        let dst_idx = *dst as usize;
                        match (ar, br) {
                            (Val::Int(x), Val::Int(y)) => {
                                let res = *x as f64 / *y as f64;
                                if res.fract() == 0.0 {
                                    assign_reg(frame_raw, regs, dst_idx, Val::Int(res as i64));
                                } else {
                                    assign_reg(frame_raw, regs, dst_idx, Val::Float(res));
                                }
                            }
                            (Val::Float(x), Val::Float(y)) => {
                                assign_reg(frame_raw, regs, dst_idx, Val::Float(x / y));
                            }
                            (Val::Int(x), Val::Float(y)) => {
                                assign_reg(frame_raw, regs, dst_idx, Val::Float(*x as f64 / y));
                            }
                            (Val::Float(x), Val::Int(y)) => {
                                assign_reg(frame_raw, regs, dst_idx, Val::Float(x / *y as f64));
                            }
                            _ => {
                                let out = BinOp::Div.eval_vals(ar, br)?;
                                assign_reg(frame_raw, regs, dst_idx, out);
                            }
                        }
                    }
                    PackedArithOp::Mod => {
                        let out =
                            BinOp::Mod.eval_vals(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, out);
                    }
                }
            }
            None
        }
        PackedHotKind::Cmp { op, dst, a, b } => {
            match op {
                PackedCmpOp::Eq => assign_reg(
                    frame_raw,
                    regs,
                    *dst as usize,
                    Val::Bool(rk_read(regs, &func.consts, *a) == rk_read(regs, &func.consts, *b)),
                ),
                PackedCmpOp::Ne => assign_reg(
                    frame_raw,
                    regs,
                    *dst as usize,
                    Val::Bool(rk_read(regs, &func.consts, *a) != rk_read(regs, &func.consts, *b)),
                ),
                PackedCmpOp::Lt => {
                    if !Vm::cmp2_try_numeric(frame_raw, regs, &func.consts, *dst, *a, *b, |x, y| x < y, |x, y| x < y) {
                        let res = BinOp::Lt.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
                PackedCmpOp::Le => {
                    if !Vm::cmp2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        |x, y| x <= y,
                        |x, y| x <= y,
                    ) {
                        let res = BinOp::Le.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
                PackedCmpOp::Gt => {
                    if !Vm::cmp2_try_numeric(frame_raw, regs, &func.consts, *dst, *a, *b, |x, y| x > y, |x, y| x > y) {
                        let res = BinOp::Gt.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
                PackedCmpOp::Ge => {
                    if !Vm::cmp2_try_numeric(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *a,
                        *b,
                        |x, y| x >= y,
                        |x, y| x >= y,
                    ) {
                        let res = BinOp::Ge.cmp(rk_read(regs, &func.consts, *a), rk_read(regs, &func.consts, *b))?;
                        assign_reg(frame_raw, regs, *dst as usize, Val::Bool(res));
                    }
                }
            }
            None
        }
        PackedHotKind::CmpInt { op, dst, a, b } => {
            let (Val::Int(lhs), Val::Int(rhs)) = (&regs[*a as usize], &regs[*b as usize]) else {
                return Err(anyhow!("CmpI expects integer registers"));
            };
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                PackedCmpOp::Lt => lhs < rhs,
                PackedCmpOp::Le => lhs <= rhs,
                PackedCmpOp::Gt => lhs > rhs,
                PackedCmpOp::Ge => lhs >= rhs,
            };
            assign_reg(frame_raw, regs, *dst as usize, Val::Bool(cmp));
            None
        }
        PackedHotKind::CmpIntJmp { op, a, b, ofs } => {
            record_branch(true);
            let (Val::Int(lhs), Val::Int(rhs)) = (&regs[*a as usize], &regs[*b as usize]) else {
                return Err(anyhow!("CmpI expects integer registers"));
            };
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                PackedCmpOp::Lt => lhs < rhs,
                PackedCmpOp::Le => lhs <= rhs,
                PackedCmpOp::Gt => lhs > rhs,
                PackedCmpOp::Ge => lhs >= rhs,
            };
            if !cmp {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::CmpIntMove {
            op,
            a,
            b,
            dst,
            src,
            ofs,
        } => {
            record_branch(true);
            let (Val::Int(lhs), Val::Int(rhs)) = (&regs[*a as usize], &regs[*b as usize]) else {
                return Err(anyhow!("CmpI expects integer registers"));
            };
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                PackedCmpOp::Lt => lhs < rhs,
                PackedCmpOp::Le => lhs <= rhs,
                PackedCmpOp::Gt => lhs > rhs,
                PackedCmpOp::Ge => lhs >= rhs,
            };
            if !cmp {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                let value = regs[*src as usize].clone();
                assign_reg(frame_raw, regs, *dst as usize, value);
                None
            }
        }
        PackedHotKind::CmpIntAddIntImm {
            op,
            a,
            b,
            dst,
            src,
            imm,
            ofs,
        } => {
            record_branch(true);
            let (Val::Int(lhs), Val::Int(rhs)) = (&regs[*a as usize], &regs[*b as usize]) else {
                return Err(anyhow!("CmpI expects integer registers"));
            };
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                PackedCmpOp::Lt => lhs < rhs,
                PackedCmpOp::Le => lhs <= rhs,
                PackedCmpOp::Gt => lhs > rhs,
                PackedCmpOp::Ge => lhs >= rhs,
            };
            if !cmp {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                let dst_idx = *dst as usize;
                let src_idx = *src as usize;
                if let Val::Int(value) = regs[src_idx] {
                    assign_reg(frame_raw, regs, dst_idx, Val::Int(value + *imm as i64));
                } else {
                    int_binop_imm(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *src,
                        *imm,
                        |x, y| x + y,
                        BinOp::Add,
                    )?;
                }
                None
            }
        }
        PackedHotKind::CmpJmp { op, a, b, ofs } => {
            record_branch(false);
            let lhs = rk_read(regs, &func.consts, *a);
            let rhs = rk_read(regs, &func.consts, *b);
            let cmp = match (op, lhs, rhs) {
                (PackedCmpOp::Eq, left, right) => left == right,
                (PackedCmpOp::Ne, left, right) => left != right,
                (PackedCmpOp::Lt, Val::Int(left), Val::Int(right)) => left < right,
                (PackedCmpOp::Le, Val::Int(left), Val::Int(right)) => left <= right,
                (PackedCmpOp::Gt, Val::Int(left), Val::Int(right)) => left > right,
                (PackedCmpOp::Ge, Val::Int(left), Val::Int(right)) => left >= right,
                (PackedCmpOp::Lt, _, _) => BinOp::Lt.cmp(lhs, rhs)?,
                (PackedCmpOp::Le, _, _) => BinOp::Le.cmp(lhs, rhs)?,
                (PackedCmpOp::Gt, _, _) => BinOp::Gt.cmp(lhs, rhs)?,
                (PackedCmpOp::Ge, _, _) => BinOp::Ge.cmp(lhs, rhs)?,
            };
            if !cmp {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::Jmp { ofs } => {
            record_branch(false);
            Some(((pc as isize) + (*ofs as isize)) as usize)
        }
        PackedHotKind::JmpFalse { r, ofs } => {
            record_branch(false);
            if matches!(regs[*r as usize], Val::Nil | Val::Bool(false)) {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::JmpFalseSet { r, dst, ofs } => {
            record_branch(false);
            if matches!(regs[*r as usize], Val::Nil | Val::Bool(false)) {
                assign_reg(frame_raw, regs, *dst as usize, Val::Bool(false));
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::JmpTrueSet { r, dst, ofs } => {
            record_branch(false);
            if !matches!(regs[*r as usize], Val::Nil | Val::Bool(false)) {
                assign_reg(frame_raw, regs, *dst as usize, Val::Bool(true));
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::Ret { .. } => unreachable!("Ret is handled directly by run_packed_code"),
        PackedHotKind::ListPush { list, val } => {
            record_container(VmContainerMetric::List);
            let pushed_val = regs[*val as usize].clone();
            match &mut regs[*list as usize] {
                Val::List(arc) => {
                    push_list_entry(arc, pushed_val);
                }
                _ => return Err(anyhow!("ListPush target is not a List")),
            }
            None
        }
        PackedHotKind::ListPushMove { list, val } => {
            record_container(VmContainerMetric::List);
            let list_idx = *list as usize;
            let val_idx = *val as usize;
            if list_idx == val_idx {
                let pushed_val = regs[val_idx].clone();
                match &mut regs[list_idx] {
                    Val::List(arc) => {
                        push_list_entry(arc, pushed_val);
                    }
                    _ => return Err(anyhow!("ListPush target is not a List")),
                }
                return Ok(None);
            }
            if !matches!(regs[list_idx], Val::List(_)) {
                return Err(anyhow!("ListPush target is not a List"));
            }
            let pushed_val = std::mem::replace(&mut regs[val_idx], Val::Nil);
            match &mut regs[list_idx] {
                Val::List(arc) => {
                    push_list_entry(arc, pushed_val);
                }
                _ => unreachable!("ListPush target was checked before moving value"),
            }
            None
        }
        PackedHotKind::MapSet { map, key, val } => {
            record_container(VmContainerMetric::Map);
            let key_arc = regs[*key as usize]
                .string_key_arcstr()
                .ok_or_else(|| anyhow!("MapSet key must be a String"))?;
            let pushed_val = regs[*val as usize].clone();
            match &mut regs[*map as usize] {
                Val::Map(arc) => {
                    insert_map_entry(arc, key_arc, pushed_val);
                }
                _ => return Err(anyhow!("MapSet target is not a Map")),
            }
            None
        }
        PackedHotKind::MapSetMove { map, key, val } => {
            record_container(VmContainerMetric::Map);
            let map_idx = *map as usize;
            let key_idx = *key as usize;
            let val_idx = *val as usize;
            if map_idx == key_idx || map_idx == val_idx || key_idx == val_idx {
                let key_arc = regs[key_idx]
                    .string_key_arcstr()
                    .ok_or_else(|| anyhow!("MapSet key must be a String"))?;
                let pushed_val = regs[val_idx].clone();
                match &mut regs[map_idx] {
                    Val::Map(arc) => {
                        insert_map_entry(arc, key_arc, pushed_val);
                    }
                    _ => return Err(anyhow!("MapSet target is not a Map")),
                }
                return Ok(None);
            }
            if !matches!(regs[map_idx], Val::Map(_)) {
                return Err(anyhow!("MapSet target is not a Map"));
            }
            let key_val = std::mem::replace(&mut regs[key_idx], Val::Nil);
            let key_arc = match key_val.string_key_arcstr() {
                Some(key_arc) => key_arc,
                None => {
                    regs[key_idx] = key_val;
                    return Err(anyhow!("MapSet key must be a String"));
                }
            };
            let pushed_val = std::mem::replace(&mut regs[val_idx], Val::Nil);
            match &mut regs[map_idx] {
                Val::Map(arc) => {
                    insert_map_entry(arc, key_arc, pushed_val);
                }
                _ => unreachable!("MapSet target was checked before moving key/value"),
            }
            None
        }
        PackedHotKind::CallNativeFast { f, base, argc, retc } => {
            record_call(VmCallMetric::Native);
            let pc_slot = call_ic
                .get_mut(pc)
                .ok_or_else(|| anyhow!("call IC slot out of range for pc {}", pc))?;
            let callable = NativeCallable::from_val(&regs[*f as usize])
                .ok_or_else(|| anyhow!("CallNativeFast target is not a native function"))?;
            let ret_layout = CallReturnLayout::new(*base, *retc);
            invoke_native_callable_with_ic(ctx, regs, pc_slot, callable, *argc, ret_layout)?;
            None
        }
        PackedHotKind::CallMethod0 { dst, receiver, method } => {
            record_call(VmCallMetric::Method);
            method_ops::run_call_method0(frame_raw, regs, ctx, func, *dst, *receiver, *method)?;
            None
        }
        PackedHotKind::CallGlobalMethod0 { dst, receiver, method } => {
            record_call(VmCallMetric::Method);
            method_ops::run_call_global_method0(frame_raw, regs, ctx, func, global_ic, pc, *dst, *receiver, *method)?;
            None
        }
        PackedHotKind::Call { .. } | PackedHotKind::CallClosureExact { .. } | PackedHotKind::CallExact { .. } => {
            unreachable!("call hot slots are handled by run_packed_code")
        }
        PackedHotKind::MoveCall { .. } => unreachable!("move+call hot slots are handled by run_packed_code"),
        PackedHotKind::AddIntImm { dst, src, imm } => {
            let dst_idx = *dst as usize;
            let src_idx = *src as usize;
            if let Val::Int(x) = regs[src_idx] {
                assign_reg(frame_raw, regs, dst_idx, Val::Int(x + *imm as i64));
            } else {
                int_binop_imm(
                    frame_raw,
                    regs,
                    &func.consts,
                    *dst,
                    *src,
                    *imm,
                    |x, y| x + y,
                    BinOp::Add,
                )?;
            }
            None
        }
        PackedHotKind::CmpImm { op, dst, src, imm } => {
            let dst_idx = *dst as usize;
            let src_idx = *src as usize;
            let imm_i64 = *imm as i64;
            match (&regs[src_idx], op) {
                (Val::Int(x), PackedCmpImmOp::Eq) => assign_reg(frame_raw, regs, dst_idx, Val::Bool(*x == imm_i64)),
                (Val::Int(x), PackedCmpImmOp::Ne) => assign_reg(frame_raw, regs, dst_idx, Val::Bool(*x != imm_i64)),
                (Val::Int(x), PackedCmpImmOp::Lt) => assign_reg(frame_raw, regs, dst_idx, Val::Bool(*x < imm_i64)),
                (Val::Int(x), PackedCmpImmOp::Le) => assign_reg(frame_raw, regs, dst_idx, Val::Bool(*x <= imm_i64)),
                (Val::Int(x), PackedCmpImmOp::Gt) => assign_reg(frame_raw, regs, dst_idx, Val::Bool(*x > imm_i64)),
                (Val::Int(x), PackedCmpImmOp::Ge) => assign_reg(frame_raw, regs, dst_idx, Val::Bool(*x >= imm_i64)),
                _ => match op {
                    PackedCmpImmOp::Eq => cmp_eq_imm(frame_raw, regs, &func.consts, *dst, *src, *imm, BinOp::Eq)?,
                    PackedCmpImmOp::Ne => cmp_ne_imm(frame_raw, regs, &func.consts, *dst, *src, *imm, BinOp::Ne)?,
                    PackedCmpImmOp::Lt => cmp_ord_imm(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *src,
                        *imm,
                        |x, y| x < y,
                        |x, y| x < y,
                        BinOp::Lt,
                    )?,
                    PackedCmpImmOp::Le => cmp_ord_imm(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *src,
                        *imm,
                        |x, y| x <= y,
                        |x, y| x <= y,
                        BinOp::Le,
                    )?,
                    PackedCmpImmOp::Gt => cmp_ord_imm(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *src,
                        *imm,
                        |x, y| x > y,
                        |x, y| x > y,
                        BinOp::Gt,
                    )?,
                    PackedCmpImmOp::Ge => cmp_ord_imm(
                        frame_raw,
                        regs,
                        &func.consts,
                        *dst,
                        *src,
                        *imm,
                        |x, y| x >= y,
                        |x, y| x >= y,
                        BinOp::Ge,
                    )?,
                },
            }
            None
        }
        PackedHotKind::CmpImmJmp { op, src, imm, ofs } => {
            record_branch(true);
            let src_idx = *src as usize;
            let imm_i64 = *imm as i64;
            let cmp = match (regs.get(src_idx), op) {
                (Some(Val::Int(x)), PackedCmpImmOp::Eq) => *x == imm_i64,
                (Some(Val::Int(x)), PackedCmpImmOp::Ne) => *x != imm_i64,
                (Some(Val::Int(x)), PackedCmpImmOp::Lt) => *x < imm_i64,
                (Some(Val::Int(x)), PackedCmpImmOp::Le) => *x <= imm_i64,
                (Some(Val::Int(x)), PackedCmpImmOp::Gt) => *x > imm_i64,
                (Some(Val::Int(x)), PackedCmpImmOp::Ge) => *x >= imm_i64,
                _ => {
                    let imm_val = Val::Int(imm_i64);
                    match op {
                        PackedCmpImmOp::Eq => rk_read(regs, &func.consts, *src) == &imm_val,
                        PackedCmpImmOp::Ne => rk_read(regs, &func.consts, *src) != &imm_val,
                        PackedCmpImmOp::Lt => BinOp::Lt.cmp(rk_read(regs, &func.consts, *src), &imm_val)?,
                        PackedCmpImmOp::Le => BinOp::Le.cmp(rk_read(regs, &func.consts, *src), &imm_val)?,
                        PackedCmpImmOp::Gt => BinOp::Gt.cmp(rk_read(regs, &func.consts, *src), &imm_val)?,
                        PackedCmpImmOp::Ge => BinOp::Ge.cmp(rk_read(regs, &func.consts, *src), &imm_val)?,
                    }
                }
            };
            if !cmp {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::CmpLtImmJmp { r, imm, ofs } => {
            record_branch(true);
            // Fused: if r < imm, fall through; else jump.
            let skip = match &regs[*r as usize] {
                Val::Int(x) => *x >= (*imm as i64),
                _ => true,
            };
            if skip {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::CmpLeImmJmp { r, imm, ofs } => {
            record_branch(true);
            // Fused: if r <= imm, fall through; else jump.
            let skip = match &regs[*r as usize] {
                Val::Int(x) => *x > (*imm as i64),
                _ => true,
            };
            if skip {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::AddIntImmJmp { r, imm, ofs } => {
            record_branch(true);
            // Fused: r += imm, then jump by ofs.
            if let Val::Int(x) = regs[*r as usize] {
                let result = x.wrapping_add(*imm as i64);
                assign_reg(frame_raw, regs, *r as usize, Val::Int(result));
            }
            Some(((pc as isize) + (*ofs as isize)) as usize)
        }
    };
    Ok(result)
}
