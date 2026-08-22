use std::cell::RefCell;

use anyhow::{Result, anyhow};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::RuntimeVal,
    vm::{NativeArgs, NativeEntry, NativeRuntime, RuntimeExport},
};

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
    let text = lk_stdlib_common::language::format_variadic(args.as_slice(), runtime)?;
    STDOUT.with(|stdout| stdout.borrow_mut().push_str(&text));
    Ok(RuntimeVal::Nil)
}

fn println(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let text = lk_stdlib_common::language::format_variadic(args.as_slice(), runtime)?;
    STDOUT.with(|stdout| {
        let mut stdout = stdout.borrow_mut();
        stdout.push_str(&text);
        stdout.push('\n');
    });
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

fn assert_ne(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    lk_stdlib_common::language::assert_ne(args, runtime)
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
