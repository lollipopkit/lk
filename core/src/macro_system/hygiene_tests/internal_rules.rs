//! `@` as an internal-rule marker, the way Rust's `macro_rules!` use it.
//!
//! A declarative macro has no accumulator: adding a column of numbers up means a
//! rule that calls itself carrying the running total. That rule must not be
//! reachable from a caller, and the way that is arranged is a token which is
//! legal in a token stream and illegal in every position a person would write
//! one. `@` had no meaning in this language, which is exactly what makes it the
//! right one — LK's macros are Rust-shaped, and a macro ported from Rust reaches
//! for it immediately.

use crate::{
    syntax::{expand_source, render_tokens},
    vm::execute_source,
};

/// The case the token was added for: offsets from widths.
///
/// Each constant is the running total *before* its field, and the size is the
/// total after the last one — so the size cannot disagree with the layout above
/// it, which is the number a hand-written column always gets wrong.
#[test]
fn an_internal_rule_can_accumulate_across_a_recursion() {
    let result = execute_source(
        r#"
        macro_rules! layout {
            (@from $prev:expr, => $size:ident) => {
                const $size = $prev;
            };
            (@from $prev:expr, $name:ident : $width:expr, $($rest:tt)*) => {
                const $name = $prev;
                layout!(@from ($prev) + ($width), $($rest)*);
            };
            ($($body:tt)*) => {
                layout!(@from 0, $($body)*);
            };
        }
        layout! {
            DEST: 6,
            SOURCE: 6,
            KIND: 2,
            => HEADER
        }
        return DEST * 1000000 + SOURCE * 10000 + KIND * 100 + HEADER;
        "#,
    )
    .expect("macro program should execute");

    // 0, 6, 12, 14 — the widths added up, not machine words.
    assert_eq!(result.display_first_return(), "61214");
}

/// The marker is what separates the rules.
///
/// Without it the internal rule and the public one would both be "some tokens",
/// and the first would shadow the second. Checked by *expanding*: a caller's
/// invocation has to reach the entry rule and come back with the internal ones
/// already resolved.
#[test]
fn a_caller_reaches_the_entry_rule_and_not_the_internal_one() {
    let expanded = expand_source(
        r#"
        macro_rules! pick {
            (@internal $x:expr) => { ($x) + 100 };
            ($x:expr) => { pick!(@internal $x) };
        }
        let a = pick!(1);
        "#,
        Default::default(),
    )
    .expect("macro program should expand");
    let text = render_tokens(&expanded.tokens);
    assert!(
        text.contains("100"),
        "the entry rule should have gone through the internal one, got: {text}"
    );
    assert!(
        !text.contains('@'),
        "no `@` should survive expansion, got: {text}"
    );
}

/// Outside a macro, `@` is still an error — a parse error rather than a lexer
/// one, which is the same answer with a better message.
#[test]
fn an_at_outside_a_macro_is_rejected() {
    let error = execute_source("let x = 1 @ 2;\nreturn x;\n").expect_err("`@` is not an operator");
    let text = format!("{error:#}");
    assert!(
        text.contains("At") || text.contains('@'),
        "the diagnostic should name the token, got: {text}"
    );
}
