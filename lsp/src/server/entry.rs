use tower_lsp::{LspService, Server};
use tracing_subscriber::EnvFilter;

use super::{cli::try_cli_analyze, state::LkLanguageServer};

fn init_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lk_lsp=debug,tower_lsp=info,lk=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

pub async fn run() {
    if let Some(output) = try_cli_analyze().unwrap_or_else(|e| {
        eprintln!("lk-lsp analyze error: {e}");
        std::process::exit(2);
    }) {
        println!("{}", output);
        return;
    }

    init_logging();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(LkLanguageServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
