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
    let mut analyzer = LkAnalyzer::new();
    let src = "req.user.role == 'admin' && req.user.id > 0";

    let start = Instant::now();
    let _res = analyzer.analyze(src);
    let elapsed = start.elapsed();

    // Debug builds vary; keep threshold generous but meaningful
    assert_under("analyze(small expr)", elapsed, Duration::from_millis(10));
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
    assert_under("analyze(complex program)", elapsed, Duration::from_millis(100));
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

    let start = Instant::now();
    let tokens = analyzer.generate_semantic_tokens(&doc);
    let elapsed = start.elapsed();

    // Ensure we produced some tokens and kept time under a relaxed budget
    assert!(!tokens.is_empty(), "semantic tokens should not be empty");
    assert_under("semantic_tokens(large doc)", elapsed, Duration::from_millis(1500));
}

#[test]
fn test_analyze_example_workspace_main_latency() {
    let root = repo_root().join("examples/lk-example-workspace");
    let app_src = root.join("apps/demo/src");
    let main_path = app_src.join("main.lk");
    let src = fs::read_to_string(&main_path).expect("read example workspace main.lk");

    let mut analyzer = LkAnalyzer::new();
    analyzer.set_base_dir(app_src);
    let start = Instant::now();
    let res = analyzer.analyze(&src);
    let elapsed = start.elapsed();

    let messages: Vec<&str> = res.diagnostics.iter().map(|diag| diag.message.as_str()).collect();
    assert!(
        !messages.iter().any(|msg| msg.contains("Unknown module")),
        "example workspace imports should resolve; diagnostics: {messages:?}"
    );
    assert_under("analyze(example workspace main)", elapsed, Duration::from_millis(100));
}

#[test]
fn test_semantic_tokens_example_workspace_latency() {
    let main_path = repo_root().join("examples/lk-example-workspace/apps/demo/src/main.lk");
    let src = fs::read_to_string(&main_path).expect("read example workspace main.lk");
    let analyzer = LkAnalyzer::new();

    let start = Instant::now();
    let tokens = analyzer.generate_semantic_tokens(&src);
    let elapsed = start.elapsed();

    assert!(
        !tokens.is_empty(),
        "example workspace semantic tokens should not be empty"
    );
    assert_under(
        "semantic_tokens(example workspace main)",
        elapsed,
        Duration::from_millis(50),
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
    let start = Instant::now();
    for file in &files {
        let src = fs::read_to_string(file).expect("read example workspace lk file");
        let tokens = analyzer.generate_semantic_tokens(&src);
        let summary = analyzer.validate_semantic_tokens(&src, &tokens);
        assert!(
            summary.valid,
            "invalid semantic tokens for {}: {:?}",
            file.display(),
            summary.errors
        );
    }
    let elapsed = start.elapsed();

    assert_under(
        "semantic_tokens(example workspace all files)",
        elapsed,
        Duration::from_millis(100),
    );
}
