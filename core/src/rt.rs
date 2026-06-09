#[cfg(feature = "async-runtime")]
mod runtime;
#[cfg(not(feature = "async-runtime"))]
mod unsupported;

#[cfg(feature = "async-runtime")]
pub use runtime::*;
#[cfg(not(feature = "async-runtime"))]
pub use unsupported::*;
