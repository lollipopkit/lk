use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::expr::{MatchArm, SelectCase, SelectPattern};

#[test]
fn generated_type_variable_origin_is_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 3, 2),
    ));
    let mut origins = Vec::new();

    collect_generated_type_origins(&Type::Variable("T".to_string()), span.clone(), &mut origins);

    assert_eq!(
        origins,
        vec![
            AstGeneratedMemberOrigin {
                label: "type_expr variable".to_string(),
                span: span.clone(),
            },
            AstGeneratedMemberOrigin {
                label: "type_var T".to_string(),
                span,
            },
        ]
    );
}

#[test]
fn generated_type_shape_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 30, 29),
    ));
    let ty = Type::Function {
        params: vec![
            Type::Tuple(vec![Type::Int, Type::Float]),
            Type::Map(
                Box::new(Type::String),
                Box::new(Type::List(Box::new(Type::Optional(Box::new(Type::Named(
                    "User".to_string(),
                )))))),
            ),
            Type::Set(Box::new(Type::Generic {
                name: "Result".to_string(),
                params: vec![Type::Variable("T".to_string()), Type::Nil],
            })),
            Type::Task(Box::new(Type::Channel(Box::new(Type::Any)))),
            Type::Boxed(Box::new(Type::Bool)),
        ],
        named_params: vec![crate::val::FunctionNamedParamType {
            name: "current".to_string(),
            ty: Type::Union(vec![Type::Named("User".to_string()), Type::Nil]),
            has_default: true,
        }],
        return_type: Box::new(Type::Named("User".to_string())),
    };
    let mut origins = Vec::new();

    collect_generated_type_origins(&ty, span, &mut origins);
    let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

    for label in [
        "type_expr function",
        "type_expr function_param",
        "type_expr function_named_param",
        "type_expr function_return",
        "type_expr tuple",
        "type_expr tuple_item",
        "type_expr int",
        "type_expr float",
        "type_expr map",
        "type_expr map_key",
        "type_expr map_value",
        "type_expr string",
        "type_expr list",
        "type_expr list_item",
        "type_expr optional",
        "type_expr optional_inner",
        "type_expr named",
        "type_expr set",
        "type_expr set_item",
        "type_expr generic",
        "type_expr generic_arg",
        "type_expr variable",
        "type_expr nil",
        "type_expr task",
        "type_expr task_output",
        "type_expr channel",
        "type_expr channel_item",
        "type_expr any",
        "type_expr boxed",
        "type_expr boxed_inner",
        "type_expr bool",
        "type_expr union",
        "type_expr union_item",
        "named_param_type current",
        "type_ref User",
        "type_ref Result",
        "type_var T",
    ] {
        assert!(
            labels.contains(&label),
            "generated type shape origin `{label}` should be recorded: {labels:?}"
        );
    }
}

#[test]
fn generated_literal_kind_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 8, 7),
    ));

    for (literal, expected) in [
        (LiteralVal::Int(1), "literal int"),
        (LiteralVal::Float(1.5), "literal float"),
        (LiteralVal::Bool(true), "literal bool"),
        (
            LiteralVal::ShortStr(crate::val::ShortStr::new("s").expect("short string")),
            "literal string",
        ),
        (LiteralVal::Nil, "literal nil"),
    ] {
        let mut origins = Vec::new();
        collect_generated_expr_origins(&Expr::Literal(literal), span.clone(), &mut origins);
        let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

        assert!(
            labels.contains(&"expr literal"),
            "literal category should be recorded: {labels:?}"
        );
        assert!(
            labels.contains(&expected),
            "literal kind `{expected}` should be recorded: {labels:?}"
        );
    }
}

