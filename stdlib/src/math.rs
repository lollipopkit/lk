use anyhow::Result;
use lk_core::module::Module;
use lk_core::val::{NativeArgs, Val};
use lk_core::vm::VmContext;
use std::collections::HashMap;

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

        // Register positional math functions on the fast native ABI.
        functions.insert("abs".to_string(), Val::RustFastFunction(Self::abs));
        functions.insert("sqrt".to_string(), Val::RustFastFunction(Self::sqrt));
        functions.insert("sin".to_string(), Val::RustFastFunction(Self::sin));
        functions.insert("cos".to_string(), Val::RustFastFunction(Self::cos));
        functions.insert("tan".to_string(), Val::RustFastFunction(Self::tan));
        functions.insert("asin".to_string(), Val::RustFastFunction(Self::asin));
        functions.insert("acos".to_string(), Val::RustFastFunction(Self::acos));
        functions.insert("atan".to_string(), Val::RustFastFunction(Self::atan));
        functions.insert("atan2".to_string(), Val::RustFastFunction(Self::atan2));
        functions.insert("log".to_string(), Val::RustFastFunction(Self::log));
        functions.insert("log10".to_string(), Val::RustFastFunction(Self::log10));
        functions.insert("log2".to_string(), Val::RustFastFunction(Self::log2));
        functions.insert("exp".to_string(), Val::RustFastFunction(Self::exp));
        functions.insert("pow".to_string(), Val::RustFastFunction(Self::pow));
        functions.insert("floor".to_string(), Val::RustFastFunction(Self::floor));
        functions.insert("ceil".to_string(), Val::RustFastFunction(Self::ceil));
        functions.insert("round".to_string(), Val::RustFastFunction(Self::round));
        functions.insert("min".to_string(), Val::RustFastFunction(Self::min));
        functions.insert("max".to_string(), Val::RustFastFunction(Self::max));
        functions.insert("clamp".to_string(), Val::RustFastFunctionNamed(Self::clamp_fast));

        // Random number generation
        functions.insert("random".to_string(), Val::RustFastFunction(Self::random));

        // Constants
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

    fn clamp(pos: &[Val], named: &[(String, Val)], _ctx: &mut VmContext) -> Result<Val> {
        if pos.is_empty() {
            return Err(anyhow::anyhow!("clamp() requires at least the value argument"));
        }
        if pos.len() > 3 {
            return Err(anyhow::anyhow!(
                "clamp() takes at most 3 positional arguments: value, min, max"
            ));
        }

        let expect_int = |val: &Val, ctx: &str| -> Result<i64> {
            match val {
                Val::Int(i) => Ok(*i),
                _ => Err(anyhow::anyhow!(format!("clamp() {} must be an integer", ctx))),
            }
        };

        let value = expect_int(&pos[0], "first argument (value)")?;
        let mut min = if pos.len() >= 2 {
            expect_int(&pos[1], "second argument (min)")?
        } else {
            0
        };
        let mut max = if pos.len() >= 3 {
            expect_int(&pos[2], "third argument (max)")?
        } else {
            100
        };

        use std::collections::HashSet;
        let mut seen: HashSet<&str> = HashSet::with_capacity(named.len());
        for (name, val) in named {
            let key = name.as_str();
            if !seen.insert(key) {
                return Err(anyhow::anyhow!(format!(
                    "clamp() received duplicate named argument '{}'",
                    name
                )));
            }
            match key {
                "min" => {
                    min = expect_int(val, "named 'min'")?;
                }
                "max" => {
                    max = expect_int(val, "named 'max'")?;
                }
                other => {
                    return Err(anyhow::anyhow!(format!(
                        "clamp() does not accept named argument '{}'",
                        other
                    )));
                }
            }
        }

        if min > max {
            return Err(anyhow::anyhow!(
                "clamp() requires 'min' to be less than or equal to 'max'"
            ));
        }

        let clamped = value.clamp(min, max);
        Ok(Val::Int(clamped))
    }

    fn clamp_fast(args: NativeArgs<'_>, named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val> {
        Self::clamp(args.as_slice(), named, ctx)
    }

    /// Random number generation (0.0 to 1.0)
    fn random(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 0 {
            return Err(anyhow::anyhow!(
                "random() takes 0 arguments; call with no args for [0,1), 1 arg for [0,n), 2 args for [a,b)"
            ));
        }
        use std::sync::atomic::AtomicU32;
        use std::sync::atomic::{AtomicU64, Ordering};
        // Simple xorshift64 PRNG seeded from a global counter + thread id
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
        // xorshift64
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        SEED.store(seed, Ordering::Relaxed);
        // Mix in a counter for extra entropy
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        seed = seed.wrapping_add(counter as u64);
        let result = (seed >> 11) as f64 / (1u64 << 53) as f64; // [0, 1)
        Ok(Val::Float(result))
    }

    /// Absolute value
    fn abs(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("abs() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        match &args[0] {
            Val::Int(x) => Ok(Val::Int(x.abs())),
            Val::Float(x) => Ok(Val::Float(x.abs())),
            _ => Err(anyhow::anyhow!("abs() argument must be a number")),
        }
    }

    /// Square root
    fn sqrt(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("sqrt() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        match &args[0] {
            Val::Int(x) if *x >= 0 => Ok(Val::Float((*x as f64).sqrt())),
            Val::Float(x) if *x >= 0.0 => Ok(Val::Float(x.sqrt())),
            Val::Int(_) | Val::Float(_) => Err(anyhow::anyhow!("sqrt() argument must be non-negative")),
            _ => Err(anyhow::anyhow!("sqrt() argument must be a number")),
        }
    }

    /// Sine function
    fn sin(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("sin() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("sin() argument must be a number")),
        };

        Ok(Val::Float(x.sin()))
    }

    /// Cosine function
    fn cos(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("cos() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("cos() argument must be a number")),
        };

        Ok(Val::Float(x.cos()))
    }

    /// Tangent function
    fn tan(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("tan() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("tan() argument must be a number")),
        };

        Ok(Val::Float(x.tan()))
    }

    /// Arcsine function
    fn asin(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("asin() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("asin() argument must be a number")),
        };

        if !(-1.0..=1.0).contains(&x) {
            return Err(anyhow::anyhow!("asin() argument must be between -1 and 1"));
        }

        Ok(Val::Float(x.asin()))
    }

    /// Arccosine function
    fn acos(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("acos() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("acos() argument must be a number")),
        };

        if !(-1.0..=1.0).contains(&x) {
            return Err(anyhow::anyhow!("acos() argument must be between -1 and 1"));
        }

        Ok(Val::Float(x.acos()))
    }

    /// Arctangent function
    fn atan(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("atan() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("atan() argument must be a number")),
        };

        Ok(Val::Float(x.atan()))
    }

    /// Arctangent2 function (atan2(y, x))
    fn atan2(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!("atan2() takes exactly 2 arguments: y, x"));
        }
        let args = args.as_slice();

        let y = match &args[0] {
            Val::Int(y) => *y as f64,
            Val::Float(y) => *y,
            _ => return Err(anyhow::anyhow!("atan2() first argument must be a number")),
        };

        let x = match &args[1] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("atan2() second argument must be a number")),
        };

        Ok(Val::Float(y.atan2(x)))
    }

    /// Natural logarithm
    fn log(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("log() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) if *x > 0 => *x as f64,
            Val::Float(x) if *x > 0.0 => *x,
            Val::Int(_) | Val::Float(_) => {
                return Err(anyhow::anyhow!("log() argument must be positive"));
            }
            _ => return Err(anyhow::anyhow!("log() argument must be a number")),
        };

        Ok(Val::Float(x.ln()))
    }

    /// Base-10 logarithm
    fn log10(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("log10() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) if *x > 0 => *x as f64,
            Val::Float(x) if *x > 0.0 => *x,
            Val::Int(_) | Val::Float(_) => {
                return Err(anyhow::anyhow!("log10() argument must be positive"));
            }
            _ => return Err(anyhow::anyhow!("log10() argument must be a number")),
        };

        Ok(Val::Float(x.log10()))
    }

    /// Base-2 logarithm
    fn log2(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("log2() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) if *x > 0 => *x as f64,
            Val::Float(x) if *x > 0.0 => *x,
            Val::Int(_) | Val::Float(_) => {
                return Err(anyhow::anyhow!("log2() argument must be positive"));
            }
            _ => return Err(anyhow::anyhow!("log2() argument must be a number")),
        };

        Ok(Val::Float(x.log2()))
    }

    /// Exponential function
    fn exp(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("exp() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        let x = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("exp() argument must be a number")),
        };

        Ok(Val::Float(x.exp()))
    }

    /// Power function (x^y)
    fn pow(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!("pow() takes exactly 2 arguments: base, exponent"));
        }
        let args = args.as_slice();

        let base = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("pow() first argument must be a number")),
        };

        let exponent = match &args[1] {
            Val::Int(y) => *y as f64,
            Val::Float(y) => *y,
            _ => return Err(anyhow::anyhow!("pow() second argument must be a number")),
        };

        Ok(Val::Float(base.powf(exponent)))
    }

    /// Floor function
    fn floor(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("floor() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        match &args[0] {
            Val::Int(x) => Ok(Val::Int(*x)),
            Val::Float(x) => Ok(Val::Int(x.floor() as i64)),
            _ => Err(anyhow::anyhow!("floor() argument must be a number")),
        }
    }

    /// Ceiling function
    fn ceil(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("ceil() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        match &args[0] {
            Val::Int(x) => Ok(Val::Int(*x)),
            Val::Float(x) => Ok(Val::Int(x.ceil() as i64)),
            _ => Err(anyhow::anyhow!("ceil() argument must be a number")),
        }
    }

    /// Round function
    fn round(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("round() takes exactly 1 argument"));
        }
        let args = args.as_slice();

        match &args[0] {
            Val::Int(x) => Ok(Val::Int(*x)),
            Val::Float(x) => Ok(Val::Int(x.round() as i64)),
            _ => Err(anyhow::anyhow!("round() argument must be a number")),
        }
    }

    /// Minimum function
    fn min(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!("min() takes exactly 2 arguments"));
        }
        let args = args.as_slice();

        let a = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("min() first argument must be a number")),
        };

        let b = match &args[1] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("min() second argument must be a number")),
        };

        if a < b {
            match &args[0] {
                Val::Int(x) => Ok(Val::Int(*x)),
                Val::Float(x) => Ok(Val::Float(*x)),
                _ => unreachable!(),
            }
        } else {
            match &args[1] {
                Val::Int(x) => Ok(Val::Int(*x)),
                Val::Float(x) => Ok(Val::Float(*x)),
                _ => unreachable!(),
            }
        }
    }

    /// Maximum function
    fn max(args: NativeArgs<'_>, _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!("max() takes exactly 2 arguments"));
        }
        let args = args.as_slice();

        let a = match &args[0] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("max() first argument must be a number")),
        };

        let b = match &args[1] {
            Val::Int(x) => *x as f64,
            Val::Float(x) => *x,
            _ => return Err(anyhow::anyhow!("max() second argument must be a number")),
        };

        if a > b {
            match &args[0] {
                Val::Int(x) => Ok(Val::Int(*x)),
                Val::Float(x) => Ok(Val::Float(*x)),
                _ => unreachable!(),
            }
        } else {
            match &args[1] {
                Val::Int(x) => Ok(Val::Int(*x)),
                Val::Float(x) => Ok(Val::Float(*x)),
                _ => unreachable!(),
            }
        }
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
        // Don't register functions globally - they should be accessed via module.function()
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}
