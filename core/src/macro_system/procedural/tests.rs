use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    macro_system::{
        PROC_MACRO_PROTOCOL_VERSION, ProcMacroKind, ProcMacroProcessConfig, ProcMacroProcessError, ProcMacroProviders,
        ProcMacroRequest, ProcMacroResponse, ProcMacroToken, run_proc_macro_process,
    },
    syntax::{ParseOptions, expand_program_source, parse_program_source},
    token::Tokenizer,
    val::RuntimeVal,
    vm::execute_source,
};
use std::{path::PathBuf, time::Duration};

mod attribute;
mod derive;
mod process;

fn test_shell() -> Option<PathBuf> {
    let shell = PathBuf::from("/bin/sh");
    shell.exists().then_some(shell)
}

fn shell_response_config(shell: PathBuf, response: &str) -> ProcMacroProcessConfig {
    // Pass response via environment variable to avoid shell injection from
    // interpolating untrusted content into a single-quoted shell literal.
    ProcMacroProcessConfig {
        program: shell,
        args: vec![
            "-c".to_string(),
            r##"cat >/dev/null; printf '%s' "$LK_PROC_MACRO_RESPONSE""##.to_string(),
        ],
        timeout: Duration::from_secs(1),
        max_output_bytes: 4096,
        env: Some(vec![("LK_PROC_MACRO_RESPONSE".to_string(), response.to_string())]),
    }
}

fn proc_macro_response_from_source(source: &str) -> String {
    let tokens = Tokenizer::tokenize(source).expect("proc macro test output should tokenize");
    let response = ProcMacroResponse {
        protocol_version: PROC_MACRO_PROTOCOL_VERSION,
        output_tokens: tokens
            .iter()
            .map(|token| ProcMacroToken::from_token(token, None))
            .collect(),
        diagnostics: Vec::new(),
        dependencies: Vec::new(),
    };
    serde_json::to_string(&response).expect("proc macro test response should serialize")
}
