use crate::{
    syntax::{expand_source, render_tokens},
    vm::execute_source,
};

#[test]
fn generated_function_param_does_not_capture_call_site_body_identifier() {
    let result = execute_source(
        r#"
        macro_rules! make_reader {
            ($body:block) => {
                fn read(item) $body
            };
        }
        let item = 10;
        make_reader!({ return item; });
        return read(7);
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_function_named_param_references_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! make_reader {
            () => {
                fn read({ item: Int = 7 }) {
                    return item;
                }
            };
        }
        let item = 10;
        make_reader!();
        return read();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "7");
}

#[test]
fn generated_default_positional_param_references_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! make_reader {
            () => {
                fn read(item = 7) {
                    return item;
                }
            };
        }
        let item = 10;
        make_reader!();
        return read();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "7");
}

#[test]
fn generated_default_positional_param_does_not_capture_its_own_default() {
    let expanded = expand_source(
        r#"
        macro_rules! make_reader {
            () => {
                fn read(item = item + 1) {
                    return item;
                }
            };
        }
        let item = 41;
        make_reader!();
        "#,
        Default::default(),
    )
    .expect("default parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn read (__lk_macro_"), "{rendered}");
    assert!(rendered.contains("= item + 1"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
}

#[test]
fn generated_default_positional_param_can_reference_previous_generated_param() {
    let result = execute_source(
        r#"
        macro_rules! make_reader {
            () => {
                fn read(seed, item = seed + 1) {
                    return item;
                }
            };
        }
        let seed = 100;
        let item = 200;
        make_reader!();
        return read(41);
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_named_param_does_not_capture_its_own_default() {
    let expanded = expand_source(
        r#"
        macro_rules! make_reader {
            () => {
                fn read({ item: Int = item + 1 }) {
                    return item;
                }
            };
        }
        let item = 41;
        make_reader!();
        "#,
        Default::default(),
    )
    .expect("named default parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("{__lk_macro_"), "{rendered}");
    assert!(rendered.contains("= item + 1"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
}

#[test]
fn generated_named_param_default_can_reference_previous_generated_param() {
    let result = execute_source(
        r#"
        macro_rules! make_reader {
            () => {
                fn read({ seed: Int = 41, item: Int = seed + 1 }) {
                    return item;
                }
            };
        }
        let seed = 100;
        let item = 200;
        make_reader!();
        return read();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_closure_param_does_not_capture_call_site_body_identifier() {
    let result = execute_source(
        r#"
        macro_rules! call_reader {
            ($body:block) => {
                let read = |item| $body;
                return read(7);
            };
        }
        let item = 10;
        call_reader!({ item });
        return 0;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_closure_param_references_are_freshened_together() {
    let expanded = expand_source(
        r#"
        macro_rules! make_reader {
            () => {
                let read = |item| item + 1;
                return read(item);
            };
        }
        let item = 10;
        make_reader!();
        "#,
        Default::default(),
    )
    .expect("closure parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("| __lk_macro_"), "{rendered}");
    assert!(
        rendered.contains("| __lk_macro_") && rendered.contains("__lk_macro_") && rendered.contains("+ 1"),
        "{rendered}"
    );
    assert!(rendered.contains("(item);"), "{rendered}");
}

#[test]
fn generated_closure_block_param_does_not_freshen_outer_generated_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! make_reader {
            () => {
                let read = |item| {
                    item + 1
                };
                return item;
            };
        }
        let item = 10;
        make_reader!();
        "#,
        Default::default(),
    )
    .expect("closure block parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("| __lk_macro_"), "{rendered}");
    assert!(
        rendered.contains("__lk_macro_") && rendered.contains("+ 1"),
        "{rendered}"
    );
    assert!(rendered.contains("return item;"), "{rendered}");
}

#[test]
fn generated_fn_closure_param_references_are_freshened_together() {
    let expanded = expand_source(
        r#"
        macro_rules! make_reader {
            () => {
                let read = fn(item) => item + 1;
                return read(item);
            };
        }
        let item = 10;
        make_reader!();
        "#,
        Default::default(),
    )
    .expect("fn closure parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn (__lk_macro_"), "{rendered}");
    assert!(
        rendered.contains("__lk_macro_") && rendered.contains("+ 1"),
        "{rendered}"
    );
    assert!(rendered.contains("(item);"), "{rendered}");
}

#[test]
fn generated_fn_closure_block_param_does_not_freshen_outer_generated_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! make_reader {
            () => {
                let read = fn(item) => {
                    item + 1
                };
                return item;
            };
        }
        let item = 10;
        make_reader!();
        "#,
        Default::default(),
    )
    .expect("fn closure block parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn (__lk_macro_"), "{rendered}");
    assert!(
        rendered.contains("__lk_macro_") && rendered.contains("+ 1"),
        "{rendered}"
    );
    assert!(rendered.contains("return item;"), "{rendered}");
}

#[test]
fn expression_pipes_are_not_treated_as_generated_closure_params() {
    let result = execute_source(
        r#"
        macro_rules! bit_or_identity {
            () => {
                let value = 1 | 2 | 4;
                return value;
            };
        }
        bit_or_identity!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "7");
}
