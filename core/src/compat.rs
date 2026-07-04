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

/// Float helpers: the inherent `f64` methods (`fract`, `abs`, …) are defined
/// in std, not core — under no_std they resolve only while some dependency
/// links std into the graph (which the bare-metal build must not). Route the
/// few VM-core uses through here; `libm` provides the exact IEEE semantics.
pub(crate) mod float {
    #[cfg(feature = "std")]
    #[inline]
    pub(crate) fn fract(x: f64) -> f64 {
        x.fract()
    }

    #[cfg(not(feature = "std"))]
    #[inline]
    pub(crate) fn fract(x: f64) -> f64 {
        libm::modf(x).0
    }

    #[cfg(feature = "std")]
    #[inline]
    pub(crate) fn abs(x: f64) -> f64 {
        x.abs()
    }

    #[cfg(not(feature = "std"))]
    #[inline]
    pub(crate) fn abs(x: f64) -> f64 {
        libm::fabs(x)
    }
}

/// Shared concurrent map for cold-path caches (module resolver): the real
/// `DashMap` under std; a spin-Mutex'd `hashbrown::HashMap` returning cloned
/// values under no_std (resolution is cold, the clone is fine). Only the
/// `new`/`insert`/`get(&Q)→.value()` surface the resolver uses is provided.
pub(crate) mod shared_map {
    #[cfg(feature = "std")]
    pub(crate) use dashmap::DashMap as SharedMap;

    #[cfg(not(feature = "std"))]
    pub(crate) struct SharedMap<K, V> {
        // Entries ride in an `Arc` so `get` hands out an owned guard without
        // requiring `V: Clone` (RuntimeExport is not `Clone`).
        inner: spin::Mutex<hashbrown::HashMap<K, alloc::sync::Arc<V>>>,
    }

    #[cfg(not(feature = "std"))]
    impl<K: Eq + core::hash::Hash, V> SharedMap<K, V> {
        pub(crate) fn new() -> Self {
            Self {
                inner: spin::Mutex::new(hashbrown::HashMap::new()),
            }
        }

        pub(crate) fn insert(&self, key: K, value: V) -> Option<alloc::sync::Arc<V>> {
            self.inner.lock().insert(key, alloc::sync::Arc::new(value))
        }

        pub(crate) fn get<Q>(&self, key: &Q) -> Option<ValueGuard<V>>
        where
            K: core::borrow::Borrow<Q>,
            Q: ?Sized + Eq + core::hash::Hash,
        {
            self.inner
                .lock()
                .get(key)
                .map(|value| ValueGuard(alloc::sync::Arc::clone(value)))
        }
    }

    #[cfg(not(feature = "std"))]
    impl<K, V> core::fmt::Debug for SharedMap<K, V> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("SharedMap").finish_non_exhaustive()
        }
    }

    /// Owned stand-in for `dashmap`'s `Ref` guard: `.value()` yields the entry.
    #[cfg(not(feature = "std"))]
    pub(crate) struct ValueGuard<V>(alloc::sync::Arc<V>);

    #[cfg(not(feature = "std"))]
    impl<V> ValueGuard<V> {
        pub(crate) fn value(&self) -> &V {
            &self.0
        }
    }
}
