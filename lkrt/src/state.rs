// `alloc`, not the std prelude: part of the computation-only subset.
#[allow(unused_imports)]
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

use alloc::ffi::CString;
use core::ffi::{c_char, c_void};

use hashbrown::{HashMap, HashSet};
use rustc_hash::FxBuildHasher;

#[cfg(feature = "std")]
use core::cell::RefCell;
#[cfg(feature = "std")]
use std::net::TcpStream;

#[cfg(feature = "std")]
thread_local! {
    static RUNTIME: RefCell<RuntimeState> = const { RefCell::new(RuntimeState::new()) };
}

/// Bare metal has no thread-local storage, so the arena is a single global
/// behind a spin lock.
///
/// The lock is not protecting against threads — there are none — but against
/// an interrupt handler that reached the runtime. Uncontended on a single core
/// it is one atomic operation, which keeps the hot path (arena registration on
/// every dynamic string and container) close to the thread-local cost.
#[cfg(not(feature = "std"))]
static RUNTIME: spin::Mutex<RuntimeState> = spin::Mutex::new(RuntimeState::new());

/// Frees one arena-registered container handle of its concrete type.
type ContainerDrop = unsafe fn(*mut c_void);

/// Collects the arena strings a container *itself* created, so releasing the
/// container can release them too.
///
/// Only the constructors that mint their own strings register one (`str.split`,
/// `str.chars`): their elements are freshly arena-allocated in the same call and
/// reachable from nowhere else. Every other container holds strings it did not
/// create and must not free.
type ContainerOwnedStrings = unsafe fn(*mut c_void) -> Vec<*mut c_char>;

struct ContainerEntry {
    drop: ContainerDrop,
    owned_strings: Option<ContainerOwnedStrings>,
}

/// Runs `f` with this thread's runtime state.
///
/// The state is thread-local rather than a process-global mutex because arena
/// registration sits on the hot path of every dynamic string and container
/// operation and must not pay for locking.
///
/// **Native binaries are not single-threaded** — `spawn`/`go` create real OS
/// threads (`std::thread::spawn` in `chan.rs`). What makes the lock-free choice
/// sound is the isolation, not an absence of threads: each thread owns its own
/// arena, and values never cross a thread boundary because channels deep-copy
/// (the isolate model). A handle or arena string must therefore never be passed
/// between threads — doing so would free it on the wrong arena.
///
/// A spawned thread reclaims its own arena on exit (see the [`Drop`] impl on
/// [`RuntimeState`]); `lkrt_cleanup()` covers the main thread.
#[cfg(feature = "std")]
pub(crate) fn with_runtime<R>(f: impl FnOnce(&mut RuntimeState) -> R) -> R {
    RUNTIME.with(|state| f(&mut state.borrow_mut()))
}

#[cfg(not(feature = "std"))]
pub(crate) fn with_runtime<R>(f: impl FnOnce(&mut RuntimeState) -> R) -> R {
    f(&mut RUNTIME.lock())
}

