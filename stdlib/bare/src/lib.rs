//! Bare-metal standard library surface for LK.
//!
//! This is the `stdlib/web` pattern applied to hardware: platform capabilities
//! are swapped at the stdlib layer rather than behind a capability-trait HAL,
//! so a target without an OS supplies its own [`ModuleRegistry`] population.
//! Everything that needs a filesystem, a network, a clock or threads is
//! registered as *present but unavailable*, so a program importing it gets a
//! clear error instead of a confusing "unknown module".
//!
//! Output has no fixed destination here: an MCU's console might be semihosting,
//! a UART, an RTT channel or a ring buffer. The host installs one with
//! [`set_output`]; until it does, `print`/`println` are silently discarded
//! rather than being an error, so a program that logs still runs headless.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};

use anyhow::{Result, anyhow};
use lk_core::{
    compat::sync::Mutex,
    module::{ModuleProvider, ModuleRegistry},
    val::RuntimeVal,
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use lk_stdlib_common::runtime_native::{runtime_display_value, runtime_values_equal};

/// Where `print`/`println` go. A plain `fn` pointer rather than a closure so
/// the slot is `const`-initialisable and needs no allocation before `main`.
type OutputSink = fn(&str);

static OUTPUT: Mutex<Option<OutputSink>> = Mutex::new(None);

/// Install the console. Call once during board bring-up, before running LK.
// `lock()` is fallible under std (poisoning) and infallible under no_std, and
// workspace feature unification decides which one this crate sees — so the
// `if let` is irrefutable in one configuration and required in the other.
#[allow(irrefutable_let_patterns)]
pub fn set_output(sink: OutputSink) {
    if let Ok(mut slot) = OUTPUT.lock() {
        *slot = Some(sink);
    }
}

fn emit(text: &str) {
    // The sink is copied out and the guard dropped before calling it: a sink
    // that itself logs would otherwise deadlock on a spin mutex.
    let sink = OUTPUT.lock().ok().and_then(|slot| *slot);
    if let Some(sink) = sink {
        sink(text);
    }
}

/// The computation-only modules, which work unchanged without an OS. Each is
/// a cargo feature so a board pays flash only for what it imports.
const BARE_MODULES: &[fn(&mut ModuleRegistry) -> Result<()>] = &[
    #[cfg(feature = "bytes")]
    lk_stdlib_bytes::register,
    #[cfg(feature = "encoding")]
    lk_stdlib_encoding::register,
    #[cfg(feature = "hash")]
    lk_stdlib_hash::register,
    #[cfg(feature = "iter")]
    lk_stdlib_iter::register,
    #[cfg(feature = "math")]
    lk_stdlib_math::register,
    #[cfg(feature = "string")]
    lk_stdlib_string::register,
];

/// Modules that exist in LK but cannot be backed by anything on bare metal.
/// Kept explicit so the error names the reason rather than the symptom.
pub const UNSUPPORTED_MODULES: &[&str] = &[
    "chan", "datetime", "env", "fs", "http", "io", "net", "os", "path", "process", "random", "regex", "stream", "task",
    "time", "uuid",
];

/// Registers the globals (`print`, `println`, `panic`, `assert*`), the
/// computation-only modules, and placeholders for the ones an OS would be
/// needed for.
pub fn register_bare_stdlib(registry: &mut ModuleRegistry) -> Result<()> {
    register_bare_stdlib_globals(registry);
    register_bare_stdlib_modules(registry)
}

pub fn register_bare_stdlib_globals(registry: &mut ModuleRegistry) {
    lk_stdlib_common::stdlib_register_runtime_builtins!(
        registry,
        [
            full_state "print" => print, NativeEntry::VARIADIC,
            full_state "println" => println, NativeEntry::VARIADIC,
            full_state "panic" => panic, NativeEntry::VARIADIC,
            full_state "assert" => assert, NativeEntry::VARIADIC,
            full_state "assert_eq" => assert_eq, NativeEntry::VARIADIC,
            full_state "assert_ne" => assert_ne, NativeEntry::VARIADIC,
            // `error`, which is what a `catch` catches.
            //
            // Not a module: a host may leave `fs` out and a program importing it
            // is told so, by name. This is a global the language's own error
            // handling is written in terms of, and without it every
            // `try { error(…) } catch` that `bare-metal-x86`'s interpreter ran
            // failed at run time — after the program had been parsed and
            // accepted — with a stage code that says only "it raised".
            full_state "error" => lk_stdlib_common::language::error, NativeEntry::VARIADIC,
            // Present and refusing, rather than absent — see `unavailable`.
            full_state "spawn" => spawn, 1,
            full_state "chan" => chan, NativeEntry::VARIADIC,
            full_state "send" => send, 2,
            full_state "recv" => recv, 1,
        ],
    );
}

/// The concurrency globals, present and refusing by name.
///
/// `chan` is already in `UNSUPPORTED_MODULES`, so `use chan` answers "not
/// available on bare metal". `spawn(f)` answered "undefined function `spawn`" —
/// the same absence, reported as if the program had a typo. There is one task on
/// this host and no way to make a second, so these cannot work; what they can do
/// is say which of the two problems the reader has.
fn unavailable(name: &str) -> Result<RuntimeVal> {
    Err(anyhow!("`{name}` is not available on bare metal: there is one task"))
}

fn spawn(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    unavailable("spawn")
}

fn chan(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    unavailable("chan")
}

fn send(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    unavailable("send")
}

fn recv(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    unavailable("recv")
}

fn assert_ne(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    if values.len() < 2 {
        return Err(anyhow!("assert_ne expects at least 2 arguments"));
    }
    if !runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    let rendered = display(&values[0], runtime)?;
    Err(anyhow!("assertion failed: expected something other than {rendered}"))
}

pub fn register_bare_stdlib_modules(registry: &mut ModuleRegistry) -> Result<()> {
    for register in BARE_MODULES {
        register(registry)?;
    }
    for name in UNSUPPORTED_MODULES {
        registry.register_module(name, Box::new(UnsupportedBareModule { name }))?;
    }
    Ok(())
}

fn print(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    emit(&format_variadic(args.as_slice(), runtime)?);
    Ok(RuntimeVal::Nil)
}

fn println(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let mut text = format_variadic(args.as_slice(), runtime)?;
    text.push('\n');
    emit(&text);
    Ok(RuntimeVal::Nil)
}

fn panic(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let message = if args.is_empty() {
        "panic".to_string()
    } else {
        format_variadic(args.as_slice(), runtime)?
    };
    Err(anyhow!("{message}"))
}

fn assert(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    let Some(condition) = values.first() else {
        return Err(anyhow!("assert expects at least 1 argument"));
    };
    if truthy(condition) {
        return Ok(RuntimeVal::Nil);
    }
    match values.get(1) {
        Some(message) => Err(anyhow!("assertion failed: {}", display(message, runtime)?)),
        None => Err(anyhow!("assertion failed")),
    }
}

fn assert_eq(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let values = args.as_slice();
    if values.len() < 2 {
        return Err(anyhow!("assert_eq expects at least 2 arguments"));
    }
    if runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    let actual = display(&values[0], runtime)?;
    let expected = display(&values[1], runtime)?;
    Err(anyhow!("assertion failed: expected {expected}, got {actual}"))
}

fn truthy(value: &RuntimeVal) -> bool {
    !matches!(value, RuntimeVal::Nil | RuntimeVal::Bool(false))
}

fn display(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<String> {
    runtime_display_value(value, runtime.heap())
}

/// `println("{} of {}", a, b)`-style formatting: a leading string argument acts
/// as a template whose `{}` holes consume the rest, and anything left over is
/// appended space-separated. Without a leading string, all arguments are simply
/// joined by spaces.
fn format_variadic(args: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<String> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(String::new());
    };

    let Some(template) = string_maybe(first, runtime)? else {
        return join_with_spaces(args, runtime);
    };

    let mut out = String::with_capacity(template.len() + rest.len() * 8);
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
                None => out.push_str("{}"),
            }
        } else {
            out.push(ch);
        }
    }
    for value in &rest[next_arg.min(rest.len())..] {
        out.push(' ');
        out.push_str(&display(value, runtime)?);
    }
    Ok(out)
}

/// A string argument may be inline (`ShortStr`) or on the heap — only short
/// ones are inline, so matching just `ShortStr` silently fails to treat any
/// realistic format string as a template.
fn string_maybe(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<Option<String>> {
    Ok(match value {
        RuntimeVal::ShortStr(value) => Some(value.as_str().to_string()),
        RuntimeVal::Obj(handle) => match runtime.heap().get(*handle) {
            Some(lk_core::val::HeapValue::String(value)) => Some(value.to_string()),
            Some(_) => None,
            None => return Err(anyhow!("heap object {} out of bounds", handle.index())),
        },
        _ => None,
    })
}

fn join_with_spaces(args: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<String> {
    let mut out = String::new();
    for (index, value) in args.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&display(value, runtime)?);
    }
    Ok(out)
}

#[derive(Debug)]
struct UnsupportedBareModule {
    name: &'static str,
}

impl ModuleProvider for UnsupportedBareModule {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Unavailable on bare metal (no OS)"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Err(anyhow!("module '{}' is not available on bare metal", self.name))
    }
}
