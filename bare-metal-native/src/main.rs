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

/// QEMU's `virt` machine puts a PL011 UART here. Writing a byte to the data
/// register transmits it; nothing else needs configuring because the firmware
/// has already brought the device up.
const UART0_DR: *mut u32 = 0x0900_0000 as *mut u32;

/// The sink `lkrt` prints through.
fn uart_write(text: &str) {
    for byte in text.bytes() {
        // SAFETY: the address is the board's UART, mapped by the machine model.
        unsafe { core::ptr::write_volatile(UART0_DR, u32::from(byte)) };
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
    unsafe {
        core::ptr::write_volatile(addr_of_mut!(LK_RESULT), result);
    }
    loop {
        core::hint::spin_loop();
    }
}
