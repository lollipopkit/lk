use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::Module,
    val::{RuntimeVal, Val},
    vm::{NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32},
};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct MathModule {
    functions: HashMap<String, Val>,
}

impl Default for MathModule {
    fn default() -> Self {
        Self::new()
    }
}

impl MathModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        register_native(&mut functions, "abs", Self::abs32, 1);
        register_native(&mut functions, "sqrt", Self::sqrt32, 1);
        register_native(&mut functions, "sin", Self::sin32, 1);
        register_native(&mut functions, "cos", Self::cos32, 1);
        register_native(&mut functions, "tan", Self::tan32, 1);
        register_native(&mut functions, "asin", Self::asin32, 1);
        register_native(&mut functions, "acos", Self::acos32, 1);
        register_native(&mut functions, "atan", Self::atan32, 1);
        register_native(&mut functions, "atan2", Self::atan2_32, 2);
        register_native(&mut functions, "log", Self::log32, 1);
        register_native(&mut functions, "log10", Self::log10_32, 1);
        register_native(&mut functions, "log2", Self::log2_32, 1);
        register_native(&mut functions, "exp", Self::exp32, 1);
        register_native(&mut functions, "pow", Self::pow32, 2);
        register_native(&mut functions, "floor", Self::floor32, 1);
        register_native(&mut functions, "ceil", Self::ceil32, 1);
        register_native(&mut functions, "round", Self::round32, 1);
        register_native(&mut functions, "min", Self::min32, 2);
        register_native(&mut functions, "max", Self::max32, 2);
        register_native(&mut functions, "clamp", Self::clamp32, NativeEntry32::VARIADIC);
        register_native(&mut functions, "random", Self::random32, 0);

        functions.insert("pi".to_string(), Val::Float(std::f64::consts::PI));
        functions.insert("e".to_string(), Val::Float(std::f64::consts::E));
        functions.insert("inf".to_string(), Val::Float(f64::INFINITY));
        functions.insert("nan".to_string(), Val::Float(f64::NAN));
        functions.insert("max_int".to_string(), Val::Int(i64::MAX));
        functions.insert("min_int".to_string(), Val::Int(i64::MIN));
        functions.insert("max_float".to_string(), Val::Float(f64::MAX));
        functions.insert("epsilon".to_string(), Val::Float(f64::EPSILON));

        Self { functions }
    }

    fn clamp32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
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

        let mut seen = HashSet::with_capacity(args.named().len());
        for (name, value) in args.named() {
            if !seen.insert(name.as_str()) {
                bail!("clamp() received duplicate named argument '{}'", name);
            }
            match name.as_str() {
                "min" => min = int_arg(value, "clamp() named 'min'")?,
                "max" => max = int_arg(value, "clamp() named 'max'")?,
                other => bail!("clamp() does not accept named argument '{}'", other),
            }
        }

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

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn register_native(
    functions: &mut HashMap<String, Val>,
    name: &str,
    function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>,
    arity: u16,
) {
    functions.insert(
        name.to_string(),
        Val::runtime_native32(NativeFunction32::Plain(function), arity),
    );
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
