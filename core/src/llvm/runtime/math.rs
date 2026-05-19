use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_add(lhs: i64, rhs: i64) -> i64 {
    lk_rt_binop(lhs, rhs, BinOp::Add)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_sub(lhs: i64, rhs: i64) -> i64 {
    lk_rt_binop(lhs, rhs, BinOp::Sub)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_mul(lhs: i64, rhs: i64) -> i64 {
    lk_rt_binop(lhs, rhs, BinOp::Mul)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_div(lhs: i64, rhs: i64) -> i64 {
    lk_rt_binop(lhs, rhs, BinOp::Div)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_mod(lhs: i64, rhs: i64) -> i64 {
    lk_rt_binop(lhs, rhs, BinOp::Mod)
}

fn lk_rt_binop(lhs: i64, rhs: i64, op: BinOp) -> i64 {
    with_state(|state| {
        if let Some(value) = immediate_int_binop(state, lhs, rhs, &op) {
            return value;
        }
        let left = state.decode_value(lhs);
        let right = state.decode_value(rhs);
        match op.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("{} error: {err}", binop_helper_name(op));
                encoding::NIL_VALUE
            }
        }
    })
}

fn immediate_int_binop(state: &RuntimeState, lhs: i64, rhs: i64, op: &BinOp) -> Option<i64> {
    if state.handles.get_ref(lhs).is_some() || state.handles.get_ref(rhs).is_some() {
        return None;
    }
    let Val::Int(left) = encoding::decode_immediate(lhs) else {
        return None;
    };
    let Val::Int(right) = encoding::decode_immediate(rhs) else {
        return None;
    };
    let value = match op {
        BinOp::Add => left + right,
        BinOp::Sub => left - right,
        BinOp::Mul => left * right,
        BinOp::Mod => left % right,
        _ => return None,
    };
    encoding::encode_immediate(&Val::Int(value)).ok()
}

fn binop_helper_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "lk_rt_add",
        BinOp::Sub => "lk_rt_sub",
        BinOp::Mul => "lk_rt_mul",
        BinOp::Div => "lk_rt_div",
        BinOp::Mod => "lk_rt_mod",
        _ => "lk_rt_binop",
    }
}
