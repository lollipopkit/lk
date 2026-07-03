use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

#[test]
fn derive_debug_generates_runtime_show_for_template_display() {
    let result = execute_source(
        r#"
            #[derive(Debug)]
            struct User {
                id: Int,
                name: String,
            }

            let user = User { id: 7, name: "Ada" };
            return "${user}" == "User { id: 7, name: Ada }";
            "#,
    )
    .expect("execute derived debug");

    assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
}

#[test]
fn derive_show_preserves_non_macro_attributes() {
    let result = execute_source(
        r#"
            #[repr("lk")]
            #[derive(Show)]
            struct Empty {}

            let value = Empty {};
            return "${value}" == "Empty {}";
            "#,
    )
    .expect("execute derived show with preserved attr");

    assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
}

#[test]
fn builtin_derive_records_ast_macro_origin_metadata() {
    let expanded = expand_program_source(
        r#"
            #[derive(Debug)]
            struct User { id: Int }
            "#,
        ParseOptions::default(),
    )
    .expect("derive expansion should succeed");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "Debug")
        .expect("Debug derive origin should be recorded");
    assert_eq!(origin.kind, AstMacroOriginKind::BuiltinDerive);
    assert!(origin.input_span.is_some());
    assert_eq!(origin.generated_items, 2);
    assert_eq!(
        origin.generated_item_labels,
        vec!["trait __LKShow".to_string(), "impl __LKShow for User".to_string()]
    );
    assert_eq!(origin.generated_item_origins.len(), 2);
    assert_eq!(origin.generated_item_origins[0].label, "trait __LKShow");
    assert!(
        origin.generated_item_origins[0].span.is_some(),
        "generated item origin should fall back to the derive input span"
    );
    let generated_impl = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "impl __LKShow for User")
        .expect("generated impl origin should be recorded");
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "fn show" && member.span.is_some()),
        "generated show method origin should carry a source-map span"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "expr self.id" && member.span.is_some()),
        "generated field expression origin should carry a source-map span"
    );
}

#[test]
fn unregistered_external_derive_reports_parse_error() {
    let err = parse_program_source(
        r#"
            #[derive(Clone)]
            struct User { id: Int }
            "#,
        ParseOptions::default(),
    )
    .expect_err("unregistered derive should fail");

    assert!(
        err.to_string()
            .contains("No procedural derive provider registered for `Clone`")
    );
}

#[test]
fn derive_on_non_struct_reports_attribute_span() {
    let err = parse_program_source(
        r#"
            #[derive(Debug)]
            fn answer() {
                return 42;
            }
            "#,
        ParseOptions::default(),
    )
    .expect_err("derive on function should fail");

    assert!(
        err.to_string()
            .contains("built-in derive macros currently support structs only")
    );
    assert!(err.span.is_some(), "derive error should keep the attribute span");
}

