use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

#[test]
fn external_attribute_provider_replaces_item() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "replace",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"123","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );
    let program = parse_program_source(
        r#"
            #[replace]
            fn old() {
                return 1;
            }

            return generated();
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let result = program.execute().expect("execute attribute macro output");
    assert_eq!(result.returns, vec![RuntimeVal::Int(123)]);
}

#[test]
fn external_attribute_records_range_pattern_reference_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "range_attr",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Let","lexeme":"let","span":null},{"kind":"Id","lexeme":"max","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Int","lexeme":"10","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Match","lexeme":"match","span":null},{"kind":"Int","lexeme":"5","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Int","lexeme":"1","span":null},{"kind":"Range","lexeme":"..","span":null},{"kind":"Id","lexeme":"max","span":null},{"kind":"Arrow","lexeme":"=>","span":null},{"kind":"Int","lexeme":"42","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"_","span":null},{"kind":"Arrow","lexeme":"=>","span":null},{"kind":"Int","lexeme":"0","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[range_attr]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "range_attr")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "binding max" && member.span.is_some()),
        "generated range helper binding should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "ref max" && member.span.is_some()),
        "generated range pattern end reference should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "expr match" && member.span.is_some()),
        "generated match expression should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "pattern range" && member.span.is_some()),
        "generated range pattern category should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "pattern wildcard" && member.span.is_some()),
        "generated wildcard pattern category should carry a source-map span: {generated_fn:?}"
    );
}

#[test]
fn external_attribute_records_static_index_expression_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "index_attr",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Let","lexeme":"let","span":null},{"kind":"Id","lexeme":"items","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"LBracket","lexeme":"[","span":null},{"kind":"Int","lexeme":"42","span":null},{"kind":"RBracket","lexeme":"]","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Id","lexeme":"items","span":null},{"kind":"Dot","lexeme":".","span":null},{"kind":"Int","lexeme":"0","span":null},{"kind":"Dot","lexeme":".","span":null},{"kind":"Id","lexeme":"render","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[index_attr]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "index_attr")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "index items.0" && member.span.is_some()),
        "generated static index expression should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "expr items.0.render" && member.span.is_some()),
        "generated field expression after static index should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "call items.0.render" && member.span.is_some()),
        "generated call callee after static index should carry a source-map span: {generated_fn:?}"
    );
}

