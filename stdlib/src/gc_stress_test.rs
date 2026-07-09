//! Host-root regressions over stdlib modules (`iter`, `stream`, `encoding`):
//! results a native HOF accumulates in Rust across VM callbacks must be pinned
//! via the host-roots discipline (see `RuntimeModuleState::host_roots`). Runs
//! with the GC threshold pinned to 1 — the deterministic in-process twin of
//! the CI `LK_GC_STRESS=1` job that caught `json_process.lk` returning
//! `[[Carol,293]] * 3` from a `map` building fresh lists.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::RuntimeVal,
        vm::{ProgramResult, VmContext},
    };

    fn run_stressed(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        lk_core::vm::execute_program_with_ctx_and_gc_threshold(&program, &mut env, 1)
    }

    #[test]
    fn iter_map_results_survive_gc_during_callbacks() -> Result<()> {
        let result = run_stressed(
            r#"
            use iter;
            let out = iter.map([1, 2, 3], |u| [u, u * 2]);
            return out[0][0] + out[0][1] + out[1][1] + out[2][1];
            "#,
        )?;
        assert_eq!(result.returns, vec![RuntimeVal::Int(1 + 2 + 4 + 6)]);
        Ok(())
    }

    #[test]
    fn iter_filter_and_reduce_survive_gc_during_callbacks() -> Result<()> {
        let result = run_stressed(
            r#"
            use iter;
            let kept = iter.filter(["aaaaaaaaaaaa-one", "bbbbbbbbbbbb-two", "cccccccccccc-three"],
                                   |s| (s + "!").len() > 17);
            let folded = iter.reduce([1, 2, 3], [], |acc, x| acc.concat([x * 2]));
            return kept.len() == 1 && kept[0] == "cccccccccccc-three"
                && folded[0] + folded[1] + folded[2] == 12;
            "#,
        )?;
        assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
        Ok(())
    }

    #[test]
    fn stream_collect_survives_gc_during_pipeline_callbacks() -> Result<()> {
        let result = run_stressed(
            r#"
            use stream;
            let s = stream.from_list([1, 2, 3]);
            let mapped = stream.map(s, |u| [u, u * 2]);
            let out = stream.collect(mapped);
            return out[0][0] + out[0][1] + out[1][1] + out[2][1];
            "#,
        )?;
        assert_eq!(result.returns, vec![RuntimeVal::Int(1 + 2 + 4 + 6)]);
        Ok(())
    }

    #[test]
    fn json_process_shape_survives_gc() -> Result<()> {
        // The exact shape that failed in CI: json.parse feeding chained `map`
        // callbacks that build fresh lists.
        let result = run_stressed(
            r#"
            use { json } from encoding;
            let data = json.parse("{ \"users\": [{ \"name\": \"Alice\", \"scores\": [95, 87] }, { \"name\": \"Bob\", \"scores\": [78, 85] }] }");
            let summaries = data.users.map(|u| [u.name, u.scores.reduce(0, |a, b| a + b)]);
            return summaries[0][0] == "Alice" && summaries[0][1] == 182
                && summaries[1][0] == "Bob" && summaries[1][1] == 163;
            "#,
        )?;
        assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
        Ok(())
    }
}
