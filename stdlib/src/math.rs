use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, RuntimeNativeExport, RuntimeValueExport, runtime_export_from_plain_native_entries},
    val::RuntimeVal,
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use std::collections::HashSet;

#[derive(Debug)]
pub struct MathModule;

impl Default for MathModule {
    fn default() -> Self {
        Self::new()
    }
}

impl MathModule {
    pub fn new() -> Self {
        Self
    }

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

    fn random(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 0, "random()")?;
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

    fn abs(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "abs()")?;
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value.abs())),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value.abs())),
            _ => bail!("abs() argument must be a number"),
        }
    }

    fn sqrt(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "sqrt()")?;
        match args.as_slice()[0] {
            RuntimeVal::Int(value) if value >= 0 => Ok(RuntimeVal::Float((value as f64).sqrt())),
            RuntimeVal::Float(value) if value >= 0.0 => Ok(RuntimeVal::Float(value.sqrt())),
            RuntimeVal::Int(_) | RuntimeVal::Float(_) => bail!("sqrt() argument must be non-negative"),
            _ => bail!("sqrt() argument must be a number"),
        }
    }

    fn sin(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "sin()", f64::sin)
    }

    fn cos(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "cos()", f64::cos)
    }

    fn tan(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "tan()", f64::tan)
    }

    fn asin(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = unary_number(args, "asin()")?;
        if !(-1.0..=1.0).contains(&value) {
            bail!("asin() argument must be between -1 and 1");
        }
        Ok(RuntimeVal::Float(value.asin()))
    }

    fn acos(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = unary_number(args, "acos()")?;
        if !(-1.0..=1.0).contains(&value) {
            bail!("acos() argument must be between -1 and 1");
        }
        Ok(RuntimeVal::Float(value.acos()))
    }

    fn atan(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "atan()", f64::atan)
    }

    fn atan2_(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "atan2()")?;
        let values = args.as_slice();
        let y = number_arg(&values[0], "atan2() first argument")?;
        let x = number_arg(&values[1], "atan2() second argument")?;
        Ok(RuntimeVal::Float(y.atan2(x)))
    }

    fn log(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log()", f64::ln)
    }

    fn log10_(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log10()", f64::log10)
    }

    fn log2_(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log2()", f64::log2)
    }

    fn exp(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "exp()", f64::exp)
    }

    fn pow(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "pow()")?;
        let values = args.as_slice();
        let base = number_arg(&values[0], "pow() first argument")?;
        let exponent = number_arg(&values[1], "pow() second argument")?;
        Ok(RuntimeVal::Float(base.powf(exponent)))
    }

    fn floor(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        integer_round(args, "floor()", f64::floor)
    }

    fn ceil(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        integer_round(args, "ceil()", f64::ceil)
    }

    fn round(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        integer_round(args, "round()", f64::round)
    }

    fn min(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        min_max(args, "min()", |left, right| left < right)
    }

    fn max(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        min_max(args, "max()", |left, right| left > right)
    }

    fn hypot(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "hypot()")?;
        let values = args.as_slice();
        let x = number_arg(&values[0], "hypot() first argument")?;
        let y = number_arg(&values[1], "hypot() second argument")?;
        Ok(RuntimeVal::Float(x.hypot(y)))
    }

    fn cbrt(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "cbrt()")?;
        let value = unary_number(args, "cbrt()")?;
        Ok(RuntimeVal::Float(value.cbrt()))
    }

    fn sinh(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "sinh()", f64::sinh)
    }

    fn cosh(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "cosh()", f64::cosh)
    }

    fn tanh(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        unary_float(args, "tanh()", f64::tanh)
    }

    fn trunc(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "trunc()")?;
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value)),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value.trunc())),
            _ => bail!("trunc() argument must be a number"),
        }
    }

    fn fract(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "fract()")?;
        match args.as_slice()[0] {
            RuntimeVal::Int(_) => Ok(RuntimeVal::Float(0.0)),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value.fract())),
            _ => bail!("fract() argument must be a number"),
        }
    }

    fn sign(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "sign()")?;
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

    fn to_int(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "to_int()")?;
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value)),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Int(value as i64)),
            RuntimeVal::Bool(value) => Ok(RuntimeVal::Int(if value { 1 } else { 0 })),
            _ => bail!("to_int() argument must be a number or bool"),
        }
    }

    fn to_float(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "to_float()")?;
        match args.as_slice()[0] {
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value)),
            RuntimeVal::Int(value) => Ok(RuntimeVal::Float(value as f64)),
            RuntimeVal::Bool(value) => Ok(RuntimeVal::Float(if value { 1.0 } else { 0.0 })),
            _ => bail!("to_float() argument must be a number or bool"),
        }
    }

    fn is_nan(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "is_nan()")?;
        Ok(RuntimeVal::Bool(
            matches!(args.as_slice()[0], RuntimeVal::Float(v) if v.is_nan()),
        ))
    }

    fn is_inf(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "is_inf()")?;
        Ok(RuntimeVal::Bool(
            matches!(args.as_slice()[0], RuntimeVal::Float(v) if v.is_infinite()),
        ))
    }
}