#[test]
fn external_attribute_records_generated_expression_category_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    let response = proc_macro_response_from_source(
        r#"
            fn generated() {
                let items = [41, 42];
                let meta = {kind: items.0};
                let dynamic = items[value];
                let value = (items.0 + 1);
                let flag = !value;
                value;
                return "value=${value}";
            }
            "#,
    );
    providers.register_attribute("expr_categories", shell_response_config(shell, &response));

    let expanded = expand_program_source(
        r#"
            #[expr_categories]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "expr_categories")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    for label in [
        "expr list",
        "expr list_item",
        "expr map",
        "expr map_key_expr",
        "expr map_value",
        "expr access",
        "expr access_base",
        "expr access_member",
        "expr paren",
        "expr binary",
        "expr binary_left",
        "expr binary_right",
        "binary add",
        "expr unary",
        "expr unary_operand",
        "unary not",
        "expr template_string",
        "template_part literal",
        "template_part expr",
        "stmt expr",
        "index items.0",
        "map_key kind",
        "ref value",
    ] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated expression category origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_remaining_generated_expression_shape_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    let response = proc_macro_response_from_source(
        r#"
            fn generated() {
                let channel = nil;
                let value = 1;
                let user = User { id: value };
                let updated = User { ..user, id: value };
                let direct = user.id;
                let maybe = user?.id;
                let chosen = value > 0 ? value..10 : nil;
                let logic = (value > 0) && (maybe ?? value) || false;
                let handler = |item| {
                    return item;
                };
                let named = helper(value, current: maybe);
                let called = handler(value);
                let selected = select {
                    case item <- recv(channel) if item > 0 => item;
                    case send(channel, value) => value;
                    default => nil;
                };
                let matched = match value {
                    1 => called,
                    _ => value
                };
                return matched;
            }
            "#,
    );
    let mut config = shell_response_config(shell, &response);
    config.max_output_bytes = 16 * 1024;
    providers.register_attribute("expr_shapes", config);

    let expanded = expand_program_source(
        r#"
            struct User { id: Int }

            #[expr_shapes]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "expr_shapes")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    for label in [
        "expr literal",
        "literal int",
        "literal bool",
        "literal nil",
        "expr access",
        "expr and",
        "expr or",
        "expr nullish",
        "expr logical_left",
        "expr logical_right",
        "expr nullish_left",
        "expr nullish_right",
        "binary gt",
        "expr struct_literal",
        "expr struct_field",
        "expr struct_field_value",
        "expr struct_update_base",
        "expr struct_update_fields",
        "expr optional_access",
        "expr conditional",
        "expr conditional_condition",
        "expr conditional_then",
        "expr conditional_else",
        "expr range",
        "range exclusive",
        "range start",
        "range end",
        "expr closure",
        "expr closure_body",
        "expr block",
        "expr block_stmt",
        "expr call",
        "expr call_expr",
        "expr call_callee",
        "expr call_arg",
        "expr call_named",
        "expr named_arg_value",
        "expr var",
        "expr match_value",
        "expr match_arm_body",
        "named_arg current",
        "struct_field id",
        "type_ref User",
        "ref user",
        "binding item",
        "ref item",
        "call helper",
        "call handler",
    ] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated expression shape origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_top_level_statement_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "stmt_attr",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Let","lexeme":"let","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Id","lexeme":"seed","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Const","lexeme":"const","span":null},{"kind":"Id","lexeme":"fixed","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Int","lexeme":"7","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"PlusAssign","lexeme":"+=", "span":null},{"kind":"Int","lexeme":"1","span":null},{"kind":"Semicolon","lexeme":";","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            struct User { id: Int }

            #[stmt_attr]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "stmt_attr")
        .expect("attribute origin should be recorded");
    let generated_statement = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "statement")
        .expect("generated statement origin");
    assert!(
        generated_statement
            .generated_member_origins
            .iter()
            .any(|member| member.label == "binding current" && member.span.is_some()),
        "top-level generated let binding should carry a source-map span: {generated_statement:?}"
    );
    assert!(
        generated_statement
            .generated_member_origins
            .iter()
            .any(|member| member.label == "type_ref User" && member.span.is_some()),
        "top-level generated let type reference should carry a source-map span: {generated_statement:?}"
    );
    assert!(
        generated_statement
            .generated_member_origins
            .iter()
            .any(|member| member.label == "ref seed" && member.span.is_some()),
        "top-level generated let value reference should carry a source-map span: {generated_statement:?}"
    );
    assert!(
        generated_statement
            .generated_member_origins
            .iter()
            .any(|member| member.label == "stmt let" && member.span.is_some()),
        "top-level generated let statement should carry a source-map span: {generated_statement:?}"
    );
    for label in ["stmt binding_pattern", "stmt type_annotation", "stmt initializer"] {
        assert!(
            generated_statement
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "top-level generated statement role `{label}` should carry a source-map span: {generated_statement:?}"
        );
    }
    assert!(
        origin
            .generated_item_origins
            .iter()
            .flat_map(|item| &item.generated_member_origins)
            .any(|member| member.label == "stmt const" && member.span.is_some()),
        "top-level generated const statement should carry a source-map span: {origin:?}"
    );
    assert!(
        origin
            .generated_item_origins
            .iter()
            .flat_map(|item| &item.generated_member_origins)
            .any(|member| member.label == "compound_assign_ref current" && member.span.is_some()),
        "top-level generated compound assignment should carry a source-map span: {origin:?}"
    );
    assert!(
        origin
            .generated_item_origins
            .iter()
            .flat_map(|item| &item.generated_member_origins)
            .any(|member| member.label == "stmt compound_assign" && member.span.is_some()),
        "top-level generated compound assignment statement should carry a source-map span: {origin:?}"
    );
    {
        let label = "stmt compound_assign_value";
        assert!(
            origin
                .generated_item_origins
                .iter()
                .flat_map(|item| &item.generated_member_origins)
                .any(|member| member.label == label && member.span.is_some()),
            "top-level generated assignment role `{label}` should carry a source-map span: {origin:?}"
        );
    }
}

