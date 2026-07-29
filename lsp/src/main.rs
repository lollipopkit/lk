//! The `lk-lsp` binary: a shell over the crate's own library (see `lib.rs`).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    lk_lsp::server::run().await
}
