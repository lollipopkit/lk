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

// The float ABI this image is built with, asserted rather than assumed.
//
// `x86_64-unknown-none` is a soft-float target: without the override in
// `.cargo/config.toml`, Rust passes `f64` in integer registers while the
// Cranelift-emitted LK object passes them in XMM. Nothing fails to link — the
// symbol names agree — and the program computes wrong numbers. A `RUSTFLAGS`
// environment variable replaces that table rather than extending it, so the
// override is one `env RUSTFLAGS=...` away from being lost; this turns that
// into a compile error that says where to look.
#[cfg(not(target_feature = "sse2"))]
compile_error!(
    "this image must be built with `-C target-feature=-soft-float,+sse,+sse2` (see .cargo/config.toml). \
     A RUSTFLAGS environment variable replaces that table rather than extending it — clear it for this crate."
);

extern crate alloc;

mod boot;
mod interrupts;
mod tasks;
mod user;
mod user_programs;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicUsize, Ordering};

// The machine's memory, decided in `link.ld` and read from here.
//
// Not a table in this comment any more. Three things want RAM and none of them
// can ask — the kernel image, the Rust heap the interpreter allocates from,
// and the page allocator the LK program hands out — so the map has to be
// written down somewhere, and the somewhere has to be a place *both* languages
// can read. A linker script is that place: Rust takes the address of an
// `extern static`, LK asks `symbol_address`, and there is one answer.
//
// It used to be a doc table here plus a literal in each language. `0x00380000`
// in particular was written twice and cross-checked at run time by
// `kernel_run` refusing any other address — which notices the drift rather
// than preventing it.
unsafe extern "C" {
    static __heap_base: u8;
    static __heap_size: u8;
    static __run_heap_base: u8;
    static __run_heap_size: u8;
    static __source_base: u8;
    static __source_max: u8;
}

/// A linker symbol's value. It is an address, and an address is a number — the
/// size symbols are ones whose number happens to be a length.
///
/// # Safety
///
/// Taking a symbol's address reads nothing, so this is safe for any of them.
macro_rules! linker_value {
    ($name:ident) => {
        // SAFETY: taking the address of a linker-placed symbol reads no memory.
        unsafe { (&raw const $name) as usize }
    };
}

static OFFSET: AtomicUsize = AtomicUsize::new(0);
static RUN_OFFSET: AtomicUsize = AtomicUsize::new(0);
/// Set for the duration of a hosted run, so allocation goes to the run's arena.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

