//! LK on bare metal: no OS, no std anywhere in the crate graph.
//!
//! Boots through `cortex-m-rt` on QEMU's MPS2 AN386 (Cortex-M4), installs the
//! `stdlib/bare` module surface with semihosting as the console, then parses,
//! type-checks and runs an LK program on the bytecode VM. The exit code is the
//! verdict, so `cargo run --release` doubles as a CI gate.
//!
//! Run it with `cargo run --release` (the runner in `.cargo/config.toml` starts
//! QEMU), or see README.md for the raw command.

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

/// Exercises recursion, loops, arithmetic, a local and a stdlib global, so a
/// broken compiler or executor surfaces as a wrong answer rather than a crash.
const SOURCE: &str = r#"
fn fib(n) {
    if (n < 2) { return n; }
    return fib(n - 1) + fib(n - 2);
}
let total = 0;
for i in 0..10 {
    total = total + fib(i);
}
println("sum(fib(0..9)) = {}", total);
return total;
"#;

/// 0+1+1+2+3+5+8+13+21+34
const EXPECTED: i64 = 88;

#[entry]
fn main() -> ! {
    use lk_core::module::ModuleRegistry;
    use lk_core::syntax::{ParseOptions, parse_program_source};
    use lk_core::val::RuntimeVal;
    use lk_core::vm::{ModuleResolver, VmContext, execute_program_with_ctx};

    lk_stdlib_bare::set_output(semihosting_output);

    let options = ParseOptions {
        // Macro expansion resolves imports through the filesystem, and there
        // is none here.
        expand_macros: false,
        ..Default::default()
    };
    let program = match parse_program_source(SOURCE, options) {
        Ok(program) => program,
        Err(err) => fail("parse", err),
    };

    let mut registry = ModuleRegistry::new();
    if let Err(err) = lk_stdlib_bare::register_bare_stdlib(&mut registry) {
        fail("stdlib registration", err);
    }
    let mut ctx = VmContext::new().with_resolver(Arc::new(ModuleResolver::with_registry(registry)));

    let result = match execute_program_with_ctx(&program, &mut ctx) {
        Ok(result) => result,
        Err(err) => fail("execute", err),
    };

    match result.returns.first() {
        Some(RuntimeVal::Int(value)) if *value == EXPECTED => {
            hprintln!("OK: lk ran on bare metal, returned {}", value);
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
