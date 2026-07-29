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

use anyhow::{Result, anyhow};
use lk_core::{
    compat::sync::Mutex,
    module::{ModuleProvider, ModuleRegistry},
    val::RuntimeVal,
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};

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
    lk_stdlib_common::language::assert_ne(args, runtime)
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
    emit(&lk_stdlib_common::language::format_variadic(args.as_slice(), runtime)?);
    Ok(RuntimeVal::Nil)
}

fn println(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let mut text = lk_stdlib_common::language::format_variadic(args.as_slice(), runtime)?;
    text.push('\n');
    emit(&text);
    Ok(RuntimeVal::Nil)
}

fn panic(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::language::panic(args, runtime)
}

// `assert`/`assert_eq`/`assert_ne`/`panic` are the same on every host — an
// assertion is arithmetic on values, and only `print` needs to know where
// output goes. They were written out three times and had drifted three ways;
// see `lk_stdlib_common::language`.
fn assert(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::language::assert(args, runtime)
}

fn assert_eq(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::language::assert_eq(args, runtime)
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
