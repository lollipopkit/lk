use std::{
    env,
    io::{self, BufRead, IsTerminal, Write},
    sync::Arc,
};

use lk_core::vm::ModuleResolver;
use lk_core::{
    module::ModuleRegistry,
    syntax::{ParseOptions, parse_program_source},
    typ::TypeChecker,
    vm::{ReplExecutionResult, ReplVmSession, VmContext},
};

use crate::{
    configure_package_resolver, diagnostic, register_enabled_stdlib, repl_completion::ReplCompletionState, repl_tui,
    startup_trace,
};

pub(crate) enum ReplInput {
    Submit(String),
    Continue,
    Exit,
    FallbackToSimple,
}

enum ReplStep {
    Continue,
    Exit,
}

struct ReplSession {
    vm: ReplVmSession,
    completion_state: ReplCompletionState,
}

impl ReplSession {
    fn new() -> anyhow::Result<Self> {
        let mut startup = startup_trace::StartupTrace::new("repl session");
        let mut registry = ModuleRegistry::new();
        startup.step("module registry created");
        register_enabled_stdlib(&mut registry)?;
        startup.step("stdlib registry configured");
        let mut resolver = ModuleResolver::with_registry(registry);
        startup.step("module resolver created");
        let cwd = env::current_dir()?;
        startup.step("cwd resolved");
        resolver.set_base_dir(cwd.clone());
        configure_package_resolver(&mut resolver, &cwd)?;
        startup.step("package resolver configured");
        let resolver = Arc::new(resolver);
        let ctx = VmContext::new()
            .with_resolver(resolver)
            .with_type_checker(Some(TypeChecker::new_strict()));
        startup.step("vm context created");
        let vm = ReplVmSession::new(ctx, TypeChecker::new());
        startup.step("repl vm session created");

        Ok(Self {
            vm,
            completion_state: ReplCompletionState::new(),
        })
    }

    fn completion_state(&self) -> ReplCompletionState {
        self.completion_state.clone()
    }

    fn execute(&mut self, source: &str) -> ReplStep {
        let final_src = source.trim_end();
        if final_src.trim().is_empty() {
            return ReplStep::Continue;
        }
        if final_src.starts_with(':') {
            return self.execute_command(final_src);
        }

        // **Expression first.** A REPL's contract is "type a thing, see its
        // value", and deciding that by whether the input also happens to be a
        // valid *statement* gets it right only by accident.
        //
        // It used to try the program parse first and fall back to the
        // expression wrapper only when that failed. Most inputs need a
        // semicolon to be a statement, so most inputs fell through and echoed —
        // but everything that is a statement on its own printed nothing at all:
        //
        //     > if true { 1 } else { 2 }        (nothing)
        //     > S { x: 8 }                      (nothing)
        //     > match n { 1 => "one", _ => "" } (nothing)
        //
        // while `[1, 2, 3]` and `x + 1` printed, because a bare list or a bare
        // binary expression is not a statement. The value was computed and
        // dropped every time.
        //
        // The wrapper is `return (…)`, so an input carrying a trailing `;`, a
        // `let`, a declaration or several statements does not parse as one and
        // runs as a program — which is also how `x + 1;` keeps suppressing its
        // own echo.
        let Some(result) = self.execute_input(final_src) else {
            return ReplStep::Continue;
        };

        match result {
            Ok(result) => {
                self.completion_state.append_successful_input(final_src);
                if !result.first_return_is_nil() {
                    println!("{}", result.display_first_return());
                }
            }
            Err(e) => diagnostic::error(&e),
        }
        ReplStep::Continue
    }

    fn execute_command(&mut self, command: &str) -> ReplStep {
        match command {
            ":quit" | ":exit" | ":q" => ReplStep::Exit,
            ":help" => {
                print_repl_help();
                ReplStep::Continue
            }
            _ => {
                eprintln!("Unknown command. Type :help for help.");
                ReplStep::Continue
            }
        }
    }

    /// One expression, else a program; `None` once the parse error is reported.
    fn execute_input(&mut self, source: &str) -> Option<anyhow::Result<ReplExecutionResult>> {
        let wrapped = expression_program_source(source);
        if let Ok(program) = parse_program_source(&wrapped, ParseOptions::default()) {
            return Some(self.vm.execute_program(&program));
        }
        match parse_program_source(source, ParseOptions::default()) {
            Ok(program) => Some(self.vm.execute_program(&program)),
            // The program error, not the wrapper's: the wrapper's complains
            // about a `return (…)` the reader never typed.
            Err(program_err) => {
                diagnostic::parse_error(&program_err, source);
                None
            }
        }
    }
}

