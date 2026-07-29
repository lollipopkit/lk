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

    /// Removes every leading and trailing character that is in `chars`.
    ///
    /// The parameter has always been named `chars` — a *set* — but the body
    /// stripped the whole string as a prefix, and only if that failed as a
    /// suffix, once:
    ///
    /// ```text
    /// strip("--a--", "-")   → "-a--"
    /// ```
    ///
    /// One end, one occurrence, and `nil` when neither matched. `strip_prefix`
    /// and `strip_suffix` next door are the once-each operations; this one is
    /// what its name and its parameter both said it was, and it always has an
    /// answer.
    #[stdlib_export(params(text: String, chars: String), returns = String)]
    fn strip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let (value, chars) = two_strings(args, runtime, "strip()")?;
        let stripped = value.trim_matches(|c| chars.contains(c));
        Ok(runtime_string_value(stripped, runtime.heap_mut()))
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
        let padded = pad_to_width(value.as_ref(), width, &fill, PadSide::Left);
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
        let padded = pad_to_width(value.as_ref(), width, &fill, PadSide::Right);
        Ok(runtime_string_value(&padded, runtime.heap_mut()))
    }

    /// A number out of a String — or out of another number.
    ///
    /// The String case is the reason this exists: until it did, LK could read a
    /// config file, split a CSV or take an argument and had **no way at all** to
    /// turn `"42"` into `42`. `to_int` was the one name that looked like the
    /// answer and refused a String outright.
    ///
    /// Two failure kinds, deliberately different:
    ///
    /// - **Text that is not a number → `nil`.** "Is this line a number?" is a
    ///   question about input, not a program error, so it answers with a value:
    ///   `line.trim()` then `?? 0` or `!` — the same shape as `index_of`.
    /// - **A Float with no Int → raise.** NaN, the infinities and anything
    ///   outside `i64` are program errors; Rust's `as` would hand back `0` or
    ///   `i64::MAX`, which is a wrong answer dressed as a right one.
    ///
    /// Surrounding whitespace is trimmed: a line read from a file carries its
    /// newline, and `"42\n"` is the same answer as `"42"` to every reader.
    /// `base` (2–36) reads the digits in another radix; the sign may lead it
    /// (`"-ff"`, base 16).
    #[stdlib_export(params(value: String | Number | Bool, base?: Int), returns = Int?)]
    fn to_int(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let values = args.as_slice();
        if values.len() > 2 {
            bail!("to_int() takes 1 or 2 arguments: value[, base]");
        }
        let base = match values.get(1) {
            Some(value) => {
                let base = int_arg(value, "to_int() base")?;
                if !(2..=36).contains(&base) {
                    bail!("to_int() base must be between 2 and 36, got {base}");
                }
                base as u32
            }
            None => 10,
        };
        match &values[0] {
            RuntimeVal::Int(value) => Ok(RuntimeVal::Int(*value)),
            RuntimeVal::Float(value) => Ok(RuntimeVal::Int(float_to_int(*value)?)),
            RuntimeVal::Bool(value) => Ok(RuntimeVal::Int(i64::from(*value))),
            other => {
                let text = runtime_string_arg(other, runtime.heap(), "to_int() first argument")?;
                Ok(match i64::from_str_radix(text.trim(), base) {
                    Ok(value) => RuntimeVal::Int(value),
                    Err(_) => RuntimeVal::Nil,
                })
            }
        }
    }

    /// The `Float` half of [`to_int`], with the same split: unparseable text is
    /// `nil`, everything else converts. `"nan"`, `"inf"` and `"-inf"` parse —
    /// they are Float values, unlike for `to_int`.
    #[stdlib_export(params(value: String | Number | Bool), returns = Float?)]
    fn to_float(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        match &args.as_slice()[0] {
            RuntimeVal::Float(value) => Ok(RuntimeVal::Float(*value)),
            RuntimeVal::Int(value) => Ok(RuntimeVal::Float(*value as f64)),
            RuntimeVal::Bool(value) => Ok(RuntimeVal::Float(if *value { 1.0 } else { 0.0 })),
            other => {
                let text = runtime_string_arg(other, runtime.heap(), "to_float() first argument")?;
                Ok(match text.trim().parse::<f64>() {
                    Ok(value) => RuntimeVal::Float(value),
                    Err(_) => RuntimeVal::Nil,
                })
            }
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

/// A Float as an Int, or a raise.
///
/// `as i64` answers `0` for NaN and saturates at the ends — a wrong number that
/// looks like a right one. LK's rule for a value with no meaning is to fail
/// loudly (`math.sqrt(-4.0)` does), so this does.
fn float_to_int(value: f64) -> Result<i64> {
    if value.is_nan() {
        bail!("to_int() cannot convert NaN to an Int");
    }
    if value.is_infinite() {
        bail!("to_int() cannot convert {value} to an Int");
    }
    // Half-open against the powers of two, not against `i64::MIN`/`MAX` as
    // floats: `i64::MAX as f64` rounds *up* to 2^63, so comparing against it
    // would admit a value one step past the end. No `trunc`/`powi` — neither
    // exists without std, and neither is needed: every representable f64
    // outside this range truncates to something outside it too (the spacing up
    // there is 2048), so the cast below is exact for everything that passes.
    const MIN: f64 = -9_223_372_036_854_775_808.0; // -2^63
    const LIMIT: f64 = 9_223_372_036_854_775_808.0; // 2^63
    if !(MIN..LIMIT).contains(&value) {
        bail!("to_int() cannot convert {value} to an Int: it is outside the Int range");
    }
    // Truncates toward zero, which is what `to_int(3.99)` means.
    Ok(value as i64)
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

enum PadSide {
    Left,
    Right,
}

/// `value` widened to `width` **characters** with `fill`, repeated from its
/// start and cut to length.
///
/// Characters, because that is the unit everything else in the language counts
/// — `s.len()`, `s[i]`, `s.slice(a, b)`. Both pad functions measured in *bytes*
/// and then sliced the repeated fill by byte offset, so a multi-byte fill cut
/// inside a character and **panicked the process**:
///
/// ```text
/// pad_left("a", 5, "中")
/// → panicked: byte index 2 is not a char boundary; it is inside '中'
/// ```
///
/// A Rust panic is not something a script can catch, which puts this in the
/// same family as any other way a program could take the process down.
fn pad_to_width(value: &str, width: usize, fill: &str, side: PadSide) -> String {
    let current = value.chars().count();
    if width <= current {
        return value.to_string();
    }
    let needed = width - current;
    // `cycle().take(n)` needs no slicing, so there is no boundary to get wrong.
    // A multi-character fill therefore reads from its start on both sides;
    // `pad_left` used to align it to the right edge instead, which differed
    // only when the fill did not divide the gap.
    let pad: String = fill.chars().cycle().take(needed).collect();
    match side {
        PadSide::Left => format!("{pad}{value}"),
        PadSide::Right => format!("{value}{pad}"),
    }
}