#[test]
fn cfg_false_removes_item_before_type_check_and_execution() {
    let result = execute_source(
        r#"
            #[cfg(false)]
            fn value() {
                return unknown_symbol;
            }

            #[cfg(true)]
            fn value() {
                return 42;
            }

            return value();
            "#,
    )
    .expect("execute cfg-filtered program");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn cfg_feature_not_any_all_selects_items_from_parse_options() {
    let program = parse_program_source(
        r#"
            #[cfg(all(feature = "debug", any(feature = "cli", feature("lsp"))))]
            fn value() {
                return 7;
            }

            #[cfg(not(feature = "debug"))]
            fn value() {
                return 1;
            }

            return value();
            "#,
        ParseOptions {
            macro_features: vec!["debug".to_string(), "cli".to_string()],
            ..ParseOptions::default()
        },
    )
    .expect("parse cfg feature program");

    let result = program.execute().expect("execute cfg feature program");
    assert_eq!(result.returns, vec![RuntimeVal::Int(7)]);
}

#[test]
fn external_derive_provider_appends_generated_items() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_derive(
        "MakeAnswer",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"99","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );
    let program = parse_program_source(
        r#"
            #[derive(MakeAnswer)]
            struct User { id: Int }

            return generated();
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external derive should expand");

    let result = program.execute().expect("execute external derive output");
    assert_eq!(result.returns, vec![RuntimeVal::Int(99)]);
}

#[test]
fn external_derive_provider_accepts_type_alias() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_derive(
        "MakeAliasHelper",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"42","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );
    let program = parse_program_source(
        r#"
            struct User { id: Int }

            #[derive(MakeAliasHelper)]
            type UserId = User;

            return generated();
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external derive should accept type aliases");

    let result = program.execute().expect("execute external derive output");
    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn external_derive_provider_accepts_trait() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_derive(
        "MakeTraitHelper",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"7","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );
    let program = parse_program_source(
        r#"
            struct User { id: Int }

            #[derive(MakeTraitHelper)]
            trait Reader {
                fn read(self: User) -> Int;
            }

            return generated();
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external derive should accept traits");

    let result = program.execute().expect("execute external derive output");
    assert_eq!(result.returns, vec![RuntimeVal::Int(7)]);
}

#[test]
fn external_derive_on_function_reports_attribute_span() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_derive(
        "MakeAnswer",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"1","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let err = parse_program_source(
        r#"
            #[derive(MakeAnswer)]
            fn answer() {
                return 42;
            }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect_err("external derive on function should fail");

    assert!(
        err.to_string()
            .contains("external derive macros currently support structs, type aliases, and traits only")
    );
    assert!(err.span.is_some(), "derive error should keep the attribute span");
}

#[test]
fn external_derive_on_type_alias_records_generated_type_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_derive(
        "MakeAlias",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Type","lexeme":"type","span":null},{"kind":"Id","lexeme":"GeneratedAlias","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Id","lexeme":"UserId","span":null},{"kind":"Semicolon","lexeme":";","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            struct User { id: Int }

            #[derive(MakeAlias)]
            type UserId = User;
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external derive should expand on type alias");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "MakeAlias")
        .expect("external derive origin should be recorded");
    assert_eq!(origin.kind, AstMacroOriginKind::ExternalDerive);
    assert_eq!(origin.generated_item_labels, vec!["type GeneratedAlias".to_string()]);
    let generated_alias = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "type GeneratedAlias")
        .expect("generated type alias item origin");
    assert!(
        generated_alias
            .generated_member_origins
            .iter()
            .any(|member| member.label == "type_ref UserId" && member.span.is_some()),
        "external generated type alias target should carry a source-map span: {generated_alias:?}"
    );
}

#[test]
fn external_derive_records_generated_impl_member_origins() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_derive(
        "MakeValue",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[{"kind":"Impl","lexeme":"impl","span":null},{"kind":"Id","lexeme":"Value","span":null},{"kind":"Id","lexeme":"for","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"value","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"self","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"FnArrow","lexeme":"->","span":null},{"kind":"Id","lexeme":"Int","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Let","lexeme":"let","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Id","lexeme":"seed","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"PlusAssign","lexeme":"+=", "span":null},{"kind":"Int","lexeme":"1","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Let","lexeme":"let","span":null},{"kind":"Id","lexeme":"meta","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"kind","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"seed","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"Assign","lexeme":"=","span":null},{"kind":"Id","lexeme":"helper","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Id","lexeme":"id","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"seed","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"self","span":null},{"kind":"Dot","lexeme":".","span":null},{"kind":"Id","lexeme":"id","span":null},{"kind":"Comma","lexeme":",","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"meta","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Id","lexeme":"current","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[]}"#,
        ),
    );

    let expanded = expand_program_source(
        r#"
            trait Value {
                fn value(self: User) -> Int;
            }

            #[derive(MakeValue)]
            struct User { id: Int }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect("external derive should expand");

    let origin = expanded
        .ast_macro_origins
        .iter()
        .find(|origin| origin.macro_name == "MakeValue")
        .expect("external derive origin should be recorded");
    assert_eq!(origin.kind, AstMacroOriginKind::ExternalDerive);
    let generated_impl = origin
        .generated_item_origins
        .iter()
        .find(|item| item.label == "impl Value for User")
        .expect("generated impl item origin");
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "fn value" && member.span.is_some()),
        "external generated impl method should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "binding self" && member.span.is_some()),
        "external generated function parameter binding should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "binding current" && member.span.is_some()),
        "external generated let binding should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "binding meta" && member.span.is_some()),
        "external generated semantic-name helper binding should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "expr self.id" && member.span.is_some()),
        "external generated field expression should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "call helper" && member.span.is_some()),
        "external generated call callee should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "ref seed" && member.span.is_some()),
        "external generated variable reference should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "assign_ref current" && member.span.is_some()),
        "external generated assignment target reference should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "compound_assign_ref current" && member.span.is_some()),
        "external generated compound assignment target reference should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "struct_field id" && member.span.is_some()),
        "external generated struct literal field name should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "map_key kind" && member.span.is_some()),
        "external generated map key should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "named_arg current" && member.span.is_some()),
        "external generated named argument key should carry a member source-map span: {generated_impl:?}"
    );
    assert!(
        generated_impl
            .generated_member_origins
            .iter()
            .any(|member| member.label == "type_ref Value" && member.span.is_some()),
        "external generated impl trait type reference should carry a member source-map span: {generated_impl:?}"
    );
    let user_type_refs = generated_impl
        .generated_member_origins
        .iter()
        .filter(|member| member.label == "type_ref User" && member.span.is_some())
        .count();
    assert!(
        user_type_refs >= 3,
        "external generated target, parameter, and struct-literal type references should carry source-map spans: {generated_impl:?}"
    );
}

#[test]
fn external_derive_provider_error_diagnostic_fails_parse() {
    let Some(shell) = test_shell() else {
        return;
    };
    let mut providers = ProcMacroProviders::default();
    providers.register_derive(
        "Fail",
        shell_response_config(
            shell,
            r#"{"protocol_version":1,"output_tokens":[],"diagnostics":[{"level":"Error","message":"derive refused input","span":null,"notes":["check provider logs"]}],"dependencies":[]}"#,
        ),
    );

    let err = parse_program_source(
        r#"
            #[derive(Fail)]
            struct User { id: Int }
            "#,
        ParseOptions {
            proc_macro_providers: providers,
            ..ParseOptions::default()
        },
    )
    .expect_err("provider diagnostic should fail parsing");

    let message = err.to_string();
    assert!(message.contains("derive refused input"));
    assert!(message.contains("check provider logs"));
}
