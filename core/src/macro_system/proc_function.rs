use super::{
    MacroExpandOptions, MacroOriginFrame, MacroOriginKind, SourceToken, origin,
    proc_output::source_tokens_from_proc_output,
};
use crate::{
    macro_system::{
        PROC_MACRO_PROTOCOL_VERSION, ProcMacroDiagnostic, ProcMacroDiagnosticLevel, ProcMacroKind, ProcMacroRequest,
        ProcMacroSpan, ProcMacroToken, run_proc_macro_process,
    },
    token::{ParseError, Span},
};

pub(in crate::macro_system) fn expand_function_like_proc_macro(
    name: &str,
    input: &[SourceToken],
    options: &MacroExpandOptions,
    call_span: &Span,
    call_origins: &[MacroOriginFrame],
) -> Result<Vec<SourceToken>, ParseError> {
    let Some(config) = options.proc_macro_providers.function_like_provider(name) else {
        return Err(ParseError::with_span(
            format!("No procedural function-like provider registered for `{name}`"),
            call_span.clone(),
        ));
    };
    let request = ProcMacroRequest {
        protocol_version: PROC_MACRO_PROTOCOL_VERSION,
        kind: ProcMacroKind::FunctionLike,
        macro_name: name.to_string(),
        input_tokens: input.iter().map(proc_token_from_source).collect(),
        item_tokens: Vec::new(),
        package: None,
        module: None,
        features: options.proc_macro_features.clone(),
    };
    let response = run_proc_macro_process(&request, config).map_err(|err| {
        ParseError::with_span(
            format!("Procedural function-like macro `{name}` failed: {err}"),
            call_span.clone(),
        )
    })?;
    reject_error_diagnostics(name, &response.diagnostics, call_span)?;
    options.proc_macro_dependency_recorder.record(&response.dependencies);
    let mut output = source_tokens_from_proc_output(name, &response.output_tokens, call_span)?;
    for token in &mut output {
        origin::inherit_call_origin(token, call_origins);
        origin::push_origin(token, name, call_span, MacroOriginKind::ProcMacroOutput);
    }
    Ok(output)
}

fn proc_token_from_source(token: &SourceToken) -> ProcMacroToken {
    ProcMacroToken::from_token(&token.token, Some(&token.span))
}

fn reject_error_diagnostics(
    macro_name: &str,
    diagnostics: &[ProcMacroDiagnostic],
    fallback_span: &Span,
) -> Result<(), ParseError> {
    let Some(diagnostic) = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.level == ProcMacroDiagnosticLevel::Error)
    else {
        return Ok(());
    };
    let mut message = format!(
        "Procedural macro `{macro_name}` reported an error: {}",
        diagnostic.message
    );
    if !diagnostic.notes.is_empty() {
        message.push_str("; notes: ");
        message.push_str(&diagnostic.notes.join("; "));
    }
    let span = diagnostic
        .span
        .as_ref()
        .map(ProcMacroSpan::to_span)
        .unwrap_or_else(|| fallback_span.clone());
    Err(ParseError::with_span(message, span))
}

#[cfg(test)]
mod tests {
    use crate::{
        macro_system::{ProcMacroProcessConfig, ProcMacroProviders},
        syntax::{ParseOptions, expand_program_source, parse_program_source},
        token::Token,
        val::RuntimeVal,
    };
    use std::{path::PathBuf, time::Duration};

    #[test]
    fn function_like_provider_expands_invocation_tokens() {
        let Some(shell) = test_shell() else {
            return;
        };
        let mut providers = ProcMacroProviders::default();
        providers.register_function_like(
            "answer",
            shell_response_config(
                shell,
                r#"{"protocol_version":1,"output_tokens":[{"kind":"Int","lexeme":"55","span":null}],"diagnostics":[],"dependencies":[]}"#,
            ),
        );
        let program = parse_program_source(
            "return answer!();",
            ParseOptions {
                proc_macro_providers: providers,
                ..ParseOptions::default()
            },
        )
        .expect("function-like proc macro should expand");

        let result = program.execute().expect("execute function-like output");
        assert_eq!(result.returns, vec![RuntimeVal::Int(55)]);
    }

