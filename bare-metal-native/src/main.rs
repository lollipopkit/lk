//! LK running as native machine code with no OS.
//!
//! The interpreter demo next door (`bare-metal/`) runs the bytecode VM on the
//! board. This one runs LK's *compiled output*: `build.rs` calls
//! `lk compile object:<triple>` and the linker places the resulting aarch64
//! object beside `lkrt`. What executes is real instructions, not a dispatch
//! loop — which is the reason the AOT path exists.
//!
//! What this binary supplies is what a bare-metal image always must: a reset
//! path (`boot.rs`), a memory map (`link.ld`), an allocator and a panic
//! handler. `lkrt` needs the last two because it manages arena-allocated
//! strings and containers.

#![no_std]
#![no_main]

extern crate alloc;

mod boot;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Bump allocation: enough for a one-shot program, and it keeps the demo about
/// the compiled code rather than about allocator choice.
const HEAP_SIZE: usize = 256 * 1024;
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

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOC: Bump = Bump;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

unsafe extern "C" {
    /// The compiled entry point.
    ///
    /// Codegen exports the module's entry as `main` (its other functions stay
    /// local `lk_fn_N`). That is the C `main` name, which is fine here: with
    /// `#![no_main]` there is no Rust `main` to collide with, and nothing
    /// starts it automatically — `_start` calls it explicitly.
    fn main() -> i64;
}

/// QEMU's `virt` machine puts a PL011 UART here; so does most Arm hardware,
/// at some address the board tells you. `boot.rs` enables it.
const UART0_DR: *mut u32 = 0x0900_0000 as *mut u32;
/// Flag register. Bit 5 (`TXFF`) is set while the transmit FIFO is full.
const UART0_FR: *const u32 = 0x0900_0018 as *const u32;

/// The sink `lkrt` prints through: a PL011 driver, three instructions long.
///
/// This is the real output path — a device on a bus, not a debugger service —
/// which is the point of the demo. Semihosting was useful while the boot path
/// was still suspect precisely because it bypasses the device; now that the
/// device works, using it keeps the image honest about running on hardware.
fn uart_write(text: &str) {
    for byte in text.bytes() {
        // SAFETY: the board's UART, mapped Device by the boot page table.
        unsafe {
            while core::ptr::read_volatile(UART0_FR) & (1 << 5) != 0 {}
            core::ptr::write_volatile(UART0_DR, u32::from(byte));
        }
    }
}

/// Write a value as 16 hex digits, so a fault report needs no formatting
/// machinery (`core::fmt` in a fault handler is a good way to fault again).
fn write_hex(value: u64) {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    for (i, slot) in buf.iter_mut().enumerate() {
        *slot = digits[((value >> (60 - i * 4)) & 0xf) as usize];
    }
    // SAFETY: every byte written above came from an ASCII digit table.
    uart_write(unsafe { core::str::from_utf8_unchecked(&buf) });
}

/// Where the vector table sends every exception.
///
/// It reports and halts rather than trying to recover: nothing here knows how
/// to resume a faulted program, and a fault that prints its cause is the whole
/// difference between a debuggable board and a board that stops.
#[unsafe(no_mangle)]
pub extern "C" fn fault_report(kind: u64, esr: u64, far: u64, elr: u64) -> ! {
    uart_write("\n!! fault kind=");
    write_hex(kind);
    uart_write(" esr=");
    write_hex(esr);
    uart_write(" far=");
    write_hex(far);
    uart_write(" elr=");
    write_hex(elr);
    uart_write("\n");
    loop {
        core::hint::spin_loop();
    }
}

/// Where the compiled code's result is left, so it cannot be optimised away and
/// a debugger or test harness can read it.
#[unsafe(no_mangle)]
pub static mut LK_RESULT: i64 = 0;

/// Called from the boot stub once there is a stack and `.bss` is zeroed.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    // Referencing `lkrt` is what puts its rlib on the link line at all. Without
    // it cargo sees an unused dependency and the runtime the compiled object
    // calls into is simply absent.
    let _ = lkrt::link_anchor();

    // Give the runtime somewhere to print. Until this is installed `println`
    // is discarded rather than an error, which is right for a headless board
    // but not much use for a demo.
    lkrt::set_output(uart_write);

    // SAFETY: `main` is the object emitted by `lk compile object:`, linked by
    // build.rs, and takes no arguments.
    let result = unsafe { main() };
    // Control coming back here is the other half of the demo: the compiled
    // program is a callable, not a takeover.
    uart_write("[lk returned to the board, status ");
    write_hex(result as u64);
    uart_write("]\n");
    unsafe {
        core::ptr::write_volatile(addr_of_mut!(LK_RESULT), result);
    }
    loop {
        core::hint::spin_loop();
    }
}
