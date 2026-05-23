use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, RuntimeNativeExport32, RuntimeValueExport32, runtime_export_from_plain_native_entries},
    val::RuntimeVal,
    vm::{NativeArgs32, NativeEntry32, NativeRuntime32, RuntimeExport32},
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

    fn clamp32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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

    fn random32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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

    fn abs32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "abs()")?;
        match args.as_slice()[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value.abs())),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(value.abs())),
            _ => bail!("abs() argument must be a number"),
        }
    }

    fn sqrt32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "sqrt()")?;
        match args.as_slice()[0] {
            RuntimeVal::Int(value) if value >= 0 => Ok(RuntimeVal::Float((value as f64).sqrt())),
            RuntimeVal::Float(value) if value >= 0.0 => Ok(RuntimeVal::Float(value.sqrt())),
            RuntimeVal::Int(_) | RuntimeVal::Float(_) => bail!("sqrt() argument must be non-negative"),
            _ => bail!("sqrt() argument must be a number"),
        }
    }

    fn sin32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        unary_float(args, "sin()", f64::sin)
    }

    fn cos32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        unary_float(args, "cos()", f64::cos)
    }

    fn tan32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        unary_float(args, "tan()", f64::tan)
    }

    fn asin32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = unary_number(args, "asin()")?;
        if !(-1.0..=1.0).contains(&value) {
            bail!("asin() argument must be between -1 and 1");
        }
        Ok(RuntimeVal::Float(value.asin()))
    }

    fn acos32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = unary_number(args, "acos()")?;
        if !(-1.0..=1.0).contains(&value) {
            bail!("acos() argument must be between -1 and 1");
        }
        Ok(RuntimeVal::Float(value.acos()))
    }

    fn atan32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        unary_float(args, "atan()", f64::atan)
    }

    fn atan2_32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "atan2()")?;
        let values = args.as_slice();
        let y = number_arg(&values[0], "atan2() first argument")?;
        let x = number_arg(&values[1], "atan2() second argument")?;
        Ok(RuntimeVal::Float(y.atan2(x)))
    }

    fn log32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log()", f64::ln)
    }

    fn log10_32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log10()", f64::log10)
    }

    fn log2_32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        positive_unary_float(args, "log2()", f64::log2)
    }

    fn exp32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        unary_float(args, "exp()", f64::exp)
    }

    fn pow32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "pow()")?;
        let values = args.as_slice();
        let base = number_arg(&values[0], "pow() first argument")?;
        let exponent = number_arg(&values[1], "pow() second argument")?;
        Ok(RuntimeVal::Float(base.powf(exponent)))
    }

    fn floor32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        integer_round(args, "floor()", f64::floor)
    }

    fn ceil32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        integer_round(args, "ceil()", f64::ceil)
    }

    fn round32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        integer_round(args, "round()", f64::round)
    }

    fn min32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        min_max(args, "min()", |left, right| left < right)
    }

    fn max32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        min_max(args, "max()", |left, right| left > right)
    }
}

impl Module for MathModule {
    fn name(&self) -> &str {
        "math"
    }

    fn description(&self) -> &str {
        "Mathematical functions and constants"
    }

    fn register(&self, _registry: &mut lk_core::module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("abs", Self::abs32, 1),
                RuntimeNativeExport32::plain("sqrt", Self::sqrt32, 1),
                RuntimeNativeExport32::plain("sin", Self::sin32, 1),
                RuntimeNativeExport32::plain("cos", Self::cos32, 1),
                RuntimeNativeExport32::plain("tan", Self::tan32, 1),
                RuntimeNativeExport32::plain("asin", Self::asin32, 1),
                RuntimeNativeExport32::plain("acos", Self::acos32, 1),
                RuntimeNativeExport32::plain("atan", Self::atan32, 1),
                RuntimeNativeExport32::plain("atan2", Self::atan2_32, 2),
                RuntimeNativeExport32::plain("log", Self::log32, 1),
                RuntimeNativeExport32::plain("log10", Self::log10_32, 1),
                RuntimeNativeExport32::plain("log2", Self::log2_32, 1),
                RuntimeNativeExport32::plain("exp", Self::exp32, 1),
                RuntimeNativeExport32::plain("pow", Self::pow32, 2),
                RuntimeNativeExport32::plain("floor", Self::floor32, 1),
                RuntimeNativeExport32::plain("ceil", Self::ceil32, 1),
                RuntimeNativeExport32::plain("round", Self::round32, 1),
                RuntimeNativeExport32::plain("min", Self::min32, 2),
                RuntimeNativeExport32::plain("max", Self::max32, 2),
                RuntimeNativeExport32::plain("clamp", Self::clamp32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("random", Self::random32, 0),
            ],
            &[
                RuntimeValueExport32::new("pi", RuntimeVal::Float(std::f64::consts::PI)),
                RuntimeValueExport32::new("e", RuntimeVal::Float(std::f64::consts::E)),
                RuntimeValueExport32::new("inf", RuntimeVal::Float(f64::INFINITY)),
                RuntimeValueExport32::new("nan", RuntimeVal::Float(f64::NAN)),
                RuntimeValueExport32::new("max_int", RuntimeVal::Int(i64::MAX)),
                RuntimeValueExport32::new("min_int", RuntimeVal::Int(i64::MIN)),
                RuntimeValueExport32::new("max_float", RuntimeVal::Float(f64::MAX)),
                RuntimeValueExport32::new("epsilon", RuntimeVal::Float(f64::EPSILON)),
            ],
        ))
    }
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
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

fn unary_number(args: NativeArgs32<'_>, name: &str) -> Result<f64> {
    expect_arity(args, 1, name)?;
    number_arg(&args.as_slice()[0], &format!("{name} argument"))
}

fn unary_float(args: NativeArgs32<'_>, name: &str, op: fn(f64) -> f64) -> Result<RuntimeVal> {
    Ok(RuntimeVal::Float(op(unary_number(args, name)?)))
}

fn positive_unary_float(args: NativeArgs32<'_>, name: &str, op: fn(f64) -> f64) -> Result<RuntimeVal> {
    let value = unary_number(args, name)?;
    if value <= 0.0 {
        bail!("{name} argument must be positive");
    }
    Ok(RuntimeVal::Float(op(value)))
}

fn integer_round(args: NativeArgs32<'_>, name: &str, op: fn(f64) -> f64) -> Result<RuntimeVal> {
    expect_arity(args, 1, name)?;
    match args.as_slice()[0] {
        RuntimeVal::Int(value) => Ok(RuntimeVal::Int(value)),
        RuntimeVal::Float(value) => Ok(RuntimeVal::Int(op(value) as i64)),
        _ => bail!("{name} argument must be a number"),
    }
}

fn min_max(args: NativeArgs32<'_>, name: &str, take_left: fn(f64, f64) -> bool) -> Result<RuntimeVal> {
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
