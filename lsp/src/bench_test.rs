#[cfg(test)]
mod bench_tests {
    use crate::analyzer::LkrAnalyzer;
    use std::time::Instant;

    #[test]
    fn bench_analyzer_performance() {
        let mut analyzer = LkrAnalyzer::new();

        // Test expression analysis
        let expr = "req.user.role == 'admin' && req.user.level > 5";
        let start = Instant::now();

        // First run - no cache
        let _result1 = analyzer.analyze(expr);
        let first_run = start.elapsed();

        let start = Instant::now();
        // Second run - with cache
        let _result2 = analyzer.analyze(expr);
        let second_run = start.elapsed();

        println!("First run: {:?}, Second run: {:?}", first_run, second_run);
        println!(
            "Speedup: {:.2}x",
            first_run.as_nanos() as f64 / second_run.as_nanos() as f64
        );

        // Test statement analysis
        let program = r#"
            import math;
            let user_level = req.user.level;
            let user_role = req.user.role;
            
            fn calculate_access_score(base_score) {
                if user_role == "admin" {
                    return base_score * 2;
                } else if user_role == "user" {
                    return base_score;
                }
                return 0;
            }
            
            let access_score = calculate_access_score(100);
            if access_score > 50 {
                return true;
            } else {
                return false;
            }
        "#;

        let start = Instant::now();
        let _result3 = analyzer.analyze(program);
        let program_time = start.elapsed();

        println!("Program analysis: {:?}", program_time);

        // Test semantic tokens
        let start = Instant::now();
        let _tokens = analyzer.generate_semantic_tokens(program);
        let token_time = start.elapsed();

        println!("Semantic tokens: {:?}", token_time);
    }

    #[test]
    fn bench_completion_caching() {
        let mut analyzer = LkrAnalyzer::new();

        let start = Instant::now();
        let _completions1 = analyzer.get_var_completions("req");
        let first_completion = start.elapsed();

        let start = Instant::now();
        let _completions2 = analyzer.get_var_completions("req");
        let second_completion = start.elapsed();

        println!(
            "First completion: {:?}, Second completion: {:?}",
            first_completion, second_completion
        );
        println!(
            "Completion speedup: {:.2}x",
            first_completion.as_nanos() as f64 / second_completion.as_nanos() as f64
        );
    }
}
