//! LK running as native x86-64 machine code with no OS.
//!
//! The aarch64 demo next door proves the same thing on a memory-mapped
//! machine. This one exists because x86 devices live in a *second address
//! space* reached by the `in`/`out` instructions rather than by loads and
//! stores — `program.lk` drives a 16550 UART through `port_in_u8` /
//! `port_out_u8`, which is what a kernel on this architecture has to do before
//! it can say anything at all.

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
    /// The compiled entry point. Codegen exports the module's entry as `main`;
    /// with `#![no_main]` there is no Rust `main` to collide with, and nothing
    /// starts it automatically.
    fn main() -> i64;
}

/// COM1's base port. The transmit register is at +0, the line status at +5.
const COM1: u16 = 0x3f8;

/// One byte out of the 16550, for the runtime's own output.
///
/// `program.lk` has its own copy of this — that one is the demo. This exists
/// because `lkrt` needs a sink for the value a script evaluates to, and it
/// cannot call back into LK.
fn serial_write(text: &str) {
    for byte in text.bytes() {
        // SAFETY: COM1 is a fixed ISA port; `in`/`out` on it cannot touch
        // memory. Waiting on bit 5 of the line status (transmit holding
        // register empty) is what makes the write safe *for the device*.
        unsafe {
            while port_in_u8(COM1 + 5) & 0x20 == 0 {}
            port_out_u8(COM1, byte);
        }
    }
}

/// # Safety
///
/// The caller must know what device answers at `port`.
unsafe fn port_in_u8(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// # Safety
///
/// As [`port_in_u8`], for a write.
unsafe fn port_out_u8(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Where the compiled code's result is left, so it cannot be optimised away and
/// a debugger or test harness can read it.
#[unsafe(no_mangle)]
pub static mut LK_RESULT: i64 = 0;

/// Called from the boot stub once the CPU is in long mode with a stack.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() -> ! {
    // Referencing `lkrt` is what puts its rlib on the link line at all. Without
    // it cargo sees an unused dependency and the runtime the compiled object
    // calls into is simply absent.
    let _ = lkrt::link_anchor();
    lkrt::set_output(serial_write);

    // SAFETY: `main` is the object emitted by `lk compile object:`, linked by
    // build.rs, and takes no arguments.
    let result = unsafe { main() };
    unsafe {
        core::ptr::write_volatile(addr_of_mut!(LK_RESULT), result);
    }
    serial_write("[lk returned to the board]\n");
    // QEMU's `isa-debug-exit` device: writing here ends the machine, so the
    // smoke test finishes instead of needing a timeout to decide it is done.
    // SAFETY: a fixed ISA port; the machine is configured with the device.
    unsafe { port_out_u8(0xf4, 0) };
    loop {
        core::hint::spin_loop();
    }
}
