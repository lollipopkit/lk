use std::cell::RefCell;

use anyhow::{Result, anyhow};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::RuntimeVal,
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use lk_stdlib_common::runtime_native::{runtime_display_value, runtime_values_equal};

thread_local! {
    static STDOUT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The concurrency globals, present and refusing by name.
///
/// `chan` is already an unsupported *module* here, so `use chan` says so.
/// `spawn(f)` said "undefined function `spawn`" — the same absence, reported as
/// if the program had a typo. The playground runs on one thread, so these cannot
/// work; what they can do is say which of the two problems the reader has.
fn unavailable(name: &str) -> Result<RuntimeVal> {
    Err(anyhow!(
        "`{name}` is not available in the browser: the playground is single-threaded"
    ))
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

pub const UNSUPPORTED_MODULES: &[&str] = &[
    "chan", "datetime", "env", "fs", "http", "io", "net", "os", "process", "random", "stream", "task", "time", "uuid",
];

struct WebModuleEntry {
    register: fn(&mut ModuleRegistry) -> Result<()>,
}

macro_rules! define_web_modules {
    ($($register:path),+ $(,)?) => {
        const WEB_MODULES: &[WebModuleEntry] = &[
            $(
                WebModuleEntry {
                    register: $register,
                },
            )+
        ];
    };
}

define_web_modules!(
    lk_stdlib_bytes::register,
    lk_stdlib_encoding::register,
    lk_stdlib_hash::register,
    lk_stdlib_iter::register,
    lk_stdlib_math::register,
    lk_stdlib_path::register,
    lk_stdlib_regex::register,
    lk_stdlib_string::register,
);

pub fn clear_stdout() {
    STDOUT.with(|stdout| stdout.borrow_mut().clear());
}

pub fn take_stdout() -> String {
    STDOUT.with(|stdout| std::mem::take(&mut *stdout.borrow_mut()))
}

pub fn register_web_stdlib_globals(registry: &mut ModuleRegistry) {
    lk_stdlib_common::stdlib_register_runtime_builtins!(
        registry,
        [
            full_state "print" => print, NativeEntry::VARIADIC,
            full_state "println" => println, NativeEntry::VARIADIC,
            full_state "panic" => panic, NativeEntry::VARIADIC,
            full_state "assert" => assert, NativeEntry::VARIADIC,
            full_state "assert_eq" => assert_eq, NativeEntry::VARIADIC,
            full_state "assert_ne" => assert_ne, NativeEntry::VARIADIC,
            // `error`, which is what a `catch` catches. Not a module: a host
            // may leave `fs` out and a program is told so, but a program that
            // raises on this host was told "undefined function" instead.
            full_state "error" => lk_stdlib_common::language::error, NativeEntry::VARIADIC,
            // Present and refusing, rather than absent — see `unavailable`.
            full_state "spawn" => spawn, 1,
            full_state "chan" => chan, NativeEntry::VARIADIC,
            full_state "send" => send, 2,
            full_state "recv" => recv, 1,
        ],
    );
}

pub fn register_web_stdlib_modules(registry: &mut ModuleRegistry) -> Result<()> {
    for module in WEB_MODULES {
        (module.register)(registry)?;
    }
    for name in UNSUPPORTED_MODULES {
        registry.register_module(name, Box::new(UnsupportedWebModule { name }))?;
    }
    Ok(())
}

pub fn register_web_stdlib(registry: &mut ModuleRegistry) -> Result<()> {
    register_web_stdlib_globals(registry);
    register_web_stdlib_modules(registry)
}

fn print(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let text = format_variadic_runtime(args.as_slice(), runtime)?;
    STDOUT.with(|stdout| stdout.borrow_mut().push_str(&text));
    Ok(RuntimeVal::Nil)
}

fn println(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let text = format_variadic_runtime(args.as_slice(), runtime)?;
    STDOUT.with(|stdout| {
        let mut stdout = stdout.borrow_mut();
        stdout.push_str(&text);
        stdout.push('\n');
    });
    Ok(RuntimeVal::Nil)
}

fn panic(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let message = if args.is_empty() {
        "panic".to_string()
    } else {
        join_runtime_display(args.as_slice(), runtime)?
    };
    Err(anyhow!("{message}"))
}

fn assert(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(args, 1, 2, "assert")?;
    let values = args.as_slice();
    if assert_truthy(&values[0]) {
        return Ok(RuntimeVal::Nil);
    }
    let message = if let Some(message) = values.get(1) {
        format!("assertion failed: {}", runtime_display(message, runtime)?)
    } else {
        "assertion failed".to_string()
    };
    Err(anyhow!("{message}"))
}

fn assert_eq(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(args, 2, 3, "assert_eq")?;
    let values = args.as_slice();
    if runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    let actual = runtime_display(&values[0], runtime)?;
    let expected = runtime_display(&values[1], runtime)?;
    let mut message = format!("assertion failed: expected {expected}, got {actual}");
    if let Some(extra) = values.get(2) {
        message.push_str(" - ");
        message.push_str(&runtime_display(extra, runtime)?);
    }
    Err(anyhow!("{message}"))
}

fn assert_ne(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_assert_args(args, 2, 3, "assert_ne")?;
    let values = args.as_slice();
    if !runtime_values_equal(&values[0], &values[1], runtime.heap())? {
        return Ok(RuntimeVal::Nil);
    }
    let mut message = "assertion failed: values should not be equal".to_string();
    if let Some(extra) = values.get(2) {
        message.push_str(" - ");
        message.push_str(&runtime_display(extra, runtime)?);
    }
    Err(anyhow!("{message}"))
}

fn format_variadic_runtime(args: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<String> {
    if args.is_empty() {
        return Ok(String::new());
    }
    let Some(format) = runtime_string_maybe(&args[0], runtime)? else {
        return join_runtime_display(args, runtime);
    };
    let rest = &args[1..];
    let mut out = String::with_capacity(format.len() + rest.len() * 8);
    let mut chars = format.chars().peekable();
    let mut arg_index = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if let Some(value) = rest.get(arg_index) {
                out.push_str(&runtime_display(value, runtime)?);
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
        out.push_str(&join_runtime_display(&rest[arg_index..], runtime)?);
    }
    Ok(out)
}

fn join_runtime_display(args: &[RuntimeVal], runtime: &mut NativeRuntime<'_>) -> Result<String> {
    let mut out = String::new();
    for (index, value) in args.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&runtime_display(value, runtime)?);
    }
    Ok(out)
}

fn runtime_display(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<String> {
    runtime_display_value(value, runtime.heap())
}

fn runtime_string_maybe(value: &RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<Option<String>> {
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

fn assert_truthy(value: &RuntimeVal) -> bool {
    !matches!(value, RuntimeVal::Nil | RuntimeVal::Bool(false))
}

#[derive(Debug)]
struct UnsupportedWebModule {
    name: &'static str,
}

impl ModuleProvider for UnsupportedWebModule {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Unavailable in the browser playground"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Err(anyhow!(
            "module '{}' is not available in the browser playground",
            self.name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::stmt::stmt_parser::StmtParser;
    use lk_core::token::Tokenizer;
    use lk_core::vm::{ModuleResolver, ProgramExec, VmContext};
    use std::sync::Arc;

    fn run(source: &str) -> Result<()> {
        let tokens = Tokenizer::tokenize(source)?;
        let program = StmtParser::new(&tokens).parse_program()?;
        let mut registry = ModuleRegistry::new();
        register_web_stdlib(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)?;
        Ok(())
    }

    /// `assert_eq` in the playground compared values, not handles.
    ///
    /// It was `left == right` — the *derived* `PartialEq` on `RuntimeVal`,
    /// which is structural for a `ShortStr` and handle identity for an `Obj`.
    /// So an assertion held or failed depending on whether its strings fitted
    /// in seven bytes, and the same program passed in the CLI and failed in the
    /// browser.
    #[test]
    fn assert_eq_compares_values_not_handles() {
        // Seven bytes or fewer: inline, and this always worked.
        run(r#"assert_eq("ab", "ab");"#).expect("short strings");
        // Eight or more: a heap object each, and this did not.
        run(r#"assert_eq("abcdefghij", "abcdefghij");"#).expect("long strings");
        run("assert_eq([1, 2], [1, 2]);").expect("lists");
        run(r#"assert_eq({"a": 1}, {"a": 1});"#).expect("maps");
        // And it still tells unequal values apart.
        run(r#"assert_eq("abcdefghij", "abcdefghik");"#).expect_err("different strings must fail");
        run("assert_eq([1, 2], [1, 3]);").expect_err("different lists must fail");
    }
}