#[test]
fn generated_operator_kind_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 8, 7),
    ));
    let literal = || Box::new(Expr::Literal(LiteralVal::Int(1)));

    for (op, expected) in [
        (BinOp::Add, "binary add"),
        (BinOp::Sub, "binary sub"),
        (BinOp::Mul, "binary mul"),
        (BinOp::Div, "binary div"),
        (BinOp::Mod, "binary mod"),
        (BinOp::Eq, "binary eq"),
        (BinOp::Ne, "binary ne"),
        (BinOp::Gt, "binary gt"),
        (BinOp::Lt, "binary lt"),
        (BinOp::Ge, "binary ge"),
        (BinOp::Le, "binary le"),
        (BinOp::In, "binary in"),
    ] {
        let mut origins = Vec::new();
        collect_generated_expr_origins(&Expr::Bin(literal(), op, literal()), span.clone(), &mut origins);
        let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

        assert!(
            labels.contains(&"expr binary"),
            "binary category should be recorded: {labels:?}"
        );
        assert!(
            labels.contains(&expected),
            "binary operator kind `{expected}` should be recorded: {labels:?}"
        );
    }

    let mut origins = Vec::new();
    collect_generated_expr_origins(
        &Expr::Unary(UnaryOp::Not, Box::new(Expr::Literal(LiteralVal::Bool(true)))),
        span,
        &mut origins,
    );
    let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

    assert!(
        labels.contains(&"expr unary"),
        "unary category should be recorded: {labels:?}"
    );
    assert!(
        labels.contains(&"unary not"),
        "unary operator kind should be recorded: {labels:?}"
    );
}

#[test]
fn generated_statement_shape_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 40, 39),
    ));
    let body = Box::new(Stmt::Block {
        statements: vec![
            Box::new(Stmt::CompoundAssign {
                name: "current".to_string(),
                op: BinOp::Add,
                value: Box::new(Expr::Literal(LiteralVal::Int(1))),
                span: None,
            }),
            Box::new(Stmt::Empty),
        ],
    });
    let function = Stmt::Function {
        name: "generated".to_string(),
        params: vec!["current".to_string()],
        param_types: vec![Some(Type::Int)],
        named_params: vec![crate::stmt::NamedParamDecl {
            name: "limit".to_string(),
            type_annotation: Some(Type::Int),
            default: Some(Expr::Literal(LiteralVal::Int(1))),
        }],
        return_type: Some(Type::Int),
        body,
    };
    let trait_stmt = Stmt::Trait {
        name: "Reader".to_string(),
        methods: vec![(
            "read".to_string(),
            Type::Function {
                params: vec![Type::Int],
                named_params: vec![],
                return_type: Box::new(Type::String),
            },
        )],
    };
    let impl_stmt = Stmt::Impl {
        trait_name: "Reader".to_string(),
        target_type: Type::Named("File".to_string()),
        methods: vec![Stmt::Function {
            name: "read".to_string(),
            params: vec![],
            param_types: vec![],
            named_params: vec![],
            return_type: Some(Type::String),
            body: Box::new(Stmt::Return {
                value: Some(Box::new(Expr::Literal(LiteralVal::String("ok".into())))),
            }),
        }],
    };
    let mut origins = Vec::new();

    collect_generated_expr_origins_from_stmt(&function, span.clone(), &mut origins);
    collect_generated_expr_origins_from_stmt(&trait_stmt, span.clone(), &mut origins);
    collect_generated_expr_origins_from_stmt(&impl_stmt, span, &mut origins);
    let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

    for label in [
        "stmt param",
        "binding current",
        "stmt param_type",
        "stmt named_param",
        "binding limit",
        "stmt named_param_type",
        "stmt param_default",
        "stmt return_type",
        "stmt compound_assign",
        "compound_assign add",
        "compound_assign_ref current",
        "stmt empty",
        "trait Reader",
        "stmt trait_method",
        "fn read",
        "stmt trait_method_type",
        "type_ref Reader",
        "stmt impl_trait",
        "stmt impl_target",
        "type_ref File",
        "stmt impl_method",
    ] {
        assert!(
            labels.contains(&label),
            "generated statement shape origin `{label}` should be recorded: {labels:?}"
        );
    }
}

