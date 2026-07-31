use lk_lsp::LkAnalyzer;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("lsp crate has workspace parent")
        .to_path_buf()
}

fn assert_under(label: &str, dur: Duration, max: Duration) {
    eprintln!("{} took: {:?} (limit: {:?})", label, dur, max);
    assert!(dur <= max, "{} exceeded budget: {:?} > {:?}", label, dur, max);
}

/// The fastest of five runs of `work`.
///
/// Two reasons, and the second is why the budgets below are what they are.
///
/// **Noise.** These tests run on whatever core the scheduler gives them,
/// alongside every other test in the binary. Contention, page faults and
/// frequency scaling can only make a sample *slower*, so the minimum is the
/// closest one to the work being measured. A single sample under a tight
/// budget is a coin flip — a lesson `compiling_many_functions_stays_linear`
/// taught by failing once inside `cargo test --workspace` and passing five
/// times on its own.
///
/// **Meaning.** With the noise gone the budget can be *tight*, and it has to
/// be: these six assertions were 67x to **2381x** above what they measure
/// (`semantic_tokens(example workspace main)` took 21µs against a 50ms limit).
/// A budget three orders of magnitude above the measurement cannot fail, so it
/// says nothing — a ten-fold LSP slowdown, which is the difference between an
/// editor that feels instant and one that does not, passed every one of them.
/// Each `max` below is ~10x the slowest observed minimum on 2026-08-01, so a
/// 10x regression is caught and ordinary machine-to-machine variation is not.
fn fastest_of_five(mut work: impl FnMut() -> Duration) -> Duration {
    (0..5).map(|_| work()).min().expect("five samples")
}

fn collect_lk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read directory") {
        let entry = entry.expect("read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_lk_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "lk") {
            out.push(path);
        }
    }
}

#[test]
fn test_analyze_small_expression_latency() {
    let src = "req.user.role == 'admin' && req.user.id > 0";

    // A fresh analyzer per sample: reusing one would measure whatever it
    // cached, and the cold path is the one a keystroke hits.
    let elapsed = fastest_of_five(|| {
        let mut analyzer = LkAnalyzer::new();
        let start = Instant::now();
        let _res = analyzer.analyze(src);
        start.elapsed()
    });

    // Observed 0.12ms (debug, 2026-08-01).
    assert_under("analyze(small expr)", elapsed, Duration::from_micros(1_500));
}

#[test]
fn test_analyze_complex_program_latency() {
    let mut analyzer = LkAnalyzer::new();
    let program = r#"
        use math;
        use string;
        use datetime;

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
    // Observed 0.94ms (debug, 2026-08-01).
    assert_under("analyze(complex program)", elapsed, Duration::from_millis(10));
}

#[test]
fn test_semantic_tokens_large_document_latency() {
    let analyzer = LkAnalyzer::new();
    // Generate a moderately large document (~1000 lines)
    let mut doc = String::with_capacity(100_000);
    for i in 0..1000 {
        let _ = i; // keep loop simple for debug
        doc.push_str("// line comment\n");
        doc.push_str("let x = foo(1, 2); /* block */\n");
        doc.push_str("if (x >= 2 && x <= 10) { return x }\n");
    }

    let tokens = analyzer.generate_semantic_tokens(&doc);
    assert!(!tokens.is_empty(), "semantic tokens should not be empty");
    let elapsed = fastest_of_five(|| {
        let start = Instant::now();
        analyzer.generate_semantic_tokens(&doc);
        start.elapsed()
    });

    // Observed 3.3ms (debug, 2026-08-01).
    assert_under("semantic_tokens(large doc)", elapsed, Duration::from_millis(35));
}

#[test]
fn test_analyze_example_workspace_main_latency() {
    let root = repo_root().join("examples/lk-example-workspace");
    let app_src = root.join("apps/demo/src");
    let main_path = app_src.join("main.lk");
    let src = fs::read_to_string(&main_path).expect("read example workspace main.lk");

    let mut analyzer = LkAnalyzer::new();
    analyzer.set_base_dir(app_src);
    let res = analyzer.analyze(&src);
    let elapsed = fastest_of_five(|| {
        let start = Instant::now();
        analyzer.analyze(&src);
        start.elapsed()
    });

    let messages: Vec<&str> = res.diagnostics.iter().map(|diag| diag.message.as_str()).collect();
    assert!(
        !messages.iter().any(|msg| msg.contains("Unknown module")),
        "example workspace imports should resolve; diagnostics: {messages:?}"
    );
    // Observed 1.0ms (debug, 2026-08-01).
    assert_under("analyze(example workspace main)", elapsed, Duration::from_millis(12));
}

#[test]
fn test_semantic_tokens_example_workspace_latency() {
    let main_path = repo_root().join("examples/lk-example-workspace/apps/demo/src/main.lk");
    let src = fs::read_to_string(&main_path).expect("read example workspace main.lk");
    let analyzer = LkAnalyzer::new();

    let tokens = analyzer.generate_semantic_tokens(&src);
    assert!(
        !tokens.is_empty(),
        "example workspace semantic tokens should not be empty"
    );
    let elapsed = fastest_of_five(|| {
        let start = Instant::now();
        analyzer.generate_semantic_tokens(&src);
        start.elapsed()
    });

    // Observed 16µs (debug, 2026-08-01). The floor is 1ms rather than 10x that:
    // below it the timer's own granularity is a visible part of the number.
    assert_under(
        "semantic_tokens(example workspace main)",
        elapsed,
        Duration::from_millis(1),
    );
}

#[test]
fn test_semantic_tokens_example_workspace_all_files_are_valid_and_fast() {
    let root = repo_root().join("examples/lk-example-workspace");
    let mut files = Vec::new();
    collect_lk_files(&root, &mut files);
    files.sort();
    assert!(!files.is_empty(), "example workspace should contain .lk files");

    let analyzer = LkAnalyzer::new();
    // Read once: the budget is for the analyzer, and leaving the file reads
    // inside it would have measured the page cache.
    let sources: Vec<String> = files
        .iter()
        .map(|file| fs::read_to_string(file).expect("read example workspace lk file"))
        .collect();
    for (file, src) in files.iter().zip(&sources) {
        let tokens = analyzer.generate_semantic_tokens(src);
        let summary = analyzer.validate_semantic_tokens(src, &tokens);
        assert!(
            summary.valid,
            "invalid semantic tokens for {}: {:?}",
            file.display(),
            summary.errors
        );
    }

    let elapsed = fastest_of_five(|| {
        let start = Instant::now();
        for src in &sources {
            analyzer.generate_semantic_tokens(src);
        }
        start.elapsed()
    });

    // Observed 56µs (debug, 2026-08-01).
    assert_under(
        "semantic_tokens(example workspace all files)",
        elapsed,
        Duration::from_millis(1),
    );
}
