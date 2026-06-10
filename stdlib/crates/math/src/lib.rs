use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime},
};
use std::collections::HashSet;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "math", docs = "Mathematical functions and constants")]
pub struct MathModule;

#[lk_stdlib_common::stdlib_exports(module = "math")]
#[stdlib_value("pi" => RuntimeVal::Float(std::f64::consts::PI))]
#[stdlib_value("e" => RuntimeVal::Float(std::f64::consts::E))]
#[stdlib_value("inf" => RuntimeVal::Float(f64::INFINITY))]
#[stdlib_value("nan" => RuntimeVal::Float(f64::NAN))]
#[stdlib_value("max_int" => RuntimeVal::Int(i64::MAX))]
#[stdlib_value("min_int" => RuntimeVal::Int(i64::MIN))]
#[stdlib_value("max_float" => RuntimeVal::Float(f64::MAX))]
#[stdlib_value("epsilon" => RuntimeVal::Float(f64::EPSILON))]
impl MathModule {
    #[stdlib_export(params(value: Int, min?: Int = 0, max?: Int = 100), named(min, max), returns = Int)]
    fn clamp(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let pos = args.as_slice();
        if pos.is_empty() {
            bail!("clamp() requires at least the value argument");
        }
        if pos.len() > 3 {
            bail!("clamp() takes at most 3 positional arguments: value, min, max");
        }

        let value = int_arg(&pos[0], "clamp() first argument (value)")?;
        let mut min = if pos.len() >= 2 {
            int_arg(&pos[1], "clamp() second argument (min)")?
        } else {
            0
        };
        let mut max = if pos.len() >= 3 {
            int_arg(&pos[2], "clamp() third argument (max)")?
        } else {
            100
        };

        let mut seen = HashSet::with_capacity(args.named_len());
        args.try_for_each_named(runtime.heap(), |name, value| {
            if !seen.insert(name.to_string()) {
                bail!("clamp() received duplicate named argument '{}'", name);
            }
            match name {
                "min" => min = int_arg(value, "clamp() named 'min'")?,
                "max" => max = int_arg(value, "clamp() named 'max'")?,
                other => bail!("clamp() does not accept named argument '{}'", other),
            }
            Ok(())
        })?;

        if min > max {
            bail!("clamp() requires 'min' to be less than or equal to 'max'");
        }
        Ok(RuntimeVal::Int(value.clamp(min, max)))
    }

