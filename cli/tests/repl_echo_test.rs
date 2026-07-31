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
