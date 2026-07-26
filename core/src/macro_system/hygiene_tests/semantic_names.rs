use crate::{
    syntax::{expand_source, render_tokens},
    vm::execute_source,
};

#[test]
fn generated_let_type_annotation_name_is_not_freshened_as_local_binding_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_local {
            () => {
                let item: item = 1;
                return item;
            };
        }
        let item = 99;
        typed_local!();
        "#,
        Default::default(),
    )
    .expect("typed local macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains(": item = 1;"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": __lk_macro_"), "{rendered}");
}

#[test]
fn generated_function_param_type_name_is_not_freshened_as_param_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_param {
            () => {
                fn read(item: item) {
                    return item;
                }
            };
        }
        let item = 99;
        typed_param!();
        "#,
        Default::default(),
    )
    .expect("typed parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn read (__lk_macro_"), "{rendered}");
    assert!(rendered.contains(": item)"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": __lk_macro_"), "{rendered}");
}

#[test]
fn generated_function_param_named_function_type_names_are_not_freshened_as_param_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_named_function_param {
            () => {
                fn read(item: ({item: item = _}) -> item) {
                    return item;
                }
            };
        }
        let item = 99;
        typed_named_function_param!();
        "#,
        Default::default(),
    )
    .expect("typed named function parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn read (__lk_macro_"), "{rendered}");
    assert!(rendered.contains(": ({item : item = _}) -> item)"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("{__lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("-> __lk_macro_"), "{rendered}");
}

#[test]
fn generated_named_param_named_function_type_names_are_not_freshened_as_param_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_named_param {
            () => {
                fn read({ item: ({item: item = _}) -> item = item }) {
                    return item;
                }
            };
        }
        let item = 99;
        typed_named_param!();
        "#,
        Default::default(),
    )
    .expect("typed named parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn read ({__lk_macro_"), "{rendered}");
    assert!(rendered.contains(": ({item : item = _}) -> item = item}"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": ({__lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": ({item : __lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("-> __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("= __lk_macro_"), "{rendered}");
}

#[test]
fn generated_function_return_type_name_is_not_freshened_as_local_binding_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_return {
            () => {
                fn read() -> item {
                    let item = 1;
                    return item;
                }
            };
        }
        let item = 99;
        typed_return!();
        "#,
        Default::default(),
    )
    .expect("typed return macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn read () -> item {"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("-> __lk_macro_"), "{rendered}");
}

#[test]
fn generated_trait_method_return_type_name_is_not_freshened_as_param_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_trait {
            () => {
                trait Reader {
                    fn read(item: item) -> item;
                }
            };
        }
        let item = 99;
        typed_trait!();
        "#,
        Default::default(),
    )
    .expect("typed trait macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn read (item"), "{rendered}");
    assert!(rendered.contains(": item) -> item;"), "{rendered}");
    assert!(!rendered.contains("fn read (__lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("-> __lk_macro_"), "{rendered}");
}

#[test]
fn generated_trait_method_param_does_not_freshen_following_generated_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! trait_then_reference {
            () => {
                trait Reader {
                    fn read(item);
                }
                return item;
            };
        }
        let item = 99;
        trait_then_reference!();
        "#,
        Default::default(),
    )
    .expect("trait method parameter macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn read (item);"), "{rendered}");
    assert!(rendered.contains("return item;"), "{rendered}");
    assert!(!rendered.contains("return __lk_macro_"), "{rendered}");
}

#[test]
fn generated_type_alias_target_name_is_not_freshened_as_local_binding_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_alias {
            () => {
                type Alias = item;
                let item = 1;
                return item;
            };
        }
        let item = 99;
        typed_alias!();
        "#,
        Default::default(),
    )
    .expect("typed alias macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("type Alias = item;"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("type Alias = __lk_macro_"), "{rendered}");
}

