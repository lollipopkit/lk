//! no_std compatibility shims.
//!
//! Under the default `std` feature these re-export the corresponding std types
//! so the normal build is byte-for-byte unchanged. Under
//! `--no-default-features` (no_std + `alloc`) they map onto
//! `alloc` / `hashbrown` / `spin` instead, letting the VM core compile without
//! `std`. See plan M0.7/8 (`lk-vm-core` no_std groundwork).

/// Hash collections with a stable hasher across std/no_std.
///
/// std's `HashMap` needs `RandomState` (a std-only RNG seed); `hashbrown`'s
/// default hasher is no_std-friendly. Routing through this module keeps the std
/// build on std's `HashMap` (identical behaviour) while the no_std build gets
/// `hashbrown`.
pub mod collections {
    #[cfg(not(feature = "std"))]
    pub use hashbrown::{HashMap, HashSet};
    #[cfg(feature = "std")]
    pub use std::collections::{HashMap, HashSet};
}

/// A `Mutex` that keeps std's `.lock() -> Result<Guard, _>` shape so existing
/// call sites (`.lock().unwrap()`, `.map_err(..)`, …) compile unchanged. Under
/// no_std it wraps `spin::Mutex` and reports an infallible lock.
pub mod sync {
    #[cfg(feature = "std")]
    pub type MutexGuard<'a, T> = std::sync::MutexGuard<'a, T>;
    #[cfg(not(feature = "std"))]
    pub type MutexGuard<'a, T> = spin::MutexGuard<'a, T>;

    #[cfg(feature = "std")]
    #[derive(Debug, Default)]
    pub struct Mutex<T: ?Sized>(std::sync::Mutex<T>);
    #[cfg(not(feature = "std"))]
    #[derive(Debug, Default)]
    pub struct Mutex<T: ?Sized>(spin::Mutex<T>);

    impl<T> Mutex<T> {
        #[cfg(feature = "std")]
        pub fn new(value: T) -> Self {
            Mutex(std::sync::Mutex::new(value))
        }
        #[cfg(not(feature = "std"))]
        pub fn new(value: T) -> Self {
            Mutex(spin::Mutex::new(value))
        }
    }

    #[cfg(feature = "std")]
    impl<T: ?Sized> Mutex<T> {
        #[allow(clippy::result_unit_err)]
        pub fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, T>> {
            self.0.lock()
        }
    }
    #[cfg(not(feature = "std"))]
    impl<T: ?Sized> Mutex<T> {
        #[allow(clippy::result_unit_err)]
        pub fn lock(&self) -> Result<spin::MutexGuard<'_, T>, core::convert::Infallible> {
            Ok(self.0.lock())
        }
    }
}

/// Path types. Under std these are the real `std::path` types; under no_std
/// (no filesystem) they degrade to owned/borrowed strings so signatures that
/// merely *carry* a path compile, while the code that actually touches the
/// filesystem is `#[cfg(feature = "std")]`-gated out.
pub mod path {
    #[cfg(feature = "std")]
    pub use std::path::{Path, PathBuf};
    #[cfg(not(feature = "std"))]
    pub type PathBuf = alloc::string::String;
    #[cfg(not(feature = "std"))]
    pub type Path = str;
}

/// Items missing from the (absent) prelude under no_std. Glob-import in each
/// VM-core module behind `#[cfg(not(feature = "std"))]`; under std the std
/// prelude already provides these so the import is compiled out.
#[cfg(not(feature = "std"))]
pub mod prelude {
    pub use alloc::borrow::ToOwned;
    pub use alloc::boxed::Box;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec::Vec;
    pub use alloc::{format, vec};
}