#[test]
fn external_attribute_records_generated_function_type_annotation_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    let response = proc_macro_response_from_source(
        r#"
            fn generated() {
                let mapper: (User, {current: User = _}) -> User = seed;
                return mapper(seed, current: seed);
            }
            "#,
    );
    providers.register_attribute("function_type_annotation", shell_response_config(shell, &response));

    let expanded = expand_program_source(
        r#"
            struct User { id: Int }

            #[function_type_annotation]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "function_type_annotation")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    let user_type_refs = generated_fn
        .generated_member_origins
        .iter()
        .filter(|member| member.label == "type_ref User" && member.span.is_some())
        .count();
    assert!(
        user_type_refs >= 3,
        "generated function type annotation should expose positional, named, and return type refs: {generated_fn:?}"
    );
    for label in [
        "binding mapper",
        "type_expr function",
        "type_expr function_param",
        "type_expr function_named_param",
        "type_expr function_return",
        "type_expr named",
        "stmt binding_pattern",
        "stmt type_annotation",
        "stmt initializer",
        "named_param_type current",
        "ref seed",
        "call mapper",
        "named_arg current",
    ] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated function type annotation origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_generated_control_flow_statement_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "control_flow",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Let","lexeme":"let","span":null},{"kind":"Id","lexeme":"i","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Int","lexeme":"0","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"If","lexeme":"if","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"i","span":null},{"kind":"Eq","lexeme":"==","span":null},{"kind":"Int","lexeme":"0","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"i","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Int","lexeme":"1","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Else","lexeme":"else","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"i","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Int","lexeme":"2","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"While","lexeme":"while","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"i","span":null},{"kind":"Lt","lexeme":"<","span":null},{"kind":"Int","lexeme":"3","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"i","span":null},{"kind":"PlusAssign","lexeme":"+=", "span":null},{"kind":"Int","lexeme":"1","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Break","lexeme":"break","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Continue","lexeme":"continue","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Id","lexeme":"i","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[control_flow]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "control_flow")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    for label in [
        "stmt let",
        "stmt binding_pattern",
        "stmt initializer",
        "stmt if",
        "stmt if_condition",
        "stmt if_then",
        "stmt if_else",
        "stmt assign",
        "stmt assign_value",
        "stmt while",
        "stmt while_condition",
        "stmt while_body",
        "stmt compound_assign",
        "stmt compound_assign_value",
        "stmt break",
        "stmt continue",
        "stmt return",
        "stmt return_value",
        "stmt block_item",
        "stmt function_body",
    ] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated control-flow statement origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_generated_let_and_for_statement_role_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    let response = proc_macro_response_from_source(
        r#"
            fn generated() {
                let seed = 1;
                if let item if item > 0 = seed {
                    item;
                } else {
                    seed;
                }
                while let current if current < 2 = seed {
                    current;
                }
                for row in rows {
                    row;
                }
                return seed;
            }
            "#,
    );
    let mut config = shell_response_config(shell, &response);
    config.max_output_bytes = 16 * 1024;
    providers.register_attribute("let_control_flow", config);

    let expanded = expand_program_source(
        r#"
            #[let_control_flow]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "let_control_flow")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    for label in [
        "stmt if let",
        "stmt if_let_pattern",
        "stmt if_let_value",
        "stmt if_let_then",
        "stmt if_let_else",
        "stmt while let",
        "stmt while_let_pattern",
        "stmt while_let_value",
        "stmt while_let_body",
        "stmt for",
        "stmt for_pattern",
        "stmt for_iterable",
        "stmt for_body",
    ] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated let/for statement role origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_nested_declaration_type_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "nested_types",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Type","lexeme":"type","span":null},{"kind":"Id","lexeme":"Alias","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Struct","lexeme":"struct","span":null},{"kind":"Id","lexeme":"Boxed","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"item","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Trait","lexeme":"trait","span":null},{"kind":"Id","lexeme":"Reader","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"read","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"value","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"FnArrow","lexeme":"->","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"0","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            struct User { id: Int }

            #[nested_types]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "nested_types")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    let user_type_refs = generated_fn
        .generated_member_origins
        .iter()
        .filter(|member| member.label == "type_ref User" && member.span.is_some())
        .count();
    assert!(
        user_type_refs >= 4,
        "nested generated declarations should expose alias, struct-field, trait-param, and trait-return type refs: {generated_fn:?}"
    );
    for label in [
        "type Alias",
        "stmt type_alias_target",
        "struct Boxed",
        "struct_field item",
        "stmt struct_field_type",
        "trait Reader",
        "fn read",
        "stmt trait_method_type",
        "stmt return_value",
    ] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "nested generated declaration label `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_top_level_declaration_member_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "top_level_decls",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Type","lexeme":"type","span":null},{"kind":"Id","lexeme":"Alias","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Struct","lexeme":"struct","span":null},{"kind":"Id","lexeme":"Boxed","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"item","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Trait","lexeme":"trait","span":null},{"kind":"Id","lexeme":"Reader","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"read","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"value","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"FnArrow","lexeme":"->","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            struct User { id: Int }

            #[top_level_decls]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "top_level_decls")
        .expect("attribute origin should be recorded");
    let all_member_origins = origin
        .generated_item_origins
        .iter()
        .flat_map(|item| &item.generated_member_origins)
        .collect::<Vec<_>>();
    for label in [
        "type Alias",
        "struct Boxed",
        "struct_field item",
        "trait Reader",
        "fn read",
    ] {
        assert!(
            all_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "top-level generated declaration label `{label}` should carry a source-map span: {origin:?}"
        );
    }
    let user_type_refs = all_member_origins
        .iter()
        .filter(|member| member.label == "type_ref User" && member.span.is_some())
        .count();
    assert!(
        user_type_refs >= 4,
        "top-level generated alias, struct-field, trait-param, and trait-return type refs should carry spans: {origin:?}"
    );
}

