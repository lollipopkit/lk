use super::*;

#[test]
fn proc_macro_process_decodes_versioned_response() {
    let Some(shell) = test_shell() else {
        return;
    };
    let request = empty_request("demo");
    let config = ProcMacroProcessConfig {
        program: shell,
        args: vec![
            "-c".to_string(),
            concat!(
                "cat >/dev/null; printf '%s' ",
                "'{\"protocol_version\":1,\"output_tokens\":[],\"diagnostics\":[],\"dependencies\":[]}'"
            )
            .to_string(),
        ],
        timeout: Duration::from_secs(1),
        max_output_bytes: 4096,
    };

    let response = run_proc_macro_process(&request, &config).expect("run proc macro process");
    assert_eq!(response.protocol_version, PROC_MACRO_PROTOCOL_VERSION);
}

#[test]
fn proc_macro_process_enforces_timeout() {
    let Some(shell) = test_shell() else {
        return;
    };
    let request = empty_request("slow");
    let config = ProcMacroProcessConfig {
        program: shell,
        args: vec!["-c".to_string(), "sleep 1".to_string()],
        timeout: Duration::from_millis(10),
        max_output_bytes: 4096,
    };

    let err = run_proc_macro_process(&request, &config).expect_err("process should time out");
    assert!(matches!(err, ProcMacroProcessError::Timeout { .. }));
}

fn empty_request(name: &str) -> ProcMacroRequest {
    ProcMacroRequest {
        protocol_version: PROC_MACRO_PROTOCOL_VERSION,
        kind: ProcMacroKind::Attribute,
        macro_name: name.to_string(),
        input_tokens: Vec::new(),
        item_tokens: Vec::new(),
        package: None,
        module: None,
        features: Vec::new(),
    }
}
