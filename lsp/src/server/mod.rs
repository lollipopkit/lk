mod analysis;
mod cli;
mod completion;
mod config;
mod entry;
mod formatting;
mod handlers;
mod hover;
mod inlay_hints;
mod macro_definition;
mod semantic;
mod signature;
mod state;
mod text;
mod utils;
mod watch;
mod workspace_cache;

pub use entry::run;
pub use inlay_hints::compute_inlay_hints;

pub(crate) const MAX_SEMANTIC_TOKENS: usize = 8000;