struct Bump;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let running = RUNNING.load(Ordering::Relaxed) != 0;
        let (base, size, offset) = if running {
            (linker_value!(__run_heap_base), linker_value!(__run_heap_size), &RUN_OFFSET)
        } else {
            (linker_value!(__heap_base), linker_value!(__heap_size), &OFFSET)
        };
        let mut cur = offset.load(Ordering::Relaxed);
        loop {
            let start = (base + cur + layout.align() - 1) & !(layout.align() - 1);
            let end = start - base + layout.size();
            if end > size {
                return core::ptr::null_mut();
            }
            match offset.compare_exchange_weak(cur, end, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return start as *mut u8,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Neither arena reclaims. The kernel's is sized for a session; the run
    /// arena is reset wholesale at the start of each run, which is the only
    /// point at which nothing in it is live.
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
///
/// It does *not* configure the device, and no longer needs to. The board used
/// to bring COM1 up before `main()` because it enabled interrupts itself, and a
/// tick landing before the program's `uart_init()` would have transmitted
/// through an unconfigured UART. The program owns interrupts now and turns them
/// on long after its own first statement, which is `uart_init()`. A fault
/// earlier than that has no gate to land in either, so there is nothing left
/// for a second initialisation to protect.
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

/// A deliberate fault, so the exception path is exercised rather than merely
/// present. Without a build that takes it, a broken reporter looks exactly like
/// a working one — right up until the day something faults.
///
/// Called from `program.lk`, immediately after it installs its interrupt table,
/// and that is not a detail: the table is the program's now, so before `main()`
/// there is no gate for anything. Faulting here used to be a report; faulting
/// there is a triple fault, which on this machine is a silent reset — the exact
/// failure this probe exists to make impossible. Moving the probe to the other
/// side of the install also makes it a stronger claim, because the table it
/// lands in is the one the program actually built.
///
/// Empty without the feature, rather than absent: `program.lk` calls it either
/// way, and a call that does nothing costs less than two versions of the
/// program's boot sequence.
#[unsafe(no_mangle)]
pub extern "C" fn lk_fault_probe() {
    #[cfg(feature = "fault-probe")]
    // SAFETY: nothing about this is safe — that is the point. The address is
    // 36 bits wide, far past the identity map, so the access cannot land on
    // anything real.
    unsafe {
        core::ptr::write_volatile(0x9_0000_0000u64 as *mut u64, 1)
    };
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
    // No task table to prepare any more: the program spawns what it wants by
    // address (`lk_spawn`), and until it does there is one task — this one.
    // The TSS before the IDT: a gate that can be raised from ring 3 needs a
    // ring-0 stack to switch to, and the CPU reads that from the TSS.
    // No `user::init()` either: the descriptor table and the task state
    // segment are the program's now, built in `install_descriptor_table()`
    // right after the interrupt table. The board's share of them is one static
    // (`lk_boot_kernel_stack`), because a ring-0 stack has to exist before
    // there is an allocator to ask for one.
    // No `interrupts::init()` here any more. The interrupt table is the
    // program's — `program.lk` builds its own gates and loads them — so the
    // board cannot enable interrupts before it, and does not try. What the
    // board still owns is `interrupts::stop()` below, because it runs after
    // the program has returned and there is no program left to ask.
    //
    // The window this opens is real and was already there: between here and
    // the program's `idt_install()` a fault has no gate, and a fault with no
    // gate is a triple fault, which on this machine is a silent reset. It is
    // the first thing `program.lk` does for exactly that reason.

    // SAFETY: `main` is the object emitted by `lk compile object:`, linked by
    // build.rs, and takes no arguments.
    let result = unsafe { main() };
    // The clock is already stopped, and by the program: masking the flag and
    // then the chip is the last thing `program.lk` does. That is where it
    // belongs now that the chip is the program's — and the board could not do
    // it here without naming the PIC's ports a second time, for the sake of a
    // line the program has already handled.
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
    // The address is checked rather than trusted, and it is checked against the
    // *same symbol* the program staged into — one answer, not two that agree.
    if address as usize != linker_value!(__source_base) || length < 0 || length as usize > linker_value!(__source_max)
    {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length as usize) };
    let Ok(source) = core::str::from_utf8(bytes) else {
        return -2;
    };

    // A fresh arena for this run. Everything the last one allocated is dead —
    // its output has already been printed and its result was an `i64`.
    RUN_OFFSET.store(0, Ordering::Relaxed);
    RUNNING.store(1, Ordering::Relaxed);
    let outcome = run_program(source);
    RUNNING.store(0, Ordering::Relaxed);
    outcome
}

/// The run itself, split out so the arena flag is cleared on every path out.
fn run_program(source: &str) -> i64 {
    use alloc::sync::Arc;
    use lk_core::module::ModuleRegistry;
    use lk_core::syntax::{ParseOptions, parse_program_source};
    use lk_core::vm::{ModuleResolver, VmContext, execute_program_with_ctx};

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
        Err(error) => {
            // What it said, not just that it said no.
            //
            // The stage code alone is `-5`, which means "it ran and raised" and
            // nothing more. That is enough to know the parser and the type
            // checker were happy and useless for anything after: a program that
            // used `try`/`catch` failed here for a whole round before anyone
            // found out the bare host had no `error` global, because "it
            // raised" reads the same whether the cause is the program or the
            // host.
            //
            // Printed through the same console the program prints through, so
            // the report lands where the output the reader was watching for
            // would have. `{:#}` rather than `{}`: `anyhow` puts the cause
            // chain behind the alternate flag, and the cause is the useful end.
            console_write("run: ");
            console_write(&alloc::format!("{error:#}"));
            console_write("\n");
            -5
        }
    }
}