impl Drop for ReplSession {
    fn drop(&mut self) {
        self.vm.ctx().shutdown_async_runtime();
    }
}

fn print_repl_help() {
    eprintln!("Commands: :quit | :exit | :q, :help");
}

/// The program the expression fallback runs for a semicolon-less input.
///
/// `return`, not `println`. Wrapping in `println` made the *program* print the
/// value, so an input that already prints — or that evaluates to nil — printed
/// twice: `println(a)` ran `println((println(a)))` and echoed the inner call's
/// nil under its `1`. Returning the value hands it to the REPL instead, which
/// applies the same nil-suppressing rule as the statement path
/// (`first_return_is_nil`) and renders it with the same `runtime_display_value`
/// that `println` uses, so a real value looks exactly as it did before.
fn expression_program_source(source: &str) -> String {
    format!("return ({});", normalize_binary_signs(source))
}

pub(crate) fn should_continue_multiline(buf: &str) -> bool {
    let mut paren = 0i32;
    let mut brace = 0i32;
    let mut bracket = 0i32;
    for ch in buf.chars() {
        match ch {
            '(' => paren += 1,
            ')' => paren -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            '[' => bracket += 1,
            ']' => bracket -= 1,
            _ => {}
        }
    }
    let trailing_backslash = buf.trim_end().ends_with('\\');
    paren > 0 || brace > 0 || bracket > 0 || trailing_backslash
}

fn normalize_binary_signs(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 8);
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let len = chars.len();
    let mut in_single = false;
    let mut in_double = false;
    while i < len {
        let c = chars[i];
        if !in_single && c == '"' && !is_escaped_quote(&chars, i) {
            in_double = !in_double;
            out.push(c);
            i += 1;
            continue;
        }
        if !in_double && c == '\'' && !is_escaped_quote(&chars, i) {
            in_single = !in_single;
            out.push(c);
            i += 1;
            continue;
        }
        if in_single || in_double {
            out.push(c);
            i += 1;
            continue;
        }

        if (c == '+' || c == '-') && i + 1 < len && chars[i + 1].is_ascii_digit() {
            let mut j = i as isize - 1;
            let mut prev: Option<char> = None;
            while j >= 0 {
                let pj = chars[j as usize];
                if pj.is_whitespace() {
                    j -= 1;
                    continue;
                }
                prev = Some(pj);
                break;
            }
            let prev_is_value_like = matches!(
                prev,
                Some(ch)
                    if ch.is_ascii_alphanumeric()
                        || ch == '_'
                        || ch == ')'
                        || ch == ']'
                        || ch == '}'
                        || ch == '"'
                        || ch == '\''
            );

            if prev_is_value_like {
                out.push(c);
                out.push(' ');
                i += 1;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }
    out
}

fn is_escaped_quote(chars: &[char], quote_index: usize) -> bool {
    let mut backslashes = 0usize;
    let mut index = quote_index;
    while index > 0 {
        index -= 1;
        if chars[index] != '\\' {
            break;
        }
        backslashes += 1;
    }
    backslashes % 2 == 1
}

pub fn run(_is_statement_mode: bool) -> anyhow::Result<()> {
    let mut startup = startup_trace::StartupTrace::new("repl");
    let mut session = ReplSession::new()?;
    startup.step("session initialized");
    print_repl_help();
    startup.step("help printed");

    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    startup.step("terminal mode detected");
    if interactive && should_use_reedline_repl() {
        startup.step("enter reedline repl");
        run_tui(&mut session)
    } else if interactive {
        startup.step("enter simple repl");
        run_simple_interactive(&mut session)
    } else {
        startup.step("enter fallback repl");
        run_fallback(&mut session)
    }
}

fn should_use_reedline_repl() -> bool {
    should_use_reedline_repl_from_env(
        std::env::var("LK_REPL_TUI").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
        std::env::var_os("CODEX_CI").is_some(),
        std::env::var_os("CODEX_SANDBOX").is_some(),
        std::env::var_os("CI").is_some(),
    )
}

fn should_use_reedline_repl_from_env(
    lk_repl_tui: Option<&str>,
    term: Option<&str>,
    codex_ci: bool,
    codex_sandbox: bool,
    ci: bool,
) -> bool {
    match lk_repl_tui {
        Some("always" | "1" | "true" | "yes") => return true,
        Some("never" | "0" | "false" | "no") => return false,
        _ => {}
    }

    if codex_ci || codex_sandbox || ci {
        return false;
    }

    !matches!(term, None | Some("") | Some("dumb"))
}

fn run_tui(session: &mut ReplSession) -> anyhow::Result<()> {
    let mut editor = repl_tui::new_editor(session.completion_state())?;
    loop {
        match repl_tui::read_input(&mut editor)? {
            ReplInput::Submit(source) => {
                if matches!(session.execute(&source), ReplStep::Exit) {
                    return Ok(());
                }
            }
            ReplInput::Continue => {
                eprintln!("^C");
            }
            ReplInput::Exit => return Ok(()),
            ReplInput::FallbackToSimple => return run_simple_interactive(session),
        }
    }
}

fn run_simple_interactive(session: &mut ReplSession) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut acc = String::new();
    loop {
        let prompt = if acc.is_empty() { "> " } else { "... " };
        eprint!("{prompt}");
        io::stderr().flush()?;

        let Some(line) = lines.next().transpose()? else {
            return Ok(());
        };
        let trimmed = line.trim_end();
        if trimmed.ends_with('\\') {
            acc.push_str(trimmed.strip_suffix('\\').unwrap_or(trimmed));
            acc.push('\n');
            continue;
        }
        acc.push_str(trimmed);
        acc.push('\n');
        if should_continue_multiline(&acc) {
            continue;
        }
        if matches!(session.execute(&acc), ReplStep::Exit) {
            return Ok(());
        }
        acc.clear();
    }
}

