mod analyzer;
mod server;

#[cfg(test)]
mod bench_test;
#[cfg(test)]
mod inlay_hint_test;

pub use server::compute_inlay_hints;

#[tokio::main]
async fn main() {
    server::run().await;
}
