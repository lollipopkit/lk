#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// From `alloc` directly, not `lk_core::compat::prelude`: feature
// unification can give lk-core `std` while this crate stays no_std, and
// then that prelude does not exist. What alloc provides does not depend
// on anyone else's features.
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeRuntime},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use crate::runtime_native::{runtime_display_value, runtime_string_arg, runtime_string_value};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "string", docs = "String manipulation functions")]
pub struct StringModule;

#[lk_stdlib_common::stdlib_exports(module = "string")]
impl StringModule {
    #[stdlib_export(params(text: String), returns = Int)]
    fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "len()")?;
        // Characters. This answered bytes while the method form `s.len()`
        // answered characters, so `"héllo wörld"` was 13 here and 11 there.
        Ok(RuntimeVal::Int(lk_core::util::text::char_len(&value) as i64))
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn lower(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "lower()")?;
        Ok(runtime_string_value(&value.to_lowercase(), runtime.heap_mut()))
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn upper(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "upper()")?;
        Ok(runtime_string_value(&value.to_uppercase(), runtime.heap_mut()))
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn trim(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "trim()")?;
        Ok(runtime_string_value(value.trim(), runtime.heap_mut()))
    }

    #[stdlib_export(params(text: String, prefix: String), returns = Bool)]
    fn starts_with(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, prefix) = two_strings(args, runtime, "starts_with()")?;
        Ok(RuntimeVal::Bool(value.starts_with(prefix.as_ref())))
    }

    #[stdlib_export(params(text: String, suffix: String), returns = Bool)]
    fn ends_with(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, suffix) = two_strings(args, runtime, "ends_with()")?;
        Ok(RuntimeVal::Bool(value.ends_with(suffix.as_ref())))
    }

    #[stdlib_export(params(text: String, needle: String), returns = Bool)]
    fn contains(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, needle) = two_strings(args, runtime, "contains()")?;
        Ok(RuntimeVal::Bool(value.contains(needle.as_ref())))
    }

    /// `replace(text, pattern, with, all = true)`.
    ///
    /// `all` used to default to whether the *call* spelled its arguments by
    /// name: `replace("aaa", "a", "b")` answered `"bbb"` and
    /// `replace("aaa", pattern: "a", with: "b")` answered `"baa"` — the same
    /// arguments, a different answer, decided by punctuation. Naming an
    /// argument is supposed to mean exactly what passing it positionally
    /// means, so there is one default now, and it is the positional one.
    #[stdlib_export(
        params(text: String, pattern: String, with: String, all?: Bool = true),
        named(pattern, with, all),
        returns = String
    )]
    fn replace(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        // Named arguments have already been folded into their positional slots
        // by the export wrapper, which is also what rejects a duplicate or an
        // unknown name. This body reads one shape.
        let pos = args.as_slice();
        if pos.len() < 3 {
            bail!("replace() requires a source string, a pattern and a replacement");
        }
        if pos.len() > 4 {
            bail!("replace() received too many positional arguments (expected at most 4)");
        }
        let source = runtime_string_arg(&pos[0], runtime.heap(), "replace() first argument")?;
        let pattern = runtime_string_arg(&pos[1], runtime.heap(), "replace() second argument (pattern)")?;
        let with = runtime_string_arg(&pos[2], runtime.heap(), "replace() third argument (with)")?;
        let all = match pos.get(3) {
            Some(value) => bool_arg(value, "replace() fourth argument (all flag)")?,
            None => true,
        };
        let result = if all {
            source.replace(pattern.as_ref(), with.as_ref())
        } else {
            source.replacen(pattern.as_ref(), with.as_ref(), 1)
        };
        Ok(runtime_string_value(&result, runtime.heap_mut()))
    }

    /// `s.slice(start[, end])`, spelled as a function.
    ///
    /// **Start and end**, not start and length. This was `substring(s, start,
    /// length)` — the one place in the language where a window was given a
    /// count, so `xs.slice(1, 3)` and `substring(s, 1, 3)` cut different
    /// windows out of the same two numbers. The method form is gone; this is
    /// what it became.
    ///
    /// Character positions, and never a panic: byte slicing halted on a
    /// multi-byte boundary, which on an MCU is a halt rather than a message.
    /// Out of range clamps.
    #[stdlib_export(params(text: String, start: Int, end?: Int), named(start, end), returns = String)]
    fn slice(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        if args.len() != 2 && args.len() != 3 {
            bail!("slice() takes 2 or 3 arguments: string, start[, end]");
        }
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "slice() first argument")?;
        let start = usize_arg(&values[1], "slice() second argument")?;
        let total = lk_core::util::text::char_len(&value);
        let end = match values.get(2) {
            Some(RuntimeVal::Nil) | None => total,
            Some(_) => usize_arg(&values[2], "slice() third argument")?,
        };
        let end = end.min(total);
        let text = lk_core::util::text::substring(&value, start, end.saturating_sub(start));
        Ok(runtime_string_value(text, runtime.heap_mut()))
    }

    #[stdlib_export(params(text: String, separator: String), returns = List<String>)]
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

    #[stdlib_export(params(values: List, separator: String), returns = String)]
    fn join(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let strings = string_list_arg(&values[0], runtime.heap(), "join() first argument")?;
        let delimiter = runtime_string_arg(&values[1], runtime.heap(), "join() second argument")?;
        Ok(runtime_string_value(
            &strings.join(delimiter.as_ref()),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn reverse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "reverse()")?;
        let mut reversed = String::new();
        for value in value.chars().rev() {
            reversed.push(value);
        }
        Ok(runtime_string_value(&reversed, runtime.heap_mut()))
    }

    #[stdlib_export(params(text: String, count: Int), returns = String)]
    fn repeat(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "repeat() first argument")?;
        let count = int_arg(&values[1], "repeat() second argument")?;
        if count < 0 {
            bail!("repeat() count must be non-negative");
        }
        Ok(runtime_string_value(&value.repeat(count as usize), runtime.heap_mut()))
    }

    // Exported under the name it is written with. It used to be `string.char`
    // while the method form is `s.byte_at(i)`'s sibling — one operation with
    // two names, which no test could compare and no reader could pair up.
    #[stdlib_export(params(text: String, index: Int), returns = String?)]
    fn char_at(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "char() first argument")?;
        let index = usize_arg(&values[1], "char() second argument")?;
        Ok(value.chars().nth(index).map_or(RuntimeVal::Nil, |value| {
            runtime_string_value(&value.to_string(), runtime.heap_mut())
        }))
    }

    #[stdlib_export(params(text: String, index: Int), returns = Int?)]
    fn byte_at(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "byte() first argument")?;
        let index = usize_arg(&values[1], "byte() second argument")?;
        Ok(value
            .as_bytes()
            .get(index)
            .map_or(RuntimeVal::Nil, |value| RuntimeVal::Int(*value as i64)))
    }

    #[stdlib_export(params(text: String), returns = List<String>)]
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

    /// `s.index_of(needle)`, spelled as a function, plus an optional position
    /// to start looking from — which the method form has no room for.
    ///
    /// This was `find`. The sequence surface calls it `index_of` everywhere
    /// else, and a module function that is a spelling of a method should not
    /// need a second name.
    #[stdlib_export(params(text: String, needle: String, start?: Int), returns = Int?)]
    fn index_of(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        if args.len() != 2 && args.len() != 3 {
            bail!("index_of() takes 2 or 3 arguments: string, needle[, start]");
        }
        let values = args.as_slice();
        let value = runtime_string_arg(&values[0], runtime.heap(), "index_of() first argument")?;
        let pattern = runtime_string_arg(&values[1], runtime.heap(), "index_of() second argument")?;
        let start = if values.len() == 3 {
            usize_arg(&values[2], "index_of() third argument")?
        } else {
            0
        };
        // Character positions in and out, so the answer can be handed straight
        // to `slice`. `start` past the end simply finds nothing.
        Ok(
            lk_core::util::text::find_char_index_from(&value, pattern.as_ref(), start)
                .map_or(RuntimeVal::Nil, |index| RuntimeVal::Int(index as i64)),
        )
    }

    #[stdlib_export(params(text: String), returns = Bool)]
    fn is_empty(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let value = one_string(args, runtime, "is_empty()")?;
        Ok(RuntimeVal::Bool(value.is_empty()))
    }

    #[stdlib_export(params(template: String, ...values: Any), returns = String)]
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

    #[stdlib_export(params(text: String, chars: String), returns = String?)]
    fn strip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, pattern) = two_strings(args, runtime, "strip()")?;
        Ok(value
            .strip_prefix(pattern.as_ref())
            .or_else(|| value.strip_suffix(pattern.as_ref()))
            .map_or(RuntimeVal::Nil, |s| runtime_string_value(s, runtime.heap_mut())))
    }

    #[stdlib_export(params(text: String, prefix: String), returns = String?)]
    fn strip_prefix(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, prefix) = two_strings(args, runtime, "strip_prefix()")?;
        Ok(value
            .strip_prefix(prefix.as_ref())
            .map_or(RuntimeVal::Nil, |s| runtime_string_value(s, runtime.heap_mut())))
    }

    #[stdlib_export(params(text: String, suffix: String), returns = String?)]
    fn strip_suffix(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, suffix) = two_strings(args, runtime, "strip_suffix()")?;
        Ok(value
            .strip_suffix(suffix.as_ref())
            .map_or(RuntimeVal::Nil, |s| runtime_string_value(s, runtime.heap_mut())))
    }

    #[stdlib_export(params(text: String, needle: String), returns = Int)]
    fn count(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, pattern) = two_strings(args, runtime, "count()")?;
        if pattern.is_empty() {
            // Count empty pattern matches between each char + at start and end
            return Ok(RuntimeVal::Int(value.len() as i64 + 1));
        }
        Ok(RuntimeVal::Int(value.matches(pattern.as_ref()).count() as i64))
    }

    #[stdlib_export(params(text: String, width: Int, pad?: String), returns = String)]
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

    #[stdlib_export(params(text: String, width: Int, pad?: String), returns = String)]
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

    #[stdlib_export(params(value: Number | Bool), returns = Int)]
    fn to_int(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match &args.as_slice()[0] {
            RuntimeVal::Int(v) => Ok(RuntimeVal::Int(*v)),
            RuntimeVal::Float(v) => Ok(RuntimeVal::Int(*v as i64)),
            RuntimeVal::Bool(v) => Ok(RuntimeVal::Int(if *v { 1 } else { 0 })),
            _ => bail!("to_int() argument must be a number or bool"),
        }
    }

    #[stdlib_export(params(value: Number | Bool), returns = Float)]
    fn to_float(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match &args.as_slice()[0] {
            RuntimeVal::Float(v) => Ok(RuntimeVal::Float(*v)),
            RuntimeVal::Int(v) => Ok(RuntimeVal::Float(*v as f64)),
            RuntimeVal::Bool(v) => Ok(RuntimeVal::Float(if *v { 1.0 } else { 0.0 })),
            _ => bail!("to_float() argument must be a number or bool"),
        }
    }

    #[stdlib_export(params(text: String), returns = String)]
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

    #[stdlib_export(params(text: String), returns = String)]
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

fn one_string(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<Arc<str>> {
    runtime_string_arg(&args.as_slice()[0], runtime.heap(), name)
}

fn two_strings(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<(Arc<str>, Arc<str>)> {
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
