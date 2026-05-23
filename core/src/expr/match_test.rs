#[cfg(test)]
mod tests {
    use crate::vm::execute_source32;

    fn expect_source(source: &str, expected_display: &str) {
        let result = execute_source32(source).expect("execute source");
        assert_eq!(result.display_first_return(), expected_display);
    }

    fn expect_expr(expr: &str, expected_display: &str) {
        expect_source(&format!("return {expr};"), expected_display);
    }

    #[test]
    fn literal_patterns() {
        expect_expr(
            r#"
            match 42 {
                42 => "matched",
                _ => "default",
            }
            "#,
            "matched",
        );

        expect_expr(
            r#"
            match "hello" {
                "world" => "world",
                "hello" => "hello",
            }
            "#,
            "hello",
        );
    }

    #[test]
    fn variable_pattern() {
        expect_expr(
            r#"
            match 100 {
                x => x,
            }
            "#,
            "100",
        );
    }

    #[test]
    fn wildcard_pattern() {
        expect_expr(
            r#"
            match true {
                false => "false",
                _ => "wildcard",
            }
            "#,
            "wildcard",
        );
    }

    #[test]
    fn list_pattern() {
        expect_expr(
            r#"
            match [1, 2, 3, 4] {
                [first, second, ..rest] => first,
            }
            "#,
            "1",
        );
    }

    #[test]
    fn map_pattern() {
        expect_source(
            r#"
            let data = {"name": "Alice", "age": 30};
            return match data {
                {"name": name, "age": age} => name,
            };
            "#,
            "Alice",
        );
    }

    #[test]
    fn or_pattern() {
        expect_expr(
            r#"
            match 2 {
                1 | 2 | 3 => "one_two_three",
                _ => "other",
            }
            "#,
            "one_two_three",
        );
    }

    #[test]
    fn guard_pattern() {
        expect_expr(
            r#"
            match 15 {
                x if x > 10 => "greater_than_10",
                _ => "not_greater",
            }
            "#,
            "greater_than_10",
        );
    }

    #[test]
    fn range_pattern() {
        expect_expr(
            r#"
            match 25 {
                0..=18 => "child",
                19..=64 => "adult",
                _ => "senior",
            }
            "#,
            "adult",
        );
    }

    #[test]
    fn no_match_falls_back_to_nil() {
        expect_expr(
            r#"
            match 99 {
                1 => "one",
                2 => "two",
            } == nil
            "#,
            "true",
        );
    }

    #[test]
    fn complex_nested_pattern() {
        expect_source(
            r#"
            let data = {
                "users": [
                    {"name": "Alice", "id": 1},
                    {"name": "Bob", "id": 2},
                ],
                "count": 2,
            };
            return match data {
                {"users": [{"name": first_name}, ..other_users]} => first_name,
            };
            "#,
            "Alice",
        );
    }

    #[test]
    fn float_range_pattern() {
        expect_expr(
            r#"
            match 85.5 {
                0.0..60.0 => "fail",
                60.0..80.0 => "pass",
                80.0..=100.0 => "excellent",
                _ => "invalid",
            }
            "#,
            "excellent",
        );

        for (value, expected) in [
            (59.9, "fail"),
            (60.0, "pass"),
            (79.9, "pass"),
            (80.0, "excellent"),
            (100.0, "excellent"),
            (100.1, "invalid"),
        ] {
            expect_expr(
                &format!(
                    r#"
                    match {value} {{
                        0.0..60.0 => "fail",
                        60.0..80.0 => "pass",
                        80.0..=100.0 => "excellent",
                        _ => "invalid",
                    }}
                    "#
                ),
                expected,
            );
        }
    }
}