    #[stdlib_export(params(), returns = Float)]
    fn random(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

        static SEED: AtomicU64 = AtomicU64::new(0x12345678_9ABCDEF0);
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let mut seed = SEED.load(Ordering::Relaxed);
        if seed == 0 {
            seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            if seed == 0 {
                seed = 1;
            }
        }
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        SEED.store(seed, Ordering::Relaxed);
        seed = seed.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed) as u64);
        Ok(RuntimeVal::Float((seed >> 11) as f64 / (1u64 << 53) as f64))
    }

    #[stdlib_export(params(value: Int | Float), returns = Int | Float)]
    fn abs(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value.abs())),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value.abs())),
            _ => bail!("abs() argument must be a number"),
        }
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn sqrt(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match args.as_slice()[0] {
            RuntimeVal::Int(value) if value >= 0 => Ok(RuntimeVal::Float((value as f64).sqrt())),
            RuntimeVal::Float(value) if value >= 0.0 => Ok(RuntimeVal::Float(value.sqrt())),
            RuntimeVal::Int(_) | RuntimeVal::Float(_) => bail!("sqrt() argument must be non-negative"),
            _ => bail!("sqrt() argument must be a number"),
        }
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn sin(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "sin()", f64::sin)
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn cos(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "cos()", f64::cos)
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn tan(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "tan()", f64::tan)
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn asin(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = unary_number(args, "asin()")?;
        if !(-1.0..=1.0).contains(&value) {
            bail!("asin() argument must be between -1 and 1");
        }
        Ok(RuntimeVal::Float(value.asin()))
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn acos(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = unary_number(args, "acos()")?;
        if !(-1.0..=1.0).contains(&value) {
            bail!("acos() argument must be between -1 and 1");
        }
        Ok(RuntimeVal::Float(value.acos()))
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn atan(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "atan()", f64::atan)
    }

    #[stdlib_export(name = "atan2", params(y: Number, x: Number), returns = Float)]
    fn atan2_(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let y = number_arg(&values[0], "atan2() first argument")?;
        let x = number_arg(&values[1], "atan2() second argument")?;
        Ok(RuntimeVal::Float(y.atan2(x)))
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn log(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log()", f64::ln)
    }

    #[stdlib_export(name = "log10", params(value: Number), returns = Float)]
    fn log10_(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log10()", f64::log10)
    }

    #[stdlib_export(name = "log2", params(value: Number), returns = Float)]
    fn log2_(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log2()", f64::log2)
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn exp(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "exp()", f64::exp)
    }

    #[stdlib_export(params(base: Number, exponent: Number), returns = Float)]
    fn pow(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let base = number_arg(&values[0], "pow() first argument")?;
        let exponent = number_arg(&values[1], "pow() second argument")?;
        Ok(RuntimeVal::Float(base.powf(exponent)))
    }

    #[stdlib_export(params(value: Number), returns = Int)]
    fn floor(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        integer_round(args, "floor()", f64::floor)
    }

    #[stdlib_export(params(value: Number), returns = Int)]
    fn ceil(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        integer_round(args, "ceil()", f64::ceil)
    }

    #[stdlib_export(params(value: Number), returns = Int)]
    fn round(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        integer_round(args, "round()", f64::round)
    }

    #[stdlib_export(params(left: Number, right: Number), returns = Number)]
    fn min(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        min_max(args, "min()", |left, right| left < right)
    }

    #[stdlib_export(params(left: Number, right: Number), returns = Number)]
    fn max(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        min_max(args, "max()", |left, right| left > right)
    }

    #[stdlib_export(params(x: Number, y: Number), returns = Float)]
    fn hypot(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let x = number_arg(&values[0], "hypot() first argument")?;
        let y = number_arg(&values[1], "hypot() second argument")?;
        Ok(RuntimeVal::Float(x.hypot(y)))
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn cbrt(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = unary_number(args, "cbrt()")?;
        Ok(RuntimeVal::Float(value.cbrt()))
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn sinh(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "sinh()", f64::sinh)
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn cosh(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "cosh()", f64::cosh)
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn tanh(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "tanh()", f64::tanh)
    }

    #[stdlib_export(params(value: Number), returns = Int | Float)]
    fn trunc(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value)),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value.trunc())),
            _ => bail!("trunc() argument must be a number"),
        }
    }

    #[stdlib_export(params(value: Number), returns = Float)]
    fn fract(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match args.as_slice()[0] {
            RuntimeVal::Int(_) => Ok(RuntimeVal::Float(0.0)),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value.fract())),
            _ => bail!("fract() argument must be a number"),
        }
    }

    #[stdlib_export(params(value: Number), returns = Int | Float)]
    fn sign(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value.signum())),
            RuntimeVal::Float(value) => {
                if value > 0.0 {
                    Ok(RuntimeVal::Float(1.0))
                } else if value < 0.0 {
                    Ok(RuntimeVal::Float(-1.0))
                } else {
                    Ok(RuntimeVal::Float(0.0))
                }
            }
            _ => bail!("sign() argument must be a number"),
        }
    }

    #[stdlib_export(params(value: Number | Bool), returns = Int)]
    fn to_int(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value)),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Int(value as i64)),
            RuntimeVal::Bool(value) => Ok(RuntimeVal::Int(if value { 1 } else { 0 })),
            _ => bail!("to_int() argument must be a number or bool"),
        }
    }

    #[stdlib_export(params(value: Number | Bool), returns = Float)]
    fn to_float(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match args.as_slice()[0] {
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value)),
            RuntimeVal::Int(value) => Ok(RuntimeVal::Float(value as f64)),
            RuntimeVal::Bool(value) => Ok(RuntimeVal::Float(if value { 1.0 } else { 0.0 })),
            _ => bail!("to_float() argument must be a number or bool"),
        }
    }

    #[stdlib_export(params(value: Float), returns = Bool)]
    fn is_nan(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(RuntimeVal::Bool(
            matches!(args.as_slice()[0], RuntimeVal::Float(v) if v.is_nan()),
        ))
    }

    #[stdlib_export(params(value: Float), returns = Bool)]
    fn is_inf(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(RuntimeVal::Bool(
            matches!(args.as_slice()[0], RuntimeVal::Float(v) if v.is_infinite()),
        ))
    }
}

fn number_arg(value: &RuntimeVal, context: &str) -> Result<f64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value as f64),
        RuntimeVal::Float(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be a number")),
    }
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be an integer")),
    }
}

fn unary_number(args: NativeArgs<'_>, name: &str) -> Result<f64> {
    number_arg(&args.as_slice()[0], &format!("{name} argument"))
}

fn unary_float(args: NativeArgs<'_>, name: &str, op: fn(f64) -> f64) -> Result<RuntimeVal> {
    Ok(RuntimeVal::Float(op(unary_number(args, name)?)))
}

fn positive_unary_float(args: NativeArgs<'_>, name: &str, op: fn(f64) -> f64) -> Result<RuntimeVal> {
    let value = unary_number(args, name)?;
    if value <= 0.0 {
        bail!("{name} argument must be positive");
    }
    Ok(RuntimeVal::Float(op(value)))
}

fn integer_round(args: NativeArgs<'_>, name: &str, op: fn(f64) -> f64) -> Result<RuntimeVal> {
    match args.as_slice()[0] {
        RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value)),
        RuntimeVal::Float(value) => Ok(RuntimeVal::Int(op(value) as i64)),
        _ => bail!("{name} argument must be a number"),
    }
}

fn min_max(args: NativeArgs<'_>, name: &str, take_left: fn(f64, f64) -> bool) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let left = number_arg(&values[0], &format!("{name} first argument"))?;
    let right = number_arg(&values[1], &format!("{name} second argument"))?;
    Ok(if take_left(left, right) {
        values[0].clone()
    } else {
        values[1].clone()
    })
}
