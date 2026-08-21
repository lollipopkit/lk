use std::{
    env,
    io::{self, BufRead, IsTerminal, Write},
    sync::Arc,
};

use lk_core::token::{Token, Tokenizer};
use lk_core::vm::ModuleResolver;
use lk_core::{
    macro_system::MacroDefinitions,
    module::ModuleRegistry,
    syntax::{ParseOptions, ProgramExpansion, expand_program_source, parse_program_source},
    typ::TypeChecker,
    vm::{ReplExecutionResult, ReplVmSession, VmContext},
};

use crate::{
    configure_package_resolver, diagnostic, register_enabled_stdlib, repl_completion::ReplCompletionState, repl_tui,
    startup_trace,
};

/// What an input would have defined, had it succeeded.
///
/// Only a declaring input is worth saying "nothing was defined" about: `g()`
/// failing defines nothing either way. And only a *body-bearing* declaration
/// gets the second sentence — for `let q = Q { b: 1 };` the reason is simply
/// that the input failed, and the rule about bodies would be a wrong
/// explanation rather than an unhelpful one.
///
/// Re-parses, which only happens on the error path — and the input is known to
/// parse, because a parse failure returns before this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputDeclares {
    Nothing,
    /// A `let` / `:=` / type declaration: a name, no body.
    AName,
    /// A `fn` or `impl`: a body, compiled against what the session has *now*.
    ABody,
}

fn input_declares(source: &str, options: ParseOptions) -> InputDeclares {
    use lk_core::stmt::Stmt;
    fn unwrap_attributes(stmt: &Stmt) -> &Stmt {
        match stmt {
            Stmt::Attributed { item, .. } => unwrap_attributes(item),
            other => other,
        }
    }
    // The session's options, not the defaults: an input that uses a macro the
    // session defined does not parse without them, and would be reported as
    // declaring nothing.
    let Ok(program) = parse_program_source(source, options) else {
        return InputDeclares::Nothing;
    };
    let mut declares = InputDeclares::Nothing;
    for stmt in &program.statements {
        match unwrap_attributes(stmt) {
            // A `struct S` brings a generated `fn S$new` with it
            // (`stmt::struct_ctors`), and that body reads nothing but its own
            // parameters — it cannot fail for a name the session lacks. Judging
            // by the *source* declaration keeps `struct Q { … }` out of the
            // body case.
            Stmt::Function { name, .. } if lk_core::stmt::struct_ctors::constructed_struct_name(name).is_some() => {
                declares = InputDeclares::AName;
            }
            Stmt::Function { .. } | Stmt::Impl { .. } => return InputDeclares::ABody,
            Stmt::Struct { .. }
            | Stmt::Trait { .. }
            | Stmt::TypeAlias { .. }
            | Stmt::Let { .. }
            | Stmt::Define { .. } => {
                declares = InputDeclares::AName;
            }
            _ => {}
        }
    }
    declares
}

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
    /// `macro_rules!` definitions entered so far.
    ///
    /// Macros are expanded during *parsing*, and the REPL parses each input as
    /// its own source text — so a definition used to last exactly as long as
    /// the line that made it. `macro_rules! m { … }` was accepted in silence
    /// and `m!()` on the next line answered "no macro named `m` is defined",
    /// while `fn`, `struct`, `impl` and `let` all persisted. Carried into the
    /// next parse, they behave like every other definition the session holds.
    macro_definitions: MacroDefinitions,
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
            macro_definitions: MacroDefinitions::default(),
        })
    }

    /// Parse options carrying what the session has defined so far.
    fn parse_options(&self) -> ParseOptions {
        ParseOptions {
            carried_macro_definitions: self.macro_definitions.clone(),
            ..ParseOptions::default()
        }
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
            Err(e) => {
                diagnostic::error(&e);
                let declares = input_declares(final_src, self.parse_options());
                if declares != InputDeclares::Nothing {
                    // The input takes effect whole or not at all: the session's
                    // state is only updated after `execute_program` returns.
                    // Without saying so, a failed `fn` definition produces two
                    // errors one line apart with nothing connecting them —
                    //
                    //     > fn g() -> Int { return LATER; }
                    //     Error: undefined name `LATER`
                    //     > g()
                    //     Error: undefined function `g`
                    //
                    // and the second reads as a second, unrelated bug.
                    //
                    // The rule it explains: a body is compiled when the line is
                    // entered, so it can only read names that already exist. In
                    // a file the whole program is compiled at once, so a body
                    // there may read a binding declared below it. Making the
                    // REPL match would mean compiling a read of a name that may
                    // never be bound and answering nil for it — the silent
                    // wrong answer `stmt::init_order` exists to refuse.
                    eprint!("  nothing from this input was defined");
                    if declares == InputDeclares::ABody {
                        eprint!(
                            " — a body is compiled as you enter it, so it can only read names the \
                             session already has"
                        );
                    }
                    eprintln!(".");
                }
            }
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
        if let Ok(expansion) = expand_program_source(&wrapped, self.parse_options()) {
            return Some(self.run_expansion(expansion));
        }
        match expand_program_source(source, self.parse_options()) {
            Ok(expansion) => Some(self.run_expansion(expansion)),
            // The program error, not the wrapper's: the wrapper's complains
            // about a `return (…)` the reader never typed.
            Err(program_err) => {
                diagnostic::parse_error(&program_err, source);
                None
            }
        }
    }

    /// Runs an expansion, keeping its macro definitions only if it succeeded —
    /// the same "whole or not at all" rule the session's other state follows.
    fn run_expansion(&mut self, expansion: ProgramExpansion) -> anyhow::Result<ReplExecutionResult> {
        let result = self.vm.execute_program(&expansion.program)?;
        self.macro_definitions = expansion.source.macro_definitions;
        Ok(result)
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
    format!("return ({source});")
}

