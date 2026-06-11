use super::*;

#[test]
fn local_lookup_resolves_same_file_macro_call_name() {
    let content = "macro_rules! answer { () => { 42 }; }\nlet x = answer!();\n";
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
    let uri = Url::parse("file:///tmp/macros.lk").expect("uri");
    let call_offset = content.rfind("answer").expect("call answer") + 2;

    let location = find_local_macro_definition(&tokens, &spans, call_offset, &uri).expect("macro definition location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(0, 13));
    assert_eq!(location.range.end, Position::new(0, 19));
}

#[test]
fn local_lookup_resolves_same_file_macro_call_bang() {
    let content = "macro_rules! answer { () => { 42 }; }\nlet x = answer!();\n";
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
    let uri = Url::parse("file:///tmp/macros.lk").expect("uri");
    let bang_offset = content.rfind('!').expect("call bang");

    let location = find_local_macro_definition(&tokens, &spans, bang_offset, &uri).expect("macro definition location");

    assert_eq!(location.range.start, Position::new(0, 13));
    assert_eq!(location.range.end, Position::new(0, 19));
}

#[test]
fn imported_named_macro_tracks_alias_to_source_name() {
    let content = "use { answer as ans } from \"macros\";\nlet x = ans!();\n";
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
    let offset = content.rfind("ans").expect("call") + 1;

    let imported = imported_macro_definition(&tokens, &spans, offset).expect("imported macro");

    assert_eq!(
        imported,
        ImportedMacroDefinition {
            source: ImportedMacroSource::File("macros".to_string()),
            name: "answer".to_string(),
        }
    );
}

#[test]
fn imported_namespace_macro_tracks_explicit_alias() {
    let content = "use * as m from util;\nlet x = m::answer!();\n";
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
    let offset = content.rfind("answer").expect("call") + 1;

    let imported = imported_macro_definition(&tokens, &spans, offset).expect("imported macro");

    assert_eq!(
        imported,
        ImportedMacroDefinition {
            source: ImportedMacroSource::Package("util".to_string()),
            name: "answer".to_string(),
        }
    );
}

#[test]
fn imported_namespace_macro_tracks_default_file_namespace() {
    let content = "use \"macros\";\nlet x = macros::answer!();\n";
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
    let offset = content.rfind("answer").expect("call") + 1;

    let imported = imported_macro_definition(&tokens, &spans, offset).expect("imported macro");

    assert_eq!(
        imported,
        ImportedMacroDefinition {
            source: ImportedMacroSource::File("macros".to_string()),
            name: "answer".to_string(),
        }
    );
}

#[test]
fn imported_namespace_macro_tracks_package_alias() {
    let content = "use util as u;\nlet x = u::answer!();\n";
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).expect("tokens");
    let offset = content.rfind("answer").expect("call") + 1;

    let imported = imported_macro_definition(&tokens, &spans, offset).expect("imported macro");

    assert_eq!(
        imported,
        ImportedMacroDefinition {
            source: ImportedMacroSource::Package("util".to_string()),
            name: "answer".to_string(),
        }
    );
}

#[test]
fn exported_macro_definition_follows_re_export_alias() {
    let content = "macro_rules! hidden { () => { 42 }; }\nexport { hidden as answer };\n";
    let uri = Url::parse("file:///tmp/macros.lk").expect("uri");

    let location =
        find_exported_macro_definition_in_content(content, "answer", &uri).expect("re-exported macro location");

    assert_eq!(location.range.start, Position::new(0, 13));
    assert_eq!(location.range.end, Position::new(0, 19));
}

#[test]
fn exported_macro_definition_finds_direct_export() {
    let content = "export macro_rules! answer { () => { 42 }; }\n";
    let uri = Url::parse("file:///tmp/macros.lk").expect("uri");

    let location = find_exported_macro_definition_in_content(content, "answer", &uri).expect("exported macro location");

    assert_eq!(location.range.start, Position::new(0, 20));
    assert_eq!(location.range.end, Position::new(0, 26));
}