#[test]
fn generated_top_level_declaration_shape_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 40, 39),
    ));
    let items = vec![
        Box::new(Stmt::Struct {
            name: "Boxed".to_string(),
            fields: vec![("item".to_string(), Some(Type::Named("User".to_string())))],
        }),
        Box::new(Stmt::TypeAlias {
            name: "Alias".to_string(),
            target: Type::Generic {
                name: "Result".to_string(),
                params: vec![Type::Named("User".to_string())],
            },
        }),
        Box::new(Stmt::Impl {
            trait_name: "Show".to_string(),
            target_type: Type::Named("User".to_string()),
            methods: vec![Stmt::Function {
                name: "show".to_string(),
                params: vec![],
                param_types: vec![],
                named_params: vec![],
                return_type: Some(Type::String),
                body: Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Literal(LiteralVal::String("user".into())))),
                }),
            }],
        }),
    ];

    let origins = generated_item_origins(&items, span);
    let labels = origins
        .iter()
        .flat_map(|origin| origin.generated_member_origins.iter())
        .map(|origin| origin.label.as_str())
        .collect::<Vec<_>>();

    for label in [
        "struct Boxed",
        "struct_field item",
        "stmt struct_field_type",
        "type_ref User",
        "type Alias",
        "stmt type_alias_target",
        "type_expr generic",
        "type_expr generic_arg",
        "type_ref Result",
        "type_ref Show",
        "stmt impl_trait",
        "stmt impl_target",
        "stmt impl_method",
        "fn show",
    ] {
        assert!(
            labels.contains(&label),
            "top-level generated declaration origin `{label}` should be recorded: {labels:?}"
        );
    }
}

#[test]
fn generated_range_shape_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 12, 11),
    ));
    let expr = Expr::Range {
        start: Some(Box::new(Expr::Literal(LiteralVal::Int(1)))),
        end: Some(Box::new(Expr::Literal(LiteralVal::Int(10)))),
        inclusive: true,
        step: Some(Box::new(Expr::Literal(LiteralVal::Int(2)))),
    };
    let mut origins = Vec::new();

    collect_generated_expr_origins(&expr, span, &mut origins);
    let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

    for label in [
        "expr range",
        "range inclusive",
        "range start",
        "range end",
        "range step",
    ] {
        assert!(
            labels.contains(&label),
            "generated range shape origin `{label}` should be recorded: {labels:?}"
        );
    }
}

#[test]
fn generated_remaining_expression_child_role_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 40, 39),
    ));
    let literal = || Box::new(Expr::Literal(LiteralVal::Int(1)));
    let expr = Expr::Block(vec![
        Box::new(Stmt::Expr(Box::new(Expr::Paren(literal())))),
        Box::new(Stmt::Expr(Box::new(Expr::StructLiteral {
            name: "User".to_string(),
            fields: vec![("id".to_string(), literal())],
        }))),
        Box::new(Stmt::Expr(Box::new(Expr::Access(
            Box::new(Expr::Var("items".to_string())),
            Box::new(Expr::Var("current".to_string())),
        )))),
        Box::new(Stmt::Expr(Box::new(Expr::OptionalAccess(
            Box::new(Expr::Var("maybe_items".to_string())),
            Box::new(Expr::Var("fallback".to_string())),
        )))),
        Box::new(Stmt::Expr(Box::new(Expr::Call("make".to_string(), vec![literal()])))),
        Box::new(Stmt::Expr(Box::new(Expr::CallNamed(
            Box::new(Expr::Var("make".to_string())),
            vec![literal()],
            vec![("id".to_string(), literal())],
        )))),
        Box::new(Stmt::Expr(Box::new(Expr::Closure {
            params: vec!["current".to_string()],
            body: Box::new(Expr::Var("current".to_string())),
        }))),
        Box::new(Stmt::Expr(Box::new(Expr::Match {
            value: Box::new(Expr::Var("current".to_string())),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Variable("matched".to_string()),
                    body: Box::new(Expr::Var("matched".to_string())),
                },
                MatchArm {
                    pattern: Pattern::Guard {
                        pattern: Box::new(Pattern::Variable("guarded".to_string())),
                        guard: Box::new(Expr::Var("ready".to_string())),
                    },
                    body: Box::new(Expr::Var("guarded".to_string())),
                },
                MatchArm {
                    pattern: Pattern::Literal(LiteralVal::Int(0)),
                    body: Box::new(Expr::Literal(LiteralVal::Nil)),
                },
            ],
        }))),
        Box::new(Stmt::Expr(Box::new(Expr::Select {
            cases: vec![SelectCase {
                pattern: SelectPattern::Recv {
                    binding: Some("message".to_string()),
                    channel: Box::new(Expr::Var("channel".to_string())),
                },
                guard: Some(Box::new(Expr::Var("ready".to_string()))),
                body: Box::new(Expr::Var("message".to_string())),
            }],
            default_case: Some(Box::new(Expr::Literal(LiteralVal::Nil))),
        }))),
    ]);
    let mut origins = Vec::new();

    collect_generated_expr_origins(&expr, span, &mut origins);
    let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

    for label in [
        "expr paren_inner",
        "expr struct_type",
        "type_ref User",
        "expr struct_field",
        "expr access_member",
        "ref current",
        "expr optional_access",
        "ref fallback",
        "expr call_callee",
        "call make",
        "expr call_named",
        "expr closure_param",
        "binding current",
        "expr match_arm_pattern",
        "expr match_arm_guard",
        "binding matched",
        "binding guarded",
        "ref ready",
        "pattern literal_int",
        "select body",
    ] {
        assert!(
            labels.contains(&label),
            "generated expression child-role origin `{label}` should be recorded: {labels:?}"
        );
    }
    assert!(
        !labels.contains(&"expr items.current"),
        "dynamic generated index references must not be flattened as static field origins: {labels:?}"
    );
}

