mod analysis;
mod cli;
mod config;
mod entry;
mod formatting;
mod handlers;
mod inlay_hints;
mod semantic;
mod signature;
mod state;
mod text;
mod utils;

pub use entry::run;
pub use inlay_hints::compute_inlay_hints;

pub(crate) const MAX_SEMANTIC_TOKENS: usize = 8000;