#[test]
fn generated_ast_definition_finds_external_generated_impl_member() {
    let uri = Url::parse("file:///tmp/external-member-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(2, 1, 1), token::Position::new(2, 21, 21));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "MakeValue".to_string(),
        kind: macro_system::AstMacroOriginKind::ExternalDerive,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["impl Value for User".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "impl Value for User".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "fn value".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "value", &uri).expect("generated member location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(1, 0));
    assert_eq!(location.range.end, Position::new(1, 20));
}

#[test]
fn generated_ast_definition_finds_generated_impl_item_trait_and_target_names() {
    let uri = Url::parse("file:///tmp/external-impl-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(3, 2, 20), token::Position::new(3, 26, 44));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "MakeValue".to_string(),
        kind: macro_system::AstMacroOriginKind::ExternalDerive,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["impl Value for User".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "impl Value for User".to_string(),
            span: Some(span),
            generated_member_origins: vec![],
        }],
    }];

    let trait_location =
        generated_ast_item_definition_location(&origins, "Value", &uri).expect("generated impl trait location");
    let target_location =
        generated_ast_item_definition_location(&origins, "User", &uri).expect("generated impl target location");

    assert_eq!(trait_location.uri, uri);
    assert_eq!(trait_location.range.start, Position::new(2, 1));
    assert_eq!(trait_location.range.end, Position::new(2, 25));
    assert_eq!(target_location.range.start, Position::new(2, 1));
    assert_eq!(target_location.range.end, Position::new(2, 25));
}

#[test]
fn generated_ast_definition_finds_generated_inherent_impl_item_target_name() {
    let uri = Url::parse("file:///tmp/external-inherent-impl-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(4, 3, 40), token::Position::new(4, 18, 55));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "MakeMethods".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["impl User".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "impl User".to_string(),
            span: Some(span),
            generated_member_origins: vec![],
        }],
    }];

    let location =
        generated_ast_item_definition_location(&origins, "User", &uri).expect("generated inherent impl location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(3, 2));
    assert_eq!(location.range.end, Position::new(3, 17));
}