#[test]
fn generated_lowered_struct_update_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 48, 47),
    ));
    let expr = Expr::Call(
        "__lk_make_struct".to_string(),
        vec![
            Box::new(Expr::Literal(LiteralVal::from_str("User"))),
            Box::new(Expr::Call(
                "__lk_merge_fields".to_string(),
                vec![
                    Box::new(Expr::Var("existing".to_string())),
                    Box::new(Expr::Map(vec![(
                        Box::new(Expr::Literal(LiteralVal::from_str("id"))),
                        Box::new(Expr::Var("current".to_string())),
                    )])),
                ],
            )),
        ],
    );
    let mut origins = Vec::new();

    collect_generated_expr_origins(&expr, span, &mut origins);
    let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

    for label in [
        "expr struct_literal",
        "expr struct_type",
        "type_ref User",
        "expr struct_update_base",
        "ref existing",
        "expr struct_update_fields",
        "expr struct_field",
        "struct_field id",
        "expr struct_field_value",
        "ref current",
    ] {
        assert!(
            labels.contains(&label),
            "lowered generated struct update origin `{label}` should be recorded: {labels:?}"
        );
    }
    assert!(
        !labels.contains(&"call __lk_make_struct"),
        "lowered struct updates should expose struct-literal origins, not internal constructor calls: {labels:?}"
    );
    assert!(
        !labels.contains(&"call __lk_merge_fields"),
        "lowered struct updates should expose update origins, not internal merge calls: {labels:?}"
    );
}

#[test]
fn generated_attribute_argument_origins_are_recorded() {
    let span = Some(Span::new(
        crate::token::Position::new(1, 1, 0),
        crate::token::Position::new(1, 52, 51),
    ));
    let stmt = Stmt::Attributed {
        attributes: vec![Attribute {
            tokens: vec![
                Token::Id("cfg".to_string()),
                Token::LParen,
                Token::Id("all".to_string()),
                Token::LParen,
                Token::Id("feature".to_string()),
                Token::Assign,
                Token::Str("debug".to_string()),
                Token::Comma,
                Token::Id("feature".to_string()),
                Token::LParen,
                Token::Str("lsp".to_string()),
                Token::Comma,
                Token::Str("cli".to_string()),
                Token::RParen,
                Token::Comma,
                Token::Id("enabled".to_string()),
                Token::Assign,
                Token::Bool(true),
                Token::Comma,
                Token::Id("retries".to_string()),
                Token::Assign,
                Token::Int(3),
                Token::Comma,
                Token::Id("ratio".to_string()),
                Token::Assign,
                Token::Float(1.5),
                Token::Comma,
                Token::Id("fallback".to_string()),
                Token::Assign,
                Token::Nil,
                Token::RParen,
                Token::RParen,
            ],
            span: span.clone(),
        }],
        item: Box::new(Stmt::Struct {
            name: "Generated".to_string(),
            fields: vec![],
        }),
    };

    let origins = generated_member_origins_for_stmt(&stmt, span);
    let labels = origins.iter().map(|origin| origin.label.as_str()).collect::<Vec<_>>();

    for label in [
        "attr cfg",
        "attr_arg all",
        "attr_key feature",
        "attr_value debug",
        "attr_arg feature",
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
            labels.contains(&label),
            "generated attribute argument origin `{label}` should be recorded: {labels:?}"
        );
    }
}