#[test]
fn generated_impl_header_type_names_are_not_freshened_as_local_binding_references() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_impl {
            () => {
                impl item for item {
                    fn read(self) -> item {
                        let item = 1;
                        return item;
                    }
                }
            };
        }
        let item = 99;
        typed_impl!();
        "#,
        Default::default(),
    )
    .expect("typed impl macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("impl item for item {"), "{rendered}");
    assert!(rendered.contains("-> item {"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("impl __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("for __lk_macro_"), "{rendered}");
}

#[test]
fn generated_impl_header_named_function_type_names_are_not_freshened_as_local_binding_references() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_impl {
            () => {
                impl item for ({item: item = _}) -> item {
                    fn read(self) -> item {
                        let item = 1;
                        return item;
                    }
                }
            };
        }
        let item = 99;
        typed_impl!();
        "#,
        Default::default(),
    )
    .expect("typed impl macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(
        rendered.contains("impl item for ({item : item = _}) -> item {"),
        "{rendered}"
    );
    assert!(rendered.contains("-> item {"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("impl __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("for ({__lk_macro_"), "{rendered}");
    assert!(!rendered.contains(": __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("-> __lk_macro_"), "{rendered}");
}

#[test]
fn generated_impl_header_generic_union_type_names_are_not_freshened_as_local_binding_references() {
    let expanded = expand_source(
        r#"
        macro_rules! typed_impl {
            () => {
                impl item for List<item>? | Map<String, item> {
                    fn read(self) -> item {
                        let item = 1;
                        return item;
                    }
                }
            };
        }
        let item = 99;
        typed_impl!();
        "#,
        Default::default(),
    )
    .expect("typed impl macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(
        rendered.contains("impl item for List < item > ? | Map < String, item > {"),
        "{rendered}"
    );
    assert!(rendered.contains("-> item {"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("impl __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("< __lk_macro_"), "{rendered}");
    assert!(!rendered.contains(", __lk_macro_"), "{rendered}");
}

#[test]
fn generated_function_definition_name_is_not_freshened_by_local_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! define_item_named_function {
            () => {
                fn item() {
                    let item = 1;
                    return item;
                }
            };
        }
        let item = 99;
        define_item_named_function!();
        "#,
        Default::default(),
    )
    .expect("function item macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("fn item ()"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("fn __lk_macro_"), "{rendered}");
}

#[test]
fn generated_type_alias_name_is_not_freshened_by_local_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! define_item_named_alias {
            () => {
                type item = item;
                let item = 1;
                return item;
            };
        }
        let item = 99;
        define_item_named_alias!();
        "#,
        Default::default(),
    )
    .expect("type alias item macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("type item = item;"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("type __lk_macro_"), "{rendered}");
}

#[test]
fn generated_struct_definition_and_field_names_are_not_freshened_by_local_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! define_item_named_struct {
            () => {
                struct item {
                    item: item,
                }
                let item = 1;
                return item;
            };
        }
        let item = 99;
        define_item_named_struct!();
        "#,
        Default::default(),
    )
    .expect("struct item macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("struct item {"), "{rendered}");
    assert!(rendered.contains("item : item"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("struct __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("{__lk_macro_"), "{rendered}");
}

#[test]
fn generated_trait_and_method_names_are_not_freshened_by_local_binding() {
    let expanded = expand_source(
        r#"
        macro_rules! define_item_named_trait {
            () => {
                trait item {
                    fn item(item: item) -> item;
                }
                let item = 1;
                return item;
            };
        }
        let item = 99;
        define_item_named_trait!();
        "#,
        Default::default(),
    )
    .expect("trait item macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("trait item {fn item"), "{rendered}");
    assert!(rendered.contains(": item) -> item;"), "{rendered}");
    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("trait __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("fn __lk_macro_"), "{rendered}");
}

#[test]
fn generated_member_access_field_name_is_not_freshened_as_local_binding_reference() {
    let result = execute_source(
        r#"
        macro_rules! read_generated_field {
            ($object:expr) => {
                let item = 7;
                return $object.item + item;
            };
        }
        let item = 99;
        let object = { item: 35 };
        read_generated_field!(object);
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_map_key_is_not_freshened_as_local_binding_reference() {
    let result = execute_source(
        r#"
        macro_rules! read_generated_map_key {
            () => {
                let item = 7;
                return { item: 35 }.item + item;
            };
        }
        let item = 99;
        read_generated_map_key!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_struct_literal_field_name_is_not_freshened_as_local_binding_reference() {
    let result = execute_source(
        r#"
        struct Boxed {
            item: Int,
        }

        macro_rules! read_generated_struct_field {
            () => {
                let item = 7;
                let boxed = Boxed { item: 35 };
                return boxed.item + item;
            };
        }
        let item = 99;
        read_generated_struct_field!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_struct_literal_type_name_is_not_freshened_as_local_binding_reference() {
    let expanded = expand_source(
        r#"
        struct Boxed {
            item: Int,
        }

        macro_rules! generated_struct_literal_type {
            () => {
                let Boxed = 7;
                let boxed = Boxed { item: 35 };
                return Boxed + boxed.item;
            };
        }
        let Boxed = 99;
        generated_struct_literal_type!();
        "#,
        Default::default(),
    )
    .expect("struct literal type macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("= Boxed {item : 35};"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("= __lk_macro_"), "{rendered}");
}

#[test]
fn generated_if_condition_before_block_is_still_freshened_as_local_reference() {
    let result = execute_source(
        r#"
        macro_rules! generated_if_condition {
            () => {
                let condition = true;
                if condition {
                    return 42;
                }
                return 0;
            };
        }
        let condition = false;
        generated_if_condition!();
        "#,
    )
    .expect("if condition macro should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_named_argument_key_is_not_freshened_as_local_binding_reference() {
    let result = execute_source(
        r#"
        fn read({ item: Int }) {
            return item;
        }

        macro_rules! read_generated_named_arg {
            () => {
                let item = 7;
                return read(item: 35) + item;
            };
        }
        let item = 99;
        read_generated_named_arg!();
        "#,
    )
    .expect("macro program should execute");

    assert_eq!(result.display_first_return(), "42");
}

#[test]
fn generated_import_statement_names_are_not_freshened_as_local_binding_references() {
    let expanded = expand_source(
        r#"
        macro_rules! import_with_local {
            () => {
                let item = 1;
                use { item as alias } from item;
                return item;
            };
        }
        let item = 99;
        import_with_local!();
        "#,
        Default::default(),
    )
    .expect("import macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("use {item as alias} from item;"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("use {__lk_macro_"), "{rendered}");
    assert!(!rendered.contains("from __lk_macro_"), "{rendered}");
}

#[test]
fn generated_path_segments_are_not_freshened_as_local_binding_references() {
    let expanded = expand_source(
        r#"
        macro_rules! path_with_local {
            () => {
                let item = 1;
                item::helper();
                return item;
            };
        }
        let item = 99;
        path_with_local!();
        "#,
        Default::default(),
    )
    .expect("path macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("item::helper ();"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
}

#[test]
fn generated_attribute_name_is_not_freshened_as_local_binding_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! attr_with_local {
            () => {
                let item = 1;
                #[item]
                fn read() {
                    return 0;
                }
                return item;
            };
        }
        let item = 99;
        attr_with_local!();
        "#,
        Default::default(),
    )
    .expect("attribute macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("# [item]"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("#[__lk_macro_"), "{rendered}");
}

#[test]
fn generated_derive_name_is_not_freshened_as_local_binding_reference() {
    let expanded = expand_source(
        r#"
        macro_rules! derive_with_local {
            () => {
                let item = 1;
                #[derive(item)]
                struct Boxed {
                    value: Int,
                }
                return item;
            };
        }
        let item = 99;
        derive_with_local!();
        "#,
        Default::default(),
    )
    .expect("derive macro should expand");
    let rendered = render_tokens(&expanded.tokens);

    assert!(rendered.contains("let __lk_macro_"), "{rendered}");
    assert!(rendered.contains("# [derive (item)]"), "{rendered}");
    assert!(rendered.contains("return __lk_macro_"), "{rendered}");
    assert!(!rendered.contains("#[derive (__lk_macro_"), "{rendered}");
}
