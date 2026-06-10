use crate::{
    macro_system::MacroOriginKind,
    syntax::{ParseOptions, expand_source, parse_program_source},
    token::Token,
};

#[test]
fn records_nested_declarative_macro_token_origins() {
    let expanded = expand_source(
        r#"
        macro_rules! inner {
            () => { return 42; };
        }
        macro_rules! outer {
            () => { inner!() };
        }
        outer!();
        "#,
        ParseOptions::default(),
    )
    .expect("nested macro expansion should succeed");

    let index = expanded
        .tokens
        .iter()
        .position(|token| matches!(token, Token::Int(42)))
        .expect("expanded integer should be present");
    let origin = &expanded.origins[index];

    assert_eq!(origin.lexeme, "42");
    assert_eq!(origin.frames.len(), 2);
    assert_eq!(origin.frames[0].macro_name, "outer");
    assert_eq!(origin.frames[0].kind, MacroOriginKind::Definition);
    assert_eq!(origin.frames[1].macro_name, "inner");
    assert_eq!(origin.frames[1].kind, MacroOriginKind::Definition);
}

#[test]
fn parse_errors_after_macro_expansion_include_origin_stack() {
    let err = parse_program_source(
        r#"
        macro_rules! bad_binding {
            () => { let = 1; };
        }
        bad_binding!();
        "#,
        ParseOptions::default(),
    )
    .expect_err("macro-generated invalid syntax should fail during parsing");

    let message = err.to_string();
    assert!(message.contains("Expected pattern after 'let'"));
    assert!(message.contains("Macro origin stack:"));
    assert!(message.contains("token `=` from definition of `bad_binding`"));
}
