//! Macros and template strings, which used to be blind to each other.
//!
//! A template is a *single token* to the expander — the lexer hands over its
//! whole content as `Token::TemplateString`, and only the parser, much later,
//! tokenizes what sits inside `${…}`. So neither direction worked: a macro
//! invocation written in a hole was never expanded, and a metavariable written
//! in a hole was never substituted. Both failed with messages that named
//! something the program did not contain.

use crate::vm::execute_source;

#[test]
fn a_macro_invocation_inside_an_interpolation_expands() {
    // The parser reported "no macro named `twice` is defined", resting on
    // "expansion runs first, so anything left over is undefined" — true in every
    // position except this one. The same macro worked two lines away.
    let result = execute_source(
        r#"
        macro_rules! twice {
            ($e:expr) => { ($e) + ($e) };
        }
        return "${twice!(3)}";
        "#,
    )
    .expect("a macro in a template hole should expand");

    assert_eq!(result.display_first_return(), "6");
}

#[test]
fn a_metavariable_inside_an_interpolation_is_substituted() {
    // `Unexpected token: Dollar` before this — formatting an argument into a
    // message is most of the reason to write a macro here, and it was the one
    // shape that did not work.
    let result = execute_source(
        r#"
        macro_rules! show {
            ($e:expr) => { "value = ${$e}" };
        }
        return show!(1 + 2);
        "#,
    )
    .expect("a metavariable in a template hole should substitute");

    assert_eq!(result.display_first_return(), "value = 3");
}

#[test]
fn a_metavariable_and_an_invocation_nest_in_one_template() {
    let result = execute_source(
        r#"
        macro_rules! twice { ($e:expr) => { ($e) + ($e) }; }
        macro_rules! show {
            ($label:expr, $e:expr) => { "${$label} = ${twice!($e)}" };
        }
        return show!("total", 21);
        "#,
    )
    .expect("both rewrites apply to one template");

    assert_eq!(result.display_first_return(), "total = 42");
}

#[test]
fn a_dollar_in_literal_text_is_not_a_metavariable() {
    // Only `${…}` interiors are rewritten. `$e` in the literal part is the two
    // characters, exactly as it is outside a macro — substituting there would
    // silently edit message text.
    let result = execute_source(
        r#"
        macro_rules! show {
            ($e:expr) => { "cost: $e is ${$e}" };
        }
        return show!(5);
        "#,
    )
    .expect("literal text is left alone");

    assert_eq!(result.display_first_return(), "cost: $e is 5");
}

#[test]
fn an_unknown_metavariable_in_an_interpolation_is_an_error() {
    // Passing it through would fail later as `Unexpected token: Dollar`, which
    // names a `$` the program did write but blames the wrong layer.
    let error = execute_source(
        r#"
        macro_rules! show {
            ($e:expr) => { "${$typo}" };
        }
        return show!(1);
        "#,
    )
    .expect_err("an undefined metavariable must be reported as one");

    assert!(
        format!("{error}").contains("Unknown macro metavariable `$typo`"),
        "{error}"
    );
}

#[test]
fn a_template_with_no_macros_keeps_its_text_byte_for_byte() {
    // Rewriting is by lexeme, so an interior that round-trips through the token
    // stream would come back as `a . b` and `{ "k" : 1 }`. A segment whose tokens
    // are unchanged has to keep its original text, or every program in the tree
    // would be quietly reformatted inside its strings.
    let result = execute_source(
        r#"
        struct P { a: Int }
        let p = P { a: 7 };
        return "${p.a} ${ {"k": 1}.len() } ${[1, 2].len()}";
        "#,
    )
    .expect("a template with no macro in it still parses");

    assert_eq!(result.display_first_return(), "7 1 2");
}

#[test]
fn nested_braces_still_bound_the_interpolation() {
    // The scan lives in `token::split_template_string` now because there were
    // two of them and they disagreed here: the parser's copy cut at the first
    // `}`, so `"${R {}}"` arrived as `R {`. The expander is the third reader.
    let result = execute_source(
        r#"
        macro_rules! id { ($e:expr) => { $e }; }
        return "${ id!({"k": 1}.len()) }";
        "#,
    )
    .expect("braces nest inside a hole");

    assert_eq!(result.display_first_return(), "1");
}
