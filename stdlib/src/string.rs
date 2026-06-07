use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
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

    fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "len()")?;
        Ok(RuntimeVal::Int(value.len() as i64))
    }

    fn lower(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "lower()")?;
        Ok(runtime_string_value(&value.to_lowercase(), runtime.heap_mut()))
    }

    fn upper(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "upper()")?;
        Ok(runtime_string_value(&value.to_uppercase(), runtime.heap_mut()))
    }

    fn trim(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "trim()")?;
        Ok(runtime_string_value(value.trim(), runtime.heap_mut()))
    }

    fn starts_with(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, prefix) = two_strings(args, runtime, "starts_with()")?;
        Ok(RuntimeVal::Bool(value.starts_with(prefix.as_ref())))
    }

    fn ends_with(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, suffix) = two_strings(args, runtime, "ends_with()")?;
        Ok(RuntimeVal::Bool(value.ends_with(suffix.as_ref())))
    }

    fn contains(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, needle) = two_strings(args, runtime, "contains()")?;
        Ok(RuntimeVal::Bool(value.contains(needle.as_ref())))
    }

    fn replace(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    fn substring(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    fn split(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    fn join(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "join()")?;
        let values = args.as_slice();
        let strings = string_list_arg(&values[0], runtime.heap(), "join() first argument")?;
        let delimiter = runtime_string_arg(&values[1], runtime.heap(), "join() second argument")?;
        Ok(runtime_string_value(
            &strings.join(delimiter.as_ref()),
            runtime.heap_mut(),
        ))
    }

    fn reverse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "reverse()")?;
        let mut reversed = String::new();
        for value in value.chars().rev() {
            reversed.push(value);
        }
        Ok(runtime_string_value(&reversed, runtime.heap_mut()))
    }

    fn repeat(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "repeat()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "repeat() first argument")?;
        let count = int_arg(&values[1], "repeat() second argument")?;
        if count < 0 {
            bail!("repeat() count must be non-negative");
        }
        Ok(runtime_string_value(&value.repeat(count as usize), runtime.heap_mut()))
    }

    fn char_at(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "char()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "char() first argument")?;
        let index = usize_arg(&values[1], "char() second argument")?;
        Ok(value.chars().nth(index).map_or(RuntimeVal::Nil, |value| {
            runtime_string_value(&value.to_string(), runtime.heap_mut())
        }))
    }

    fn byte_at(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 2, "byte()")?;
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "byte() first argument")?;
        let index = usize_arg(&values[1], "byte() second argument")?;
        Ok(value
            .as_bytes()
            .get(index)
            .map_or(RuntimeVal::Nil, |value| RuntimeVal::Int(*value as i64)))
    }

    fn chars(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "chars()")?;
        let mut chars = Vec::new();
        for value in value.chars() {
            chars.push(Arc::<str>::from(value.to_string()));
        }
        Ok(RuntimeVal::Obj(
            runtime.heap_mut().alloc(HeapValue::List(TypedList::String(chars))),
        ))
    }

    fn find(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    fn is_empty(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "is_empty()")?;
        Ok(RuntimeVal::Bool(value.is_empty()))
    }

    fn format(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    fn strip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, pattern) = two_strings(args, runtime, "strip()")?;
        Ok(value
            .strip_prefix(pattern.as_ref())
            .or_else(|| value.strip_suffix(pattern.as_ref()))
            .map_or(RuntimeVal::Nil, |s| runtime_string_value(s, runtime.heap_mut())))
    }

    fn strip_prefix(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, prefix) = two_strings(args, runtime, "strip_prefix()")?;
        Ok(value
            .strip_prefix(prefix.as_ref())
            .map_or(RuntimeVal::Nil, |s| runtime_string_value(s, runtime.heap_mut())))
    }

    fn strip_suffix(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, suffix) = two_strings(args, runtime, "strip_suffix()")?;
        Ok(value
            .strip_suffix(suffix.as_ref())
            .map_or(RuntimeVal::Nil, |s| runtime_string_value(s, runtime.heap_mut())))
    }

    fn count(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, pattern) = two_strings(args, runtime, "count()")?;
        if pattern.is_empty() {
            // Count empty pattern matches between each char + at start and end
            return Ok(RuntimeVal::Int(value.len() as i64 + 1));
        }
        Ok(RuntimeVal::Int(value.matches(pattern.as_ref()).count() as i64))
    }

    fn pad_left(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        if args.len() < 2 || args.len() > 3 {
            bail!("pad_left() takes 2 or 3 arguments: string, width[, fill]");
        }
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "pad_left() string")?;
        let width = usize_arg(&values[1], "pad_left() width")?;
        let fill = if values.len() >= 3 {
            let f = runtime_string_arg(&values[2], runtime.heap(), "pad_left() fill")?;
            if f.is_empty() {
                bail!("pad_left() fill must not be empty");
            }
            f.to_string()
        } else {
            " ".to_string()
        };
        if width <= value.len() {
            return Ok(runtime_string_value(value.as_ref(), runtime.heap_mut()));
        }
        let needed = width - value.len();
        let pad = fill.repeat(needed / fill.len() + 1);
        let padded = format!("{}{}", &pad[pad.len() - needed..], value.as_ref());
        Ok(runtime_string_value(&padded, runtime.heap_mut()))
    }

    fn pad_right(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        if args.len() < 2 || args.len() > 3 {
            bail!("pad_right() takes 2 or 3 arguments: string, width[, fill]");
        }
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "pad_right() string")?;
        let width = usize_arg(&values[1], "pad_right() width")?;
        let fill = if values.len() >= 3 {
            let f = runtime_string_arg(&values[2], runtime.heap(), "pad_right() fill")?;
            if f.is_empty() {
                bail!("pad_right() fill must not be empty");
            }
            f.to_string()
        } else {
            " ".to_string()
        };
        if width <= value.len() {
            return Ok(runtime_string_value(value.as_ref(), runtime.heap_mut()));
        }
        let needed = width - value.len();
        let pad = fill.repeat(needed / fill.len() + 1);
        let padded = format!("{}{}", value.as_ref(), &pad[..needed]);
        Ok(runtime_string_value(&padded, runtime.heap_mut()))
    }

    fn to_int(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "to_int()")?;
        match &args.as_slice()[0] {
            RuntimeVal::Int(v) => Ok(RuntimeVal::Int(*v)),
            RuntimeVal::Float(v) => Ok(RuntimeVal::Int(*v as i64)),
            RuntimeVal::Bool(v) => Ok(RuntimeVal::Int(if *v { 1 } else { 0 })),
            _ => bail!("to_int() argument must be a number or bool"),
        }
    }

    fn to_float(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        expect_arity(args, 1, "to_float()")?;
        match &args.as_slice()[0] {
            RuntimeVal::Float(v) => Ok(RuntimeVal::Float(*v)),
            RuntimeVal::Int(v) => Ok(RuntimeVal::Float(*v as f64)),
            RuntimeVal::Bool(v) => Ok(RuntimeVal::Float(if *v { 1.0 } else { 0.0 })),
            _ => bail!("to_float() argument must be a number or bool"),
        }
    }

    fn title(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "title()")?;
        let mut result = String::with_capacity(value.len());
        let mut capitalize_next = true;
        for ch in value.chars() {
            if ch.is_whitespace() {
                capitalize_next = true;
                result.push(ch);
            } else if capitalize_next {
                for c in ch.to_uppercase() {
                    result.push(c);
                }
                capitalize_next = false;
            } else {
                for c in ch.to_lowercase() {
                    result.push(c);
                }
            }
        }
        Ok(runtime_string_value(&result, runtime.heap_mut()))
    }

    fn capitalize(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "capitalize()")?;
        let mut chars = value.chars();
        let mut result = String::with_capacity(value.len());
        if let Some(first) = chars.next() {
            for c in first.to_uppercase() {
                result.push(c);
            }
        }
        for ch in chars {
            for c in ch.to_lowercase() {
                result.push(c);
            }
        }
        Ok(runtime_string_value(&result, runtime.heap_mut()))
    }
}