/// Is this input still open — should the session read another line?
///
/// Decided on **tokens**, not characters. Counting raw `(`/`{`/`[` cannot tell
/// a bracket from a bracket inside a string or a comment, so
///
/// ```text
/// > let s = "(";
/// > s
/// Error: Syntax error: Unexpected tokens at end (found Let) at 2:1-2
/// ```
///
/// — the session went on waiting for a `)` that was never missing, swallowed
/// the next line into the same input, and blamed that line. `// (` at the end
/// of a line did the same. The tokenizer is the thing that decides what a
/// string and a comment are; asking it costs one pass over a line of input.
///
/// A tokenizer error means the line cannot be read as tokens at all — an
/// unterminated string, say — and that is the parser's message to deliver, not
/// a reason to keep waiting. (Waiting would hang the session on any typo.)
pub(crate) fn should_continue_multiline(buf: &str) -> bool {
    if buf.trim_end().ends_with('\\') {
        return true;
    }
    let Ok(tokens) = Tokenizer::tokenize(buf) else {
        return false;
    };
    let mut depth = 0i32;
    for token in &tokens {
        match token {
            Token::LParen | Token::LBrace | Token::LBracket => depth += 1,
            Token::RParen | Token::RBrace | Token::RBracket => depth -= 1,
            _ => {}
        }
    }
    depth > 0
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

    /// A failed input defines nothing, and only a body-bearing declaration
    /// gets told *why* it could not see the name.
    ///
    /// The confusing shape was two unrelated-looking errors one line apart:
    /// `fn g() -> Int { return LATER; }` fails, and then `g()` on the next line
    /// fails with "undefined function `g`" — because the definition never took.
    #[test]
    fn only_a_declaring_input_reports_that_nothing_was_defined() {
        assert_eq!(input_declares("g()", ParseOptions::default()), InputDeclares::Nothing);
        assert_eq!(input_declares("1 + 1", ParseOptions::default()), InputDeclares::Nothing);
        assert_eq!(
            input_declares("let q = 1;", ParseOptions::default()),
            InputDeclares::AName
        );
        assert_eq!(
            input_declares("struct Q { a: Int }", ParseOptions::default()),
            InputDeclares::AName
        );
        assert_eq!(
            input_declares("type N = Int;", ParseOptions::default()),
            InputDeclares::AName
        );
        assert_eq!(
            input_declares("fn g() -> Int { return 1; }", ParseOptions::default()),
            InputDeclares::ABody
        );
        assert_eq!(
            input_declares("impl Q { fn m(self) -> Int { return 1; } }", ParseOptions::default()),
            InputDeclares::ABody
        );
        // A body anywhere in the input wins: that is the one that can fail for
        // a reason the reader cannot see.
        assert_eq!(
            input_declares("let a = 1;\nfn g() -> Int { return a; }", ParseOptions::default()),
            InputDeclares::ABody
        );
        // Unparseable input is reported by the parser, not here.
        assert_eq!(input_declares("fn (", ParseOptions::default()), InputDeclares::Nothing);
    }

    #[test]
    fn multiline_detects_unclosed_delimiters() {
        assert!(should_continue_multiline("println((1)\n"));
        assert!(should_continue_multiline("let xs = [1,\n"));
        assert!(!should_continue_multiline("println(1)\n"));
    }

    /// A bracket inside a string or a comment is not an open bracket.
    ///
    /// Counting characters, `let s = "(";` looked unfinished: the session went
    /// on reading, swallowed the next line into the same input, and reported
    /// `Unexpected tokens at end (found Let)` against it.
    #[test]
    fn a_bracket_in_a_string_or_comment_does_not_hold_the_line_open() {
        assert!(!should_continue_multiline("let s = \"(\";\n"));
        assert!(!should_continue_multiline("let t = \"}\";\n"));
        assert!(!should_continue_multiline("let u = 1; // (\n"));
        assert!(!should_continue_multiline("// [\n"));
        // A real open bracket next to a decoy one still holds.
        assert!(should_continue_multiline("let xs = [\")\",\n"));
    }

    /// Input the tokenizer cannot read is the parser's error to report.
    ///
    /// Treating it as "keep waiting" would hang the session on a typo — there
    /// is no line the reader can type that closes an unterminated string they
    /// did not mean to open.
    #[test]
    fn unlexable_input_does_not_hold_the_line_open() {
        assert!(!should_continue_multiline("let s = \"unterminated\n"));
    }

    #[test]
    fn expression_fallback_returns_the_value_rather_than_printing_it() {
        // The nesting this asserts against — println((println(a))) — is what
        // printed a spurious `nil` after the real output.
        assert_eq!(expression_program_source("println(a)"), "return (println(a));");
        // No textual rewriting of the input on the way in: the wrapper is the
        // only thing added. `normalize_binary_signs` used to insert a space
        // after a binary `+`/`-`, and a with/without differential over fifteen
        // inputs (`a-1`, `a--1`, `-a`, `[1,2][0]-1`, `"a-1 ${a-1}"`, …) was
        // identical — it was patching a lexer behaviour that is not there.
        assert_eq!(expression_program_source("1+1"), "return (1+1);");
    }

    /// A macro defined on one input is usable on the next.
    ///
    /// Driven through `execute_input`, which is the path that carries the
    /// definitions — parsing an input on its own does not, and that was the
    /// defect: the session kept `fn`, `struct`, `impl` and `let`, and dropped
    /// `macro_rules!` without saying so.
    #[cfg(feature = "stdlib")]
    #[test]
    fn a_macro_defined_in_one_input_survives_into_the_next() {
        let mut session = ReplSession::new().expect("repl session");

        session
            .execute_input("macro_rules! twice { ($x:expr) => { ($x) * 2 }; }")
            .expect("the definition is accepted")
            .expect("the definition runs");
        let used = session
            .execute_input("twice!(21)")
            .expect("the macro resolves on a later input")
            .expect("the expansion runs");
        assert_eq!(used.display_first_return(), "42");

        // Re-entering the name replaces it, the way `let` and `fn` do here.
        session
            .execute_input("macro_rules! twice { ($x:expr) => { ($x) * 3 }; }")
            .expect("the redefinition is accepted")
            .expect("the redefinition runs");
        let again = session
            .execute_input("twice!(21)")
            .expect("the redefined macro resolves")
            .expect("the expansion runs");
        assert_eq!(again.display_first_return(), "63");

        // An *import* is collected into the same set, so it was equally lost:
        // `use { vec } from macros;` on its own line left `vec!` undefined, and
        // the builtin macro module was unusable from the REPL entirely.
        session
            .execute_input("use { vec } from macros;")
            .expect("the import is accepted")
            .expect("the import runs");
        let imported = session
            .execute_input("vec![1, 2, 3].len()")
            .expect("the imported macro resolves on a later input")
            .expect("the expansion runs");
        assert_eq!(imported.display_first_return(), "3");
    }

    /// An unannotated parameter is not pinned by the first call.
    ///
    /// The checker applies its solved substitutions to everything it has
    /// recorded once the program is checked — right for one program, and the
    /// REPL checks a sequence of them. `fn f(x) { return x; }` followed by
    /// `f(1)` left `f` recorded as `(Int) -> Int`, so `f("a")` on the next input
    /// answered "Cannot unify Int with String". The same three lines in a file
    /// are fine.
    #[cfg(feature = "stdlib")]
    #[test]
    fn an_open_parameter_is_not_pinned_by_the_first_call() {
        let mut session = ReplSession::new().expect("repl session");

        session
            .execute_input("fn f(x) { return x; }")
            .expect("the definition is accepted")
            .expect("the definition runs");
        for (input, expected) in [("f(1)", "1"), ("f(\"a\")", "a"), ("f([1, 2])", "[1,2]")] {
            let result = session
                .execute_input(input)
                .expect("the call is accepted")
                .unwrap_or_else(|error| panic!("`{input}` after an earlier call: {error}"));
            assert_eq!(result.display_first_return(), expected);
        }

        // A parameter the source *did* annotate still holds its claim.
        session
            .execute_input("fn h(x: Int) -> Int { return x; }")
            .expect("the definition is accepted")
            .expect("the definition runs");
        assert!(
            session
                .execute_input("h(\"a\")")
                .expect("the call is accepted")
                .is_err(),
            "an annotated parameter must still reject a String"
        );
    }

    /// A struct declared on one input keeps its field order on the next.
    ///
    /// Declaration order travels with the type, and the two paths that build an
    /// instance read it from the module being executed. Every REPL input is its
    /// own module, so a struct built after the line that declared it had no
    /// declaration to order by and printed the field map's own iteration.
    /// Six fields, deliberately: with fewer the two orders can coincide.
    #[cfg(feature = "stdlib")]
    #[test]
    fn a_struct_keeps_its_field_order_on_a_later_input() {
        let mut session = ReplSession::new().expect("repl session");

        session
            .execute_input("struct Reading { zebra: Int, apple: Int, mango: Int, kiwi: Int, pear: Int, fig: Int }")
            .expect("the declaration is accepted")
            .expect("the declaration runs");
        let built = session
            .execute_input("\"{}\".format(Reading { zebra: 1, apple: 2, mango: 3, kiwi: 4, pear: 5, fig: 6 })")
            .expect("the construction is accepted")
            .expect("the construction runs");
        assert_eq!(
            built.display_first_return(),
            "Reading{zebra:1,apple:2,mango:3,kiwi:4,pear:5,fig:6}"
        );

        // A spread rebuild goes through the other construction path, which reads
        // the same declaration.
        session
            .execute_input("let base = Reading { zebra: 1, apple: 2, mango: 3, kiwi: 4, pear: 5, fig: 6 };")
            .expect("the binding is accepted")
            .expect("the binding runs");
        let bumped = session
            .execute_input("\"{}\".format(Reading { ..base, apple: 99 })")
            .expect("the rebuild is accepted")
            .expect("the rebuild runs");
        assert_eq!(
            bumped.display_first_return(),
            "Reading{zebra:1,apple:99,mango:3,kiwi:4,pear:5,fig:6}"
        );
    }

    /// A trait's default method reaches an `impl` written on a later input.
    ///
    /// The default bodies are copied into the impls that leave them out during
    /// *parsing*, over one program's statements. An input carrying the `impl`
    /// without the `trait` beside it never saw them, and the checker reported
    /// "Method 'tripled' required by trait 'Scaled' not implemented for type
    /// 'Rect'" — for a method the source never had to write.
    #[cfg(feature = "stdlib")]
    #[test]
    fn a_trait_default_reaches_an_impl_on_a_later_input() {
        let mut session = ReplSession::new().expect("repl session");

        for input in [
            "struct Rect { w: Int }",
            "trait Scaled { fn base(self) -> Int; fn tripled(self) -> Int { return self.w * 3; } }",
            "impl Scaled for Rect { fn base(self) -> Int { return self.w; } }",
        ] {
            session
                .execute_input(input)
                .expect("the declaration is accepted")
                .unwrap_or_else(|error| panic!("`{input}`: {error}"));
        }

        let used = session
            .execute_input("Rect { w: 4 }.tripled()")
            .expect("the call is accepted")
            .expect("the default body runs");
        assert_eq!(used.display_first_return(), "12");

        // The impl's own method still wins over the default.
        let own = session
            .execute_input("Rect { w: 4 }.base()")
            .expect("the call is accepted")
            .expect("the impl's method runs");
        assert_eq!(own.display_first_return(), "4");
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
