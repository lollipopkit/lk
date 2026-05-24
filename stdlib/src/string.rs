use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs32, NativeEntry32, NativeRuntime32, RuntimeExport32},
};

use crate::runtime_native::{runtime_display_value, runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct StringModule;

impl Default for StringModule {
    fn default() -> Self {
        Self::new()
    }
}

impl StringModule {
    pub fn new() -> Self {
        Self
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

        let source = runtime_string_arg(&pos[0], runtime.heap(), "replace() first argument")?;
        let mut pattern = None;
        let mut with = None;
        let mut all_flag = None;
        let mut used_named_core = false;

        if pos.len() >= 2 {
            pattern = Some(runtime_string_arg(
                &pos[1],
                runtime.heap(),
                "replace() second argument (pattern)",
            )?);
        }
        if pos.len() >= 3 {
            with = Some(runtime_string_arg(
                &pos[2],
                runtime.heap(),
                "replace() third argument (with)",
            )?);
        }
        if pos.len() >= 4 {
            all_flag = Some(bool_arg(&pos[3], "replace() fourth argument (all flag)")?);
        }

        let mut seen = HashSet::with_capacity(args.named_len());
        args.try_for_each_named(runtime.heap(), |name, value| {
            if !seen.insert(name.to_string()) {
                bail!("replace() received duplicate named argument '{}'", name);
            }
            match name {
                "pattern" => {
                    pattern = Some(runtime_string_arg(value, runtime.heap(), "replace() named 'pattern'")?);
                    used_named_core = true;
                }
                "with" => {
                    with = Some(runtime_string_arg(value, runtime.heap(), "replace() named 'with'")?);
                    used_named_core = true;
                }
                "all" => all_flag = Some(bool_arg(value, "replace() named 'all'")?),
                other => bail!("replace() does not accept named argument '{}'", other),
            }
            Ok(())
        })?;

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
        let value = runtime_string_arg(&values[0], runtime.heap(), "substring() first argument")?;
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
        let mut parts = Vec::new();
        if delimiter.is_empty() {
            for value in value.chars() {
                parts.push(Arc::<str>::from(value.to_string()));
            }
        } else {
            for value in value.split(delimiter.as_ref()) {
                parts.push(Arc::<str>::from(value));
            }
        }
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(parts))),
        ))
    }

    fn join32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "join()")?;
        let values = args.as_slice();
        let strings = string_list_arg(&values[0], runtime.heap(), "join() first argument")?;
        let delimiter = runtime_string_arg(&values[1], runtime.heap(), "join() second argument")?;
        Ok(runtime_string_value(
            &strings.join(delimiter.as_ref()),
            runtime.heap_mut(),
        ))
    }

    fn reverse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "reverse()")?;
        let mut reversed = String::new();
        for value in value.chars().rev() {
            reversed.push(value);
        }
        Ok(runtime_string_value(&reversed, runtime.heap_mut()))
    }

    fn repeat32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "repeat()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "repeat() first argument")?;
        let count = int_arg(&values[1], "repeat() second argument")?;
        if count < 0 {
            bail!("repeat() count must be non-negative");
        }
        Ok(runtime_string_value(&value.repeat(count as usize), runtime.heap_mut()))
    }

    fn char_at32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "char()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "char() first argument")?;
        let index = usize_arg(&values[1], "char() second argument")?;
        Ok(value.chars().nth(index).map_or(RuntimeVal::Nil, |value| {
            runtime_string_value(&value.to_string(), runtime.heap_mut())
        }))
    }

    fn byte_at32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "byte()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "byte() first argument")?;
        let index = usize_arg(&values[1], "byte() second argument")?;
        Ok(value
            .as_bytes()
            .get(index)
            .map_or(RuntimeVal::Nil, |value| RuntimeVal::Int(*value as i64)))
    }

    fn chars32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "chars()")?;
        let mut chars = Vec::new();
        for value in value.chars() {
            chars.push(Arc::<str>::from(value.to_string()));
        }
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(chars))),
        ))
    }

    fn find32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        if args.len() != 2 && args.len() != 3 {
            bail!("find() takes 2 or 3 arguments: string, pattern[, start]");
        }
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "find() first argument")?;
        let pattern = runtime_string_arg(&values[1], runtime.heap(), "find() second argument")?;
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
        let fmt = runtime_string_arg(&values[0], runtime.heap(), "format() first argument")?;
        let rest = &values[1..];
        let mut out = String::with_capacity(fmt.len());
        let mut chars = fmt.chars().peekable();
        let mut arg_index = 0usize;
        while let Some(ch) = chars.next() {
            if ch == '{' && chars.peek() == Some(&'}') {
                chars.next();
                if arg_index < rest.len() {
                    out.push_str(&runtime_display_value(&rest[arg_index], runtime.heap())?);
                    arg_index += 1;
                } else {
                    out.push_str("{}");
                }
            } else {
                out.push(ch);
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
                out.push_str(&runtime_display_value(value, runtime.heap())?);
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

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("len", Self::len32, 1),
                RuntimeNativeExport32::plain("lower", Self::lower32, 1),
                RuntimeNativeExport32::plain("upper", Self::upper32, 1),
                RuntimeNativeExport32::plain("trim", Self::trim32, 1),
                RuntimeNativeExport32::plain("starts_with", Self::starts_with32, 2),
                RuntimeNativeExport32::plain("ends_with", Self::ends_with32, 2),
                RuntimeNativeExport32::plain("contains", Self::contains32, 2),
                RuntimeNativeExport32::plain("replace", Self::replace32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("substring", Self::substring32, 3),
                RuntimeNativeExport32::plain("split", Self::split32, 2),
                RuntimeNativeExport32::plain("join", Self::join32, 2),
                RuntimeNativeExport32::plain("reverse", Self::reverse32, 1),
                RuntimeNativeExport32::plain("repeat", Self::repeat32, 2),
                RuntimeNativeExport32::plain("char", Self::char_at32, 2),
                RuntimeNativeExport32::plain("byte", Self::byte_at32, 2),
                RuntimeNativeExport32::plain("chars", Self::chars32, 1),
                RuntimeNativeExport32::plain("find", Self::find32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("is_empty", Self::is_empty32, 1),
                RuntimeNativeExport32::plain("format", Self::format32, NativeEntry32::VARIADIC),
            ],
            &[],
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

fn one_string(args: NativeArgs32<'_>, runtime: &NativeRuntime32<'_>, name: &str) -> Result<Arc<str>> {
    expect_arity(args, 1, name)?;
    runtime_string_arg(&args.as_slice()[0], runtime.heap(), name)
}

fn two_strings(args: NativeArgs32<'_>, runtime: &NativeRuntime32<'_>, name: &str) -> Result<(Arc<str>, Arc<str>)> {
    expect_arity(args, 2, name)?;
    let values = args.as_slice();
    Ok((
        runtime_string_arg(&values[0], runtime.heap(), name)?,
        runtime_string_arg(&values[1], runtime.heap(), name)?,
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
        TypedList::String(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(value.to_string());
            }
            Ok(out)
        }
        TypedList::Mixed(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(
                    runtime_string_arg(value, heap, context)
                        .map(|value| value.to_string())
                        .map_err(|_| anyhow!("join() list must contain only strings"))?,
                );
            }
            Ok(out)
        }
        _ => Err(anyhow!("join() list must contain only strings")),
    }
}
