use super::*;

#[inline(always)]
pub(super) fn exec_access_hot(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    access_ic: &mut [Option<AccessIc>],
    pc: usize,
    dst: u16,
    base: u16,
    field: u16,
) {
    let hit_val = match (&regs[base as usize], &regs[field as usize]) {
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
        let value = regs[base as usize].access(&regs[field as usize]).unwrap_or(Val::Nil);
        if let (Val::Object(object), field_val) = (&regs[base as usize], &regs[field as usize])
            && let Some(key) = field_val.as_str()
        {
            let fields = &object.fields;
            let object_ptr = Arc::as_ptr(fields) as usize;
            Vm::update_object_ic(access_ic, pc, object_ptr, key, &value);
        }
        value
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

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
pub(super) fn packed_value_operand(regs: &[Val], func: &Function, operand: PackedValueOperand) -> Val {
    match operand {
        PackedValueOperand::Reg(reg) => regs[reg as usize].clone(),
        PackedValueOperand::Const(kidx) => func.consts[kidx as usize].clone(),
    }
}

#[inline(always)]
pub(super) fn packed_add_operand_value(regs: &[Val], operand: PackedAddOperand) -> Val {
    match operand {
        PackedAddOperand::Reg(reg) => regs[reg as usize].clone(),
        PackedAddOperand::Imm(value) => Val::Int(value as i64),
    }
}

#[inline(always)]
pub(super) fn exec_map_upsert_add(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    get_dst: u16,
    cmp_dst: u16,
    map: u16,
    lookup_key: Option<ArcStr>,
    key: ArcStr,
    default: PackedValueOperand,
    default_load: Option<(u16, u16)>,
    add_dst: u16,
    add_rhs: PackedAddOperand,
) -> Result<()> {
    let current = match &regs[map as usize] {
        Val::Map(map) => lookup_key
            .as_ref()
            .and_then(|key| Val::map_get_str(map, key.as_str()).cloned())
            .unwrap_or(Val::Nil),
        _ => Val::Nil,
    };
    assign_reg(frame_raw, regs, get_dst as usize, current);
    let is_nil = matches!(regs[get_dst as usize], Val::Nil);
    let _ = cmp_dst;

    let value = if is_nil {
        let value = packed_value_operand(regs, func, default);
        if let Some((reg, kidx)) = default_load {
            assign_reg(frame_raw, regs, reg as usize, func.consts[kidx as usize].clone());
        }
        value
    } else {
        let rhs = packed_add_operand_value(regs, add_rhs);
        let current = &regs[get_dst as usize];
        let value = match (current, &rhs) {
            (Val::Int(lhs), Val::Int(rhs)) => Val::Int(lhs + rhs),
            _ => BinOp::Add.eval_vals(current, &rhs)?,
        };
        assign_reg(frame_raw, regs, add_dst as usize, value.clone());
        value
    };

    match &mut regs[map as usize] {
        Val::Map(map) => insert_map_entry(map, key, value),
        _ => return Err(anyhow!("MapSet target is not a Map")),
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
pub(super) fn exec_arith_add_int_imm(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    arith_dst: u16,
    a: u16,
    b: u16,
    add_dst: u16,
    add_imm: i16,
) -> Result<()> {
    let arith_value = match (rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b)) {
        (Val::Int(x), Val::Int(y)) => match op {
            PackedArithOp::Add => Val::Int(x + y),
            PackedArithOp::Sub => Val::Int(x - y),
            PackedArithOp::Mul => Val::Int(x * y),
            PackedArithOp::Mod => Val::Int(x % y),
            PackedArithOp::Div => {
                let res = *x as f64 / *y as f64;
                if res.fract() == 0.0 {
                    Val::Int(res as i64)
                } else {
                    Val::Float(res)
                }
            }
        },
        (lhs, rhs) => match op {
            PackedArithOp::Add => BinOp::Add.eval_vals(lhs, rhs)?,
            PackedArithOp::Sub => BinOp::Sub.eval_vals(lhs, rhs)?,
            PackedArithOp::Mul => BinOp::Mul.eval_vals(lhs, rhs)?,
            PackedArithOp::Mod => BinOp::Mod.eval_vals(lhs, rhs)?,
            PackedArithOp::Div => BinOp::Div.eval_vals(lhs, rhs)?,
        },
    };
    assign_reg(frame_raw, regs, arith_dst as usize, arith_value);
    if let Val::Int(x) = regs[arith_dst as usize] {
        assign_reg(frame_raw, regs, add_dst as usize, Val::Int(x + add_imm as i64));
    } else {
        int_binop_imm(
            frame_raw,
            regs,
            &func.consts,
            add_dst,
            arith_dst,
            add_imm,
            |x, y| x + y,
            BinOp::Add,
        )?;
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_arith_hot(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    dst: u16,
    a: u16,
    b: u16,
) -> Result<()> {
    if let (Val::Int(x), Val::Int(y)) = (rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b)) {
        match op {
            PackedArithOp::Add => assign_reg(frame_raw, regs, dst as usize, Val::Int(*x + *y)),
            PackedArithOp::Sub => assign_reg(frame_raw, regs, dst as usize, Val::Int(*x - *y)),
            PackedArithOp::Mul => assign_reg(frame_raw, regs, dst as usize, Val::Int(*x * *y)),
            PackedArithOp::Div => {
                let res = *x as f64 / *y as f64;
                if res.fract() == 0.0 {
                    assign_reg(frame_raw, regs, dst as usize, Val::Int(res as i64));
                } else {
                    assign_reg(frame_raw, regs, dst as usize, Val::Float(res));
                }
            }
            PackedArithOp::Mod => assign_reg(frame_raw, regs, dst as usize, Val::Int(*x % *y)),
        }
        return Ok(());
    }

    match op {
        PackedArithOp::Add => {
            let a_val = rk_read(regs, &func.consts, a);
            let b_val = rk_read(regs, &func.consts, b);
            if let Some(a_str) = a_val.as_str()
                && let Some(out) = Val::concat_str_add_rhs(a_str, b_val)
            {
                assign_reg(frame_raw, regs, dst as usize, out);
            } else if let Some(b_str) = b_val.as_str()
                && let Some(out) = Val::concat_add_lhs_str(a_val, b_str)
            {
                assign_reg(frame_raw, regs, dst as usize, out);
            } else if !Vm::arith2_try_numeric(
                frame_raw,
                regs,
                &func.consts,
                dst,
                a,
                b,
                "add",
                |x, y| x + y,
                |x, y| x + y,
            ) {
                let out = BinOp::Add.eval_vals(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, out);
            }
        }
        PackedArithOp::Sub => {
            if !Vm::arith2_try_numeric(
                frame_raw,
                regs,
                &func.consts,
                dst,
                a,
                b,
                "sub",
                |x, y| x - y,
                |x, y| x - y,
            ) {
                let out = BinOp::Sub.eval_vals(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, out);
            }
        }
        PackedArithOp::Mul => {
            if !Vm::arith2_try_numeric(
                frame_raw,
                regs,
                &func.consts,
                dst,
                a,
                b,
                "mul",
                |x, y| x * y,
                |x, y| x * y,
            ) {
                let out = BinOp::Mul.eval_vals(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
                assign_reg(frame_raw, regs, dst as usize, out);
            }
        }
        PackedArithOp::Div => {
            let ar = rk_read(regs, &func.consts, a);
            let br = rk_read(regs, &func.consts, b);
            match (ar, br) {
                (Val::Int(x), Val::Int(y)) => {
                    let res = *x as f64 / *y as f64;
                    if res.fract() == 0.0 {
                        assign_reg(frame_raw, regs, dst as usize, Val::Int(res as i64));
                    } else {
                        assign_reg(frame_raw, regs, dst as usize, Val::Float(res));
                    }
                }
                (Val::Float(x), Val::Float(y)) => assign_reg(frame_raw, regs, dst as usize, Val::Float(x / y)),
                (Val::Int(x), Val::Float(y)) => assign_reg(frame_raw, regs, dst as usize, Val::Float(*x as f64 / y)),
                (Val::Float(x), Val::Int(y)) => assign_reg(frame_raw, regs, dst as usize, Val::Float(x / *y as f64)),
                _ => {
                    let out = BinOp::Div.eval_vals(ar, br)?;
                    assign_reg(frame_raw, regs, dst as usize, out);
                }
            }
        }
        PackedArithOp::Mod => {
            let out = BinOp::Mod.eval_vals(rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b))?;
            assign_reg(frame_raw, regs, dst as usize, out);
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
