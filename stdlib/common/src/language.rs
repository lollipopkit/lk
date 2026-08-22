//! The globals the *language* is written in terms of, whatever the host is.
//!
//! Everything else in the standard library is a module a program asks for by
//! name, and a host that cannot back one says so — `use fs` on bare metal
//! answers "not available on bare metal". `error` is not that: it is the global
//! `catch` catches, and a host without it turns every raising program into
//! "undefined function" at run time, after the parser and the type checker have
//! both approved it. `bare-metal-x86`'s interpreter answered exactly that, and
//! the browser playground did too — both build their global list by hand, and
//! both lists were written before `error` was.
//!
//! Nothing here needs an OS: these call through the VM's own machinery and
//! allocate from its heap, both of which a bare host has.

use alloc::sync::Arc;

use anyhow::{Result, anyhow};
use lk_core::val::RuntimeVal;
use lk_core::vm::{NativeArgs, NativeRuntime};

/// `error(value)` — raise, carrying `value` itself where it can be carried.
///
/// A raised heap value has to survive the collection that can happen at any
/// native-call safepoint while the error unwinds, so it is pinned as a GC root
/// until a `catch` binds it. A primitive is `Copy` and needs no pinning; a
/// host with no full VM state cannot pin at all, and falls back to the rendered
/// message — the value is lost, the report is not.
pub fn error(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if let [value] = args.as_slice() {
        let value = *value;
        // Rendered up front: once the error unwinds out of the program, the heap
        // it points into is gone and there is nothing left to render.
        let rendered = joined_display(args.as_slice(), runtime)?;
        let carry_first_class = if matches!(value, RuntimeVal::Obj(_)) {
            match runtime.state_ctx_module_mut() {
                Some((state, _, _)) => {
                    state.set_pending_raise_root(Some(value));
                    true
                }
                None => false,
            }
        } else {
            true
        };
        if carry_first_class {
            return Err(anyhow!(lk_core::vm::LkRaisedValue {
                value,
                rendered: Arc::<str>::from(rendered.as_str()),
            }));
        }
        return Err(anyhow!("{rendered}"));
    }
    let message = if args.is_empty() {
        alloc::string::String::from("error")
    } else {
        joined_display(args.as_slice(), runtime)?
    };
    Err(anyhow!("{message}"))
}

/// Values joined with spaces, each rendered the way the language renders it.
///
/// Through [`display`], which asks the value's `show` — the same question the
/// `{}` path asks. It used to go straight to `runtime_display_value`, so
///
/// ```text
/// println("{}", p)   →  P!        (the impl)
/// println(p)         →  P{a:1}    (the raw struct)
/// ```
///
/// — one value, two renderings, decided by whether a template happened to be
/// there.
fn joined_display(values: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<alloc::string::String> {
    let mut out = alloc::string::String::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let piece = display(value, runtime)?;
        out.push_str(&piece);
    }
    Ok(out)
}

/// `assert`/`assert_eq`/`assert_ne`/`panic` — the same on every host.
///
/// These were written out three times, once per host, and the copies had
/// drifted in three ways at once: `assert_eq` compared *handles* on web and
/// bare (so it failed on any string past seven bytes), `panic` was a Rust
/// `panic!` on the desktop and a catchable error on the other two, and
/// `assert_ne`'s failure message differed. None of that is a platform
/// difference — an assertion is arithmetic on values, and only `print` needs to
/// know where output goes.
///
/// The message text matters as much as the outcome: a program can `catch` a
/// failed assertion and read it.
pub fn assert(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(args, ASSERT_ARITY.0 as usize, ASSERT_ARITY.1 as usize, "assert")?;
    let values = args.as_slice();
    if truthy(&values[0]) {
        return Ok(RuntimeVal::Nil);
    }
    let message = match values.get(1) {
        Some(message) => alloc::format!("assertion failed: {}", display(message, runtime)?),
        None => alloc::string::String::from("assertion failed"),
    };
    Err(anyhow!("{message}"))
}

