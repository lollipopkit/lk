use super::*;

#[inline(always)]
pub(super) fn exec_len(frame_raw: *mut FrameState<'_>, regs: &mut [Val], dst: u16, src: u16) {
    let out = match &regs[src as usize] {
        Val::List(value) => Val::Int(value.len() as i64),
        Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
        Val::Str(value) => Val::Int(value.len() as i64),
        Val::Map(value) => Val::Int(value.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline(always)]
pub(super) fn exec_index(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    index_ic: &mut [Option<IndexIc>],
    pc: usize,
    dst: u16,
    base: u16,
    idx: u16,
) {
    let out = match (&regs[base as usize], &regs[idx as usize]) {
        (Val::List(list), Val::Int(index)) => {
            if *index < 0 {
                list.len()
                    .checked_sub(index.unsigned_abs() as usize)
                    .and_then(|idx| list.get(idx).cloned())
                    .unwrap_or(Val::Nil)
            } else {
                let list_ptr = Arc::as_ptr(list) as *const Val as usize;
                let hit = match index_ic[pc].as_mut() {
                    Some(IndexIc::List(slots)) => {
                        Vm::lookup_promote(slots, |entry| entry.base_ptr == list_ptr && entry.idx == *index)
                            .map(|entry| entry.value.clone())
                    }
                    _ => None,
                };
                if let Some(value) = hit {
                    value
                } else {
                    let value = list.get(*index as usize).cloned().unwrap_or(Val::Nil);
                    Vm::update_list_ic(index_ic, pc, list_ptr, *index, &value);
                    value
                }
            }
        }
        (base_val, Val::Int(index)) if base_val.as_str().is_some() => {
            let text = base_val.as_str().unwrap();
            if *index < 0 {
                text_index_value(text, *index)
            } else {
                let text_ptr = text.as_ptr() as usize;
                let hit = match index_ic[pc].as_mut() {
                    Some(IndexIc::Str(slots)) => {
                        Vm::lookup_promote(slots, |entry| entry.base_ptr == text_ptr && entry.idx == *index)
                            .map(|entry| entry.value.clone())
                    }
                    _ => None,
                };
                if let Some(value) = hit {
                    value
                } else {
                    let value = if text.is_ascii() {
                        text.as_bytes()
                            .get(*index as usize)
                            .copied()
                            .map_or(Val::Nil, Val::ascii_char_value)
                    } else {
                        text.chars()
                            .nth(*index as usize)
                            .map(|character| Val::from_str(&character.to_string()))
                            .unwrap_or(Val::Nil)
                    };
                    Vm::update_str_ic(index_ic, pc, text_ptr, *index, &value);
                    value
                }
            }
        }
        (base_val, key) => base_val.access(key).unwrap_or(Val::Nil),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

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

#[inline(always)]
pub(super) fn exec_map_set_interned(func: &Function, regs: &mut [Val], map: u16, key: u16, val: u16) -> Result<()> {
    let key = func.consts[key as usize]
        .string_key_arcstr()
        .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
    let value = regs[val as usize].clone();
    match &mut regs[map as usize] {
        Val::Map(map) => insert_map_entry(map, key, value),
        _ => return Err(anyhow!("MapSet target is not a Map")),
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_map_set_interned_move(
    func: &Function,
    regs: &mut [Val],
    map: u16,
    key: u16,
    val: u16,
) -> Result<()> {
    let map_idx = map as usize;
    let val_idx = val as usize;
    if map_idx == val_idx {
        return exec_map_set_interned(func, regs, map, key, val);
    }
    let key = func.consts[key as usize]
        .string_key_arcstr()
        .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
    if !matches!(regs[map_idx], Val::Map(_)) {
        return Err(anyhow!("MapSet target is not a Map"));
    }
    let value = std::mem::replace(&mut regs[val_idx], Val::Nil);
    match &mut regs[map_idx] {
        Val::Map(map) => insert_map_entry(map, key, value),
        _ => unreachable!("MapSet target was checked before moving value"),
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_int_arith(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if let (Val::Int(lhs), Val::Int(rhs)) = (&regs[a as usize], &regs[b as usize]) {
        let out = match op {
            PackedArithOp::Add => lhs + rhs,
            PackedArithOp::Sub => lhs - rhs,
            PackedArithOp::Mul => lhs * rhs,
            PackedArithOp::Mod => lhs % rhs,
            PackedArithOp::Div => {
                let out = BinOp::Div.eval_vals(&regs[a as usize], &regs[b as usize])?;
                assign_reg(frame_raw, regs, dst as usize, out);
                return Ok(());
            }
        };
        assign_reg(frame_raw, regs, dst as usize, Val::Int(out));
    } else {
        match op {
            PackedArithOp::Add => int_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x + y, BinOp::Add)?,
            PackedArithOp::Sub => int_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x - y, BinOp::Sub)?,
            PackedArithOp::Mul => int_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x * y, BinOp::Mul)?,
            PackedArithOp::Div => {
                let out = BinOp::Div.eval_vals(&regs[a as usize], &regs[b as usize])?;
                assign_reg(frame_raw, regs, dst as usize, out);
            }
            PackedArithOp::Mod => int_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x % y, BinOp::Mod)?,
        }
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_float_arith(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    let lhs = &regs[a as usize];
    let rhs = &regs[b as usize];
    let fast = match (lhs, rhs) {
        (Val::Float(lhs), Val::Float(rhs)) => Some((*lhs, *rhs)),
        (Val::Int(lhs), Val::Float(rhs)) => Some((*lhs as f64, *rhs)),
        (Val::Float(lhs), Val::Int(rhs)) => Some((*lhs, *rhs as f64)),
        (Val::Int(lhs), Val::Int(rhs)) => Some((*lhs as f64, *rhs as f64)),
        _ => None,
    };
    if let Some((lhs, rhs)) = fast {
        let out = match op {
            PackedArithOp::Add => lhs + rhs,
            PackedArithOp::Sub => lhs - rhs,
            PackedArithOp::Mul => lhs * rhs,
            PackedArithOp::Div => lhs / rhs,
            PackedArithOp::Mod => lhs % rhs,
        };
        assign_reg(frame_raw, regs, dst as usize, Val::Float(out));
    } else {
        match op {
            PackedArithOp::Add => float_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x + y, BinOp::Add)?,
            PackedArithOp::Sub => float_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x - y, BinOp::Sub)?,
            PackedArithOp::Mul => float_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x * y, BinOp::Mul)?,
            PackedArithOp::Div => float_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x / y, BinOp::Div)?,
            PackedArithOp::Mod => float_binop(frame_raw, regs, &func.consts, dst, a, b, |x, y| x % y, BinOp::Mod)?,
        }
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_floor(frame_raw: *mut FrameState<'_>, regs: &mut [Val], dst: u16, src: u16) {
    let out = match &regs[src as usize] {
        Val::Float(value) => Val::Int(value.floor() as i64),
        Val::Int(value) => Val::Int(*value),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline(always)]
pub(super) fn exec_floor_div_imm(frame_raw: *mut FrameState<'_>, regs: &mut [Val], dst: u16, src: u16, imm: i16) {
    let divisor = imm as i64;
    let out = match &regs[src as usize] {
        Val::Int(value) => Val::Int(floor_div_i64(*value, divisor)),
        Val::Float(value) => Val::Int((value / divisor as f64).floor() as i64),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline(always)]
pub(super) fn exec_starts_with_k(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    dst: u16,
    src: u16,
    key: u16,
) {
    let prefix = func.consts[key as usize].as_str().unwrap_or("");
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Bool(value.as_str().starts_with(prefix)),
        Val::Str(value) => Val::Bool(value.as_str().starts_with(prefix)),
        _ => Val::Bool(false),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline(always)]
pub(super) fn exec_contains_k(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    dst: u16,
    src: u16,
    key: u16,
) {
    let needle = func.consts[key as usize].as_str().unwrap_or("");
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Bool(value.as_str().contains(needle)),
        Val::Str(value) => Val::Bool(value.as_str().contains(needle)),
        _ => Val::Bool(false),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline(always)]
pub(super) fn exec_to_iter(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
) {
    let use_thread_local = region_plan
        .as_ref()
        .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
        .unwrap_or(false);
    let out = match &regs[src as usize] {
        value if matches!(value, Val::List(_)) || value.as_str().is_some() => regs[src as usize].clone(),
        Val::Map(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
            if use_thread_local && !entries.is_empty() {
                let allocator = region_allocator(region_allocator_ptr);
                allocator.with_val_buffer(entries.len(), |scratch| {
                    for (key, value) in entries.iter() {
                        scratch.push(Val::List(vec![Val::from_str(key.as_str()), (*value).clone()].into()));
                    }
                    let data = scratch.split_off(0);
                    Val::List(data.into())
                })
            } else {
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    pairs.push(Val::List(vec![Val::from_str(key.as_str()), value.clone()].into()));
                }
                Val::List(pairs.into())
            }
        }
        _ => Val::List(Vec::<Val>::new().into()),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}
