use crate::{
    syntax::{expand_source, render_tokens},
    vm::execute_source,
};

#[test]
fn generated_for_binding_does_not_capture_call_site_block_identifier() {
    let result = execute_source(
        r#"
        macro_rules! with_item_loop {
            ($body:block) => {
                for item in [1] $body
            };
        }
        let item = 10;
        with_item_loop!({ return item; });
        return 0;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_for_binding_references_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! first_seen {
            ($values:expr) => {
                for item in $values {
                    return item;
                }
            };
        }
        let item = 99;
        first_seen!([7]);
        return item;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "7");
}

#[test]
fn generated_for_binding_does_not_freshen_outer_generated_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! scoped_for {
            () => {
                for item in [1] {
                    item;
                }
                return item;
            };
        }
        let item = 10;
        scoped_for!();
        "#,
        Default::default(),
    )
    .expect("for binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("for __lk_macro_"), "{rendered}");
    assert!(rendered.contains("{__lk_macro_"), "{rendered}");
    assert!(rendered.contains("return item;"), "{rendered}");
}

#[test]
fn generated_for_iterable_reference_uses_outer_binding() {
    let result = execute_source(
        r#"
        macro_rules! iterate_outer {
            () => {
                for item in item {
                    return item;
                }
            };
        }
        let item = [42];
        iterate_outer!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_for_tuple_bindings_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! first_pair_sum {
            () => {
                for (left, right) in [[20, 22]] {
                    return left + right;
                }
            };
        }
        let left = 1;
        let right = 2;
        first_pair_sum!();
        return left + right;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_for_array_rest_bindings_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! generated_array_rest_loop {
            () => {
                for [head, ..tail] in [[40, 1, 1]] {
                    return head + tail[0] + tail[1];
                }
            };
        }
        let head = 1;
        let tail = [2, 3];
        generated_array_rest_loop!();
        return head + tail[0];
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_for_object_bindings_are_freshened_without_renaming_keys() {
    let expanded = expand_source(
        r#"
        macro_rules! generated_object_loop {
            () => {
                for {"kind": kind, "value": value} in rows {
                    return kind + value;
                }
            };
        }
        let kind = 1;
        let value = 2;
        let rows = [{"kind": 20, "value": 22}];
        generated_object_loop!();
        "#,
        Default::default(),
    )
    .expect("for object pattern macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("\"kind\""), "{rendered}");
    assert!(rendered.contains("\"value\""), "{rendered}");
    assert!(rendered.contains("__lk_macro_"), "{rendered}");
    assert!(!rendered.contains("\"__lk_macro_"), "{rendered}");
}

#[test]
fn generated_if_let_binding_does_not_capture_call_site_block_identifier() {
    let result = execute_source(
        r#"
        macro_rules! with_generated_if_let {
            ($body:block) => {
                if let item = 1 $body
            };
        }
        let item = 10;
        with_generated_if_let!({ return item; });
        return 0;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_if_let_binding_does_not_freshen_outer_generated_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! scoped_if_let {
            () => {
                if let item = 1 {
                    item;
                }
                return item;
            };
        }
        let item = 10;
        scoped_if_let!();
        "#,
        Default::default(),
    )
    .expect("if-let binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("if let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("{__lk_macro_"), "{rendered}");
    assert!(rendered.contains("return item;"), "{rendered}");
}

#[test]
fn generated_if_let_value_reference_uses_outer_binding_and_guard_uses_generated_binding() {
    let result = execute_source(
        r#"
        macro_rules! match_outer {
            () => {
                if let item if item > 40 = item {
                    return item;
                }
            };
        }
        let item = 42;
        match_outer!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_while_let_binding_does_not_capture_call_site_block_identifier() {
    let result = execute_source(
        r#"
        macro_rules! with_generated_while_let {
            ($body:block) => {
                while let item = 1 $body
            };
        }
        let item = 10;
        with_generated_while_let!({ return item; });
        return 0;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_while_let_binding_does_not_freshen_outer_generated_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! scoped_while_let {
            () => {
                while let item = 1 {
                    item;
                    break;
                }
                return item;
            };
        }
        let item = 10;
        scoped_while_let!();
        "#,
        Default::default(),
    )
    .expect("while-let binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("while let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("{__lk_macro_"), "{rendered}");
    assert!(rendered.contains("return item;"), "{rendered}");
}

#[test]
fn generated_while_let_value_reference_uses_outer_binding_and_guard_uses_generated_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! poll_outer {
            () => {
                while let item if item > 40 = item {
                    item;
                    break;
                }
            };
        }
        let item = 42;
        poll_outer!();
        "#,
        Default::default(),
    )
    .expect("while-let binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("while let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("if __lk_macro_"), "{rendered}");
    assert!(rendered.contains("= item {"), "{rendered}");
}

#[test]
fn generated_match_case_binding_does_not_capture_call_site_block_identifier() {
    let result = execute_source(
        r#"
        macro_rules! with_generated_match_case {
            ($expr:expr) => {
                return match 1 {
                    item => $expr
                };
            };
        }
        let item = 10;
        with_generated_match_case!(item);
        return 0;
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "10");
}

#[test]
fn generated_match_guard_binding_references_are_freshened_together() {
    let result = execute_source(
        r#"
        macro_rules! guarded_match {
            () => {
                return match 3 {
                    item if item > 0 => item,
                    _ => 0
                };
            };
        }
        let item = 10;
        guarded_match!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "3");
}

#[test]
fn generated_match_or_pattern_bindings_share_one_fresh_name() {
    let expanded = expand_source(
        r#"
        macro_rules! guarded_or_match {
            () => {
                return match 3 {
                    item | item if item > 0 => item,
                    _ => 0
                };
            };
        }
        let item = 10;
        guarded_or_match!();
        "#,
        Default::default(),
    )
    .expect("or-pattern macro should expand");
    let rendered = render_tokens(&expanded.tokens);
    let Some(generated_name) = rendered
        .split_whitespace()
        .find(|part| part.starts_with("__lk_macro_") && part.ends_with("_item"))
    else {
        panic!("or-pattern binding should be freshened: {rendered}");
    };
    assert!(
        rendered.contains(&format!("{generated_name}|{generated_name}"))
            || rendered.contains(&format!("{generated_name} | {generated_name}")),
        "or-pattern alternatives should share the same fresh name: {rendered}"
    );
    assert!(
        rendered.contains(&format!("if {generated_name} > 0")),
        "or-pattern guard should use the same fresh name: {rendered}"
    );
    assert!(
        rendered.contains(&format!("=> {generated_name}")),
        "or-pattern body should use the same fresh name: {rendered}"
    );
}

#[test]
fn generated_match_or_pattern_binding_executes_with_freshened_references() {
    let result = execute_source(
        r#"
        macro_rules! guarded_or_match {
            () => {
                return match 3 {
                    item | item if item > 0 => item,
                    _ => 0
                };
            };
        }
        let item = 10;
        guarded_or_match!();
        "#,
    )
    .expect("or-pattern macro program should execute");

    assert_eq!(result.display_first_return(), "3");
}

#[test]
fn generated_match_value_reference_uses_outer_binding() {
    let result = execute_source(
        r#"
        macro_rules! match_outer {
            () => {
                return match item {
                    item if item > 40 => item,
                    _ => 0
                };
            };
        }
        let item = 42;
        match_outer!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_select_recv_operation_name_is_not_freshened_as_a_case_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! choose_ready {
            ($ch:expr) => {
                select {
                    case recv($ch) => 1;
                    default => 0;
                }
            };
        }
        return choose_ready!(ch);
        "#,
        Default::default(),
    )
    .expect("select macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("case recv (ch) => 1;"), "{rendered}");
    assert!(!rendered.contains("__lk_macro"), "{rendered}");
}

#[test]
fn generated_select_recv_binding_references_are_freshened_together() {
    let expanded = expand_source(
        r#"
        macro_rules! choose_ready {
            ($ch:expr) => {
                select {
                    case item <- recv($ch) if item > 0 => item;
                    default => 0;
                }
            };
        }
        let item = 10;
        return choose_ready!(ch);
        "#,
        Default::default(),
    )
    .expect("select binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("case __lk_macro_"), "{rendered}");
    assert!(rendered.contains("<- recv (ch) if __lk_macro_"), "{rendered}");
    assert!(rendered.contains("=> __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("case recv"), "{rendered}");
}

#[test]
fn generated_select_recv_binding_does_not_freshen_default_case_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! choose_ready {
            ($ch:expr) => {
                select {
                    case item <- recv($ch) if item > 0 => item;
                    default => item;
                }
            };
        }
        let item = 10;
        return choose_ready!(ch);
        "#,
        Default::default(),
    )
    .expect("select binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("case __lk_macro_"), "{rendered}");
    assert!(rendered.contains("=> __lk_macro_"), "{rendered}");
    assert!(rendered.contains("default => item;"), "{rendered}");
}

#[test]
fn generated_select_recv_argument_uses_outer_binding_and_guard_uses_case_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! choose_ready {
            () => {
                select {
                    case item <- recv(item) if item > 0 => item;
                    default => 0;
                }
            };
        }
        let item = ch;
        return choose_ready!();
        "#,
        Default::default(),
    )
    .expect("select binding macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("case __lk_macro_"), "{rendered}");
    assert!(rendered.contains("<- recv (item) if __lk_macro_"), "{rendered}");
    assert!(rendered.contains("=> __lk_macro_"), "{rendered}");
}
