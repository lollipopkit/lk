//! What the REPL echoes.
//!
//! Its contract is "type a thing, see its value", and that used to be decided
//! by whether the input *also* happened to be a valid statement: the session
//! tried the program parse first and only fell back to wrapping the input in
//! `return (…)` when that failed. Most expressions need a semicolon to be a
//! statement, so most of them fell through and echoed — while everything that
//! stands alone as a statement computed its value and dropped it:
//!
//! ```text
//! > if true { 1 } else { 2 }        (nothing)
//! > S { x: 8 }                      (nothing)
//! > [1, 2, 3]                       [1,2,3]
//! ```
//!
//! These cases are the ones the accident got wrong, plus the ones it got right,
//! so a future "simplification" back to statement-first fails here.

use assert_cmd::prelude::*;
use std::error::Error;
use std::io::Write;
use std::process::{Command, Stdio};

/// Everything the session should print, in order, for one scripted session.
fn repl_stdout(input: &str) -> Result<String, Box<dyn Error>> {
    let mut child = Command::cargo_bin("lk")?
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child.stdin.as_mut().expect("piped stdin").write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    Ok(String::from_utf8(output.stdout)?)
}

#[test]
fn a_statement_shaped_expression_still_shows_its_value() -> Result<(), Box<dyn Error>> {
    let out = repl_stdout(
        "struct S { x: Int }\n\
         S { x: 8 }\n\
         if true { 1 } else { 2 }\n\
         match 2 { 1 => \"one\", _ => \"other\" }\n\
         { let b = 2; b * 3 }\n",
    )?;
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["S{x:8}", "1", "other", "6"]);
    Ok(())
}

#[test]
fn a_declaration_or_a_terminated_statement_stays_quiet() -> Result<(), Box<dyn Error>> {
    let out = repl_stdout(
        "let a = 5;\n\
         fn f() -> Int { return 9; }\n\
         a + 1;\n\
         a + 1\n\
         f()\n",
    )?;
    // The `let`, the `fn` and the semicolon-terminated `a + 1;` print nothing;
    // a trailing `;` is how a session suppresses its own echo.
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["6", "9"]);
    Ok(())
}

/// A call that prints and returns nil prints once.
///
/// This is why the wrapper returns the value instead of wrapping it in
/// `println`: `println(a)` once ran `println((println(a)))` and echoed the
/// inner call's nil under its output.
#[test]
fn a_printing_call_is_not_echoed_twice() -> Result<(), Box<dyn Error>> {
    let out = repl_stdout("println(\"side effect\")\n")?;
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["side effect"]);
    Ok(())
}

/// Collecting a session's heaps must not re-walk the module graph once per path
/// into it.
///
/// A heap holding an imported function reaches another module's heap through
/// it, and the collector followed every such edge without remembering where it
/// had been — so a module reachable by K paths was collected K times, each
/// repeating the walk beneath it. Every REPL input is its own module and holds
/// a callable for every earlier one, so the paths multiply with the session:
/// cross-module collections went 8 closures -> ~1_000, 12 -> ~20_000,
/// 16 -> ~327_000, and 40 closures under `LK_GC_STRESS=1` did not finish in
/// 200 seconds. It is now flat — the whole session below runs in about a tenth
/// of a second.
///
/// Wall-clock, but not a close call: the bound is over a hundred times the
/// fixed cost, and what it catches is a return to exponential.
#[test]
fn a_session_of_closures_collects_without_re_walking_the_module_graph() -> Result<(), Box<dyn Error>> {
    let mut input = String::new();
    for index in 0..40 {
        input.push_str(&format!("let f{index} = |x| x + {index};\n"));
    }
    input.push_str("f0(1)\n");

    let start = std::time::Instant::now();
    let mut child = Command::cargo_bin("lk")?
        .env("LK_GC_STRESS", "1")
        .env("LK_FORCE_VM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child.stdin.as_mut().expect("piped stdin").write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    let elapsed = start.elapsed();

    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains('1'), "the session should still answer: {stdout}");
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "40 closures under GC stress took {elapsed:?}; the module graph is being re-walked per path"
    );
    Ok(())
}

/// A statement typed over several lines, where the continuation opens no
/// bracket.
///
/// Continuation used to be decided on bracket depth alone, and a method chain
/// closes every bracket it opens on each line. So `let out = nums` ran on its
/// own — "Expected Semicolon, found end of input" — and the `.map(…)` beneath
/// it arrived as a line starting with `.`. Three example programs failed in the
/// session for exactly this and no other reason.
#[test]
fn a_method_chain_split_across_lines_is_one_input() -> Result<(), Box<dyn Error>> {
    let out = repl_stdout(
        "let nums = [1, 2, 3];\n\
         let out = nums\n\
             .map(|v| v * 2)\n\
             .filter(|v| v > 2);\n\
         out\n",
    )?;
    assert!(out.contains("[4,6]"), "{out}");
    Ok(())
}

/// An input that is *wrong* rather than unfinished still stops.
///
/// The continuation test asks the parser whether it ran out of input, and a
/// session that waited on every parse error would hang on a typo with no way
/// out.
#[test]
fn a_wrong_input_is_reported_rather_than_waited_on() -> Result<(), Box<dyn Error>> {
    let out = repl_stdout("let x = 1 2;\nprintln(7)\n")?;
    // The second line still ran, which it could not have if the session were
    // still collecting the first.
    assert!(out.contains('7'), "{out}");
    Ok(())
}