impl ModuleProvider for StringModule {
    fn name(&self) -> &str {
        "string"
    }

    fn description(&self) -> &str {
        "String manipulation functions"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("len", Self::len, 1),
                RuntimeNativeExport::plain("lower", Self::lower, 1),
                RuntimeNativeExport::plain("upper", Self::upper, 1),
                RuntimeNativeExport::plain("trim", Self::trim, 1),
                RuntimeNativeExport::plain("starts_with", Self::starts_with, 2),
                RuntimeNativeExport::plain("ends_with", Self::ends_with, 2),
                RuntimeNativeExport::plain("contains", Self::contains, 2),
                RuntimeNativeExport::plain("replace", Self::replace, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("substring", Self::substring, 3),
                RuntimeNativeExport::plain("split", Self::split, 2),
                RuntimeNativeExport::plain("join", Self::join, 2),
                RuntimeNativeExport::plain("reverse", Self::reverse, 1),
                RuntimeNativeExport::plain("repeat", Self::repeat, 2),
                RuntimeNativeExport::plain("char", Self::char_at, 2),
                RuntimeNativeExport::plain("byte", Self::byte_at, 2),
                RuntimeNativeExport::plain("chars", Self::chars, 1),
                RuntimeNativeExport::plain("find", Self::find, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("is_empty", Self::is_empty, 1),
                RuntimeNativeExport::plain("format", Self::format, NativeEntry::VARIADIC),
                RuntimeNativeExport::plain("strip", Self::strip, 2),
                RuntimeNativeExport::plain("strip_prefix", Self::strip_prefix, 2),
                RuntimeNativeExport::plain("strip_suffix", Self::strip_suffix, 2),
                RuntimeNativeExport::plain("count", Self::count, 2),
                RuntimeNativeExport::plain("pad_left", Self::pad_left, 3),
                RuntimeNativeExport::plain("pad_right", Self::pad_right, 3),
                RuntimeNativeExport::plain("to_int", Self::to_int, 1),
                RuntimeNativeExport::plain("to_float", Self::to_float, 1),
                RuntimeNativeExport::plain("title", Self::title, 1),
                RuntimeNativeExport::plain("capitalize", Self::capitalize, 1),
            ],
            &[],
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

fn one_string(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<Arc<str>> {
    expect_arity(args, 1, name)?;
    runtime_string_arg(&args.as_slice()[0], runtime.heap(), name)
}

fn two_strings(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<(Arc<str>, Arc<str>)> {
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
