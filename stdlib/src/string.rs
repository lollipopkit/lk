use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry},
    val::{HeapStore, HeapValue, RuntimeVal, TypedList, Val, runtime_val_to_val},
    vm::{NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, VmContext},
};

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct StringModule {
    functions: HashMap<String, Val>,
}

impl Default for StringModule {
    fn default() -> Self {
        Self::new()
    }
}

impl StringModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        register_native(&mut functions, "len", Self::len32, 1);
        register_native(&mut functions, "lower", Self::lower32, 1);
        register_native(&mut functions, "upper", Self::upper32, 1);
        register_native(&mut functions, "trim", Self::trim32, 1);
        register_native(&mut functions, "starts_with", Self::starts_with32, 2);
        register_native(&mut functions, "ends_with", Self::ends_with32, 2);
        register_native(&mut functions, "contains", Self::contains32, 2);
        register_native(&mut functions, "replace", Self::replace32, NativeEntry32::VARIADIC);
        register_native(&mut functions, "substring", Self::substring32, 3);
        register_native(&mut functions, "split", Self::split32, 2);
        register_native(&mut functions, "join", Self::join32, 2);
        register_native(&mut functions, "reverse", Self::reverse32, 1);
        register_native(&mut functions, "repeat", Self::repeat32, 2);
        register_native(&mut functions, "char", Self::char_at32, 2);
        register_native(&mut functions, "byte", Self::byte_at32, 2);
        register_native(&mut functions, "chars", Self::chars32, 1);
        register_native(&mut functions, "find", Self::find32, NativeEntry32::VARIADIC);
        register_native(&mut functions, "is_empty", Self::is_empty32, 1);
        register_native(&mut functions, "format", Self::format32, NativeEntry32::VARIADIC);

        Self { functions }
    }

    fn len32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "len()")?;
        Ok(RuntimeVal::Int(value.len() as i64))
    }

    fn lower32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "lower()")?;
        Ok(runtime_string_value(&value.to_lowercase(), runtime.heap_mut()))
    }

    fn upper32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "upper()")?;
        Ok(runtime_string_value(&value.to_uppercase(), runtime.heap_mut()))
    }

    fn trim32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "trim()")?;
        Ok(runtime_string_value(value.trim(), runtime.heap_mut()))
    }

    fn starts_with32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let (value, prefix) = two_strings(args, runtime, "starts_with()")?;
        Ok(RuntimeVal::Bool(value.starts_with(prefix.as_ref())))
    }

    fn ends_with32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let (value, suffix) = two_strings(args, runtime, "ends_with()")?;
        Ok(RuntimeVal::Bool(value.ends_with(suffix.as_ref())))
    }

    fn contains32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let (value, needle) = two_strings(args, runtime, "contains()")?;
        Ok(RuntimeVal::Bool(value.contains(needle.as_ref())))
    }

    fn replace32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let pos = args.as_slice();
        if pos.is_empty() {
            bail!("replace() requires at least the source string as the first argument");
        }
        if pos.len() > 4 {
            bail!("replace() received too many positional arguments (expected at most 4)");
        }

        let source = runtime_string_arg(&pos[0], &runtime.state.heap, "replace() first argument")?;
        let mut pattern = None;
        let mut with = None;
        let mut all_flag = None;
        let mut used_named_core = false;

        if pos.len() >= 2 {
            pattern = Some(runtime_string_arg(
                &pos[1],
                &runtime.state.heap,
                "replace() second argument (pattern)",
            )?);
        }
        if pos.len() >= 3 {
            with = Some(runtime_string_arg(
                &pos[2],
                &runtime.state.heap,
                "replace() third argument (with)",
            )?);
        }
        if pos.len() >= 4 {
            all_flag = Some(bool_arg(&pos[3], "replace() fourth argument (all flag)")?);
        }

        let mut seen = HashSet::with_capacity(args.named().len());
        for (name, value) in args.named() {
            if !seen.insert(name.as_str()) {
                bail!("replace() received duplicate named argument '{}'", name);
            }
            match name.as_str() {
                "pattern" => {
                    pattern = Some(runtime_string_arg(
                        value,
                        &runtime.state.heap,
                        "replace() named 'pattern'",
                    )?);
                    used_named_core = true;
                }
                "with" => {
                    with = Some(runtime_string_arg(
                        value,
                        &runtime.state.heap,
                        "replace() named 'with'",
                    )?);
                    used_named_core = true;
                }
                "all" => all_flag = Some(bool_arg(value, "replace() named 'all'")?),
                other => bail!("replace() does not accept named argument '{}'", other),
            }
        }

        let pattern = pattern.ok_or_else(|| {
            anyhow!("replace() requires a pattern string (provide it positionally or via named 'pattern')")
        })?;
        let with = with.ok_or_else(|| {
            anyhow!("replace() requires a replacement string (provide it positionally or via named 'with')")
        })?;
        let all = all_flag.unwrap_or(!used_named_core);
        let result = if all {
            source.replace(pattern.as_ref(), with.as_ref())
        } else {
            source.replacen(pattern.as_ref(), with.as_ref(), 1)
        };
        Ok(runtime_string_value(&result, runtime.heap_mut()))
    }

    fn substring32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 3, "substring()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], &runtime.state.heap, "substring() first argument")?;
        let start = usize_arg(&values[1], "substring() second argument")?;
        let length = usize_arg(&values[2], "substring() third argument")?;
        if start > value.len() {
            bail!("substring() start index out of bounds");
        }
        let end = std::cmp::min(start + length, value.len());
        Ok(runtime_string_value(&value[start..end], runtime.heap_mut()))
    }

    fn split32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let (value, delimiter) = two_strings(args, runtime, "split()")?;
        let parts = if delimiter.is_empty() {
            value.chars().map(|value| Arc::<str>::from(value.to_string())).collect()
        } else {
            value.split(delimiter.as_ref()).map(Arc::<str>::from).collect()
        };
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(parts))),
        ))
    }

    fn join32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "join()")?;
        let values = args.as_slice();
        let strings = string_list_arg(&values[0], &runtime.state.heap, "join() first argument")?;
        let delimiter = runtime_string_arg(&values[1], &runtime.state.heap, "join() second argument")?;
        Ok(runtime_string_value(
            &strings.join(delimiter.as_ref()),
            runtime.heap_mut(),
        ))
    }

    fn reverse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "reverse()")?;
        Ok(runtime_string_value(
            &value.chars().rev().collect::<String>(),
            runtime.heap_mut(),
        ))
    }

    fn repeat32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "repeat()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], &runtime.state.heap, "repeat() first argument")?;
        let count = int_arg(&values[1], "repeat() second argument")?;
        if count < 0 {
            bail!("repeat() count must be non-negative");
        }
        Ok(runtime_string_value(&value.repeat(count as usize), runtime.heap_mut()))
    }

    fn char_at32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "char()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], &runtime.state.heap, "char() first argument")?;
        let index = usize_arg(&values[1], "char() second argument")?;
        Ok(value.chars().nth(index).map_or(RuntimeVal::Nil, |value| {
            runtime_string_value(&value.to_string(), runtime.heap_mut())
        }))
    }

    fn byte_at32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "byte()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], &runtime.state.heap, "byte() first argument")?;
        let index = usize_arg(&values[1], "byte() second argument")?;
        Ok(value
            .as_bytes()
            .get(index)
            .map_or(RuntimeVal::Nil, |value| RuntimeVal::Int(*value as i64)))
    }

    fn chars32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "chars()")?;
        let chars = value.chars().map(|value| Arc::<str>::from(value.to_string())).collect();
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(chars))),
        ))
    }

    fn find32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        if args.len() != 2 && args.len() != 3 {
            bail!("find() takes 2 or 3 arguments: string, pattern[, start]");
        }
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], &runtime.state.heap, "find() first argument")?;
        let pattern = runtime_string_arg(&values[1], &runtime.state.heap, "find() second argument")?;
        let start = if values.len() == 3 {
            usize_arg(&values[2], "find() third argument")?
        } else {
            0
        };
        if start > value.len() {
            return Ok(RuntimeVal::Nil);
        }
        Ok(value[start..]
            .find(pattern.as_ref())
            .map_or(RuntimeVal::Nil, |index| RuntimeVal::Int((start + index) as i64)))
    }

    fn is_empty32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "is_empty()")?;
        Ok(RuntimeVal::Bool(value.is_empty()))
    }

    fn format32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        if args.is_empty() {
            bail!("format() requires at least 1 argument (format string)");
        }
        let values = args.as_slice();
        let fmt = runtime_string_arg(&values[0], &runtime.state.heap, "format() first argument")?;
        let rest = &values[1..];
        let mut out = String::with_capacity(fmt.len());
        let chars = fmt.chars().collect::<Vec<_>>();
        let mut i = 0usize;
        let mut arg_index = 0usize;
        while i < chars.len() {
            if chars[i] == '{' && i + 1 < chars.len() && chars[i + 1] == '}' {
                if arg_index < rest.len() {
                    out.push_str(&display_runtime_value(
                        &rest[arg_index],
                        &runtime.state.heap,
                        runtime.ctx.as_deref(),
                    ));
                    arg_index += 1;
                } else {
                    out.push_str("{}");
                }
                i += 2;
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
        if arg_index < rest.len() {
            if !out.is_empty() {
                out.push(' ');
            }
            for (index, value) in rest[arg_index..].iter().enumerate() {
                if index > 0 {
                    out.push(' ');
                }
                out.push_str(&display_runtime_value(
                    value,
                    &runtime.state.heap,
                    runtime.ctx.as_deref(),
                ));
            }
        }
        Ok(runtime_string_value(&out, runtime.heap_mut()))
    }
}

