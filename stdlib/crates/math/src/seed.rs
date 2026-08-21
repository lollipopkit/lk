//! Backing state for `math.random()`.
//!
//! The generator is a 64-bit xorshift either way; only how its state is held
//! differs. Cortex-M has no 64-bit atomics (its exclusive-access instructions
//! are 32-bit), so `AtomicU64` does not exist on `thumbv7em-none-eabi` and the
//! no_std path keeps the state behind a spin mutex instead. `math.random()` is
//! not a hot path, and this is not a CSPRNG in either build.

/// Advances the generator and returns the next raw 64-bit state.
pub fn next() -> u64 {
    let mut seed = load();
    if seed == 0 {
        // The first call on a host: take the state from the clock.
        //
        // This used to be unreachable. The state started at `INITIAL` and
        // xorshift never produces 0 from a non-zero one, so the reseed never
        // ran and **`math.random()` returned the same sequence in every
        // process** — the three numbers a program printed on Monday were the
        // three it printed on Tuesday. A function called `random` may not do
        // that.
        //
        // Bare metal keeps `INITIAL`: it has no clock, and a fixed stream is
        // the honest answer there rather than a pretence.
        seed = reseed();
    }
    seed ^= seed << 13;
    seed ^= seed >> 7;
    seed ^= seed << 17;
    store(seed);
    seed.wrapping_add(bump_counter() as u64)
}

/// The bare-metal starting state: no clock to seed from, so the stream is fixed
/// and says so. A host starts at 0 instead, which is what makes the first call
/// reseed.
#[cfg(not(feature = "std"))]
const INITIAL: u64 = 0x12345678_9ABCDEF0;

#[cfg(feature = "std")]
mod imp {
    use core::sync::atomic::{AtomicU64, Ordering};

    /// Zero means "not seeded yet", which is what sends the first call through
    /// `reseed`. Xorshift never *produces* zero from a non-zero state, so this
    /// value cannot recur once the stream is running.
    static SEED: AtomicU64 = AtomicU64::new(0);

    pub(super) fn load() -> u64 {
        SEED.load(Ordering::Relaxed)
    }

    pub(super) fn store(value: u64) {
        SEED.store(value, Ordering::Relaxed);
    }

    /// A host has a wall clock to pull entropy from.
    pub(super) fn reseed() -> u64 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        if nanos == 0 { 1 } else { nanos }
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    use lk_core::compat::sync::Mutex;

    static SEED: Mutex<u64> = Mutex::new(super::INITIAL);

    pub(super) fn load() -> u64 {
        SEED.lock().map(|seed| *seed).unwrap_or(super::INITIAL)
    }

    // `lock()` is infallible here but fallible under std (poisoning), and
    // workspace feature unification picks which shape this sees.
    #[allow(irrefutable_let_patterns)]
    pub(super) fn store(value: u64) {
        if let Ok(mut seed) = SEED.lock() {
            *seed = value;
        }
    }

    /// Bare metal has no wall clock, so there is nothing to reseed *from*:
    /// `math.random()` is a deterministic sequence from the fixed initial
    /// state. A board with an entropy source should seed the VM itself rather
    /// than have this module guess.
    pub(super) fn reseed() -> u64 {
        1
    }
}

use imp::{load, reseed, store};

/// 32-bit atomics exist everywhere this builds, so the counter needs no split.
fn bump_counter() -> u32 {
    use core::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
