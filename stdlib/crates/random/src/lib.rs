use anyhow::{Result, bail};
use lk_core::{
    val::{HeapValue, RuntimeVal},
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_bytes::runtime_bytes_value;
use lk_stdlib_common::runtime_native::runtime_string_value;
use rand::Rng as _;

const MAX_RANDOM_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "random", docs = "Random value helpers")]
pub struct RandomModule;

#[lk_stdlib_common::stdlib_exports(module = "random")]
impl RandomModule {
    #[stdlib_export(name = "int", params(min: Int, max: Int), returns = Int)]
    fn int(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let min = int_arg(args.get(0).expect("checked arity"), "random.int min")?;
        let max = int_arg(args.get(1).expect("checked arity"), "random.int max")?;
        if max < min {
            bail!("random.int() max must be >= min");
        }
        Ok(RuntimeVal::Int(rand::rng().random_range(min..=max)))
    }

    #[stdlib_export(name = "float", params(), returns = Float)]
    fn float(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(RuntimeVal::Float(rand::rng().random()))
    }

    #[stdlib_export(name = "bool", params(probability?: Float), returns = Bool)]
    fn bool_value(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        if args.len() > 1 {
            bail!("random.bool() expects at most 1 argument");
        }
        let probability = if let Some(value) = args.get(0) {
            float_arg(value, "random.bool probability")?
        } else {
            0.5
        };
        if !(0.0..=1.0).contains(&probability) {
            bail!("random.bool() probability must be in 0..=1");
        }
        Ok(RuntimeVal::Bool(rand::rng().random_bool(probability)))
    }

    #[stdlib_export(name = "bytes", params(len: Int), returns = Bytes)]
    fn bytes(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let len = usize_arg(args.get(0).expect("checked arity"), "random.bytes len")?;
        if len > MAX_RANDOM_BYTES {
            bail!("random.bytes() len exceeds {MAX_RANDOM_BYTES}");
        }
        let mut data = vec![0u8; len];
        rand::rng().fill(data.as_mut_slice());
        Ok(runtime_bytes_value(data, runtime.heap_mut()))
    }

    #[stdlib_export(name = "choice", params(values: List), returns = Any)]
    fn choice(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = list_values(args.get(0).expect("checked arity"), runtime, "random.choice list")?;
        if values.is_empty() {
            return Ok(RuntimeVal::Nil);
        }
        let index = rand::rng().random_range(0..values.len());
        Ok(values[index])
    }

    #[stdlib_export(name = "shuffle", params(values: List), returns = List)]
    fn shuffle(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let mut values = list_values(args.get(0).expect("checked arity"), runtime, "random.shuffle list")?;
        for i in (1..values.len()).rev() {
            let j = rand::rng().random_range(0..=i);
            values.swap(i, j);
        }
        let list = lk_stdlib_common::typed_list_from_values(values, runtime.heap());
        Ok(RuntimeVal::Obj(runtime.heap_mut().alloc(HeapValue::List(list))))
    }
}

fn list_values(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>, context: &str) -> Result<Vec<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} expects list");
    };
    let list = {
        let value = runtime
            .heap()
            .get(*handle)
            .ok_or_else(|| anyhow::anyhow!("heap object out of bounds"))?;
        let HeapValue::List(list) = value else {
            bail!("{context} expects list, got {}", value.type_name());
        };
        list.clone()
    };
    Ok(match list {
        lk_core::val::TypedList::Mixed(values) => values,
        lk_core::val::TypedList::Int(values) => values.into_iter().map(RuntimeVal::Int).collect(),
        lk_core::val::TypedList::Float(values) => values.into_iter().map(RuntimeVal::Float).collect(),
        lk_core::val::TypedList::Bool(values) => values.into_iter().map(RuntimeVal::Bool).collect(),
        lk_core::val::TypedList::String(values) => values
            .into_iter()
            .map(|value| runtime_string_value(value.as_ref(), runtime.heap_mut()))
            .collect(),
    })
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        other => bail!("{context} expects Int, got {:?}", other.kind()),
    }
}

fn usize_arg(value: &RuntimeVal, context: &str) -> Result<usize> {
    match value {
        RuntimeVal::Int(value) if *value >= 0 => {
            usize::try_from(*value).map_err(|_| anyhow::anyhow!("{context} exceeds usize::MAX, got {value}"))
        }
        other => bail!("{context} expects non-negative Int, got {:?}", other.kind()),
    }
}

fn float_arg(value: &RuntimeVal, context: &str) -> Result<f64> {
    match value {
        RuntimeVal::Float(value) => Ok(*value),
        RuntimeVal::Int(value) => Ok(*value as f64),
        other => bail!("{context} expects number, got {:?}", other.kind()),
    }
}