    #[test]
    fn function_like_provider_preserves_output_spans() {
        let Some(shell) = test_shell() else {
            return;
        };
        let mut providers = ProcMacroProviders::default();
        providers.register_function_like(
            "answer",
            shell_response_config(
                shell,
                r#"{"protocol_version":1,"output_tokens":[{"kind":"Int","lexeme":"55","span":{"start_line":44,"start_column":9,"start_offset":880,"end_line":44,"end_column":11,"end_offset":882}}],"diagnostics":[],"dependencies":[]}"#,
            ),
        );
        let expanded = expand_program_source(
            "return answer!();",
            ParseOptions {
                proc_macro_providers: providers,
                ..ParseOptions::default()
            },
        )
        .expect("function-like proc macro should expand");

        let span = expanded
            .source
            .tokens
            .iter()
            .zip(&expanded.source.spans)
            .find_map(|(token, span)| matches!(token, Token::Int(55)).then_some(span))
            .expect("generated integer should be present");
        assert_eq!(span.start.line, 44);
        assert_eq!(span.start.column, 9);
        assert_eq!(span.start.offset, 880);
        assert_eq!(span.end.column, 11);
    }

    #[test]
    fn function_like_provider_invalid_output_reports_generated_span() {
        let Some(shell) = test_shell() else {
            return;
        };
        let mut providers = ProcMacroProviders::default();
        providers.register_function_like(
            "bad",
            shell_response_config(
                shell,
                r#"{"protocol_version":1,"output_tokens":[{"kind":"Bad","lexeme":"@","span":{"start_line":77,"start_column":5,"start_offset":700,"end_line":77,"end_column":6,"end_offset":701}}],"diagnostics":[],"dependencies":[]}"#,
            ),
        );
        let err = parse_program_source(
            "return bad!();",
            ParseOptions {
                proc_macro_providers: providers,
                ..ParseOptions::default()
            },
        )
        .expect_err("invalid generated token should fail");

        let span = err.span.expect("error should keep generated token span");
        assert_eq!(span.start.line, 77);
        assert_eq!(span.start.column, 5);
        assert_eq!(span.start.offset, 700);
    }

    #[test]
    fn function_like_provider_records_dependencies() {
        let Some(shell) = test_shell() else {
            return;
        };
        let mut providers = ProcMacroProviders::default();
        providers.register_function_like(
            "answer",
            shell_response_config(
                shell,
                r#"{"protocol_version":1,"output_tokens":[{"kind":"Int","lexeme":"55","span":null}],"diagnostics":[],"dependencies":[{"path":"answer.schema","digest":"sha256:test"}]}"#,
            ),
        );
        let expanded = expand_program_source(
            "return answer!();",
            ParseOptions {
                proc_macro_providers: providers,
                ..ParseOptions::default()
            },
        )
        .expect("function-like proc macro should expand");

        assert_eq!(expanded.proc_macro_dependencies.len(), 1);
        assert_eq!(expanded.proc_macro_dependencies[0].path, "answer.schema");
        assert_eq!(
            expanded.proc_macro_dependencies[0].digest.as_deref(),
            Some("sha256:test")
        );
    }

    #[test]
    fn function_like_provider_error_diagnostic_fails_parse() {
        let Some(shell) = test_shell() else {
            return;
        };
        let mut providers = ProcMacroProviders::default();
        providers.register_function_like(
            "fail_proc",
            shell_response_config(
                shell,
                r#"{"protocol_version":1,"output_tokens":[],"diagnostics":[{"level":"Error","message":"function macro refused input","span":null,"notes":["check function provider logs"]}],"dependencies":[]}"#,
            ),
        );

        let err = parse_program_source(
            "return fail_proc!();",
            ParseOptions {
                proc_macro_providers: providers,
                ..ParseOptions::default()
            },
        )
        .expect_err("provider diagnostic should fail parsing");

        let message = err.to_string();
        assert!(message.contains("function macro refused input"));
        assert!(message.contains("check function provider logs"));
    }

    fn test_shell() -> Option<PathBuf> {
        let shell = PathBuf::from("/bin/sh");
        shell.exists().then_some(shell)
    }

    fn shell_response_config(shell: PathBuf, response: &str) -> ProcMacroProcessConfig {
        ProcMacroProcessConfig {
            program: shell,
            args: vec!["-c".to_string(), format!("cat >/dev/null; printf '%s' '{response}'")],
            timeout: Duration::from_secs(1),
            max_output_bytes: 4096,
        }
    }
}