#[test]
fn external_attribute_records_pattern_map_key_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "pattern_keys",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Let","lexeme":"let","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Str","lexeme":"\"kind\"","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Range","lexeme":"..","span":null},{"kind":"Id","lexeme":"rest","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Id","lexeme":"seed","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"For","lexeme":"for","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Str","lexeme":"\"kind\"","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"each","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"In","lexeme":"in","span":null},{"kind":"Id","lexeme":"rows","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Id","lexeme":"each","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[pattern_keys]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "pattern_keys")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    let key_origins = generated_fn
        .generated_member_origins
        .iter()
        .filter(|member| member.label == "map_key kind" && member.span.is_some())
        .count();
    assert!(
        key_origins >= 2,
        "generated let map pattern and for object pattern keys should carry source-map spans: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "binding current" && member.span.is_some()),
        "generated map pattern binding should still carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "binding each" && member.span.is_some()),
        "generated for object pattern binding should still carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "stmt for" && member.span.is_some()),
        "generated for statement should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "pattern map" && member.span.is_some()),
        "generated let map pattern category should carry a source-map span: {generated_fn:?}"
    );
    for label in ["pattern key", "pattern value", "pattern rest"] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated map pattern structure origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "for_pattern object" && member.span.is_some()),
        "generated for object pattern category should carry a source-map span: {generated_fn:?}"
    );
    for label in ["for_pattern key", "for_pattern value"] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated for object pattern structure origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_generated_pattern_category_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    let response = proc_macro_response_from_source(
        r#"
            fn generated() {
                let [head, ..tail] = rows;
                for (left, right) in pairs {
                    left;
                }
                for [first, ..rest] in rows {
                    first;
                }
                for _ in rows {
                    rows;
                }
                let limit = 0;
                return match value {
                    item | item if item > limit => item,
                    1..limit => 2,
                    0 => 0,
                    _ => 1
                };
            }
            "#,
    );
    let mut config = shell_response_config(shell, &response);
    config.max_output_bytes = 16 * 1024;
    providers.register_attribute("pattern_shapes", config);

    let expanded = expand_program_source(
        r#"
            #[pattern_shapes]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "pattern_shapes")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    for label in [
        "pattern list",
        "pattern element",
        "pattern rest",
        "pattern variable",
        "pattern or",
        "pattern alternative",
        "pattern guard",
        "pattern guard_expr",
        "pattern range",
        "pattern range_exclusive",
        "pattern range_start",
        "pattern range_end",
        "match guard",
        "pattern literal",
        "pattern literal_int",
        "pattern wildcard",
        "match_arm",
        "for_pattern tuple",
        "for_pattern element",
        "for_pattern variable",
        "for_pattern array",
        "for_pattern rest",
        "for_pattern ignore",
        "binding head",
        "binding tail",
        "binding item",
        "binding left",
        "binding right",
        "binding first",
        "binding rest",
        "binding limit",
        "ref limit",
    ] {
        assert!(
            generated_fn
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated pattern category or binding origin `{label}` should carry a source-map span: {generated_fn:?}"
        );
    }
}

#[test]
fn external_attribute_records_generated_import_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "imports_attr",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Use","lexeme":"use","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"sqrt","span":null},{"kind":"As","lexeme":"as","span":null},{"kind":"Id","lexeme":"root","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"abs","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"From","lexeme":"from","span":null},{"kind":"Id","lexeme":"math","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Use","lexeme":"use","span":null},{"kind":"Mul","lexeme":"*","span":null},{"kind":"As","lexeme":"as","span":null},{"kind":"Id","lexeme":"m","span":null},{"kind":"From","lexeme":"from","span":null},{"kind":"Id","lexeme":"math","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Use","lexeme":"use","span":null},{"kind":"Id","lexeme":"math","span":null},{"kind":"As","lexeme":"as","span":null},{"kind":"Id","lexeme":"numbers","span":null},{"kind":"Semicolon","lexeme":";","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[imports_attr]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "imports_attr")
        .expect("attribute origin should be recorded");
    let all_member_origins = origin
        .generated_item_origins
        .iter()
        .flat_map(|item| &item.generated_member_origins)
        .collect::<Vec<_>>();
    for label in [
        "import_module math",
        "import_item sqrt",
        "import_alias root",
        "import_item abs",
        "import_namespace m",
        "import_alias numbers",
    ] {
        assert!(
            all_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated import origin `{label}` should carry a source-map span: {origin:?}"
        );
    }
}

