use super::super::helpers::assign_reg_with_metrics;
use super::*;
use crate::vm::{copy_const_value_for_register_with_metrics, copy_container_value_for_register_with_metrics};

#[inline(always)]
pub(super) fn exec_access_hot(
    regs: &mut [Val],
    access_ic: &mut [Option<AccessIc>],
    pc: usize,
    dst: u16,
    base: u16,
    field: u16,
    collect_metrics: bool,
) {
    let hit_val = match (&regs[base as usize], &regs[field as usize]) {
        (Val::List(list), Val::Int(index)) => {
            if *index < 0 {
                Some(Val::Nil)
            } else {
                Some(
                    list.get(*index as usize)
                        .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics))
                        .unwrap_or(Val::Nil),
                )
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
        (Val::Map(map), key) if key.as_str().is_some() => Val::map_get_str(map, key.as_str().unwrap())
            .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics)),
        (Val::Object(object), key) if key.as_str().is_some() => {
            let fields = &object.fields;
            let object_ptr = Arc::as_ptr(fields) as usize;
            let key = key.as_str().unwrap();
            match access_ic[pc].as_mut() {
                Some(AccessIc::ObjectStr(slots)) => {
                    Vm::lookup_promote(slots, |entry| entry.obj_ptr == object_ptr && entry.key.as_str() == key)
                        .map(|entry| copy_container_value_for_register_with_metrics(&entry.value, collect_metrics))
                }
                _ => None,
            }
        }
        _ => None,
    };
    let result = if let Some(value) = hit_val {
        value
    } else {
        let value = regs[base as usize]
            .access_with_metrics(&regs[field as usize], collect_metrics)
            .unwrap_or(Val::Nil);
        if let (Val::Object(object), field_val) = (&regs[base as usize], &regs[field as usize])
            && let Some(key) = field_val.as_str()
        {
            let fields = &object.fields;
            let object_ptr = Arc::as_ptr(fields) as usize;
            Vm::update_object_ic(access_ic, pc, object_ptr, key, &value, collect_metrics);
        }
        value
    };
    assign_reg_with_metrics(regs, dst as usize, result, collect_metrics);
}

#[inline(always)]
fn access_int_value(regs: &[Val], base: u16, field: u16) -> Option<i64> {
    match (&regs[base as usize], &regs[field as usize]) {
        (Val::List(list), Val::Int(index)) if *index >= 0 => match list.get(*index as usize) {
            Some(Val::Int(value)) => Some(*value),
            _ => None,
        },
        _ => None,
    }
}

