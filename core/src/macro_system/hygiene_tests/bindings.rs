use crate::{
    syntax::{expand_source, render_tokens},
    vm::execute_source,
};

#[test]
fn generated_let_destructuring_does_not_capture_call_site_block_identifier() {
    let result = execute_source(
        r#"
        macro_rules! with_generated_destructure {
            ($body:block) => {
                let [item] = [1];
                $body
            };
        }
        let item = 10;
        with_generated_destructure!({ return item; });
        return 0;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_block_local_let_does_not_freshen_outer_generated_reference() {
    let result = execute_source(
        r#"
        macro_rules! scoped_local {
            () => {
                {
                    let item = 1;
                }
                return item;
            };
        }
        let item = 10;
        scoped_local!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_block_local_const_does_not_freshen_outer_generated_reference() {
    let result = execute_source(
        r#"
        macro_rules! scoped_const {
            () => {
                {
                    const item = 1;
                }
                return item;
            };
        }
        let item = 10;
        scoped_const!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_map_rest_destructuring_references_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! generated_map_rest {
            () => {
                let { "a": first, ..rest } = { "a": 1, "b": 41 };
                return first + rest.b;
            };
        }
        let first = 99;
        let rest = { "b": 100 };
        generated_map_rest!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_let_initializer_reference_uses_outer_binding() {
    let result = execute_source(
        r#"
        macro_rules! increment_outer {
            () => {
                let item = item + 1;
                return item;
            };
        }
        let item = 41;
        increment_outer!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_let_pattern_guard_uses_generated_binding_and_value_uses_outer_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! guarded_let {
            () => {
                let item if item > 40 = item;
                return item;
            };
        }
        let item = 42;
        guarded_let!();
        "#,
        Default::default(),
    )
    .expect("guarded let binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("if __lk_macro_"), "{rendered}");
    assert!(rendered.contains("= item;"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
}

#[test]
fn generated_define_binding_does_not_capture_call_site_block_identifier() {
    let result = execute_source(
        r#"
        macro_rules! with_generated_define {
            ($body:block) => {
                item := 1;
                $body
            };
        }
        let item = 10;
        with_generated_define!({ return item; });
        return 0;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_define_binding_references_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! first_defined {
            () => {
                item := 7;
                return item;
            };
        }
        let item = 99;
        first_defined!();
        return item;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "7");
}

#[test]
fn generated_define_initializer_reference_uses_outer_binding() {
    let result = execute_source(
        r#"
        macro_rules! define_from_outer {
            () => {
                item := item + 1;
                return item;
            };
        }
        let item = 41;
        define_from_outer!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_block_local_define_does_not_freshen_outer_generated_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! scoped_define {
            () => {
                {
                    item := 1;
                    item;
                }
                return item;
            };
        }
        let item = 10;
        scoped_define!();
        "#,
        Default::default(),
    )
    .expect("short declaration macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("__lk_macro_"), "{rendered}");
    assert!(rendered.contains(": = 1"), "{rendered}");
    assert!(rendered.contains("return item;"), "{rendered}");
}
