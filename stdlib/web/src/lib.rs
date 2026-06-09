use std::cell::RefCell;

use anyhow::{Result, anyhow};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::RuntimeVal,
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};
use lk_stdlib_common::runtime_native::runtime_display_value;

thread_local! {
    static STDOUT: RefCell<String> = const { RefCell::new(String::new()) };
}

const UNSUPPORTED_MODULES: &[&str] = &[
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
    lk_stdlib_slice::register,
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
    if runtime_values_equal(&values[0], &values[1]) {
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
    if !runtime_values_equal(&values[0], &values[1]) {
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

fn runtime_values_equal(left: &RuntimeVal, right: &RuntimeVal) -> bool {
    left == right
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
