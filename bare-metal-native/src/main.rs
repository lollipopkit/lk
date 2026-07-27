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

// The GICv2 on QEMU's `virt` machine: a distributor (which interrupts exist and
// who they go to) and a per-CPU interface (acknowledge and end-of-interrupt).
const GICD_BASE: usize = 0x0800_0000;
const GICC_BASE: usize = 0x0801_0000;
const GICD_CTLR: *mut u32 = GICD_BASE as *mut u32;
const GICD_ISENABLER: *mut u32 = (GICD_BASE + 0x100) as *mut u32;
const GICD_IPRIORITYR: *mut u8 = (GICD_BASE + 0x400) as *mut u8;
const GICC_CTLR: *mut u32 = GICC_BASE as *mut u32;
const GICC_PMR: *mut u32 = (GICC_BASE + 0x004) as *mut u32;
const GICC_IAR: *const u32 = (GICC_BASE + 0x00c) as *const u32;
const GICC_EOIR: *mut u32 = (GICC_BASE + 0x010) as *mut u32;

/// The EL1 physical timer's private peripheral interrupt.
const TIMER_IRQ: u32 = 30;

/// How often the timer fires, as a fraction of the counter frequency.
/// `CNTFRQ_EL0` is 62.5 MHz on this machine, so this is a millisecond.
const TICK_DIVISOR: u64 = 1000;

unsafe extern "C" {
    /// The interrupt handler, written in LK.
    ///
    /// `#[export("lk_timer_isr")]` in `program.lk` is what makes this name
    /// exist: without it the function would be a local `lk_fn_N` that no vector
    /// table could reach.
    fn lk_timer_isr();
}

/// Route the timer interrupt to this core and let it through the priority mask.
fn gic_init() {
    // SAFETY: the GIC's registers, mapped Device by the boot page table.
    unsafe {
        core::ptr::write_volatile(GICD_CTLR, 1);
        // Priority 0 is the highest; the mask below has to be numerically
        // greater or the interrupt is never delivered.
        core::ptr::write_volatile(GICD_IPRIORITYR.add(TIMER_IRQ as usize), 0x80);
        core::ptr::write_volatile(
            GICD_ISENABLER.add((TIMER_IRQ / 32) as usize),
            1 << (TIMER_IRQ % 32),
        );
        core::ptr::write_volatile(GICC_PMR, 0xff);
        core::ptr::write_volatile(GICC_CTLR, 1);
    }
}

/// Arm the EL1 physical timer and unmask IRQs.
fn timer_start() {
    // SAFETY: system-register access on the core we are running on.
    unsafe {
        let freq: u64;
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack));
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) freq / TICK_DIVISOR, options(nomem, nostack));
        // ENABLE, with IMASK clear.
        core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
        // Until this the interrupt is pending but not taken.
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
}

/// Rearm the timer for another period. A countdown timer stays fired until its
/// counter is reloaded, so without this the first interrupt is also the last —
/// and, since it is never deasserted, the core would spin in the handler.
fn timer_rearm() {
    // SAFETY: system-register access on the core we are running on.
    unsafe {
        let freq: u64;
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack));
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) freq / TICK_DIVISOR, options(nomem, nostack));
    }
}

/// Mask interrupts and disable the timer, in that order: disabling first would
/// leave a pending interrupt to be taken the moment anything else unmasks.
fn timer_stop() {
    // SAFETY: system-register access on the core we are running on.
    unsafe {
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
        core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 0u64, options(nomem, nostack));
    }
}

/// Called from the IRQ vector with every caller-saved register already spilled.
///
/// The board's share of an interrupt is acknowledging it, rearming the device
/// and signalling completion; what the tick *means* is the program's, and that
/// part is LK code.
#[unsafe(no_mangle)]
pub extern "C" fn irq_dispatch() {
    // SAFETY: the GIC CPU interface, mapped Device by the boot page table.
    let ack = unsafe { core::ptr::read_volatile(GICC_IAR) };
    if ack & 0x3ff == TIMER_IRQ {
        timer_rearm();
        // SAFETY: `lk_timer_isr` is the LK function `#[export]`ed under that
        // name, compiled to a `void(void)` by the same build.
        unsafe { lk_timer_isr() };
    }
    // SAFETY: as above. The write must carry the value `IAR` returned.
    unsafe { core::ptr::write_volatile(GICC_EOIR, ack) };
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

    // The handler transmits, and it can fire from here on — which is why the
    // boot stub already brought the UART up rather than leaving it to
    // `program.lk`'s `uart_init()`.
    gic_init();
    timer_start();

    // SAFETY: `main` is the object emitted by `lk compile object:`, linked by
    // build.rs, and takes no arguments.
    let result = unsafe { main() };
    // Stop the clock before reporting. The handler is still armed, and a tick
    // landing mid-line would splice a '.' into it.
    timer_stop();
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