pub fn assert_eq(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(
        args,
        ASSERT_PAIR_ARITY.0 as usize,
        ASSERT_PAIR_ARITY.1 as usize,
        "assert_eq",
    )?;
    let values = args.as_slice();
    if crate::runtime_native::runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    let actual = display(&values[0], runtime)?;
    let expected = display(&values[1], runtime)?;
    let mut message = alloc::format!("assertion failed: expected {expected}, got {actual}");
    append_note(&mut message, values.get(2), runtime)?;
    Err(anyhow!("{message}"))
}

pub fn assert_ne(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(
        args,
        ASSERT_PAIR_ARITY.0 as usize,
        ASSERT_PAIR_ARITY.1 as usize,
        "assert_ne",
    )?;
    let values = args.as_slice();
    if !crate::runtime_native::runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    // Names the value, which "values should not be equal" did not — and which
    // is the whole reason to read a failed assertion.
    let rendered = display(&values[0], runtime)?;
    let mut message = alloc::format!("assertion failed: expected something other than {rendered}");
    append_note(&mut message, values.get(2), runtime)?;
    Err(anyhow!("{message}"))
}

/// `panic(msg...)` — stop, and do not let `catch` intervene.
///
/// A [`lk_core::vm::LkPanic`], never Rust's `panic!`: unwinding the *host*
/// works on a desktop, is an unrecoverable trap in wasm, and has no unwinder at
/// all on bare metal.
pub fn panic(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let message = if args.is_empty() {
        alloc::string::String::from("panic")
    } else {
        joined_display(args.as_slice(), runtime)?
    };
    Err(anyhow!(lk_core::vm::LkPanic {
        message: alloc::sync::Arc::<str>::from(message.as_str()),
    }))
}

/// Only nil and false are falsy — the VM's rule (`truthy_unchecked`).
fn truthy(value: &RuntimeVal) -> bool {
    !matches!(value, RuntimeVal::Nil | RuntimeVal::Bool(false))
}

