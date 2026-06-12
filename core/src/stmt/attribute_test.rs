use crate::{
    stmt::{Stmt, stmt_parser::StmtParser},
    token::{Token, Tokenizer},
};

fn parse_one(source: &str) -> Stmt {
    let tokens = Tokenizer::tokenize(source).expect("tokenize source");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    assert_eq!(program.statements.len(), 1);
    *program.statements.into_iter().next().expect("one stmt")
}

#[test]
fn parses_attribute_tokens_on_item_declaration() {
    let stmt = parse_one("#[derive(Debug)] struct User { id: Int }");

    let Stmt::Attributed { attributes, item } = stmt else {
        panic!("expected attributed item");
    };
    assert_eq!(attributes.len(), 1);
    assert_eq!(
        attributes[0].tokens,
        vec![
            Token::Id("derive".to_string()),
            Token::LParen,
            Token::Id("Debug".to_string()),
            Token::RParen,
        ]
    );
    assert!(matches!(item.as_ref(), Stmt::Struct { name, .. } if name == "User"));
}

#[test]
fn rejects_attribute_on_non_item_statement() {
    let tokens = Tokenizer::tokenize("#[inline] let x = 1;").expect("tokenize source");
    let mut parser = StmtParser::new(&tokens);

    let err = parser.parse_program().expect_err("attribute on let must fail");

    assert!(
        err.to_string()
            .contains("Attributes can only be applied to item declarations")
    );
}
