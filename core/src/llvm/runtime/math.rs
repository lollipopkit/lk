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
        match runtime_binop(&left, &right, op.clone(), state) {
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
    let RuntimeVal::Int(left) = encoding::decode_immediate(lhs) else {
        return None;
    };
    let RuntimeVal::Int(right) = encoding::decode_immediate(rhs) else {
        return None;
    };
    let value = match op {
        BinOp::Add => left + right,
        BinOp::Sub => left - right,
        BinOp::Mul => left * right,
        BinOp::Mod => left % right,
        _ => return None,
    };
    encoding::encode_immediate(&RuntimeVal::Int(value)).ok()
}

fn runtime_binop(left: &RuntimeVal, right: &RuntimeVal, op: BinOp, state: &mut RuntimeState) -> Result<RuntimeVal> {
    match op {
        BinOp::Add => match (left, right) {
            (RuntimeVal::Int(a), RuntimeVal::Int(b)) => Ok(RuntimeVal::Int(a + b)),
            (RuntimeVal::Float(a), RuntimeVal::Float(b)) => Ok(RuntimeVal::Float(a + b)),
            (RuntimeVal::Float(a), RuntimeVal::Int(b)) => Ok(RuntimeVal::Float(a + *b as f64)),
            (RuntimeVal::Int(a), RuntimeVal::Float(b)) => Ok(RuntimeVal::Float(*a as f64 + b)),
            _ => {
                let Some(lhs) = state.runtime_string(left) else {
                    return Err(anyhow!("Add expected numbers or strings"));
                };
                let Some(rhs) = state.runtime_string(right) else {
                    return Err(anyhow!("Add expected numbers or strings"));
                };
                Ok(state.runtime_string_value(format!("{lhs}{rhs}")))
            }
        },
        BinOp::Sub => numeric_binop(
            left,
            right,
            |a, b| RuntimeVal::Int(a - b),
            |a, b| RuntimeVal::Float(a - b),
            "Sub",
        ),
        BinOp::Mul => numeric_binop(
            left,
            right,
            |a, b| RuntimeVal::Int(a * b),
            |a, b| RuntimeVal::Float(a * b),
            "Mul",
        ),
        BinOp::Div => numeric_binop(
            left,
            right,
            |a, b| {
                let value = a as f64 / b as f64;
                if value.fract() == 0.0 {
                    RuntimeVal::Int(value as i64)
                } else {
                    RuntimeVal::Float(value)
                }
            },
            |a, b| RuntimeVal::Float(a / b),
            "Div",
        ),
        BinOp::Mod => numeric_binop(
            left,
            right,
            |a, b| RuntimeVal::Int(a % b),
            |a, b| RuntimeVal::Float(a % b),
            "Mod",
        ),
        _ => Err(anyhow!("unsupported LLVM runtime binary op {:?}", op)),
    }
}

fn numeric_binop(
    left: &RuntimeVal,
    right: &RuntimeVal,
    int_op: impl FnOnce(i64, i64) -> RuntimeVal,
    float_op: impl FnOnce(f64, f64) -> RuntimeVal,
    name: &str,
) -> Result<RuntimeVal> {
    match (left, right) {
        (RuntimeVal::Int(a), RuntimeVal::Int(b)) => Ok(int_op(*a, *b)),
        (RuntimeVal::Float(a), RuntimeVal::Float(b)) => Ok(float_op(*a, *b)),
        (RuntimeVal::Float(a), RuntimeVal::Int(b)) => Ok(float_op(*a, *b as f64)),
        (RuntimeVal::Int(a), RuntimeVal::Float(b)) => Ok(float_op(*a as f64, *b)),
        _ => Err(anyhow!("{name} expected numbers")),
    }
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
