use std::{
    env,
    io::{self, BufRead, IsTerminal, Write},
    sync::Arc,
};

use lk_core::{
    module::ModuleRegistry,
    rt,
    stmt::{ModuleResolver, StmtParser},
    token::Tokenizer,
    typ::TypeChecker,
    vm::{ReplExecutionResult, ReplVmSession, VmContext},
};

use crate::{configure_package_resolver, diagnostic, repl_completion::ReplCompletionState, repl_tui};

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
        let mut registry = ModuleRegistry::new();
        lk_stdlib::register_stdlib_globals(&mut registry);
        lk_stdlib::register_stdlib_modules(&mut registry)?;
        let mut resolver = ModuleResolver::with_registry(registry);
        let cwd = env::current_dir()?;
        resolver.set_base_dir(cwd.clone());
        configure_package_resolver(&mut resolver, &cwd)?;
        let resolver = Arc::new(resolver);
        let ctx = VmContext::new()
            .with_resolver(resolver)
            .with_type_checker(Some(TypeChecker::new_strict()));
        let vm = ReplVmSession::new(ctx, TypeChecker::new());

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

        let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(final_src) {
            Ok((tokens, spans)) => (tokens, spans),
            Err(parse_err) => {
                diagnostic::parse_error(&parse_err, final_src);
                return ReplStep::Continue;
            }
        };

        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let Some(result) = (match parser.parse_program_with_enhanced_errors(final_src) {
            Ok(program) => Some(self.vm.execute_program(&program)),
            Err(parse_err) => self.execute_as_expression(final_src, parse_err),
        }) else {
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

    fn execute_as_expression(
        &mut self,
        source: &str,
        statement_error: lk_core::token::ParseError,
    ) -> Option<anyhow::Result<ReplExecutionResult>> {
        let normalized = normalize_binary_signs(source);
        let wrapped = format!("println(({}));", normalized);
        let Ok((tokens, spans)) = Tokenizer::tokenize_enhanced_with_spans(&wrapped) else {
            diagnostic::parse_error(&statement_error, source);
            return None;
        };
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        match parser.parse_program_with_enhanced_errors(&wrapped) {
            Ok(program) => Some(self.vm.execute_program(&program)),
            Err(_expr_err) => {
                diagnostic::parse_error(&statement_error, source);
                None
            }
        }
    }
}

impl Drop for ReplSession {
    fn drop(&mut self) {
        rt::shutdown_runtime();
    }
}

fn print_repl_help() {
    eprintln!("Commands: :quit | :exit | :q, :help");
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
    let mut session = ReplSession::new()?;
    print_repl_help();

    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if interactive && should_use_reedline_repl() {
        run_tui(&mut session)
    } else if interactive {
        run_simple_interactive(&mut session)
    } else {
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
