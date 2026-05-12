use tower_lsp::{LspService, Server};
use tracing_subscriber::EnvFilter;

use std::{env, fs::OpenOptions, path::PathBuf};

use super::{cli::try_cli_analyze, state::LkrLanguageServer};

fn init_logging() {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let log_file = cwd.join("lkr-lsp-debug.log");

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lkr_lsp=debug,tower_lsp=info,lkr=info"));

    let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_file) else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
        return;
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .with_target(false)
        .init();
}

pub async fn run() {
    if let Some(output) = try_cli_analyze().unwrap_or_else(|e| {
        eprintln!("lkr-lsp analyze error: {e}");
        std::process::exit(2);
    }) {
        println!("{}", output);
        return;
    }

    init_logging();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(LkrLanguageServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
