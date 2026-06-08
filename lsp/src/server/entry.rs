use tower_lsp::{LspService, Server};
use tracing_subscriber::EnvFilter;

use super::{cli::try_cli_analyze, state::LkLanguageServer};

pub type Result<T> = anyhow::Result<T>;

fn init_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("lk_lsp=debug,tower_lsp=info,lk=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

pub async fn run() -> Result<()> {
    if let Some(output) = try_cli_analyze().unwrap_or_else(|e| {
        eprintln!("lk-lsp analyze error: {e}");
        std::process::exit(2);
    }) {
        println!("{}", output);
        return Ok(());
    }

    init_logging();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let completion_engine = lk_completion::CompletionEngine::new()?;
    let (service, socket) = LspService::new(|client| LkLanguageServer::new(client, completion_engine));
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}
