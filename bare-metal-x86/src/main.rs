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
mod interrupts;
mod tasks;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicUsize, Ordering};

/// The machine's memory, decided here and nowhere else.
///
/// Written down because three things now want RAM and none of them can ask: the
/// kernel image, the Rust heap the interpreter allocates from, and the page
/// allocator the LK program hands out. A heap in `.bss` would have been simpler
/// until it grew — `.bss` follows the image, and at a few megabytes it reaches
/// up over the shared page at 0x300000, which is a fixed address the program
/// and the interrupt handlers agree on. Fixed regions cannot creep.
///
/// | region | what |
/// | --- | --- |
/// | `0x00100000`.. | this image, and its `.bss` |
/// | `0x00300000`.. | the shared page (`SHARED_BASE` in `program.lk`) |
/// | `0x00380000`.. | 64 KiB staging for a source file read off the disk |
/// | `0x00400000`.. | the interpreter's heap, 28 MiB |
/// | `0x02000000`.. | the LK page allocator's arena |
const HEAP_BASE: usize = 0x0040_0000;
const HEAP_SIZE: usize = 28 * 1024 * 1024;
static OFFSET: AtomicUsize = AtomicUsize::new(0);

struct Bump;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = HEAP_BASE;
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
pub(crate) fn serial_write(text: &str) {
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

/// Bring COM1 up before interrupts are enabled.
///
/// `program.lk`'s driver configures it too, with the same values — this is not
/// a substitute for it. It is here because the timer handler transmits, and a
/// tick landing before `uart_init()` would write to an unconfigured device.
fn serial_init() {
    // SAFETY: fixed ISA ports. The sequence matches `program.lk`'s.
    unsafe {
        port_out_u8(COM1 + 1, 0x00); // interrupts off; this driver polls
        port_out_u8(COM1 + 3, 0x80); // DLAB: the divisor latch
        port_out_u8(COM1, 0x03); //     divisor 3 = 38400 baud
        port_out_u8(COM1 + 1, 0x00);
        port_out_u8(COM1 + 3, 0x03); // 8N1
        port_out_u8(COM1 + 2, 0xc7); // FIFOs on and cleared
        port_out_u8(COM1 + 4, 0x0b); // DTR + RTS + OUT2
    }
}

/// # Safety
///
/// The caller must know what device answers at `port`.
pub(crate) unsafe fn port_in_u8(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// # Safety
///
/// As [`port_in_u8`], for a write.
pub(crate) unsafe fn port_out_u8(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Write a value as 16 hex digits, so a fault report needs no formatting
/// machinery (`core::fmt` in a fault handler is a good way to fault again).
pub(crate) fn write_hex(value: u64) {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    for (i, slot) in buf.iter_mut().enumerate() {
        *slot = digits[((value >> (60 - i * 4)) & 0xf) as usize];
    }
    // SAFETY: every byte written above came from an ASCII digit table.
    serial_write(unsafe { core::str::from_utf8_unchecked(&buf) });
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
    serial_init();
    // Task stacks before interrupts: the first tick may switch away, and it
    // can only do that to a stack that already looks like a saved task.
    tasks::init();
    // The handler transmits, and it can fire from here on — which is why the
    // UART is already up.
    interrupts::init();

    // A deliberate fault, so the exception path is exercised rather than
    // merely present. Without a build that takes it, a broken reporter looks
    // exactly like a working one — right up to the day something faults.
    #[cfg(feature = "fault-probe")]
    unsafe {
        core::ptr::write_volatile(0x9_0000_0000u64 as *mut u64, 1)
    };
    // SAFETY: `main` is the object emitted by `lk compile object:`, linked by
    // build.rs, and takes no arguments.
    let result = unsafe { main() };
    // Stop the clock before reporting: a tick landing mid-line would splice a
    // '.' into it.
    interrupts::stop();
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

// ------------------------------------------------------------ the interpreter

/// Where a source file read off the disk is staged, and how much of one this
/// kernel will take. See the memory map above.
const SOURCE_BASE: usize = 0x0038_0000;
const SOURCE_MAX: usize = 64 * 1024;

unsafe extern "C" {
    /// The console, which belongs to the LK program: it owns the cursor, the
    /// window rectangles and the serial line. Rust holds the interpreter and
    /// nothing else — output goes back the way it came.
    fn lk_console_byte(byte: i64);
}

/// The `println` sink the bare stdlib writes through.
fn console_write(text: &str) {
    for byte in text.bytes() {
        unsafe { lk_console_byte(i64::from(byte)) };
    }
}

/// Runs an LK program the kernel read off the disk.
///
/// This is the direction the whole demo has been pointing at: the kernel is
/// compiled LK, and what it now hosts is an *interpreter* for LK, running a
/// program that was not part of the image. Nothing about the program is known
/// at build time — it arrives as bytes on a disk, is parsed here, and prints
/// through the kernel's own console.
///
/// Returns 0 on success, or a negative code naming the stage that failed. A
/// code rather than a message because the caller is LK code with no way to own
/// a string this side made, and because the stage is the useful part: a parse
/// failure and a runtime failure want different next steps.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_run(address: i64, length: i64) -> i64 {
    use alloc::sync::Arc;
    use lk_core::module::ModuleRegistry;
    use lk_core::syntax::{ParseOptions, parse_program_source};
    use lk_core::vm::{ModuleResolver, VmContext, execute_program_with_ctx};

    if address as usize != SOURCE_BASE || length < 0 || length as usize > SOURCE_MAX {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length as usize) };
    let Ok(source) = core::str::from_utf8(bytes) else {
        return -2;
    };

    lk_stdlib_bare::set_output(console_write);
    let options = ParseOptions {
        // Macro expansion resolves imports through a filesystem the parser
        // knows how to reach, and there is none here — the one this kernel has
        // is its own, three layers down in `drivers/tarfs.lk`.
        expand_macros: false,
        ..Default::default()
    };
    let Ok(program) = parse_program_source(source, options) else {
        return -3;
    };

    let mut registry = ModuleRegistry::new();
    if lk_stdlib_bare::register_bare_stdlib(&mut registry).is_err() {
        return -4;
    }
    let mut ctx = VmContext::new().with_resolver(Arc::new(ModuleResolver::with_registry(registry)));
    match execute_program_with_ctx(&program, &mut ctx) {
        Ok(_) => 0,
        Err(_) => -5,
    }
}
