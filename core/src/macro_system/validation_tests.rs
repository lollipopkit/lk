use crate::{
    syntax::{expand_source, parse_program_source, render_tokens},
    vm::execute_source,
};

#[test]
fn rejects_repeated_metavariable_outside_template_repetition_at_parse_time() {
    let err = parse_program_source(
        r#"
        macro_rules! bad {
            ($($value:expr),*) => { [$value] };
        }
        return 0;
        "#,
        Default::default(),
    )
    .expect_err("repeated metavariable must stay inside a template repetition");

    let message = err.to_string();
    assert!(message.contains("appears at repetition depth 0 in the template but was bound at depth 1"));
}

#[test]
fn rejects_non_repeated_metavariable_inside_template_repetition() {
    let err = parse_program_source(
        r#"
        macro_rules! bad {
            ($value:expr) => { $($value),* };
        }
        return 0;
        "#,
        Default::default(),
    )
    .expect_err("non-repeated metavariable cannot drive a template repetition");

    let message = err.to_string();
    assert!(message.contains("appears at repetition depth 1 in the template but was bound at depth 0"));
}

#[test]
fn rejects_duplicate_matcher_metavariable_bindings() {
    let err = parse_program_source(
        r#"
        macro_rules! bad {
            ($value:expr, $value:expr) => { $value };
        }
        return 0;
        "#,
        Default::default(),
    )
    .expect_err("matcher metavariable names must be unique");

    assert!(err.to_string().contains("Macro metavariable `$value` is already bound"));
}

#[test]
fn expands_two_dimensional_nested_repetition() {
    let result = execute_source(
        r#"
        macro_rules! matrix {
            ($($( $value:expr ),+);*) => { [$( [ $($value),* ] ),*] };
        }
        return matrix!(1, 2; 3, 4).1.0;
        "#,
    )
    .expect("nested repetition should expand and execute");

    assert_eq!(result.display_first_return(), "3");
}

#[test]
fn expands_empty_repetition_driven_by_metavariable() {
    let result = execute_source(
        r#"
        macro_rules! list {
            ($($value:expr),*) => { [$($value),*] };
        }
        return list!().len();
        "#,
    )
    .expect("zero-match repetition should still bind an empty capture shape");

    assert_eq!(result.display_first_return(), "0");
}

#[test]
fn expands_empty_outer_nested_repetition_without_panicking() {
    let result = execute_source(
        r#"
        macro_rules! matrix {
            ($($( $value:expr ),+);*) => { [$( [ $($value),* ] ),*] };
        }
        return matrix!().len();
        "#,
    )
    .expect("empty outer nested repetition should expand to an empty list");

    assert_eq!(result.display_first_return(), "0");
}

#[test]
fn expands_nested_optional_repetition_when_present_or_absent() {
    let present = execute_source(
        r#"
        macro_rules! maybe_row {
            ($($( $value:expr ),+)?) => { [$( [ $($value),* ] )?] };
        }
        return maybe_row!(1, 2).0.1;
        "#,
    )
    .expect("present optional nested repetition should expand");
    assert_eq!(present.display_first_return(), "2");

    let absent = execute_source(
        r#"
        macro_rules! maybe_row {
            ($($( $value:expr ),+)?) => { [$( [ $($value),* ] )?] };
        }
        return maybe_row!().len();
        "#,
    )
    .expect("absent optional nested repetition should expand");
    assert_eq!(absent.display_first_return(), "0");
}

#[test]
fn rejects_trailing_separator_in_repetition_invocation() {
    let err = parse_program_source(
        r#"
        macro_rules! list {
            ($($value:expr),*) => { [$($value),*] };
        }
        return list!(1,);
        "#,
        Default::default(),
    )
    .expect_err("separator repetition should not consume a trailing separator");

    assert!(err.to_string().contains("No matching rule for macro `list`"));
}

#[test]
fn nested_repetition_does_not_emit_trailing_separator() {
    let expanded = expand_source(
        r#"
        macro_rules! matrix {
            ($($( $value:expr ),+);*) => { [$( [ $($value),* ] ),*] };
        }
        return matrix!(1, 2; 3);
        "#,
        Default::default(),
    )
    .expect("nested repetition should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("[[1, 2], [3]]"), "{rendered}");
    assert!(!rendered.contains("[1, 2,]"), "{rendered}");
    assert!(!rendered.contains("[3,]"), "{rendered}");
}

#[test]
fn rejects_nested_repetition_inner_arity_mismatch() {
    let err = parse_program_source(
        r#"
        macro_rules! zip_rows {
            ($($( $left:expr ),+);* => $($( $right:expr ),+);*) => {
                [$( [ $($left + $right),* ] ),*]
            };
        }
        return zip_rows!(1, 2; 3 => 10; 20, 30);
        "#,
        Default::default(),
    )
    .expect_err("nested repeated metavariables must have matching row arity");
    let message = err.to_string();
    assert!(
        message.contains("matched 1 item(s), expected 2") || message.contains("matched 2 item(s), expected 1"),
        "{message}"
    );
}

#[test]
fn expands_template_with_metavariables_at_different_nested_depths() {
    let result = execute_source(
        r#"
        macro_rules! rows {
            ($($label:ident : [ $( $value:expr ),* ]);*) => {
                [$( [ "$label", $($value),* ] ),*]
            };
        }
        return rows!(a: [1, 2]; b: [3]).1.1;
        "#,
    )
    .expect("outer label and inner values should zip at their own depths");

    assert_eq!(result.display_first_return(), "3");
}

#[test]
fn rejects_nested_matcher_repetition_with_missing_template_depth() {
    let err = parse_program_source(
        r#"
        macro_rules! bad {
            ($($( $value:expr ),+);*) => { [$($value),*] };
        }
        return 0;
        "#,
        Default::default(),
    )
    .expect_err("two-dimensional binding needs two template repetition levels");
    let message = err.to_string();
    assert!(
        message.contains("appears at repetition depth 1 in the template but was bound at depth 2"),
        "{message}"
    );
}