/// Whether this thread is *inside* a [`with_runtime`] call.
///
/// Asked on the raise path, which is the one place the answer has to be no: a
/// raise `_longjmp`s past every Rust frame between here and the handler, so a
/// live borrow's `RefMut` never drops and the flag stays set. What follows is
/// not a crash but a puzzle — the *next* runtime operation, arbitrarily far
/// away and in unrelated code, panics with "already mutably borrowed".
///
/// Every lkrt entry that can raise is written to avoid this: the `raising` /
/// `status` wrappers make the closure return a `Result`, so ordinary Rust drops
/// run before the raise happens outside it, and the handful of direct
/// `raise_str` calls drop their guard explicitly first. That is a rule enforced
/// by reading, which is why it is also checked here — cheaply, since a raise is
/// already doing a `CString` allocation and a `longjmp`.
#[cfg(feature = "std")]
pub(crate) fn runtime_borrow_is_live() -> bool {
    RUNTIME.with(|state| state.try_borrow_mut().is_err())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandleKind {
    #[cfg(feature = "std")]
    TcpStream,
}

pub(crate) struct RuntimeState {
    next_handle: i64,
    #[cfg(feature = "std")]
    resources: HashMap<i64, Resource, FxBuildHasher>,
    owned_strings: HashSet<usize, FxBuildHasher>,
    /// Container handles (lists/maps) with their typed drop functions — the
    /// default arena of RFC aot-redesign §3.4, reclaimed by [`Self::cleanup`].
    /// Keyed by address so the scope-drop pass can release a loop-local
    /// container early (`lkrt_rt_handle_release`) in O(1); without that a
    /// long-running loop grows this table once per iteration.
    owned_containers: HashMap<usize, ContainerEntry, FxBuildHasher>,
}

impl RuntimeState {
    const fn new() -> Self {
        Self {
            next_handle: 0,
            #[cfg(feature = "std")]
            resources: HashMap::with_hasher(FxBuildHasher),
            owned_strings: HashSet::with_hasher(FxBuildHasher),
            owned_containers: HashMap::with_hasher(FxBuildHasher),
        }
    }
}

/// A host resource behind an `i64` handle.
///
/// `TcpStream` is the only kind left, so the whole family is `std`-only: without
/// an OS there is no host resource to hold. It is still an enum rather than the
/// stream itself because the *handle* machinery (`close_any`, `close_kind`) is
/// about "a resource of some kind", and a second kind is a plausible addition.
///
/// `Bytes` used to be one of these — a *one-shot* value, read with `take_bytes`,
/// which removed it. Every producer now answers the arena `Bytes` handle
/// ([`crate::lkbytes`]) instead, because a `Bytes` in the language is an
/// ordinary value you may read twice; the one-shot kind, its two accessors and
/// the `bytes.to_string_utf8`/`bytes.free` ABI entries over it are gone with it.
#[cfg(feature = "std")]
enum Resource {
    TcpStream(TcpStream),
}

#[cfg(feature = "std")]
impl Resource {
    fn kind(&self) -> HandleKind {
        match self {
            Resource::TcpStream(_) => HandleKind::TcpStream,
        }
    }
}

impl RuntimeState {
    #[cfg(feature = "std")]
    pub(crate) fn insert_stream(&mut self, stream: TcpStream) -> i64 {
        let handle = self.next_handle();
        self.resources.insert(handle, Resource::TcpStream(stream));
        handle
    }

    #[cfg(feature = "std")]
    pub(crate) fn stream(&self, handle: i64) -> Result<&TcpStream, String> {
        match self.resources.get(&handle) {
            Some(Resource::TcpStream(stream)) => Ok(stream),
            None => Err(format!("tcp stream handle {handle} is closed or invalid")),
        }
    }

    #[cfg(feature = "std")]
    pub(crate) fn close_any(&mut self, handle: i64) -> bool {
        self.resources.remove(&handle).is_some()
    }

    #[cfg(feature = "std")]
    pub(crate) fn close_kind(&mut self, handle: i64, expected: HandleKind) -> Result<bool, String> {
        let Some(resource) = self.resources.get(&handle) else {
            return Ok(false);
        };
        let actual = resource.kind();
        if actual != expected {
            return Err(wrong_kind_error(handle, expected, actual));
        }
        self.resources.remove(&handle);
        Ok(true)
    }

    fn next_handle(&mut self) -> i64 {
        self.next_handle += 1;
        self.next_handle
    }

    pub(crate) fn register_string(&mut self, ptr: *mut c_char) {
        if !ptr.is_null() {
            self.owned_strings.insert(ptr as usize);
        }
    }

    pub(crate) fn unregister_string(&mut self, ptr: *mut c_char) -> bool {
        self.owned_strings.remove(&(ptr as usize))
    }

    pub(crate) fn register_container(
        &mut self,
        ptr: *mut c_void,
        drop: ContainerDrop,
        owned_strings: Option<ContainerOwnedStrings>,
    ) {
        if !ptr.is_null() {
            self.owned_containers
                .insert(ptr as usize, ContainerEntry { drop, owned_strings });
        }
    }

    /// Removes `ptr` from the arena, returning its drop function. `None` when
    /// the handle is unknown (already released, or never arena-owned) — the
    /// caller must then leave it alone.
    pub(crate) fn unregister_container(&mut self, ptr: *mut c_void) -> Option<ContainerDrop> {
        self.owned_containers.remove(&(ptr as usize)).map(|entry| entry.drop)
    }

    /// Removes `ptr` from the arena and, if the container minted its own arena
    /// strings, deregisters and returns them so the caller can free them
    /// alongside it. The strings must be freed *before* the container is
    /// dropped: reading them out is what needs the container alive.
    pub(crate) fn unregister_container_deep(&mut self, ptr: *mut c_void) -> Option<(ContainerDrop, Vec<*mut c_char>)> {
        let entry = self.owned_containers.remove(&(ptr as usize))?;
        let strings = match entry.owned_strings {
            // SAFETY: registered by `arena_handle_owning_strings` with the
            // collector matching this handle's concrete type, and the box is
            // still alive (only its arena registration was removed).
            Some(collect) => unsafe { collect(ptr) }
                .into_iter()
                // A pointer the arena does not own (already freed, or never
                // registered) is left alone rather than double-freed.
                .filter(|string| self.owned_strings.remove(&(*string as usize)))
                .collect(),
            None => Vec::new(),
        };
        Some((entry.drop, strings))
    }

    pub(crate) fn cleanup(&mut self) {
        #[cfg(feature = "std")]
        self.resources.clear();
        for ptr in self.owned_strings.drain() {
            // SAFETY: All entries are pointers produced by CString::into_raw
            // and registered by lkrt before being returned across FFI.
            unsafe {
                drop(CString::from_raw(ptr as *mut c_char));
            }
        }
        for (ptr, entry) in self.owned_containers.drain() {
            // SAFETY: Each entry was registered by `arena_handle` with the drop
            // function matching the handle's concrete type; generated code never
            // uses a handle after `lkrt_cleanup` (it is the last call before exit).
            // Element strings need no separate pass here: they were registered in
            // `owned_strings`, which the loop above already drained.
            unsafe {
                (entry.drop)(ptr as *mut c_void);
            }
        }
    }
}

/// Reclaims a thread's arena when the thread ends.
///
/// Without this, a spawned thread's containers and strings are leaked at exit:
/// dropping the tables frees the tables, not what they point at. `lkrt_cleanup`
/// only ever runs on the main thread, so every `spawn`/`go` used to leak
/// whatever it still held.
///
/// Safe to run here because the isolate model keeps values thread-local
/// (channels deep-copy), so nothing outside this thread can still reference
/// them. Running twice is harmless — `cleanup` drains, so the main thread's
/// explicit `lkrt_cleanup()` leaves nothing for this to free.
impl Drop for RuntimeState {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Boxes `value`, registers the handle in the runtime arena with a typed drop
/// function, and returns it as an opaque pointer. All container `new` entry
/// points allocate through here so `lkrt_cleanup` can reclaim them.
pub(crate) fn arena_handle<T>(value: T) -> *mut c_void {
    let ptr = Box::into_raw(Box::new(value)) as *mut c_void;
    with_runtime(|rt| rt.register_container(ptr, drop_impl::<T>, None));
    ptr
}

unsafe fn drop_impl<T>(ptr: *mut c_void) {
    // SAFETY: `ptr` came from `Box::into_raw` with this exact `T`.
    drop(unsafe { Box::from_raw(ptr as *mut T) });
}

/// [`arena_handle`] for a container whose elements are arena strings **it
/// created itself** — `str.split`, `str.chars`.
///
/// Registering `collect` is what lets `lkrt_rt_handle_release_deep` free those
/// strings along with the container. Without it, releasing the container early
/// reclaimed the list and left every element string in the arena until exit,
/// which is most of what a `for l in lines { l.split(",") }` loop retains.
///
/// # Safety contract
///
/// `collect` must return exactly the arena strings this container owns
/// exclusively. A string reachable from anywhere else must not be listed: the
/// deep release frees them outright.
pub(crate) fn arena_handle_owning_strings<T>(value: T, collect: ContainerOwnedStrings) -> *mut c_void {
    let ptr = Box::into_raw(Box::new(value)) as *mut c_void;
    with_runtime(|rt| rt.register_container(ptr, drop_impl::<T>, Some(collect)));
    ptr
}

fn wrong_kind_error(handle: i64, expected: HandleKind, actual: HandleKind) -> String {
    format!("handle {handle} has kind {actual:?}, expected {expected:?}")
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    /// The raise-path guard has to be able to say "yes".
    ///
    /// A predicate that is always `false` costs nothing, breaks nothing, and
    /// silently stops being a check — which is the only way this one can fail,
    /// since the thing it protects against is not reachable from a test (a raise
    /// under a live borrow aborts the process by design).
    #[test]
    fn a_live_runtime_borrow_is_visible_to_the_raise_path() {
        assert!(!runtime_borrow_is_live(), "no borrow outside `with_runtime`");
        with_runtime(|_| {
            assert!(runtime_borrow_is_live(), "the borrow `with_runtime` holds is its own");
        });
        assert!(!runtime_borrow_is_live(), "and it is released on the way out");
    }
}
