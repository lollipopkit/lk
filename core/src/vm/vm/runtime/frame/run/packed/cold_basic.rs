use super::*;

pub(super) fn handles_basic_op(op: &Op) -> bool {
    matches!(
        op,
        Op::Index { .. }
            | Op::Jmp(_)
            | Op::JmpFalse(_, _)
            | Op::JmpIfNil(_, _)
            | Op::JmpIfNotNil(_, _)
            | Op::CmpLtImmJmp { .. }
            | Op::CmpLeImmJmp { .. }
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
            | Op::ForRangePrep { .. }
            | Op::ForRangeLoop { .. }
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
    frame_raw: *mut FrameState<'_>,
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
) -> Result<Option<Val>> {
    let mut pc = *pc_ref;
    match op {
        Op::Index { dst, base, idx } => {
            let res = match (&regs[base as usize], &regs[idx as usize]) {
                (Val::List(l), Val::Int(i)) => {
                    if *i < 0 {
                        Val::Nil
                    } else {
                        let lptr = Arc::as_ptr(l) as *const Val as usize;
                        let hit = match index_ic[pc].as_mut() {
                            Some(IndexIc::List(slots)) => {
                                Vm::lookup_promote(slots, |e| e.base_ptr == lptr && e.idx == *i)
                                    .map(|entry| entry.value.clone())
                            }
                            _ => None,
                        };
                        if let Some(v) = hit {
                            v
                        } else {
                            let v = l.get(*i as usize).cloned().unwrap_or(Val::Nil);
                            Vm::update_list_ic(index_ic, pc, lptr, *i, &v);
                            v
                        }
                    }
                }
                (base_val, Val::Int(i)) if base_val.as_str().is_some() => {
                    let s_str = base_val.as_str().unwrap();
                    if *i < 0 {
                        Val::Nil
                    } else {
                        let sptr = s_str.as_ptr() as usize;
                        let hit = match index_ic[pc].as_mut() {
                            Some(IndexIc::Str(slots)) => {
                                Vm::lookup_promote(slots, |e| e.base_ptr == sptr && e.idx == *i)
                                    .map(|entry| entry.value.clone())
                            }
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
                            Vm::update_str_ic(index_ic, pc, sptr, *i, &v);
                            v
                        }
                    }
                }
                _ => Val::Nil,
            };
            assign_reg(frame_raw, regs, dst as usize, res);
            pc = next_pc_default;
        }
        Op::Jmp(ofs) => {
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::JmpFalse(r, ofs) => {
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
        Op::AddIntImmJmp { r, imm, ofs } => {
            // Fused: r += imm, then jump by ofs.
            if let Val::Int(x) = regs[r as usize] {
                let result = x.wrapping_add(imm as i64);
                assign_reg(frame_raw, regs, r as usize, Val::Int(result));
            }
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::ToBool(dst, src) => {
            let truthy = !matches!(regs[src as usize], Val::Nil | Val::Bool(false));
            assign_reg(frame_raw, regs, dst as usize, Val::Bool(truthy));
            pc = next_pc_default;
        }
        Op::ToIter { dst, src } => {
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            let out = match &regs[src as usize] {
                v if matches!(v, Val::List(_)) || v.as_str().is_some() => regs[src as usize].clone(),
                Val::Map(m) => {
                    let mut entries: Vec<_> = m.iter().collect();
                    entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
                    if use_thread_local && !entries.is_empty() {
                        let allocator = unsafe { &*region_allocator_ptr };
                        allocator.with_val_buffer(entries.len(), |scratch| {
                            for (key, value) in entries.iter() {
                                let pair = Val::List(vec![Val::from_str(key.as_str()), (*value).clone()].into());
                                scratch.push(pair);
                            }
                            let data = scratch.split_off(0);
                            Val::List(data.into())
                        })
                    } else {
                        let mut pairs = Vec::with_capacity(entries.len());
                        for (key, value) in entries {
                            let pair = Val::List(vec![Val::from_str(key.as_str()), value.clone()].into());
                            pairs.push(pair);
                        }
                        Val::List(pairs.into())
                    }
                }
                _ => Val::List(Vec::<Val>::new().into()),
            };
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::Not(dst, src) => {
            match &regs[src as usize] {
                Val::Bool(b) => assign_reg(frame_raw, regs, dst as usize, Val::Bool(!b)),
                other => {
                    return frame_return_common(frame_raw, pc, Err(anyhow!("Invalid operand: !{:?}", other))).map(Some);
                }
            }
            pc = next_pc_default;
        }
        Op::NullishPick { l, dst, ofs } => {
            if !matches!(regs[l as usize], Val::Nil) {
                assign_reg(frame_raw, regs, dst as usize, regs[l as usize].clone());
                pc = ((pc as isize) + (ofs as isize)) as usize;
            } else {
                pc = next_pc_default;
            }
        }
        Op::Ret { base, retc } => {
            let retc = retc as usize;
            let base_idx = base as usize;
            let ret_val = if retc > 0 {
                std::mem::replace(&mut regs[base_idx], Val::Nil)
            } else {
                Val::Nil
            };
            return handle_return_common(frame_raw, regs, pc, base_idx, retc, ret_val, self_ptr).map(Some);
        }
        Op::Break(ofs) => {
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::Continue(ofs) => {
            pc = ((pc as isize) + (ofs as isize)) as usize;
        }
        Op::LoadGlobal(dst, name_k) => {
            let name_val = &f.consts[name_k as usize];
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
            assign_reg(frame_raw, regs, dst as usize, out);
            pc = next_pc_default;
        }
        Op::DefineGlobal(name_k, src) => {
            let name_val = &f.consts[name_k as usize];
            if let Some(s) = name_val.as_str() {
                ctx.set(s.to_string(), regs[src as usize].clone());
            }
            pc = next_pc_default;
        }
        Op::Access(dst, base, field) => {
            let hit_val = match (&regs[base as usize], &regs[field as usize]) {
                (Val::List(l), Val::Int(i)) => {
                    if *i < 0 {
                        Some(Val::Nil)
                    } else {
                        Some(l.get(*i as usize).cloned().unwrap_or(Val::Nil))
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
                (Val::Map(m), Val::Str(s)) => m.get(s.as_str()).cloned(),
                (Val::Map(m), Val::ShortStr(s)) => m.get(s.as_str()).cloned(),
                (Val::Object(object), Val::Str(s)) => {
                    let fields = &object.fields;
                    let optr = Arc::as_ptr(fields) as usize;
                    let kstr = s.as_str();
                    match access_ic[pc].as_mut() {
                        Some(AccessIc::ObjectStr(slots)) => {
                            Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == kstr)
                                .map(|entry| entry.value.clone())
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
                                .map(|entry| entry.value.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            let res = if let Some(v) = hit_val {
                v
            } else {
                let v = regs[base as usize].access(&regs[field as usize]).unwrap_or(Val::Nil);
                match (&regs[base as usize], &regs[field as usize]) {
                    (Val::Object(object), field_val) if field_val.as_str().is_some() => {
                        let s = field_val.as_str().unwrap();
                        let fields = &object.fields;
                        let optr = Arc::as_ptr(fields) as usize;
                        Vm::update_object_ic(access_ic, pc, optr, s, &v);
                    }
                    _ => {}
                }
                v
            };
            assign_reg(frame_raw, regs, dst as usize, res);
            pc = next_pc_default;
        }
        Op::AccessK(dst, base, kidx) => {
            let key = &f.consts[kidx as usize];
            let res = if let Some(s) = key.as_str() {
                let (hit_val, obj_ptr) = match &regs[base as usize] {
                    Val::Map(m) => {
                        let out = m.get(s).cloned().unwrap_or(Val::Nil);
                        (Some(out), None)
                    }
                    Val::Object(object) => {
                        let fields = &object.fields;
                        let optr = Arc::as_ptr(fields) as usize;
                        let out = match access_ic[pc].as_mut() {
                            Some(AccessIc::ObjectStr(slots)) => {
                                Vm::lookup_promote(slots, |e| e.obj_ptr == optr && e.key.as_str() == s)
                                    .map(|entry| entry.value.clone())
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
                    let v = regs[base as usize].access(key).unwrap_or(Val::Nil);
                    if let Some(optr) = obj_ptr {
                        Vm::update_object_ic(access_ic, pc, optr, s, &v);
                    }
                    v
                }
            } else {
                Val::Nil
            };
            assign_reg(frame_raw, regs, dst as usize, res);
            pc = next_pc_default;
        }
        Op::IndexK(dst, base, kidx) => {
            let key = &f.consts[kidx as usize];
            let res = if let Val::Int(i) = key {
                match &regs[base as usize] {
                    Val::List(l) => {
                        if *i < 0 {
                            Val::Nil
                        } else {
                            l.get(*i as usize).cloned().unwrap_or(Val::Nil)
                        }
                    }
                    Val::Str(s) => {
                        if *i < 0 {
                            Val::Nil
                        } else if s.is_ascii() {
                            let bi = *i as usize;
                            let bs = s.as_bytes();
                            if bi < bs.len() {
                                Val::ascii_char_value(bs[bi])
                            } else {
                                Val::Nil
                            }
                        } else {
                            s.chars()
                                .nth(*i as usize)
                                .map(|c| Val::from_str(&c.to_string()))
                                .unwrap_or(Val::Nil)
                        }
                    }
                    Val::ShortStr(ss) => {
                        if *i < 0 {
                            Val::Nil
                        } else {
                            let s_str = ss.as_str();
                            if s_str.is_ascii() {
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
                            }
                        }
                    }
                    _ => Val::Nil,
                }
            } else {
                Val::Nil
            };
            assign_reg(frame_raw, regs, dst as usize, res);
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
                assign_reg(frame_raw, regs, step_reg, Val::Int(step_val));
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
                    assign_reg(frame_raw, regs, idx_reg, Val::Int(state_entry.current));
                }
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
                    for binding in &plan.bindings {
                        if let Some((_, v)) = bound.iter().find(|(name, _)| name == &binding.name) {
                            assign_reg(frame_raw, regs, binding.reg as usize, v.clone());
                        } else {
                            assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                        }
                    }
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(true));
                }
                None => {
                    for binding in &plan.bindings {
                        assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                    }
                    assign_reg(frame_raw, regs, dst as usize, Val::Bool(false));
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
                    for binding in &plan.bindings {
                        if let Some((_, v)) = bound.iter().find(|(name, _)| name == &binding.name) {
                            let cloned = v.clone();
                            assign_reg(frame_raw, regs, binding.reg as usize, cloned.clone());
                            assigned.push((binding.name.clone(), cloned));
                        } else {
                            assign_reg(frame_raw, regs, binding.reg as usize, Val::Nil);
                        }
                    }
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
                let allocator = unsafe { &*region_allocator_ptr };
                let list_val = allocator.with_val_buffer(n, |scratch| {
                    scratch.extend((0..n).map(|i| regs[start + i].clone()));
                    let data = scratch.split_off(0);
                    Val::List(data.into())
                });
                assign_reg(frame_raw, regs, dst as usize, list_val);
            } else {
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    v.push(regs[start + i].clone());
                }
                assign_reg(frame_raw, regs, dst as usize, Val::List(v.into()));
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
                let allocator = unsafe { &*region_allocator_ptr };
                let map_val = allocator.with_map_entries(n, |entries| {
                    for i in 0..n {
                        let key_arc;
                        let value;
                        {
                            let key_val = &regs[start + 2 * i];
                            value = regs[start + 2 * i + 1].clone();
                            key_arc = match key_val {
                                Val::ShortStr(s) => Val::intern_str(s.as_str()),
                                Val::Str(s) => s.clone(),
                                Val::Int(i) => Val::intern_str(i.to_string().as_str()),
                                Val::Float(f) => Val::intern_str(f.to_string().as_str()),
                                Val::Bool(b) => Val::intern_str(b.to_string().as_str()),
                                _ => {
                                    return Err(anyhow!("Map key must be a primitive type, got: {:?}", key_val));
                                }
                            };
                        }
                        entries.push((key_arc, value));
                    }
                    let mut map = fast_hash_map_with_capacity(entries.len());
                    for (k, v) in entries.drain(..) {
                        map.insert(k, v);
                    }
                    Ok(Val::Map(Arc::new(map)))
                });
                match map_val {
                    Ok(val) => assign_reg(frame_raw, regs, dst as usize, val),
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
                        value = regs[start + 2 * i + 1].clone();
                        key_arc = match key_val {
                            Val::ShortStr(s) => Val::intern_str(s.as_str()),
                            Val::Str(s) => s.clone(),
                            Val::Int(i) => Val::intern_str(i.to_string().as_str()),
                            Val::Float(f) => Val::intern_str(f.to_string().as_str()),
                            Val::Bool(b) => Val::intern_str(b.to_string().as_str()),
                            _ => {
                                return frame_return_common(
                                    frame_raw,
                                    pc,
                                    Err(anyhow!("Map key must be a primitive type, got: {:?}", key_val)),
                                )
                                .map(Some);
                            }
                        };
                    }
                    map.insert(key_arc, value);
                }
                assign_reg(frame_raw, regs, dst as usize, Val::Map(Arc::new(map)));
            }
            pc = next_pc_default;
        }
        Op::MakeClosure { dst, proto } => {
            let clo = make_closure_value(f, proto, ctx, regs, frame_base)?;
            assign_reg(frame_raw, regs, dst as usize, clo);
            pc = next_pc_default;
        }
        Op::LoadLocal(dst, idx) => {
            assign_reg(frame_raw, regs, dst as usize, regs[idx as usize].clone());
            pc = next_pc_default;
        }
        Op::StoreLocal(idx, src) => {
            let v = regs[src as usize].clone();
            assign_reg(frame_raw, regs, idx as usize, v);
            pc = next_pc_default;
        }
        _ => unreachable!("basic packed op predicate drifted"),
    }
    *pc_ref = pc;
    Ok(None)
}