/// How a value prints: the user's `show` if its type has one, else the
/// language's own rendering.
///
/// `show` is a *language* rule — `impl Show for Rect` decides what
/// `print(rect)` says — and it lived in the desktop host alone. The other two
/// rendered the raw struct, so the same program printed `Rect(3x4)` on a
/// desktop and `Rect{h:4,w:3}` in the browser and on bare metal.
pub fn display(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<alloc::string::String> {
    if let Some(shown) = display_via_show(value, runtime)? {
        return Ok(shown);
    }
    crate::runtime_native::runtime_display_value(value, runtime.heap())
}

/// The `show` implementation for this value's type, called — or `None` when
/// there is no such type, no such impl, or no context to dispatch through.
fn display_via_show(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<Option<alloc::string::String>> {
    let RuntimeVal::Obj(handle) = value else {
        return Ok(None);
    };
    let Some(lk_core::val::HeapValue::Object(object)) = runtime.heap().get(*handle) else {
        return Ok(None);
    };
    let type_name = alloc::string::String::from(object.type_name().as_ref());
    // The declaring module is the other half of the receiver's type identity;
    // read it before the mutable borrow below takes the heap.
    let receiver_scope = lk_core::vm::receiver_type_scope(value, runtime.heap());
    let Some((state, ctx, module)) = runtime.state_ctx_module_mut() else {
        return Ok(None);
    };
    let Some(ctx) = ctx else {
        return Ok(None);
    };
    let Some(impl_ref) = ctx.trait_method(&receiver_scope, &type_name, "show").cloned() else {
        return Ok(None);
    };
    let result = lk_core::vm::call_trait_method(
        &impl_ref,
        lk_core::vm::TraitMethodRef {
            type_name: &type_name,
            method: "show",
        },
        value,
        None,
        state,
        module,
        Some(ctx),
    )?;
    Ok(match result {
        RuntimeVal::ShortStr(value) => Some(alloc::string::String::from(value.as_str())),
        RuntimeVal::Obj(handle) => match state.heap().get(handle) {
            Some(lk_core::val::HeapValue::String(value)) => Some(alloc::string::String::from(value.as_ref())),
            _ => None,
        },
        _ => None,
    })
}

fn append_note(
    message: &mut alloc::string::String,
    note: Option<&RuntimeVal>,
    runtime: &mut NativeRuntime<'_>,
) -> Result<()> {
    if let Some(note) = note {
        message.push_str(" - ");
        message.push_str(&display(note, runtime)?);
    }
    Ok(())
}

/// How many arguments each assertion takes.
///
/// Public because the registration says the same thing to the type checker, and
/// it says it *from here* — the numbers used to live only inside the check
/// below, which is why `lk check` passed `assert(true, "a", "b")`.
pub const ASSERT_ARITY: (u16, u16) = (1, 2);
/// `assert_eq` / `assert_ne`: two values, and an optional note.
pub const ASSERT_PAIR_ARITY: (u16, u16) = (2, 3);

fn expect_assert_args(args: NativeArgs<'_>, min: usize, max: usize, name: &str) -> Result<()> {
    if args.has_named() {
        return Err(anyhow!("{name}() does not accept named arguments"));
    }
    let len = args.len();
    if (min..=max).contains(&len) {
        Ok(())
    } else if min == max {
        Err(anyhow!("{name}() expects exactly {min} arguments"))
    } else {
        Err(anyhow!("{name}() expects {min} or {max} arguments"))
    }
}

/// `print`/`println`'s argument rendering — one implementation, three hosts.
///
/// The first argument is a template when it is a string: each `{}` takes the
/// next argument, a `{}` with nothing left stays literal, and arguments past
/// the last `{}` are appended space-separated. A first argument that is not a
/// string means there is no template, so everything is joined with spaces.
///
/// This was written out three times. The copies agreed on all of that and
/// disagreed on one line — the leading space when the template renders empty:
///
/// ```text
/// print("", 1, 2)     desktop and web: "1 2"      bare: " 1 2"
/// ```
///
/// Only `print` itself is a platform difference (where the bytes go). What the
/// bytes *are* is the language's, and belongs here.
pub fn format_variadic(args: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<alloc::string::String> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(alloc::string::String::new());
    };
    let Some(template) = string_maybe(first, runtime)? else {
        return joined_display(args, runtime);
    };

    let mut out = alloc::string::String::with_capacity(template.len() + rest.len() * 8);
    let mut chars = template.chars().peekable();
    let mut next_arg = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next();
            match rest.get(next_arg) {
                Some(value) => {
                    out.push_str(&display(value, runtime)?);
                    next_arg += 1;
                }
                // More holes than arguments: the hole stays, rather than
                // silently closing over nothing.
                None => out.push_str("{}"),
            }
        } else {
            out.push(ch);
        }
    }
    // More arguments than holes: append them, separated as they would be with
    // no template at all. No separator before the first if there is nothing to
    // separate it from — the line the three copies disagreed on.
    for value in rest.iter().skip(next_arg) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&display(value, runtime)?);
    }
    Ok(out)
}

/// The string a value is, or `None` when it is not a string.
///
/// A template is a template only if the first argument *is* one; `print(1, 2)`
/// has no template and joins.
fn string_maybe(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<Option<alloc::string::String>> {
    Ok(match value {
        RuntimeVal::ShortStr(value) => Some(alloc::string::String::from(value.as_str())),
        RuntimeVal::Obj(handle) => match runtime.heap().get(*handle) {
            Some(lk_core::val::HeapValue::String(value)) => Some(alloc::string::String::from(value.as_ref())),
            _ => None,
        },
        _ => None,
    })
}