impl Module for StringModule {
    fn name(&self) -> &str {
        "string"
    }

    fn description(&self) -> &str {
        "String manipulation functions"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
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

fn one_string(args: NativeArgs32<'_>, runtime: &NativeRuntime32<'_>, name: &str) -> Result<Arc<str>> {
    expect_arity(args, 1, name)?;
    runtime_string_arg(&args.as_slice()[0], &runtime.state.heap, name)
}

fn two_strings(args: NativeArgs32<'_>, runtime: &NativeRuntime32<'_>, name: &str) -> Result<(Arc<str>, Arc<str>)> {
    expect_arity(args, 2, name)?;
    let values = args.as_slice();
    Ok((
        runtime_string_arg(&values[0], &runtime.state.heap, name)?,
        runtime_string_arg(&values[1], &runtime.state.heap, name)?,
    ))
}

fn int_arg(value: &RuntimeVal, context: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be an integer")),
    }
}

fn usize_arg(value: &RuntimeVal, context: &str) -> Result<usize> {
    let value = int_arg(value, context)?;
    if value < 0 {
        bail!("{context} must be non-negative");
    }
    Ok(value as usize)
}

fn bool_arg(value: &RuntimeVal, context: &str) -> Result<bool> {
    match value {
        RuntimeVal::Bool(value) => Ok(*value),
        _ => Err(anyhow!("{context} must be a boolean")),
    }
}

fn string_list_arg(value: &RuntimeVal, heap: &HeapStore, context: &str) -> Result<Vec<String>> {
    let RuntimeVal::Obj(handle) = value else {
        bail!("{context} must be a list");
    };
    let Some(HeapValue::List(list)) = heap.get(*handle) else {
        bail!("{context} must be a list");
    };
    match list {
        TypedList::String(values) => Ok(values.iter().map(ToString::to_string).collect()),
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| {
                runtime_string_arg(value, heap, context)
                    .map(|value| value.to_string())
                    .map_err(|_| anyhow!("join() list must contain only strings"))
            })
            .collect(),
        _ => Err(anyhow!("join() list must contain only strings")),
    }
}

fn display_runtime_value(value: &RuntimeVal, heap: &HeapStore, ctx: Option<&VmContext>) -> String {
    runtime_val_to_val(value, heap)
        .map(|value| value.display_string(ctx))
        .unwrap_or_else(|_| format!("{:?}", value.kind()))
}