#[inline(always)]
fn int_arith_value(op: PackedArithOp, lhs: i64, rhs: i64) -> Option<i64> {
    match op {
        PackedArithOp::Add => Some(lhs + rhs),
        PackedArithOp::Sub => Some(lhs - rhs),
        PackedArithOp::Mul => Some(lhs * rhs),
        PackedArithOp::Mod => Some(lhs % rhs),
        PackedArithOp::Div => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn exec_access_int_arith_hot(
    regs: &mut [Val],
    func: &Function,
    access_ic: &mut [Option<AccessIc>],
    pc: usize,
    access_dst: u16,
    base: u16,
    field: u16,
    arith_op: PackedArithOp,
    arith_dst: u16,
    arith_a: u16,
    arith_b: u16,
    collect_metrics: bool,
) -> Result<()> {
    let Some(access_value) = access_int_value(regs, base, field) else {
        exec_access_hot(regs, access_ic, pc, access_dst, base, field, collect_metrics);
        return exec_int_arith(regs, func, arith_op, arith_dst, arith_a, arith_b, collect_metrics);
    };

    assign_reg_with_metrics(regs, access_dst as usize, Val::Int(access_value), collect_metrics);
    let maybe_out = match (arith_a == access_dst, arith_b == access_dst) {
        (true, _) => match &regs[arith_b as usize] {
            Val::Int(rhs) => int_arith_value(arith_op, access_value, *rhs),
            _ => None,
        },
        (_, true) => match &regs[arith_a as usize] {
            Val::Int(lhs) => int_arith_value(arith_op, *lhs, access_value),
            _ => None,
        },
        _ => None,
    };
    if let Some(out) = maybe_out {
        assign_reg_with_metrics(regs, arith_dst as usize, Val::Int(out), collect_metrics);
        Ok(())
    } else {
        exec_int_arith(regs, func, arith_op, arith_dst, arith_a, arith_b, collect_metrics)
    }
}

#[inline(always)]
pub(super) fn exec_len(regs: &mut [Val], dst: u16, src: u16, collect_metrics: bool) {
    let out = match &regs[src as usize] {
        Val::List(value) => Val::Int(value.len() as i64),
        Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
        Val::Str(value) => Val::Int(value.len() as i64),
        Val::Map(value) => Val::Int(value.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline(always)]
pub(super) fn exec_index(
    regs: &mut [Val],
    index_ic: &mut [Option<IndexIc>],
    pc: usize,
    dst: u16,
    base: u16,
    idx: u16,
    collect_metrics: bool,
) {
    let out = match (&regs[base as usize], &regs[idx as usize]) {
        (Val::List(list), Val::Int(index)) => {
            if *index < 0 {
                list.len()
                    .checked_sub(index.unsigned_abs() as usize)
                    .and_then(|idx| {
                        list.get(idx)
                            .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics))
                    })
                    .unwrap_or(Val::Nil)
            } else {
                let list_ptr = Arc::as_ptr(list) as *const Val as usize;
                let hit = match index_ic[pc].as_mut() {
                    Some(IndexIc::List(slots)) => {
                        Vm::lookup_promote(slots, |entry| entry.base_ptr == list_ptr && entry.idx == *index)
                            .map(|entry| copy_container_value_for_register_with_metrics(&entry.value, collect_metrics))
                    }
                    _ => None,
                };
                if let Some(value) = hit {
                    value
                } else {
                    let value = list
                        .get(*index as usize)
                        .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics))
                        .unwrap_or(Val::Nil);
                    Vm::update_list_ic(index_ic, pc, list_ptr, *index, &value, collect_metrics);
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
                            .map(|entry| copy_container_value_for_register_with_metrics(&entry.value, collect_metrics))
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
                    Vm::update_str_ic(index_ic, pc, text_ptr, *index, &value, collect_metrics);
                    value
                }
            }
        }
        (base_val, key) => base_val.access_with_metrics(key, collect_metrics).unwrap_or(Val::Nil),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
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
pub(super) fn exec_map_set_interned(
    func: &Function,
    regs: &mut [Val],
    map: u16,
    key: u16,
    val: u16,
    collect_metrics: bool,
) -> Result<()> {
    let key = func.consts[key as usize]
        .string_key_arcstr()
        .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
    let value = copy_container_value_for_register_with_metrics(&regs[val as usize], collect_metrics);
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
    collect_metrics: bool,
) -> Result<()> {
    let map_idx = map as usize;
    let val_idx = val as usize;
    if map_idx == val_idx {
        return exec_map_set_interned(func, regs, map, key, val, collect_metrics);
    }
    let key = func.consts[key as usize]
        .string_key_arcstr()
        .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
    if !matches!(regs[map_idx], Val::Map(_)) {
        return Err(anyhow!("MapSet target is not a Map"));
    }
    let value = take_register_value(regs, val_idx);
    match &mut regs[map_idx] {
        Val::Map(map) => insert_map_entry(map, key, value),
        _ => unreachable!("MapSet target was checked before moving value"),
    }
    Ok(())
}

#[inline(always)]
pub(super) fn packed_value_operand(
    regs: &[Val],
    func: &Function,
    operand: PackedValueOperand,
    collect_metrics: bool,
) -> Val {
    match operand {
        PackedValueOperand::Reg(reg) => {
            copy_container_value_for_register_with_metrics(&regs[reg as usize], collect_metrics)
        }
        PackedValueOperand::Const(kidx) => {
            copy_const_value_for_register_with_metrics(&func.consts[kidx as usize], collect_metrics)
        }
    }
}

#[inline(always)]
pub(super) fn packed_add_operand_value(regs: &[Val], operand: PackedAddOperand, collect_metrics: bool) -> Val {
    match operand {
        PackedAddOperand::Reg(reg) => {
            copy_container_value_for_register_with_metrics(&regs[reg as usize], collect_metrics)
        }
        PackedAddOperand::Imm(value) => Val::Int(value as i64),
    }
}

#[inline(always)]
pub(super) fn exec_map_upsert_add(
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
    write_temps: bool,
    collect_metrics: bool,
) -> Result<()> {
    let _ = cmp_dst;
    let (is_nil, get_temp, value) = {
        let current_ref = match &regs[map as usize] {
            Val::Map(map) => lookup_key.as_ref().and_then(|key| Val::map_get_str(map, key.as_str())),
            _ => None,
        };
        let is_nil = current_ref.is_none();
        let get_temp = write_temps.then(|| {
            current_ref
                .map(|value| copy_container_value_for_register_with_metrics(value, collect_metrics))
                .unwrap_or(Val::Nil)
        });
        let value = if is_nil {
            packed_value_operand(regs, func, default, collect_metrics)
        } else {
            let rhs = packed_add_operand_value(regs, add_rhs, collect_metrics);
            let current = current_ref.expect("non-nil map lookup must have a value");
            match (current, &rhs) {
                (Val::Int(lhs), Val::Int(rhs)) => Val::Int(lhs + rhs),
                _ => BinOp::Add.eval_vals_with_metrics(current, &rhs, collect_metrics)?,
            }
        };
        (is_nil, get_temp, value)
    };
    if let Some(value) = get_temp {
        assign_reg_with_metrics(regs, get_dst as usize, value, collect_metrics);
    }
    if is_nil {
        if write_temps && let Some((reg, kidx)) = default_load {
            assign_reg_const_copy_with_metrics(regs, reg as usize, &func.consts[kidx as usize], collect_metrics);
        }
    } else if write_temps {
        assign_reg_copy_with_metrics(regs, add_dst as usize, &value, collect_metrics);
    }

    match &mut regs[map as usize] {
        Val::Map(map) => insert_map_entry(map, key, value),
        _ => return Err(anyhow!("MapSet target is not a Map")),
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_int_arith(
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    if let (Val::Int(lhs), Val::Int(rhs)) = (&regs[a as usize], &regs[b as usize]) {
        let out = match op {
            PackedArithOp::Add => lhs + rhs,
            PackedArithOp::Sub => lhs - rhs,
            PackedArithOp::Mul => lhs * rhs,
            PackedArithOp::Mod => lhs % rhs,
            PackedArithOp::Div => {
                let out = BinOp::Div.eval_vals_with_metrics(&regs[a as usize], &regs[b as usize], collect_metrics)?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
                return Ok(());
            }
        };
        assign_reg_with_metrics(regs, dst as usize, Val::Int(out), collect_metrics);
    } else {
        match op {
            PackedArithOp::Add => int_binop(regs, &func.consts, dst, a, b, |x, y| x + y, BinOp::Add, collect_metrics)?,
            PackedArithOp::Sub => int_binop(regs, &func.consts, dst, a, b, |x, y| x - y, BinOp::Sub, collect_metrics)?,
            PackedArithOp::Mul => int_binop(regs, &func.consts, dst, a, b, |x, y| x * y, BinOp::Mul, collect_metrics)?,
            PackedArithOp::Div => {
                let out = BinOp::Div.eval_vals_with_metrics(&regs[a as usize], &regs[b as usize], collect_metrics)?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            }
            PackedArithOp::Mod => int_binop(regs, &func.consts, dst, a, b, |x, y| x % y, BinOp::Mod, collect_metrics)?,
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn exec_sub_access_sub_hot(
    regs: &mut [Val],
    func: &Function,
    access_ic: &mut [Option<AccessIc>],
    access_pc: usize,
    first_dst: u16,
    first_a: u16,
    first_b: u16,
    access_dst: u16,
    access_base: u16,
    access_field: u16,
    final_dst: u16,
    final_a: u16,
    final_b: u16,
    collect_metrics: bool,
) -> Result<()> {
    exec_int_arith(
        regs,
        func,
        PackedArithOp::Sub,
        first_dst,
        first_a,
        first_b,
        collect_metrics,
    )?;
    let Some(access_value) = access_int_value(regs, access_base, access_field) else {
        exec_access_hot(
            regs,
            access_ic,
            access_pc,
            access_dst,
            access_base,
            access_field,
            collect_metrics,
        );
        return exec_int_arith(
            regs,
            func,
            PackedArithOp::Sub,
            final_dst,
            final_a,
            final_b,
            collect_metrics,
        );
    };

    assign_reg_with_metrics(regs, access_dst as usize, Val::Int(access_value), collect_metrics);
    let maybe_out = match (final_a == access_dst, final_b == access_dst) {
        (true, _) => match &regs[final_b as usize] {
            Val::Int(rhs) => Some(access_value - *rhs),
            _ => None,
        },
        (_, true) => match &regs[final_a as usize] {
            Val::Int(lhs) => Some(*lhs - access_value),
            _ => None,
        },
        _ => None,
    };
    if let Some(out) = maybe_out {
        assign_reg_with_metrics(regs, final_dst as usize, Val::Int(out), collect_metrics);
        Ok(())
    } else {
        exec_int_arith(
            regs,
            func,
            PackedArithOp::Sub,
            final_dst,
            final_a,
            final_b,
            collect_metrics,
        )
    }
}

#[inline(always)]
pub(super) fn exec_arith_add_int_imm(
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    arith_dst: u16,
    a: u16,
    b: u16,
    add_dst: u16,
    add_imm: i16,
    collect_metrics: bool,
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
            PackedArithOp::Add => BinOp::Add.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
            PackedArithOp::Sub => BinOp::Sub.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
            PackedArithOp::Mul => BinOp::Mul.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
            PackedArithOp::Mod => BinOp::Mod.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
            PackedArithOp::Div => BinOp::Div.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
        },
    };
    assign_reg_with_metrics(regs, arith_dst as usize, arith_value, collect_metrics);
    if let Val::Int(x) = regs[arith_dst as usize] {
        assign_reg_with_metrics(regs, add_dst as usize, Val::Int(x + add_imm as i64), collect_metrics);
    } else {
        int_binop_imm(
            regs,
            &func.consts,
            add_dst,
            arith_dst,
            add_imm,
            |x, y| x + y,
            BinOp::Add,
            collect_metrics,
        )?;
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_arith_hot(
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
) -> Result<()> {
    if let (Val::Int(x), Val::Int(y)) = (rk_read(regs, &func.consts, a), rk_read(regs, &func.consts, b)) {
        match op {
            PackedArithOp::Add => assign_reg_with_metrics(regs, dst as usize, Val::Int(*x + *y), collect_metrics),
            PackedArithOp::Sub => assign_reg_with_metrics(regs, dst as usize, Val::Int(*x - *y), collect_metrics),
            PackedArithOp::Mul => assign_reg_with_metrics(regs, dst as usize, Val::Int(*x * *y), collect_metrics),
            PackedArithOp::Div => {
                let res = *x as f64 / *y as f64;
                if res.fract() == 0.0 {
                    assign_reg_with_metrics(regs, dst as usize, Val::Int(res as i64), collect_metrics);
                } else {
                    assign_reg_with_metrics(regs, dst as usize, Val::Float(res), collect_metrics);
                }
            }
            PackedArithOp::Mod => assign_reg_with_metrics(regs, dst as usize, Val::Int(*x % *y), collect_metrics),
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
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            } else if let Some(b_str) = b_val.as_str()
                && let Some(out) = Val::concat_add_lhs_str(a_val, b_str)
            {
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            } else if !Vm::arith2_try_numeric(
                regs,
                &func.consts,
                dst,
                a,
                b,
                "add",
                |x, y| x + y,
                |x, y| x + y,
                collect_metrics,
            ) {
                let out = BinOp::Add.eval_vals_with_metrics(
                    rk_read(regs, &func.consts, a),
                    rk_read(regs, &func.consts, b),
                    collect_metrics,
                )?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            }
        }
        PackedArithOp::Sub => {
            if !Vm::arith2_try_numeric(
                regs,
                &func.consts,
                dst,
                a,
                b,
                "sub",
                |x, y| x - y,
                |x, y| x - y,
                collect_metrics,
            ) {
                let out = BinOp::Sub.eval_vals_with_metrics(
                    rk_read(regs, &func.consts, a),
                    rk_read(regs, &func.consts, b),
                    collect_metrics,
                )?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            }
        }
        PackedArithOp::Mul => {
            if !Vm::arith2_try_numeric(
                regs,
                &func.consts,
                dst,
                a,
                b,
                "mul",
                |x, y| x * y,
                |x, y| x * y,
                collect_metrics,
            ) {
                let out = BinOp::Mul.eval_vals_with_metrics(
                    rk_read(regs, &func.consts, a),
                    rk_read(regs, &func.consts, b),
                    collect_metrics,
                )?;
                assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
            }
        }
        PackedArithOp::Div => {
            let ar = rk_read(regs, &func.consts, a);
            let br = rk_read(regs, &func.consts, b);
            match (ar, br) {
                (Val::Int(x), Val::Int(y)) => {
                    let res = *x as f64 / *y as f64;
                    if res.fract() == 0.0 {
                        assign_reg_with_metrics(regs, dst as usize, Val::Int(res as i64), collect_metrics);
                    } else {
                        assign_reg_with_metrics(regs, dst as usize, Val::Float(res), collect_metrics);
                    }
                }
                (Val::Float(x), Val::Float(y)) => {
                    assign_reg_with_metrics(regs, dst as usize, Val::Float(x / y), collect_metrics)
                }
                (Val::Int(x), Val::Float(y)) => {
                    assign_reg_with_metrics(regs, dst as usize, Val::Float(*x as f64 / y), collect_metrics)
                }
                (Val::Float(x), Val::Int(y)) => {
                    assign_reg_with_metrics(regs, dst as usize, Val::Float(x / *y as f64), collect_metrics)
                }
                _ => {
                    let out = BinOp::Div.eval_vals_with_metrics(ar, br, collect_metrics)?;
                    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
                }
            }
        }
        PackedArithOp::Mod => {
            let out = BinOp::Mod.eval_vals_with_metrics(
                rk_read(regs, &func.consts, a),
                rk_read(regs, &func.consts, b),
                collect_metrics,
            )?;
            assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
        }
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_float_arith(
    regs: &mut [Val],
    func: &Function,
    op: PackedArithOp,
    dst: u16,
    a: u16,
    b: u16,
    collect_metrics: bool,
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
        assign_reg_with_metrics(regs, dst as usize, Val::Float(out), collect_metrics);
    } else {
        match op {
            PackedArithOp::Add => {
                float_binop(regs, &func.consts, dst, a, b, |x, y| x + y, BinOp::Add, collect_metrics)?
            }
            PackedArithOp::Sub => {
                float_binop(regs, &func.consts, dst, a, b, |x, y| x - y, BinOp::Sub, collect_metrics)?
            }
            PackedArithOp::Mul => {
                float_binop(regs, &func.consts, dst, a, b, |x, y| x * y, BinOp::Mul, collect_metrics)?
            }
            PackedArithOp::Div => {
                float_binop(regs, &func.consts, dst, a, b, |x, y| x / y, BinOp::Div, collect_metrics)?
            }
            PackedArithOp::Mod => {
                float_binop(regs, &func.consts, dst, a, b, |x, y| x % y, BinOp::Mod, collect_metrics)?
            }
        }
    }
    Ok(())
}

#[inline(always)]
pub(super) fn exec_floor(regs: &mut [Val], dst: u16, src: u16, collect_metrics: bool) {
    let out = match &regs[src as usize] {
        Val::Float(value) => Val::Int(value.floor() as i64),
        Val::Int(value) => Val::Int(*value),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline(always)]
pub(super) fn exec_floor_div_imm(regs: &mut [Val], dst: u16, src: u16, imm: i16, collect_metrics: bool) {
    let divisor = imm as i64;
    let out = match &regs[src as usize] {
        Val::Int(value) => Val::Int(floor_div_i64(*value, divisor)),
        Val::Float(value) => Val::Int((value / divisor as f64).floor() as i64),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline(always)]
pub(super) fn exec_starts_with_k(
    regs: &mut [Val],
    func: &Function,
    dst: u16,
    src: u16,
    key: u16,
    collect_metrics: bool,
) {
    assign_reg_with_metrics(
        regs,
        dst as usize,
        Val::Bool(starts_with_k_bool(regs, func, src, key)),
        collect_metrics,
    );
}

#[inline(always)]
pub(super) fn exec_contains_k(regs: &mut [Val], func: &Function, dst: u16, src: u16, key: u16, collect_metrics: bool) {
    assign_reg_with_metrics(
        regs,
        dst as usize,
        Val::Bool(contains_k_bool(regs, func, src, key)),
        collect_metrics,
    );
}

#[inline(always)]
pub(super) fn starts_with_k_bool(regs: &[Val], func: &Function, src: u16, key: u16) -> bool {
    let prefix = func.consts[key as usize].as_str().unwrap_or("");
    match &regs[src as usize] {
        Val::ShortStr(value) => value.as_str().starts_with(prefix),
        Val::Str(value) => value.as_str().starts_with(prefix),
        _ => false,
    }
}

#[inline(always)]
pub(super) fn contains_k_bool(regs: &[Val], func: &Function, src: u16, key: u16) -> bool {
    let needle = func.consts[key as usize].as_str().unwrap_or("");
    match &regs[src as usize] {
        Val::ShortStr(value) => value.as_str().contains(needle),
        Val::Str(value) => value.as_str().contains(needle),
        _ => false,
    }
}

#[inline(always)]
pub(super) fn exec_to_iter(
    regs: &mut [Val],
    dst: u16,
    src: u16,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
    collect_metrics: bool,
) {
    let use_thread_local = region_plan
        .as_ref()
        .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
        .unwrap_or(false);
    let out = match &regs[src as usize] {
        value if matches!(value, Val::List(_)) || value.as_str().is_some() => {
            copy_container_value_for_register_with_metrics(value, collect_metrics)
        }
        Val::Map(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
            if use_thread_local && !entries.is_empty() {
                let allocator = region_allocator(region_allocator_ptr);
                allocator.with_val_buffer(entries.len(), |scratch| {
                    for (key, value) in entries.iter() {
                        scratch.push(Val::List(
                            vec![
                                Val::from_str(key.as_str()),
                                copy_container_value_for_register_with_metrics(value, collect_metrics),
                            ]
                            .into(),
                        ));
                    }
                    let data = scratch.split_off(0);
                    Val::List(data.into())
                })
            } else {
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    pairs.push(Val::List(
                        vec![
                            Val::from_str(key.as_str()),
                            copy_container_value_for_register_with_metrics(value, collect_metrics),
                        ]
                        .into(),
                    ));
                }
                Val::List(pairs.into())
            }
        }
        _ => Val::List(Vec::<Val>::new().into()),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}
