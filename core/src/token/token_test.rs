#[cfg(test)]
mod tests {
    use crate::token::{Token, Tokenizer};

    #[test]
    fn basic() {
        let t1 = Tokenizer::tokenize(r#"1.3+*/ % ==  "str1" 'str2' true false nil "#);
        let e1 = vec![
            Token::Float(1.3),
            Token::Add,
            Token::Mul,
            Token::Div,
            Token::Mod,
            Token::Eq,
            Token::Str("str1".to_string()),
            Token::Str("str2".to_string()),
            Token::Bool(true),
            Token::Bool(false),
            Token::Nil,
        ];
        assert_eq!(t1.unwrap(), e1);
    }

    #[test]
    fn test_compound_assignment_tokens() {
        let t1 = Tokenizer::tokenize("+= -= *= /= %=");
        let e1 = vec![
            Token::AddAssign,
            Token::SubAssign,
            Token::MulAssign,
            Token::DivAssign,
            Token::ModAssign,
        ];
        assert_eq!(t1.unwrap(), e1);
    }

    #[test]
    fn test_range_token() {
        let tokens = Tokenizer::tokenize("0..5").unwrap();
        let expected = vec![Token::Int(0), Token::Range, Token::Int(5)];
        assert_eq!(tokens, expected);
    }

    #[test]
    fn test_for_tokens() {
        let tokens = Tokenizer::tokenize("for i in [1, 2, 3] {}").unwrap();
        let expected = vec![
            Token::For,
            Token::Id("i".to_string()),
            Token::In,
            Token::LBracket,
            Token::Int(1),
            Token::Comma,
            Token::Int(2),
            Token::Comma,
            Token::Int(3),
            Token::RBracket,
            Token::LBrace,
            Token::RBrace,
        ];
        assert_eq!(tokens, expected);
    }

    #[test]
    fn punctuations() {
        let t2 = Tokenizer::tokenize(">=<= && || == != ! > <");
        let e2 = vec![
            Token::Ge,
            Token::Le,
            Token::And,
            Token::Or,
            Token::Eq,
            Token::Ne,
            Token::Not,
            Token::Gt,
            Token::Lt,
        ];
        assert_eq!(t2.unwrap(), e2);
    }

    #[test]
    fn list_map_punctuations() {
        let t = Tokenizer::tokenize("[]{}:,");
        let e = vec![
            Token::LBracket,
            Token::RBracket,
            Token::LBrace,
            Token::RBrace,
            Token::Colon,
            Token::Comma,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn ids() {
        let t3 = Tokenizer::tokenize("id1 id_2 id-3");
        let e3 = vec![
            Token::Id("id1".to_string()),
            Token::Id("id_2".to_string()),
            Token::Id("id-3".to_string()),
        ];
        assert_eq!(t3.unwrap(), e3);
    }

    #[test]
    fn unclosed_str() {
        let t = Tokenizer::tokenize(r#""str"#);
        assert!(t.is_err());
    }

    #[test]
    fn string_escape_sequences() {
        // Test basic escape sequences
        let t = Tokenizer::tokenize(r#""Hello\nWorld""#).unwrap();
        assert_eq!(t, vec![Token::Str("Hello\nWorld".to_string())]);

        let t = Tokenizer::tokenize(r#""Tab\tTest""#).unwrap();
        assert_eq!(t, vec![Token::Str("Tab\tTest".to_string())]);

        let t = Tokenizer::tokenize(r#""Quote\"Test""#).unwrap();
        assert_eq!(t, vec![Token::Str("Quote\"Test".to_string())]);

        let t = Tokenizer::tokenize(r#"'Apostrophe\'Test'"#).unwrap();
        assert_eq!(t, vec![Token::Str("Apostrophe'Test".to_string())]);

        let t = Tokenizer::tokenize(r#""Backslash\\Test""#).unwrap();
        assert_eq!(t, vec![Token::Str("Backslash\\Test".to_string())]);

        let t = Tokenizer::tokenize(r#""Carriage\rReturn""#).unwrap();
        assert_eq!(t, vec![Token::Str("Carriage\rReturn".to_string())]);

        let t = Tokenizer::tokenize(r#""Null\0Character""#).unwrap();
        assert_eq!(t, vec![Token::Str("Null\0Character".to_string())]);

        // Test unknown escape sequence (should keep both backslash and character)
        let t = Tokenizer::tokenize(r#""Unknown\xEscape""#).unwrap();
        assert_eq!(t, vec![Token::Str("Unknown\\xEscape".to_string())]);
    }

    #[test]
    fn string_escape_incomplete() {
        // Test incomplete escape sequence at end of string
        let t = Tokenizer::tokenize(r#""test\"#);
        assert!(t.is_err());
        if let Err(e) = t {
            println!("Error message: {}", e);
            assert!(
                e.to_string().contains("String not closed") || e.to_string().contains("Incomplete escape sequence")
            );
        }
    }

    #[test]
    fn raw_string_hash_levels() {
        let t = Tokenizer::tokenize(r###"r##"a "# quote"##"###).unwrap();
        assert_eq!(t, vec![Token::Str("a \"# quote".to_string())]);
    }

    #[test]
    fn raw_string_multiline_and_verbatim() {
        let t = Tokenizer::tokenize(
            r#"r"line1
line2""#,
        )
        .unwrap();
        assert_eq!(t, vec![Token::Str("line1\nline2".to_string())]);

        // No escapes or interpolation in raw strings
        let t = Tokenizer::tokenize(r#"r"${x}\n""#).unwrap();
        assert_eq!(t, vec![Token::Str("${x}\\n".to_string())]);
    }

    #[test]
    fn raw_string_unclosed_errors() {
        let t = Tokenizer::tokenize(r#"r"abc"#);
        assert!(t.is_err());
    }

    #[test]
    fn num() {
        let t = Tokenizer::tokenize("1.2.3");
        assert!(t.is_err());

        // Consider `.` as Dot if starts with ``, otherwise Float
        // It's invalid in AST(The first path of At Expr must be Str), but valid in Tokenizer
        let t = Tokenizer::tokenize("1.2");
        assert!(t.is_ok());

        let t = Tokenizer::tokenize("-1.0 +1.2");
        let e = vec![Token::Float(-1.0), Token::Float(1.2)];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn keywords() {
        let t6 = Tokenizer::tokenize(">true false nil in");
        let e6 = vec![Token::Gt, Token::Bool(true), Token::Bool(false), Token::Nil, Token::In];
        assert_eq!(t6.unwrap(), e6);
    }

    #[test]
    fn test_return_keyword() {
        let tokens = Tokenizer::tokenize("return").expect("Invalid tokens");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], Token::Return);

        let tokens = Tokenizer::tokenize("return 42;").expect("Invalid tokens");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], Token::Return);
        assert_eq!(tokens[1], Token::Int(42));
        assert_eq!(tokens[2], Token::Semicolon);
    }

    #[test]
    fn token_eq() {
        assert_eq!(Token::Str("a".to_string()), Token::Str("a".to_string()));
        assert_eq!(Token::Int(1), Token::Int(1));
        assert_eq!(Token::Float(1.0), Token::Float(1.0));
        assert_eq!(Token::Bool(true), Token::Bool(true));
        assert_eq!(Token::Nil, Token::Nil);
        assert_ne!(Token::Str("a".to_string()), Token::Str("b".to_string()));
        assert_ne!(Token::Int(1), Token::Int(2));
        assert_ne!(Token::Float(1.0), Token::Float(2.0));
        assert_ne!(Token::Bool(true), Token::Bool(false));
        assert_ne!(Token::Nil, Token::Bool(false));
    }

    #[test]
    fn at_query() {
        let t = Tokenizer::tokenize("req.user.age >= 18");
        let e = vec![
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::Dot,
            Token::Id("age".to_string()),
            Token::Ge,
            Token::Int(18),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn real_query() {
        let query = r#"
        (
            req.user.id == record.user.id && record.time > 1700000
        ) 
        ||
        req.user.role == 'admin'
        "#;
        let t = Tokenizer::tokenize(query);
        let e = vec![
            Token::LParen,
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::Dot,
            Token::Id("id".to_string()),
            Token::Eq,
            Token::Id("record".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::Dot,
            Token::Id("id".to_string()),
            Token::And,
            Token::Id("record".to_string()),
            Token::Dot,
            Token::Id("time".to_string()),
            Token::Gt,
            Token::Int(1700000),
            Token::RParen,
            Token::Or,
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::Dot,
            Token::Id("role".to_string()),
            Token::Eq,
            Token::Str("admin".to_string()),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn list_access() {
        let t = Tokenizer::tokenize("list.0");
        let e = vec![Token::Id("list".to_string()), Token::Dot, Token::Int(0)];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize("list.1.2");
        let e = vec![
            Token::Id("list".to_string()),
            Token::Dot,
            Token::Int(1),
            Token::Dot,
            Token::Int(2),
        ];
        assert_eq!(t.unwrap(), e);
    }

    // Issue #1
    #[test]
    fn t1() {
        let t = Tokenizer::tokenize("(settings.active)");
        let e = vec![
            Token::LParen,
            Token::Id("settings".to_string()),
            Token::Dot,
            Token::Id("active".to_string()),
            Token::RParen,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn empty_strings() {
        let t = Tokenizer::tokenize(r#""""''"#);
        assert!(t.is_err());
    }

    #[test]
    fn complex_numbers() {
        let t = Tokenizer::tokenize("-123 +456 -1.23 +4.56");
        let e = vec![
            Token::Int(-123),
            Token::Int(456),
            Token::Float(-1.23),
            Token::Float(4.56),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn invalid_numbers() {
        // Multiple dots in number
        assert!(Tokenizer::tokenize("1.2.3").is_err());
        // Invalid float
        assert!(Tokenizer::tokenize("1.a").is_err());
        // Just a dot
        let t = Tokenizer::tokenize(".");
        assert_eq!(t.unwrap(), vec![Token::Dot]);
    }

    #[test]
    fn whitespace_handling() {
        let t = Tokenizer::tokenize("  req.user  .  id  ==  'test'  ");
        let e = vec![
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::Dot,
            Token::Id("id".to_string()),
            Token::Eq,
            Token::Str("test".to_string()),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn nested_expressions() {
        let t = Tokenizer::tokenize("((req.id == 123) && (req.role == 'admin'))");
        let e = vec![
            Token::LParen,
            Token::LParen,
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("id".to_string()),
            Token::Eq,
            Token::Int(123),
            Token::RParen,
            Token::And,
            Token::LParen,
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("role".to_string()),
            Token::Eq,
            Token::Str("admin".to_string()),
            Token::RParen,
            Token::RParen,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn mixed_operators() {
        let t = Tokenizer::tokenize("1 + 2 * 3 / 4 % 5");
        let e = vec![
            Token::Int(1),
            Token::Add,
            Token::Int(2),
            Token::Mul,
            Token::Int(3),
            Token::Div,
            Token::Int(4),
            Token::Mod,
            Token::Int(5),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn complex_path_access() {
        let t = Tokenizer::tokenize("users.0.name items.1.tags.2");
        let e = vec![
            Token::Id("users".to_string()),
            Token::Dot,
            Token::Int(0),
            Token::Dot,
            Token::Id("name".to_string()),
            Token::Id("items".to_string()),
            Token::Dot,
            Token::Int(1),
            Token::Dot,
            Token::Id("tags".to_string()),
            Token::Dot,
            Token::Int(2),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn logic_operations() {
        let t = Tokenizer::tokenize("!(a in b) && (c || !d)");
        let e = vec![
            Token::Not,
            Token::LParen,
            Token::Id("a".to_string()),
            Token::In,
            Token::Id("b".to_string()),
            Token::RParen,
            Token::And,
            Token::LParen,
            Token::Id("c".to_string()),
            Token::Or,
            Token::Not,
            Token::Id("d".to_string()),
            Token::RParen,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn nested_at() {
        let t = Tokenizer::tokenize("a.(b.(c))");
        let e = vec![
            Token::Id("a".to_string()),
            Token::Dot,
            Token::LParen,
            Token::Id("b".to_string()),
            Token::Dot,
            Token::LParen,
            Token::Id("c".to_string()),
            Token::RParen,
            Token::RParen,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize("a.(b.(c.(d)))");
        let e = vec![
            Token::Id("a".to_string()),
            Token::Dot,
            Token::LParen,
            Token::Id("b".to_string()),
            Token::Dot,
            Token::LParen,
            Token::Id("c".to_string()),
            Token::Dot,
            Token::LParen,
            Token::Id("d".to_string()),
            Token::RParen,
            Token::RParen,
            Token::RParen,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize("a.(b - 1))");
        let e = vec![
            Token::Id("a".to_string()),
            Token::Dot,
            Token::LParen,
            Token::Id("b".to_string()),
            Token::Sub,
            Token::Int(1),
            Token::RParen,
            Token::RParen,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn list_literals() {
        let t = Tokenizer::tokenize("[1, 2, 3]");
        let e = vec![
            Token::LBracket,
            Token::Int(1),
            Token::Comma,
            Token::Int(2),
            Token::Comma,
            Token::Int(3),
            Token::RBracket,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize(r#"["hello", "world"]"#);
        let e = vec![
            Token::LBracket,
            Token::Str("hello".to_string()),
            Token::Comma,
            Token::Str("world".to_string()),
            Token::RBracket,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize("[]");
        let e = vec![Token::LBracket, Token::RBracket];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn map_literals() {
        let t = Tokenizer::tokenize(r#"{"key": "value"}"#);
        let e = vec![
            Token::LBrace,
            Token::Str("key".to_string()),
            Token::Colon,
            Token::Str("value".to_string()),
            Token::RBrace,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize(r#"{"a": 1, "b": 2}"#);
        let e = vec![
            Token::LBrace,
            Token::Str("a".to_string()),
            Token::Colon,
            Token::Int(1),
            Token::Comma,
            Token::Str("b".to_string()),
            Token::Colon,
            Token::Int(2),
            Token::RBrace,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize("{}");
        let e = vec![Token::LBrace, Token::RBrace];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn complex_list_map() {
        let t = Tokenizer::tokenize(r#"[{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]"#);
        let e = vec![
            Token::LBracket,
            Token::LBrace,
            Token::Str("name".to_string()),
            Token::Colon,
            Token::Str("Alice".to_string()),
            Token::Comma,
            Token::Str("age".to_string()),
            Token::Colon,
            Token::Int(30),
            Token::RBrace,
            Token::Comma,
            Token::LBrace,
            Token::Str("name".to_string()),
            Token::Colon,
            Token::Str("Bob".to_string()),
            Token::Comma,
            Token::Str("age".to_string()),
            Token::Colon,
            Token::Int(25),
            Token::RBrace,
            Token::RBracket,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize(r#"{"users": [1, 2, 3], "active": true}"#);
        let e = vec![
            Token::LBrace,
            Token::Str("users".to_string()),
            Token::Colon,
            Token::LBracket,
            Token::Int(1),
            Token::Comma,
            Token::Int(2),
            Token::Comma,
            Token::Int(3),
            Token::RBracket,
            Token::Comma,
            Token::Str("active".to_string()),
            Token::Colon,
            Token::Bool(true),
            Token::RBrace,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn trailing_commas() {
        let t = Tokenizer::tokenize("[1, 2, 3,]");
        let e = vec![
            Token::LBracket,
            Token::Int(1),
            Token::Comma,
            Token::Int(2),
            Token::Comma,
            Token::Int(3),
            Token::Comma,
            Token::RBracket,
        ];
        assert_eq!(t.unwrap(), e);

        let t = Tokenizer::tokenize(r#"{"a": 1, "b": 2,}"#);
        let e = vec![
            Token::LBrace,
            Token::Str("a".to_string()),
            Token::Colon,
            Token::Int(1),
            Token::Comma,
            Token::Str("b".to_string()),
            Token::Colon,
            Token::Int(2),
            Token::Comma,
            Token::RBrace,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_line_comment() {
        let t = Tokenizer::tokenize("123 // 这是一个注释\n456");
        let e = vec![Token::Int(123), Token::Int(456)];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_block_comment() {
        let t = Tokenizer::tokenize("123 /* 这是一个块注释 */ 456");
        let e = vec![Token::Int(123), Token::Int(456)];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_multiline_block_comment() {
        let t = Tokenizer::tokenize("123 /* 这是一个\n多行块注释 */ 456");
        let e = vec![Token::Int(123), Token::Int(456)];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_nested_block_comments() {
        // In most languages, nested block comments don't work as expected
        // The first */ closes the comment, leaving the rest as tokens
        let t = Tokenizer::tokenize("123 /* 外层注释 /* 内层注释 */ 外层继续 */ 456");
        let e = vec![
            Token::Int(123),
            Token::Id("外层继续".to_string()),
            Token::Mul,
            Token::Div,
            Token::Int(456),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_unclosed_block_comment() {
        let t = Tokenizer::tokenize("123 /* 未闭合的注释 456");
        assert!(t.is_err());
    }

    #[test]
    fn test_comment_before_operator() {
        let t = Tokenizer::tokenize("123 /* 注释 */ + 456");
        let e = vec![Token::Int(123), Token::Add, Token::Int(456)];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_comment_after_operator() {
        let t = Tokenizer::tokenize("123 + /* 注释 */ 456");
        let e = vec![Token::Int(123), Token::Add, Token::Int(456)];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_multiple_comments() {
        let t = Tokenizer::tokenize("123 // 注释1\n/* 注释2 */ 456 // 注释3\n789");
        let e = vec![Token::Int(123), Token::Int(456), Token::Int(789)];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_name_with_keywords() {
        let t = Tokenizer::tokenize("record.in_value + record.return_value");
        let e = vec![
            Token::Id("record".to_string()),
            Token::Dot,
            Token::Id("in_value".to_string()),
            Token::Add,
            Token::Id("record".to_string()),
            Token::Dot,
            Token::Id("return_value".to_string()),
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_name_with_keywords_stmt() {
        let t = Tokenizer::tokenize("let a = record.in_return; return a + '1';");
        let e = vec![
            Token::Let,
            Token::Id("a".to_string()),
            Token::Assign,
            Token::Id("record".to_string()),
            Token::Dot,
            Token::Id("in_return".to_string()),
            Token::Semicolon,
            Token::Return,
            Token::Id("a".to_string()),
            Token::Add,
            Token::Str("1".to_string()),
            Token::Semicolon,
        ];
        assert_eq!(t.unwrap(), e);
    }

    #[test]
    fn test_const_keyword_token() {
        let tokens = Tokenizer::tokenize("const x = 1;").expect("Failed to tokenize const statement");
        let expected = vec![
            Token::Const,
            Token::Id("x".to_string()),
            Token::Assign,
            Token::Int(1),
            Token::Semicolon,
        ];
        assert_eq!(tokens, expected);
    }

    #[test]
    fn test_identifier_with_in_prefix() {
        let tokens = Tokenizer::tokenize("in_business_hours").expect("Invalid tokens");

        // Should be a single identifier token, not 'In' + '_business_hours'
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], Token::Id("in_business_hours".to_string()));
    }

    #[test]
    fn test_standalone_in_keyword() {
        let tokens = Tokenizer::tokenize("x in list").expect("Invalid tokens");

        // Should be 'x', 'In', 'list'
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], Token::Id("x".to_string()));
        assert_eq!(tokens[1], Token::In);
        assert_eq!(tokens[2], Token::Id("list".to_string()));
    }

    #[test]
    fn test_multiple_identifiers_with_in_prefix() {
        let tokens = Tokenizer::tokenize("let in_value = in_other").expect("Invalid tokens");

        // Should be 'let', 'in_value', '=', 'in_other'
        assert_eq!(tokens.len(), 4);
        assert_eq!(tokens[0], Token::Let);
        assert_eq!(tokens[1], Token::Id("in_value".to_string()));
        assert_eq!(tokens[2], Token::Assign);
        assert_eq!(tokens[3], Token::Id("in_other".to_string()));
    }

    #[test]
    fn test_optional_chaining_operator() {
        let tokens = Tokenizer::tokenize("req.user?.profile?.name").expect("Invalid tokens");

        let expected = vec![
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::OptionalDot,
            Token::Id("profile".to_string()),
            Token::OptionalDot,
            Token::Id("name".to_string()),
        ];
        assert_eq!(tokens, expected);
    }

    #[test]
    fn test_optional_chaining_mixed_with_regular() {
        let tokens = Tokenizer::tokenize("req.user?.profile.name").expect("Invalid tokens");

        let expected = vec![
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::OptionalDot,
            Token::Id("profile".to_string()),
            Token::Dot,
            Token::Id("name".to_string()),
        ];
        assert_eq!(tokens, expected);
    }

    #[test]
    fn test_question_mark_tokenization() {
        // Question mark should tokenize successfully as Token::Question
        let result = Tokenizer::tokenize("req?user");
        assert!(result.is_ok());
        let tokens = result.unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Id("req".to_string()),
                Token::Question,
                Token::Id("user".to_string()),
            ]
        );
    }

    #[test]
    fn test_import_math_as_alias_tokenization() {
        let src = "import math as m;";
        let tokens = Tokenizer::tokenize(src).expect("tokenize failed");
        let expected = vec![
            Token::Import,
            Token::Id("math".to_string()),
            Token::As,
            Token::Id("m".to_string()),
            Token::Semicolon,
        ];
        assert_eq!(tokens, expected);
    }
}
