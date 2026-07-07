#![no_std]
//! `lk-hal` — L0 platform abstraction layer for LK.
//!
//! Trait definitions only; **no OS dependency and `core`-only** (no `alloc`), so
//! it compiles for `bare`/WASM/MCU targets. Host builds provide std-backed
//! implementations; the VM and runtime see only these traits, never concrete OS
//! calls (dependency rule: L0/L1 take capabilities through `lk-hal`, not `std`).

/// Errors surfaced by HAL providers. Kept allocation-free (no `String`) so the
/// layer stays `core`-only; richer context is the host implementation's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HalError {
    /// The capability is not available in this build/profile.
    Unsupported,
    NotFound,
    PermissionDenied,
    WouldBlock,
    Io,
}

pub type HalResult<T> = Result<T, HalError>;

/// Wall-clock time source.
pub trait Clock {
    /// Milliseconds since an unspecified but fixed epoch.
    fn now_millis(&self) -> u64;
}

/// Random byte source.
pub trait Rng {
    /// Fill `buf` with random bytes.
    fn fill(&self, buf: &mut [u8]);
}

/// Byte sink for program output.
pub trait Stdout {
    fn write(&self, bytes: &[u8]);
    fn flush(&self) {}
}

/// Optional filesystem capability. The `bare` profile may omit it (the `Hal`
/// field is `Option`). Buffer-based to stay `core`-only (no `Vec`).
pub trait FsProvider {
    /// Read up to `buf.len()` bytes of `path` into `buf`; returns the count.
    fn read(&self, path: &str, buf: &mut [u8]) -> HalResult<usize>;
    /// Write `bytes` to `path` (truncating); returns the count written.
    fn write(&self, path: &str, bytes: &[u8]) -> HalResult<usize>;
    /// Whether `path` exists.
    fn exists(&self, path: &str) -> bool;
}

/// Optional stream-network capability. Handles are opaque `u64`s owned by the
/// provider; the `bare` profile may omit it.
pub trait NetProvider {
    /// Open a stream connection to `addr`; returns an opaque handle.
    fn connect(&self, addr: &str) -> HalResult<u64>;
    /// Read from `handle` into `buf`; returns the count read.
    fn read(&self, handle: u64, buf: &mut [u8]) -> HalResult<usize>;
    /// Write `bytes` to `handle`; returns the count written.
    fn write(&self, handle: u64, bytes: &[u8]) -> HalResult<usize>;
    /// Close `handle`.
    fn close(&self, handle: u64) -> HalResult<()>;
}

/// Platform capabilities injected into a VM instance. Required capabilities
/// (`clock`/`rng`/`stdout`) are always present; OS-heavy ones (`fs`/`net`) are
/// optional so `bare`/sandboxed profiles can withhold them.
pub struct Hal<'a> {
    pub clock: &'a dyn Clock,
    pub rng: &'a dyn Rng,
    pub stdout: &'a dyn Stdout,
    pub fs: Option<&'a dyn FsProvider>,
    pub net: Option<&'a dyn NetProvider>,
}

impl<'a> Hal<'a> {
    /// Build a HAL with only the required capabilities; `fs`/`net` withheld.
    pub fn new(clock: &'a dyn Clock, rng: &'a dyn Rng, stdout: &'a dyn Stdout) -> Self {
        Self {
            clock,
            rng,
            stdout,
            fs: None,
            net: None,
        }
    }

    /// Attach a filesystem provider.
    pub fn with_fs(mut self, fs: &'a dyn FsProvider) -> Self {
        self.fs = Some(fs);
        self
    }

    /// Attach a network provider.
    pub fn with_net(mut self, net: &'a dyn NetProvider) -> Self {
        self.net = Some(net);
        self
    }
}
