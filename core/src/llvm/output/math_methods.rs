use crate::llvm::{
    ir_text::llvm_float_literal,
    straightline_value::{NativeStraightlineValue, NativeTextPart},
};

pub(super) fn emit_native_math_module_method(
    method: &str,
    args: &[NativeStraightlineValue],
) -> Option<NativeStraightlineValue> {
    match method {
        "tan" => static_f64_unary(args, f64::tan),
        "asin" => {
            let value = static_f64_arg(args.first()?)?;
            (-1.0..=1.0)
                .contains(&value)
                .then(|| NativeStraightlineValue::F64(llvm_float_literal(value.asin())))
        }
        "acos" => {
            let value = static_f64_arg(args.first()?)?;
            (-1.0..=1.0)
                .contains(&value)
                .then(|| NativeStraightlineValue::F64(llvm_float_literal(value.acos())))
        }
        "atan" => static_f64_unary(args, f64::atan),
        "log" => static_positive_f64_unary(args, f64::ln),
        "log10" => static_positive_f64_unary(args, f64::log10),
        "log2" => static_positive_f64_unary(args, f64::log2),
        "cbrt" => static_f64_unary(args, f64::cbrt),
        "sinh" => static_f64_unary(args, f64::sinh),
        "cosh" => static_f64_unary(args, f64::cosh),
        "tanh" => static_f64_unary(args, f64::tanh),
        "fract" => static_f64_unary(args, f64::fract),
        "atan2" => static_f64_binary(args, f64::atan2),
        "hypot" => static_f64_binary(args, f64::hypot),
        "trunc" => static_trunc(args),
        "sign" => static_sign(args),
        "clamp" => static_clamp(args),
        "to_int" => static_to_int(args),
        "to_float" => static_to_float(args),
        "is_nan" => static_is_nan(args),
        "is_inf" => static_is_inf(args),
        "random" => None,
        _ => None,
    }
}

fn static_f64_unary(args: &[NativeStraightlineValue], op: fn(f64) -> f64) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    Some(NativeStraightlineValue::F64(llvm_float_literal(op(static_f64_arg(
        args.first()?,
    )?))))
}

fn static_positive_f64_unary(args: &[NativeStraightlineValue], op: fn(f64) -> f64) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    let value = static_f64_arg(args.first()?)?;
    (value > 0.0).then(|| NativeStraightlineValue::F64(llvm_float_literal(op(value))))
}

fn static_f64_binary(args: &[NativeStraightlineValue], op: fn(f64, f64) -> f64) -> Option<NativeStraightlineValue> {
    (args.len() == 2).then(|| ())?;
    Some(NativeStraightlineValue::F64(llvm_float_literal(op(
        static_f64_arg(args.first()?)?,
        static_f64_arg(args.get(1)?)?,
    ))))
}

fn static_trunc(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    match args.first()? {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            Some(NativeStraightlineValue::I64(value.clone()))
        }
        value => Some(NativeStraightlineValue::F64(llvm_float_literal(
            static_f64_arg(value)?.trunc(),
        ))),
    }
}

fn static_sign(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    match args.first()? {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            let value = value.parse::<i64>().ok()?;
            Some(NativeStraightlineValue::I64(value.signum().to_string()))
        }
        value => {
            let value = static_f64_arg(value)?;
            let sign = if value > 0.0 {
                1.0
            } else if value < 0.0 {
                -1.0
            } else {
                0.0
            };
            Some(NativeStraightlineValue::F64(llvm_float_literal(sign)))
        }
    }
}

fn static_clamp(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    (1..=3).contains(&args.len()).then(|| ())?;
    let value = static_i64_arg(args.first()?)?;
    let min = match args.get(1) {
        Some(value) => static_i64_arg(value)?,
        None => 0,
    };
    let max = match args.get(2) {
        Some(value) => static_i64_arg(value)?,
        None => 100,
    };
    (min <= max).then(|| NativeStraightlineValue::I64(value.clamp(min, max).to_string()))
}

fn static_to_int(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    let value = match args.first()? {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => value.parse().ok()?,
        NativeStraightlineValue::F64(_) => static_f64_arg(args.first()?)? as i64,
        NativeStraightlineValue::Bool(value) if !value.starts_with('%') => i64::from(value != "0"),
        _ => return None,
    };
    Some(NativeStraightlineValue::I64(value.to_string()))
}

fn static_to_float(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    let value = match args.first()? {
        NativeStraightlineValue::Bool(value) if !value.starts_with('%') => {
            if value == "0" {
                0.0
            } else {
                1.0
            }
        }
        value => static_f64_arg(value)?,
    };
    Some(NativeStraightlineValue::F64(llvm_float_literal(value)))
}

fn static_is_nan(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    Some(NativeStraightlineValue::Bool(
        i64::from(static_f64_arg(args.first()?)?.is_nan()).to_string(),
    ))
}

fn static_is_inf(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    (args.len() == 1).then(|| ())?;
    Some(NativeStraightlineValue::Bool(
        i64::from(static_f64_arg(args.first()?)?.is_infinite()).to_string(),
    ))
}

fn static_i64_arg(value: &NativeStraightlineValue) -> Option<i64> {
    match value {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => value.parse().ok(),
        _ => None,
    }
}

fn static_f64_arg(value: &NativeStraightlineValue) -> Option<f64> {
    match value {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            value.parse::<i64>().ok().map(|value| value as f64)
        }
        NativeStraightlineValue::F64(value) if value == "0x7FF8000000000000" => Some(f64::NAN),
        NativeStraightlineValue::F64(value) if value == "0x7FF0000000000000" => Some(f64::INFINITY),
        NativeStraightlineValue::F64(value) if value == "0xFFF0000000000000" => Some(f64::NEG_INFINITY),
        NativeStraightlineValue::F64(value) if !value.starts_with('%') => value.parse().ok(),
        NativeStraightlineValue::Text(parts) => static_text_f64(parts),
        _ => None,
    }
}

fn static_text_f64(parts: &[NativeTextPart]) -> Option<f64> {
    let [part] = parts else {
        return None;
    };
    match part {
        NativeTextPart::I64(value) => value.parse::<i64>().ok().map(|value| value as f64),
        NativeTextPart::F64(value) => value.parse().ok(),
        _ => None,
    }
}
