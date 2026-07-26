//! Size probe: the same program run from a *precompiled* `ModuleArtifact`,
//! never touching the tokenizer, parser, type checker or VM compiler.
//!
//! Its reason to exist is the flash delta against `main.rs`: how much of LK's
//! footprint is the front end, and therefore what an artifact-only MCU profile
//! would actually buy. Build both and compare `size` output.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::sync::Arc;
use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicUsize, Ordering};

use cortex_m::asm;
use cortex_m_rt::entry;
use cortex_m_semihosting::{debug, hprint, hprintln};

// --- allocator ------------------------------------------------------------

/// LK allocates for every heap value, AST node and call frame. A real
/// deployment would use a reclaiming allocator sized to the board; a bump
/// allocator keeps this demo free of allocator-crate variables.
const HEAP_SIZE: usize = 1024 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static OFFSET: AtomicUsize = AtomicUsize::new(0);

struct Bump;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = addr_of_mut!(HEAP) as usize;
        let mut cur = OFFSET.load(Ordering::Relaxed);
        loop {
            let start = (base + cur + layout.align() - 1) & !(layout.align() - 1);
            let end = start - base + layout.size();
            if end > HEAP_SIZE {
                return core::ptr::null_mut();
            }
            match OFFSET.compare_exchange_weak(cur, end, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return start as *mut u8,
                Err(actual) => cur = actual,
            }
        }
    }

    /// A bump allocator never reclaims. Fine for a one-shot program; a
    /// long-running one needs a real allocator here.
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOC: Bump = Bump;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    hprintln!("PANIC: {}", info);
    debug::exit(debug::EXIT_FAILURE);
    park()
}

// --- console --------------------------------------------------------------

/// The LK `println` sink. Semihosting is the console every Cortex-M QEMU board
/// has; on real hardware this would be a UART or an RTT channel, and nothing in
/// `stdlib/bare` changes.
fn semihosting_output(text: &str) {
    hprint!("{}", text);
}

// --- the program under test ----------------------------------------------

/// Bytecode for the same program `main.rs` runs from source. `.lkm` files are
/// not checked in, so generate this before the first build (from the repo
/// root):
///
/// ```text
/// cargo run -p lk-cli --no-default-features --features stdlib -- \
///   compile bytecode bare-metal/demo.lk
/// ```
///
/// It is build-locked to the artifact version, so regenerate after a bump.
const ARTIFACT: &str = include_str!("../../demo.lkm");

/// 0+1+1+2+3+5+8+13+21+34
const EXPECTED: i64 = 88;

#[entry]
fn main() -> ! {
    use lk_core::module::ModuleRegistry;
    use lk_core::val::RuntimeVal;
    use lk_core::vm::{ModuleArtifact, ModuleResolver, VmContext, execute_module_artifact_with_ctx};

    lk_stdlib_bare::set_output(semihosting_output);

    let artifact = match ModuleArtifact::from_json_str(ARTIFACT) {
        Ok(artifact) => artifact,
        Err(err) => fail("decode", err),
    };

    let mut registry = ModuleRegistry::new();
    if let Err(err) = lk_stdlib_bare::register_bare_stdlib(&mut registry) {
        fail("stdlib registration", err);
    }
    let mut ctx = VmContext::new().with_resolver(Arc::new(ModuleResolver::with_registry(registry)));

    let result = match execute_module_artifact_with_ctx(artifact, &mut ctx) {
        Ok(result) => result,
        Err(err) => fail("execute", err),
    };

    match result.returns.first() {
        Some(RuntimeVal::Int(value)) if *value == EXPECTED => {
            hprintln!("OK: artifact ran on bare metal, returned {}", value);
            debug::exit(debug::EXIT_SUCCESS);
        }
        other => {
            hprintln!("FAIL: expected Int({}), got {:?}", EXPECTED, other);
            debug::exit(debug::EXIT_FAILURE);
        }
    }

    park()
}

fn fail(stage: &str, err: impl core::fmt::Display) -> ! {
    hprintln!("FAIL at {}: {}", stage, err);
    debug::exit(debug::EXIT_FAILURE);
    park()
}

/// `debug::exit` ends the process under QEMU or a debugger, but on real silicon
/// it just returns — so park the core with interrupts-wait rather than spinning
/// a busy loop and burning power.
fn park() -> ! {
    loop {
        asm::wfi();
    }
}