fn run_fallback(session: &mut ReplSession) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut acc = String::new();
    while let Some(line) = lines.next().transpose()? {
        let trimmed = line.trim_end();
        if trimmed.ends_with('\\') {
            acc.push_str(trimmed.strip_suffix('\\').unwrap_or(trimmed));
            acc.push('\n');
            continue;
        }
        acc.push_str(trimmed);
        acc.push('\n');
        if should_continue_multiline(&acc) {
            continue;
        }
        if matches!(session.execute(&acc), ReplStep::Exit) {
            return Ok(());
        }
        acc.clear();
    }
    if !acc.trim().is_empty() {
        session.execute(&acc);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_detects_unclosed_delimiters() {
        assert!(should_continue_multiline("println((1)\n"));
        assert!(should_continue_multiline("let xs = [1,\n"));
        assert!(!should_continue_multiline("println(1)\n"));
    }

    #[test]
    fn expression_fallback_returns_the_value_rather_than_printing_it() {
        // The nesting this asserts against — println((println(a))) — is what
        // printed a spurious `nil` after the real output.
        assert_eq!(expression_program_source("println(a)"), "return (println(a));");
        assert_eq!(expression_program_source("1+1"), "return (1+ 1);");
    }

    #[cfg(feature = "stdlib")]
    #[test]
    fn expression_fallback_echoes_values_but_not_nil_returns() {
        let mut session = ReplSession::new().expect("repl session");

        let run = |session: &mut ReplSession, src: &str| {
            let program = parse_program_source(&expression_program_source(src), ParseOptions::default())
                .expect("expression program parses");
            session.vm.execute_program(&program).expect("expression program runs")
        };

        // println prints its own `1`; the REPL must add nothing after it.
        let printed = run(&mut session, "println(1)");
        assert!(printed.first_return_is_nil());

        let value = run(&mut session, "1+1");
        assert!(!value.first_return_is_nil());
        assert_eq!(value.display_first_return(), "2");

        // Rendering is unchanged from the println wrapper: strings unquoted.
        let text = run(&mut session, "\"x\"");
        assert_eq!(text.display_first_return(), "x");
    }

    #[test]
    fn normalize_binary_signs_preserves_unary_signs() {
        assert_eq!(normalize_binary_signs("1+2"), "1+ 2");
        assert_eq!(normalize_binary_signs("-2"), "-2");
        assert_eq!(normalize_binary_signs("\"1+2\""), "\"1+2\"");
    }

    #[test]
    fn normalize_binary_signs_ignores_escaped_quotes() {
        assert_eq!(normalize_binary_signs(r#""a\"+1""#), r#""a\"+1""#);
        assert_eq!(normalize_binary_signs(r#"'a\'+1'"#), r#"'a\'+1'"#);
    }

    #[test]
    fn reedline_repl_is_disabled_in_codex_proxy_terminals() {
        assert!(!should_use_reedline_repl_from_env(
            None,
            Some("xterm-256color"),
            true,
            false,
            false
        ));
    }

    #[test]
    fn reedline_repl_can_be_forced_for_supported_terminals() {
        assert!(should_use_reedline_repl_from_env(
            Some("always"),
            Some("dumb"),
            true,
            false,
            false
        ));
    }

    #[test]
    fn reedline_repl_is_disabled_for_dumb_terminals() {
        assert!(!should_use_reedline_repl_from_env(
            None,
            Some("dumb"),
            false,
            false,
            false
        ));
    }
}