#[test]
fn external_attribute_records_generated_attribute_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "attrs_attr",
        shell_response_config(
            shell,
            r##"{"protocol_version":1,"output_tokens":[{"kind":"Hash","lexeme":"#","span":null},{"kind":"LBracket","lexeme":"[","span":null},{"kind":"Id","lexeme":"repr","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Str","lexeme":"\"lk\"","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"RBracket","lexeme":"]","span":null},{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"1","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"##,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[attrs_attr]
            fn old() {
                return 0;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "attrs_attr")
        .expect("attribute origin should be recorded");
    let generated_fn = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "fn generated")
        .expect("generated function origin");
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "attr repr" && member.span.is_some()),
        "generated preserved attribute should carry a source-map span: {generated_fn:?}"
    );
    assert!(
        generated_fn
            .generated_member_origins
            .iter()
            .any(|member| member.label == "fn generated" && member.span.is_some()),
        "generated function member should still carry a source-map span: {generated_fn:?}"
    );
}

#[test]
fn external_attribute_records_generated_derive_attribute_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "derive_attrs_attr",
        shell_response_config(
            shell,
            r##"{"protocol_version":1,"output_tokens":[{"kind":"Hash","lexeme":"#","span":null},{"kind":"LBracket","lexeme":"[","span":null},{"kind":"Id","lexeme":"derive","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"Debug","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"RBracket","lexeme":"]","span":null},{"kind":"Struct","lexeme":"struct","span":null},{"kind":"Id","lexeme":"Generated","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"id","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"Int","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"##,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[derive_attrs_attr]
            fn old() {
                return 0;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "derive_attrs_attr")
        .expect("attribute origin should be recorded");
    let generated_struct = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "struct Generated")
        .expect("generated struct origin");
    for label in ["attr derive", "derive Debug", "struct Generated"] {
        assert!(
            generated_struct
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated derive attribute origin `{label}` should carry a source-map span: {generated_struct:?}"
        );
    }
}

#[test]
fn external_attribute_records_generated_attribute_argument_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "meta_attrs_attr",
        shell_response_config(
            shell,
            r##"{"protocol_version":1,"output_tokens":[{"kind":"Hash","lexeme":"#","span":null},{"kind":"LBracket","lexeme":"[","span":null},{"kind":"Id","lexeme":"meta","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"feature","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Str","lexeme":"\"debug\"","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"all","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Str","lexeme":"\"lsp\"","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Str","lexeme":"\"cli\"","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"enabled","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Bool","lexeme":"true","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"retries","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Int","lexeme":"3","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"ratio","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Float","lexeme":"1.5","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"fallback","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Nil","lexeme":"nil","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"RBracket","lexeme":"]","span":null},{"kind":"Struct","lexeme":"struct","span":null},{"kind":"Id","lexeme":"Generated","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"id","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"Int","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"##,
        ),
    );

    let expanded = expand_program_source(
        r#"
            #[meta_attrs_attr]
            fn old() {
                return 0;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external attribute should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "meta_attrs_attr")
        .expect("attribute origin should be recorded");
    let generated_struct = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "struct Generated")
        .expect("generated struct origin");
    for label in [
        "attr meta",
        "attr_key feature",
        "attr_value debug",
        "attr_arg all",
        "attr_value lsp",
        "attr_value cli",
        "attr_key enabled",
        "attr_value true",
        "attr_key retries",
        "attr_value 3",
        "attr_key ratio",
        "attr_value 1.5",
        "attr_key fallback",
        "attr_value nil",
        "struct Generated",
    ] {
        assert!(
            generated_struct
                .generated_member_origins
                .iter()
                .any(|member| member.label == label && member.span.is_some()),
            "generated attribute argument origin `{label}` should carry a source-map span: {generated_struct:?}"
        );
    }
}

#[test]
fn external_attribute_provider_error_diagnostic_fails_parse() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_attribute(
        "fail_attr",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[],"diagnostics":[{"level":"Error","message":"attribute refused input","span":null,"notes":["check attribute provider logs"]}],"dependencies":[]}"#,
        ),
    );

    let err = parse_program_source(
        r#"
            #[fail_attr]
            fn old() {
                return 1;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect_err("provider diagnostic should fail parsing");

    let message = err.to_string();
    assert!(message.contains("attribute refused input"));
    assert!(message.contains("check attribute provider logs"));
}
