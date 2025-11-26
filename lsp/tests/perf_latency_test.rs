use lkr_lsp::LkrAnalyzer;
use std::time::{Duration, Instant};

fn assert_under(label: &str, dur: Duration, max: Duration) {
    eprintln!("{} took: {:?} (limit: {:?})", label, dur, max);
    assert!(dur <= max, "{} exceeded budget: {:?} > {:?}", label, dur, max);
}

#[test]
fn test_analyze_small_expression_latency() {
    let mut analyzer = LkrAnalyzer::new();
    let src = "req.user.role == 'admin' && req.user.id > 0";

    let start = Instant::now();
    let _res = analyzer.analyze(src);
    let elapsed = start.elapsed();

    // Debug builds vary; keep threshold generous but meaningful
    assert_under("analyze(small expr)", elapsed, Duration::from_millis(10));
}

#[test]
fn test_analyze_complex_program_latency() {
    let mut analyzer = LkrAnalyzer::new();
    let program = r#"
        import math;
        import string;
        import datetime;

        let user_level = req.user.level;
        let user_name = req.user.name;
        let record_id = record.id;

        fn validate_access(user_role) {
            if (user_role == "admin") { return true; }
            if (user_role == "moderator" && user_level > 5) { return true; }
            return false;
        }

        fn calculate_score(base_score) {
            let adjusted_score = base_score * math.sqrt(user_level);
            let name_bonus = string.len(user_name) * 2;
            return adjusted_score + name_bonus;
        }

        let access_granted = validate_access(req.user.role);
        if (access_granted) {
            let score = calculate_score(100);
            let ts = datetime.now();
            return score;
        } else {
            return 0;
        }
    "#;

    let start = Instant::now();
    let _res = analyzer.analyze(program);
    let elapsed = start.elapsed();

    // Keep threshold generous for debug builds
    assert_under("analyze(complex program)", elapsed, Duration::from_millis(100));
}

#[test]
fn test_semantic_tokens_large_document_latency() {
    let analyzer = LkrAnalyzer::new();
    // Generate a moderately large document (~1000 lines)
    let mut doc = String::with_capacity(100_000);
    for i in 0..1000 {
        let _ = i; // keep loop simple for debug
        doc.push_str("// line comment\n");
        doc.push_str("let x = foo(1, 2); /* block */\n");
        doc.push_str("if (x >= 2 && x <= 10) { return x }\n");
    }

    let start = Instant::now();
    let tokens = analyzer.generate_semantic_tokens(&doc);
    let elapsed = start.elapsed();

    // Ensure we produced some tokens and kept time under a relaxed budget
    assert!(!tokens.is_empty(), "semantic tokens should not be empty");
    assert_under("semantic_tokens(large doc)", elapsed, Duration::from_millis(1500));
}
