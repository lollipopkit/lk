#[cfg(test)]
mod inlay_hint_tests {
    use crate::analyzer::LkrAnalyzer;
    use crate::compute_inlay_hints;
    use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};

    fn full_range(s: &str) -> Range {
        let lines = s.lines().count();
        let end_line = (lines.saturating_sub(1)) as u32;
        let end_col = s.lines().last().map(|l| l.len() as u32).unwrap_or(0);
        Range::new(Position::new(0, 0), Position::new(end_line, end_col))
    }

    #[test]
    fn test_param_inlay_hints_simple_call() {
        let src = r#"
            fn foo(a, b) {
                return a + b;
            }
            let z = foo(1, 2);
        "#;
        let hints = compute_inlay_hints(src, full_range(src));
        // Expect parameter hints for both arguments: a:, b:
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                tower_lsp::lsp_types::InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(labels.iter().any(|l| l.trim() == "a:"), "missing a: hint: {:?}", labels);
        assert!(labels.iter().any(|l| l.trim() == "b:"), "missing b: hint: {:?}", labels);
        assert!(hints.iter().all(|h| h.kind == Some(InlayHintKind::PARAMETER)));
    }

    #[test]
    fn test_param_inlay_hints_nested_calls() {
        let src = r#"
            fn bar(x) { return x; }
            fn foo(a, b) { return a + b; }
            let z = foo(bar(1), 2);
        "#;
        let hints = compute_inlay_hints(src, full_range(src));
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                tower_lsp::lsp_types::InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        // Should include hints for both calls (x: for bar, a:/b: for foo)
        assert!(labels.iter().any(|l| l.trim() == "x:"), "missing x: hint: {:?}", labels);
        assert!(labels.iter().any(|l| l.trim() == "a:"), "missing a: hint: {:?}", labels);
        assert!(labels.iter().any(|l| l.trim() == "b:"), "missing b: hint: {:?}", labels);
    }

    #[test]
    fn test_param_inlay_hints_ignore_comments_and_strings() {
        let src = r#"
            fn foo(a, b) { return a + b; }
            let z = foo(1, 2); // trailing comment with call-like text: foo(3, 4)
            let s = "not a call: foo(7, 8)";
        "#;
        let hints = compute_inlay_hints(src, full_range(src));
        // Expect exactly two parameter hints for the real call
        assert_eq!(hints.len(), 2, "expected two hints for foo(1, 2), got {:?}", hints);
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                tower_lsp::lsp_types::InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(labels.iter().any(|l| l.trim() == "a:"));
        assert!(labels.iter().any(|l| l.trim() == "b:"));
    }

    #[test]
    fn test_param_inlay_hints_commas_inside_strings() {
        let src = r#"
            fn foo(a, b) { return a + b; }
            let z = foo("a,b", 2);
        "#;
        let hints = compute_inlay_hints(src, full_range(src));
        // Should still only produce two hints (a: for the string arg, b: for the second)
        assert_eq!(
            hints.len(),
            2,
            "unexpected hint count with comma inside string: {:?}",
            hints
        );
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                tower_lsp::lsp_types::InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(labels.iter().any(|l| l.trim() == "a:"));
        assert!(labels.iter().any(|l| l.trim() == "b:"));
    }

    #[test]
    fn test_param_inlay_hints_multiline_calls() {
        let src = r#"
            fn foo(a, b, c) { return a + b + c; }
            let z = foo(
                1,
                "a,b",
                3,
            );
        "#;
        let hints = compute_inlay_hints(src, full_range(src));
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(
            labels.iter().any(|l| l.trim() == "a:"),
            "missing a: in multiline call: {:?}",
            labels
        );
        assert!(
            labels.iter().any(|l| l.trim() == "b:"),
            "missing b: in multiline call: {:?}",
            labels
        );
        assert!(
            labels.iter().any(|l| l.trim() == "c:"),
            "missing c: in multiline call: {:?}",
            labels
        );
    }

    #[test]
    fn test_param_inlay_hints_skip_function_definition_params() {
        let src = r#"
            fn foo(a, b) { return a + b; }
            // Only the call should produce hints
            let z = foo(1, 2);
        "#;
        let hints = compute_inlay_hints(src, full_range(src));
        // Should produce exactly two hints for the call, not for the fn params
        assert_eq!(hints.len(), 2, "expected only call argument hints, got {:?}", hints);
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(labels.iter().any(|l| l.trim() == "a:"));
        assert!(labels.iter().any(|l| l.trim() == "b:"));
    }

    #[test]
    fn test_type_inlay_hints_let_and_define() {
        let src = r#"
            let x = 1;
            y := 1.0;
        "#;
        let analyzer = LkrAnalyzer::new();
        let mut hints = analyzer.compute_type_inlay_hints(src, full_range(src));
        hints.extend(analyzer.compute_define_type_hints(src, full_range(src)));
        assert!(!hints.is_empty(), "expected type hints for let/define, got none");
        assert!(hints.iter().all(|h| h.kind == Some(InlayHintKind::TYPE)));
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                tower_lsp::lsp_types::InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(labels.iter().any(|l| l.contains(": Int")) || labels.iter().any(|l| l.contains(": Float")));
    }

    #[test]
    fn test_type_inlay_hints_skip_when_annotated() {
        let src = r#"
            let x: Int = 1;
            let y = 2;
        "#;
        let analyzer = LkrAnalyzer::new();
        let hints = analyzer.compute_type_inlay_hints(src, full_range(src));
        // Should only hint for y, not for the annotated x
        let labels: Vec<String> = hints
            .iter()
            .map(|h| match &h.label {
                tower_lsp::lsp_types::InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(
            labels.iter().filter(|l| l.starts_with(": ")).count(),
            1,
            "expected exactly one type hint, got {:?}",
            labels
        );
    }

    fn filter_hints(hints: Vec<InlayHint>, show_params: bool, show_types: bool) -> Vec<InlayHint> {
        hints
            .into_iter()
            .filter(|h| match h.kind.unwrap_or(InlayHintKind::TYPE) {
                InlayHintKind::PARAMETER => show_params,
                InlayHintKind::TYPE => show_types,
                _ => true,
            })
            .collect()
    }

    fn labels(hints: &[InlayHint]) -> Vec<String> {
        hints
            .iter()
            .map(|h| match &h.label {
                InlayHintLabel::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn test_server_side_like_filtering_for_inlay_hints() {
        let src = r#"
            fn foo(a, b) { return a + b; }
            let x = 1;
            y := 2.0;
            let z = foo(x, y);
        "#;

        // Collect both parameter and type hints as the server would before filtering
        let mut combined: Vec<InlayHint> = compute_inlay_hints(src, full_range(src));
        let analyzer = LkrAnalyzer::new();
        combined.extend(analyzer.compute_type_inlay_hints(src, full_range(src)));
        combined.extend(analyzer.compute_define_type_hints(src, full_range(src)));

        assert!(!combined.is_empty(), "expected mixed inlay hints present");

        // 1) All enabled -> both kinds present
        let all_on = filter_hints(combined.clone(), true, true);
        assert!(
            all_on.iter().any(|h| h.kind == Some(InlayHintKind::PARAMETER)),
            "expected parameter hints when enabled"
        );
        assert!(
            all_on.iter().any(|h| h.kind == Some(InlayHintKind::TYPE)),
            "expected type hints when enabled"
        );

        // 2) Parameters only
        let params_only = filter_hints(combined.clone(), true, false);
        assert!(
            params_only.iter().all(|h| h.kind == Some(InlayHintKind::PARAMETER)),
            "only parameter hints should remain"
        );
        let p_labels = labels(&params_only);
        assert!(p_labels.iter().any(|l| l.trim() == "a:"));
        assert!(p_labels.iter().any(|l| l.trim() == "b:"));

        // 3) Types only
        let types_only = filter_hints(combined.clone(), false, true);
        assert!(
            types_only.iter().all(|h| h.kind == Some(InlayHintKind::TYPE)),
            "only type hints should remain"
        );
        let t_labels = labels(&types_only);
        assert!(
            t_labels.iter().any(|l| l.starts_with(": ")),
            "expected at least one type label, got {:?}",
            t_labels
        );

        // 4) All disabled -> empty
        let none = filter_hints(combined, false, false);
        assert!(none.is_empty(), "expected no hints when all disabled");
    }

    #[test]
    fn test_function_return_type_hints_simple() {
        let src = r#"
            fn sum(a, b) {
                return a + b;
            }
            fn consts() { return 42; }
        "#;
        let analyzer = LkrAnalyzer::new();
        let hints = analyzer.compute_function_return_type_hints(src, full_range(src));
        assert!(!hints.is_empty(), "expected function return type hints");
        // Should include TYPE kind hints with labels like " -> Int" (at least for the const function)
        assert!(hints.iter().all(|h| h.kind == Some(InlayHintKind::TYPE)));
        let labs = labels(&hints);
        assert!(
            labs.iter().any(|l| l.contains("-> Int"))
                || labs.iter().any(|l| l.contains("-> Float"))
                || labs.iter().any(|l| l.contains("-> Any")),
            "expected at least one arrow type hint, got {:?}",
            labs
        );
    }

    #[test]
    fn test_function_return_type_hints_multiple_returns_union() {
        let src = r#"
            fn foo(flag) {
                if (flag) {
                    return 1;
                } else {
                    return "s";
                }
            }
        "#;
        let analyzer = LkrAnalyzer::new();
        let hints = analyzer.compute_function_return_type_hints(src, full_range(src));
        assert!(!hints.is_empty(), "expected function return type hint for union");
        let labs = labels(&hints);
        assert!(
            labs.iter().any(|l| l.contains("-> Int | String")) || labs.iter().any(|l| l.contains("-> String | Int")),
            "expected union return type hint (Int | String), got {:?}",
            labs
        );
    }
}
