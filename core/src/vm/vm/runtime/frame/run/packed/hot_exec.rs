use super::*;

#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(super) fn exec_hot_slot(
    entry: &PackedHotSlot,
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    func: &Function,
    ctx: &mut VmContext,
    global_ic: &mut [Option<GlobalEntry>],
    call_ic: &[Option<CallIc>],
    for_range_ic: &mut [Option<ForRangeState>],
    pc: usize,
    frame_base: usize,
) -> Result<Option<usize>> {
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
        PackedHotKind::ForRangeLoop {
            idx,
            write_idx,
            ofs,
            fusion,
        } => {
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
                let fused_next_pc = match fusion {
                    Some(PackedRangeFusion::AddModulo {
                        acc,
                        modulo,
                        step_pc,
                        back_ofs,
                    }) => {
                        let acc_value = match &regs[*acc as usize] {
                            Val::Int(value) => Some(*value),
                            _ => None,
                        };
                        let modulo = match func.consts.get(rk_index(*modulo) as usize) {
                            Some(Val::Int(value)) if *value != 0 => Some(*value),
                            _ => None,
                        };
                        match (acc_value, modulo) {
                            (Some(acc_value), Some(modulo)) => {
                                regs[*acc as usize] = Val::Int(acc_value.wrapping_add(current % modulo));
                                Some(((*step_pc as isize) + (*back_ofs as isize)) as usize)
                            }
                            _ => None,
                        }
                    }
                    Some(PackedRangeFusion::TinyAddModCall {
                        func: call_func,
                        acc,
                        modulo,
                        call_pc,
                        step_pc,
                        back_ofs,
                    }) => {
                        let acc_value = match &regs[*acc as usize] {
                            Val::Int(value) => Some(*value),
                            _ => None,
                        };
                        let modulo = match func.consts.get(rk_index(*modulo) as usize) {
                            Some(Val::Int(value)) if *value != 0 => Some(*value),
                            _ => None,
                        };
                        match (acc_value, modulo, call_ic.get(*call_pc).and_then(Option::as_ref)) {
                            (
                                Some(acc_value),
                                Some(modulo),
                                Some(CallIc::ClosurePositional {
                                    closure_ptr,
                                    fun_ptr,
                                    argc: 2,
                                    tiny: Some(tiny),
                                    ..
                                }),
                            ) => {
                                if let Val::Closure(arc) = &regs[*call_func as usize] {
                                    let closure_matches = Arc::as_ptr(arc) as usize == *closure_ptr
                                        || arc
                                            .code
                                            .get()
                                            .map(|fun| std::ptr::eq(Arc::as_ptr(fun), *fun_ptr))
                                            .unwrap_or(false);
                                    if closure_matches
                                        && let Some(out) = tiny.try_eval_add_mod_int_params(acc_value, current % modulo)
                                    {
                                        regs[*acc as usize] = Val::Int(out);
                                        Some(((*step_pc as isize) + (*back_ofs as isize)) as usize)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    }
                    Some(PackedRangeFusion::TinyIntCall3 {
                        func: call_func,
                        acc,
                        modulo,
                        call_pc,
                        step_pc,
                        back_ofs,
                    }) => {
                        let acc_value = match &regs[*acc as usize] {
                            Val::Int(value) => Some(*value),
                            _ => None,
                        };
                        let modulo = match func.consts.get(rk_index(*modulo) as usize) {
                            Some(Val::Int(value)) if *value != 0 => Some(*value),
                            _ => None,
                        };
                        match (acc_value, modulo, call_ic.get(*call_pc).and_then(Option::as_ref)) {
                            (
                                Some(acc_value),
                                Some(modulo),
                                Some(CallIc::ClosurePositional {
                                    closure_ptr,
                                    fun_ptr,
                                    argc: 3,
                                    tiny: Some(tiny),
                                    ..
                                }),
                            ) => {
                                if let Val::Closure(arc) = &regs[*call_func as usize] {
                                    let closure_matches = Arc::as_ptr(arc) as usize == *closure_ptr
                                        || arc
                                            .code
                                            .get()
                                            .map(|fun| std::ptr::eq(Arc::as_ptr(fun), *fun_ptr))
                                            .unwrap_or(false);
                                    if closure_matches
                                        && let Some(out) =
                                            tiny.try_eval_int3_params(acc_value, current, current % modulo)
                                    {
                                        regs[*acc as usize] = Val::Int(out);
                                        Some(((*step_pc as isize) + (*back_ofs as isize)) as usize)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    }
                    None => None,
                };
                if let Some(next_pc) = fused_next_pc {
                    state_entry.current += state_entry.step;
                    return Ok(Some(next_pc));
                }
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
        PackedHotKind::ForRangeStep { back_ofs } => {
            let guard_pc = ((pc as isize) + (*back_ofs as isize)) as usize;
            Some(guard_pc)
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
                && let Some(value) = Val::concat_str_add_rhs(lhs_str, &regs[*src as usize])
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
        PackedHotKind::Jmp { ofs } => Some(((pc as isize) + (*ofs as isize)) as usize),
        PackedHotKind::JmpFalse { r, ofs } => {
            if matches!(regs[*r as usize], Val::Nil | Val::Bool(false)) {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::Ret { .. } => unreachable!("Ret is handled directly by run_packed_code"),
        PackedHotKind::ListPush { list, val } => {
            let pushed_val = regs[*val as usize].clone();
            match &mut regs[*list as usize] {
                Val::List(arc) => {
                    push_list_entry(arc, pushed_val);
                }
                _ => return Err(anyhow!("ListPush target is not a List")),
            }
            None
        }
        PackedHotKind::MapSet { map, key, val } => {
            let key_arc = match &regs[*key as usize] {
                Val::Str(s) => s.clone(),
                Val::ShortStr(s) => Val::intern_str(s.as_str()),
                _ => return Err(anyhow!("MapSet key must be a String")),
            };
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
            let map_idx = *map as usize;
            let key_idx = *key as usize;
            let val_idx = *val as usize;
            if map_idx == key_idx || map_idx == val_idx || key_idx == val_idx {
                let key_arc = match &regs[key_idx] {
                    Val::Str(s) => s.clone(),
                    Val::ShortStr(s) => Val::intern_str(s.as_str()),
                    _ => return Err(anyhow!("MapSet key must be a String")),
                };
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
            let key_arc = match key_val {
                Val::Str(s) => s,
                Val::ShortStr(s) => Val::intern_str(s.as_str()),
                other => {
                    regs[key_idx] = other;
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
        PackedHotKind::CmpLtImmJmp { r, imm, ofs } => {
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