#[test]
fn generated_ast_definition_finds_arbitrary_generated_field_expression() {
    let uri = Url::parse("file:///tmp/external-field-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(3, 3, 20), token::Position::new(3, 17, 34));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "ProjectField".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "expr user.profile.id".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location =
        generated_ast_item_definition_location(&origins, "id", &uri).expect("generated field expression location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(2, 2));
    assert_eq!(location.range.end, Position::new(2, 16));
}

#[test]
fn generated_ast_definition_ignores_expression_category_labels() {
    let uri = Url::parse("file:///tmp/external-expression-category-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(3, 3, 20), token::Position::new(3, 17, 34));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "ExpressionCategories".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr access".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr literal".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr or".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];

    assert!(generated_ast_item_definition_location(&origins, "access", &uri).is_none());
    assert!(generated_ast_item_definition_location(&origins, "literal", &uri).is_none());
    assert!(generated_ast_item_definition_location(&origins, "or", &uri).is_none());
}

#[test]
fn generated_ast_definition_finds_static_index_expression_base() {
    let uri = Url::parse("file:///tmp/external-index-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(3, 3, 20), token::Position::new(3, 17, 34));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "ProjectIndex".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "index items.0".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "items", &uri).expect("generated index location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(2, 2));
    assert_eq!(location.range.end, Position::new(2, 16));
}

#[test]
fn generated_ast_definition_finds_generated_call_callee_reference() {
    let uri = Url::parse("file:///tmp/external-call-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(5, 4, 40), token::Position::new(5, 24, 60));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateCall".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "call user.profile.render".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location =
        generated_ast_item_definition_location(&origins, "render", &uri).expect("generated call callee location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(4, 3));
    assert_eq!(location.range.end, Position::new(4, 23));
}

#[test]
fn generated_ast_definition_finds_call_callee_after_static_index() {
    let uri = Url::parse("file:///tmp/external-index-call-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(5, 4, 40), token::Position::new(5, 29, 65));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateIndexedCall".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "index items.0".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "call items.0.render".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];

    let render_location =
        generated_ast_item_definition_location(&origins, "render", &uri).expect("generated render location");
    let items_location =
        generated_ast_item_definition_location(&origins, "items", &uri).expect("generated index base location");

    assert_eq!(render_location.uri, uri);
    assert_eq!(render_location.range.start, Position::new(4, 3));
    assert_eq!(render_location.range.end, Position::new(4, 28));
    assert_eq!(items_location.range.start, Position::new(4, 3));
    assert_eq!(items_location.range.end, Position::new(4, 28));
}

#[test]
fn generated_ast_definition_finds_generated_variable_reference() {
    let uri = Url::parse("file:///tmp/external-ref-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(6, 5, 50), token::Position::new(6, 11, 56));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateRef".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "ref seed".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "seed", &uri).expect("generated ref location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(5, 4));
    assert_eq!(location.range.end, Position::new(5, 10));
}

#[test]
fn generated_ast_definition_finds_generated_range_pattern_reference() {
    let uri = Url::parse("file:///tmp/external-range-pattern-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(6, 9, 70), token::Position::new(6, 18, 79));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateRangeMatch".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "ref max".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "max", &uri).expect("generated range ref location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(5, 8));
    assert_eq!(location.range.end, Position::new(5, 17));
}

#[test]
fn generated_ast_definition_finds_generated_assignment_target_reference() {
    let uri = Url::parse("file:///tmp/external-assign-ref-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(7, 6, 70), token::Position::new(7, 18, 82));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateAssignRef".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "compound_assign_ref current".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location =
        generated_ast_item_definition_location(&origins, "current", &uri).expect("generated assign ref location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(6, 5));
    assert_eq!(location.range.end, Position::new(6, 17));
}

#[test]
fn generated_ast_definition_finds_top_level_generated_statement_origin() {
    let uri = Url::parse("file:///tmp/external-top-level-statement-origin.lk").expect("uri");
    let statement_span = token::Span::new(token::Position::new(4, 1, 40), token::Position::new(4, 24, 63));
    let ref_span = token::Span::new(token::Position::new(4, 15, 54), token::Position::new(4, 19, 58));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateStatement".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(statement_span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["statement".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "statement".to_string(),
            span: Some(statement_span),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "binding current".to_string(),
                    span: Some(ref_span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "type_ref User".to_string(),
                    span: Some(ref_span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "ref seed".to_string(),
                    span: Some(ref_span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "compound_assign_ref current".to_string(),
                    span: Some(ref_span),
                },
            ],
        }],
    }];

    let current_location =
        generated_ast_item_definition_location(&origins, "current", &uri).expect("generated binding location");
    let user_location =
        generated_ast_item_definition_location(&origins, "User", &uri).expect("generated type ref location");
    let seed_location = generated_ast_item_definition_location(&origins, "seed", &uri).expect("generated ref location");

    assert_eq!(current_location.uri, uri);
    assert_eq!(current_location.range.start, Position::new(3, 14));
    assert_eq!(current_location.range.end, Position::new(3, 18));
    assert_eq!(user_location.range.start, Position::new(3, 14));
    assert_eq!(user_location.range.end, Position::new(3, 18));
    assert_eq!(seed_location.range.start, Position::new(3, 14));
    assert_eq!(seed_location.range.end, Position::new(3, 18));
}

#[test]
fn generated_ast_definition_finds_generated_control_flow_origin() {
    let uri = Url::parse("file:///tmp/external-control-flow-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(5, 3, 60), token::Position::new(5, 19, 76));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateControlFlow".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "stmt if".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr match".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];

    let if_location =
        generated_ast_item_definition_location(&origins, "if", &uri).expect("generated if origin location");
    let match_location =
        generated_ast_item_definition_location(&origins, "match", &uri).expect("generated match origin location");

    assert_eq!(if_location.uri, uri);
    assert_eq!(if_location.range.start, Position::new(4, 2));
    assert_eq!(if_location.range.end, Position::new(4, 18));
    assert_eq!(match_location.range.start, Position::new(4, 2));
    assert_eq!(match_location.range.end, Position::new(4, 18));
}

#[test]
fn generated_ast_definition_finds_generated_select_case_origins() {
    let uri = Url::parse("file:///tmp/external-select-case-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(6, 4, 60), token::Position::new(6, 28, 84));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateSelect".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "select recv".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "select send".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "select default".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];

    let recv_location =
        generated_ast_item_definition_location(&origins, "recv", &uri).expect("generated recv case location");
    let send_location =
        generated_ast_item_definition_location(&origins, "send", &uri).expect("generated send case location");
    let default_location =
        generated_ast_item_definition_location(&origins, "default", &uri).expect("generated default case location");

    assert_eq!(recv_location.uri, uri);
    assert_eq!(recv_location.range.start, Position::new(5, 3));
    assert_eq!(recv_location.range.end, Position::new(5, 27));
    assert_eq!(send_location.range.start, Position::new(5, 3));
    assert_eq!(send_location.range.end, Position::new(5, 27));
    assert_eq!(default_location.range.start, Position::new(5, 3));
    assert_eq!(default_location.range.end, Position::new(5, 27));
}

#[test]
fn generated_ast_definition_finds_generated_binding_origin() {
    let uri = Url::parse("file:///tmp/external-binding-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(8, 7, 90), token::Position::new(8, 18, 101));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateBinding".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "binding current".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location =
        generated_ast_item_definition_location(&origins, "current", &uri).expect("generated binding location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(7, 6));
    assert_eq!(location.range.end, Position::new(7, 17));
}

#[test]
fn generated_ast_definition_finds_generated_semantic_name_origin() {
    let uri = Url::parse("file:///tmp/external-semantic-name-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(9, 8, 110), token::Position::new(9, 20, 122));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateSemanticName".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "named_arg current".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location =
        generated_ast_item_definition_location(&origins, "current", &uri).expect("generated semantic name location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(8, 7));
    assert_eq!(location.range.end, Position::new(8, 19));
}

#[test]
fn generated_ast_definition_finds_generated_named_function_type_parameter_origin() {
    let uri = Url::parse("file:///tmp/external-named-function-type-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(9, 8, 110), token::Position::new(9, 24, 126));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateNamedFunctionType".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "named_param_type current".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "current", &uri)
        .expect("generated named function type parameter location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(8, 7));
    assert_eq!(location.range.end, Position::new(8, 23));
}

#[test]
fn generated_ast_definition_finds_generated_map_pattern_key_origin() {
    let uri = Url::parse("file:///tmp/external-pattern-key-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(10, 9, 120), token::Position::new(10, 21, 132));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GeneratePatternKey".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "map_key kind".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "kind", &uri).expect("generated map key location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(9, 8));
    assert_eq!(location.range.end, Position::new(9, 20));
}

#[test]
fn generated_ast_definition_finds_generated_import_origins() {
    let uri = Url::parse("file:///tmp/external-import-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(8, 4, 80), token::Position::new(8, 28, 104));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateImports".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["statement".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "statement".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_module math".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_item sqrt".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_alias root".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_namespace m".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];

    for name in ["math", "sqrt", "root", "m"] {
        let location =
            generated_ast_item_definition_location(&origins, name, &uri).expect("generated import origin location");
        assert_eq!(location.uri, uri);
        assert_eq!(location.range.start, Position::new(7, 3));
        assert_eq!(location.range.end, Position::new(7, 27));
    }
}

#[test]
fn generated_ast_definition_finds_generated_attribute_origins() {
    let uri = Url::parse("file:///tmp/external-attribute-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(9, 5, 90), token::Position::new(9, 24, 109));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateAttrs".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["struct Generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "struct Generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "attr derive".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "derive Debug".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];

    let attr_location =
        generated_ast_item_definition_location(&origins, "derive", &uri).expect("generated attr origin location");
    let derive_location =
        generated_ast_item_definition_location(&origins, "Debug", &uri).expect("generated derive origin location");

    assert_eq!(attr_location.uri, uri);
    assert_eq!(attr_location.range.start, Position::new(8, 4));
    assert_eq!(attr_location.range.end, Position::new(8, 23));
    assert_eq!(derive_location.range.start, Position::new(8, 4));
    assert_eq!(derive_location.range.end, Position::new(8, 23));
}

#[test]
fn generated_ast_definition_finds_generated_type_reference() {
    let uri = Url::parse("file:///tmp/external-type-ref-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(7, 6, 70), token::Position::new(7, 18, 82));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateTypeRef".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "type_ref User".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "User", &uri).expect("generated type ref location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(6, 5));
    assert_eq!(location.range.end, Position::new(6, 17));
}

#[test]
fn generated_ast_definition_finds_generated_type_variable_reference() {
    let uri = Url::parse("file:///tmp/external-type-var-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(7, 6, 70), token::Position::new(7, 14, 78));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateTypeVar".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["type Alias".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "type Alias".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![macro_system::AstGeneratedMemberOrigin {
                label: "type_var T".to_string(),
                span: Some(span),
            }],
        }],
    }];

    let location = generated_ast_item_definition_location(&origins, "T", &uri).expect("generated type var location");

    assert_eq!(location.uri, uri);
    assert_eq!(location.range.start, Position::new(6, 5));
    assert_eq!(location.range.end, Position::new(6, 13));
}

#[test]
fn generated_ast_definition_finds_nested_generated_declaration_labels() {
    let uri = Url::parse("file:///tmp/external-nested-declarations-origin.lk").expect("uri");
    let span = token::Span::new(token::Position::new(11, 10, 140), token::Position::new(11, 28, 158));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "GenerateNestedDeclarations".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "type Alias".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "struct Boxed".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "trait Reader".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "fn read".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];

    for name in ["Alias", "Boxed", "Reader", "read"] {
        let location = generated_ast_item_definition_location(&origins, name, &uri)
            .expect("nested generated declaration location");
        assert_eq!(location.uri, uri);
        assert_eq!(location.range.start, Position::new(10, 9));
        assert_eq!(location.range.end, Position::new(10, 27));
    }
}
