use tower_lsp::{LspService, Server};

use super::{cli::try_cli_analyze, state::LkrLanguageServer};

pub async fn run() {
    if let Some(output) = try_cli_analyze().unwrap_or_else(|e| {
        eprintln!("lkr-lsp analyze error: {e}");
        std::process::exit(2);
    }) {
        println!("{}", output);
        return;
    }

    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(LkrLanguageServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
