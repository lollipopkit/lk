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

use anyhow::{Result, anyhow, bail};
use lk_core::{
    val::{HeapStore, HeapValue, RuntimeVal, TypedList},
    vm::{NativeArgs, NativeRuntime},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "string", docs = "String manipulation functions")]
pub struct StringModule;

#[lk_stdlib_common::stdlib_exports(module = "string")]
impl StringModule {
    #[stdlib_export(params(text: String), returns = Int)]
    fn len(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("len", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn lower(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("lower", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn upper(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("upper", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn trim(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("trim", args, runtime)
    }

    #[stdlib_export(params(text: String, prefix: String), returns = Bool)]
    fn starts_with(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("starts_with", args, runtime)
    }

    #[stdlib_export(params(text: String, suffix: String), returns = Bool)]
    fn ends_with(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("ends_with", args, runtime)
    }

    #[stdlib_export(params(text: String, needle: String), returns = Bool)]
    fn contains(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("contains", args, runtime)
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
        forward("replace", args, runtime)
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
        forward("slice", args, runtime)
    }

    #[stdlib_export(params(text: String, separator: String), returns = List<String>)]
    fn split(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("split", args, runtime)
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
        forward("reverse", args, runtime)
    }

    #[stdlib_export(params(text: String, count: Int), returns = String)]
    fn repeat(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("repeat", args, runtime)
    }

    /// `s.get(i)` — the character at a position, `nil` out of range.
    ///
    /// This was `string.char_at`, and before that `string.char`, while the
    /// element accessor every other sequence carrier spells is `get`
    /// (`xs.get(i)`, `bytes.get(b, i)`, and `s.get(i)` here). A third name for
    /// it also meant a third rule: `char_at` refused a negative index, where
    /// `s[-1]`, `s.get(-1)` and the native `str.char_at` symbol all count back
    /// from the end. `byte_at` keeps its name because it answers a different
    /// thing — a byte, not an element.
    #[stdlib_export(params(text: String, index: Int), returns = String?)]
    fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("get", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = String?)]
    fn first(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("first", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = String?)]
    fn last(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("last", args, runtime)
    }

    #[stdlib_export(params(text: String, count: Int), returns = String)]
    fn take(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("take", args, runtime)
    }

    #[stdlib_export(params(text: String, count: Int), returns = String)]
    fn skip(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("skip", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = Bytes)]
    fn bytes(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("bytes", args, runtime)
    }

    #[stdlib_export(params(text: String, index: Int), returns = Int?)]
    fn byte_at(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("byte_at", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = List<String>)]
    fn chars(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("chars", args, runtime)
    }

    /// `s.index_of(needle)`, spelled as a function, plus an optional position
    /// to start looking from — which the method form has no room for.
    ///
    /// This was `find`. The sequence surface calls it `index_of` everywhere
    /// else, and a module function that is a spelling of a method should not
    /// need a second name.
    #[stdlib_export(params(text: String, needle: String, start?: Int), returns = Int?)]
    fn index_of(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("index_of", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = Bool)]
    fn is_empty(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("is_empty", args, runtime)
    }

    /// `"{} and {}".format(a, b)` — the receiver is the template.
    #[stdlib_export(params(template: String, ...values: Any), returns = String)]
    fn format(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        if args.is_empty() {
            bail!("format() requires at least 1 argument (format string)");
        }
        forward("format", args, runtime)
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
        forward("strip", args, runtime)
    }

    #[stdlib_export(params(text: String, prefix: String), returns = String?)]
    fn strip_prefix(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("strip_prefix", args, runtime)
    }

    #[stdlib_export(params(text: String, suffix: String), returns = String?)]
    fn strip_suffix(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("strip_suffix", args, runtime)
    }

    #[stdlib_export(params(text: String, needle: String), returns = Int)]
    fn count(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("count", args, runtime)
    }

    #[stdlib_export(params(text: String, width: Int, pad?: String), returns = String)]
    fn pad_left(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("pad_left", args, runtime)
    }

    #[stdlib_export(params(text: String, width: Int, pad?: String), returns = String)]
    fn pad_right(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("pad_right", args, runtime)
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
        forward("title", args, runtime)
    }

    #[stdlib_export(params(text: String), returns = String)]
    fn capitalize(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        forward("capitalize", args, runtime)
    }
}

/// The module spelling of a method: the receiver written first.
///
/// `string.upper(s)` **is** `s.upper()`, and this is what makes that true by
/// construction rather than by two bodies agreeing. They did not agree: the
/// module refused a negative `slice` start while the method counted from the
/// end (the language's own rule), `split(s, "")` answered `["a","b","c"]` here
/// and `["","a","b","c",""]` there, and `byte_at(s, -1)` raised here and
/// answered nil there. Three different answers for three spellings of one
/// question is what two implementations buy.
///
/// `iter` was built this way for exactly this reason — see the note on
/// `core_call_method_windowed`.
fn forward(method: &'static str, args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let Some((receiver, rest)) = values.split_first() else {
        bail!("string.{method} expects a string as its first argument");
    };
    lk_core::vm::core_call_method_windowed(*receiver, method, rest, runtime)
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