impl ModuleProvider for MathModule {
    fn name(&self) -> &str {
        "math"
    }

    fn description(&self) -> &str {
        "Mathematical functions and constants"
    }

    fn register(&self, _registry: &mut lk_core::module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("abs", Self::abs, 1),
                RuntimeNativeExport::plain("sqrt", Self::sqrt, 1),
                RuntimeNativeExport::plain("sin", Self::sin, 1),
                RuntimeNativeExport::plain("cos", Self::cos, 1),
                RuntimeNativeExport::plain("tan", Self::tan, 1),
                RuntimeNativeExport::plain("asin", Self::asin, 1),
                RuntimeNativeExport::plain("acos", Self::acos, 1),
                RuntimeNativeExport::plain("atan", Self::atan, 1),
                RuntimeNativeExport::plain("atan2", Self::atan2_, 2),
                RuntimeNativeExport::plain("log", Self::log, 1),
                RuntimeNativeExport::plain("log10", Self::log10_, 1),
                RuntimeNativeExport::plain("log2", Self::log2_, 1),
                RuntimeNativeExport::plain("exp", Self::exp, 1),
                RuntimeNativeExport::plain("pow", Self::pow, 2),
                RuntimeNativeExport::plain("floor", Self::floor, 1),
                RuntimeNativeExport::plain("ceil", Self::ceil, 1),
                RuntimeNativeExport::plain("round", Self::round, 1),
                RuntimeNativeExport::plain("min", Self::min, 2),
                RuntimeNativeExport::plain("max", Self::max, 2),
                RuntimeNativeExport::plain("clamp", Self::clamp, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("random", Self::random, 0),
                RuntimeNativeExport::plain("hypot", Self::hypot, 2),
                RuntimeNativeExport::plain("cbrt", Self::cbrt, 1),
                RuntimeNativeExport::plain("sinh", Self::sinh, 1),
                RuntimeNativeExport::plain("cosh", Self::cosh, 1),
                RuntimeNativeExport::plain("tanh", Self::tanh, 1),
                RuntimeNativeExport::plain("trunc", Self::trunc, 1),
                RuntimeNativeExport::plain("fract", Self::fract, 1),
                RuntimeNativeExport::plain("sign", Self::sign, 1),
                RuntimeNativeExport::plain("to_int", Self::to_int, 1),
                RuntimeNativeExport::plain("to_float", Self::to_float, 1),
                RuntimeNativeExport::plain("is_nan", Self::is_nan, 1),
                RuntimeNativeExport::plain("is_inf", Self::is_inf, 1),
            ],
            &[
                RuntimeValueExport::new("pi", RuntimeVal::Float(std::f64::consts::PI)),
                RuntimeValueExport::new("e", RuntimeVal::Float(std::f64::consts::E)),
                RuntimeValueExport::new("inf", RuntimeVal::Float(f64::INFINITY)),
                RuntimeValueExport::new("nan", RuntimeVal::Float(f64::NAN)),
                RuntimeValueExport::new("max_int", RuntimeVal::Int(i64::MAX)),
                RuntimeValueExport::new("min_int", RuntimeVal::Int(i64::MIN)),
                RuntimeValueExport::new("max_float", RuntimeVal::Float(f64::MAX)),
                RuntimeValueExport::new("epsilon", RuntimeVal::Float(f64::EPSILON)),
            ],
        ))
    }
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "{name} takes exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        )
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
    expect_arity(args, 1, name)?;
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
    expect_arity(args, 1, name)?;
    match args.as_slice()[0] {
        RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value)),
        RuntimeVal::Float(value) => Ok(RuntimeVal::Int(op(value) as i64)),
        _ => bail!("{name} argument must be a number"),
    }
}

fn min_max(args: NativeArgs<'_>, name: &str, take_left: fn(f64, f64) -> bool) -> Result<RuntimeVal> {
    expect_arity(args, 2, name)?;
    let values = args.as_slice();
    let left = number_arg(&values[0], &format!("{name} first argument"))?;
    let right = number_arg(&values[1], &format!("{name} second argument"))?;
    Ok(if take_left(left, right) {
        values[0].clone()
    } else {
        values[1].clone()
    })
}
