//! x86 port I/O.
//!
//! A second address space, reached by the `in`/`out` instructions rather than
//! by a load or a store. Devices that predate memory-mapped I/O live here — the
//! 16550 UART, the PIC, the PIT, the PS/2 controller — so a kernel on x86 needs
//! these before it can say anything at all.
//!
//! Like `mmio`, these are opaque calls rather than inline instructions, and
//! `WritesHost` in the ABI table including the reads: reading a device port can
//! change its state (a UART's receive register empties when read), so it is not
//! pure no matter what it returns.
//!
//! No `nomem` on the asm, deliberately. It would be *true* — port I/O does not
//! touch memory — and it would let the compiler move a memory write across an
//! `out`, which is exactly the ordering a driver depends on: fill the buffer,
//! then kick the device. Without it the kick can be emitted first. The claim
//! `nomem` buys is worth less than the ordering it gives up.
//!
//! Unlike `mmio`, they are architecture-gated: no other ISA has these
//! instructions, so there is nothing to emit rather than something that would
//! merely be unusual. Elsewhere they raise.

#[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
fn unsupported(name: &str) -> ! {
    crate::panic::raise_str(name);
}

/// # Safety
///
/// What device answers at `port`, and what reading it does, is the claim the
/// caller makes by writing `unsafe` in LK. Nothing here can check it.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_port_in_u8(port: i64) -> i64 {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        let value: u8;
        unsafe {
            core::arch::asm!(
                "in al, dx",
                out("al") value,
                in("dx") port as u16,
                options(nostack, preserves_flags),
            );
        }
        i64::from(value)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = port;
        unsupported("port_in_u8 requires x86");
    }
}

/// # Safety
///
/// As [`lkrt_port_in_u8`], for a 16-bit access.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_port_in_u16(port: i64) -> i64 {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        let value: u16;
        unsafe {
            core::arch::asm!(
                "in ax, dx",
                out("ax") value,
                in("dx") port as u16,
                options(nostack, preserves_flags),
            );
        }
        i64::from(value)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = port;
        unsupported("port_in_u16 requires x86");
    }
}

/// # Safety
///
/// As [`lkrt_port_in_u8`], for a 32-bit access.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_port_in_u32(port: i64) -> i64 {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        let value: u32;
        unsafe {
            core::arch::asm!(
                "in eax, dx",
                out("eax") value,
                in("dx") port as u16,
                options(nostack, preserves_flags),
            );
        }
        i64::from(value)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = port;
        unsupported("port_in_u32 requires x86");
    }
}

/// # Safety
///
/// As [`lkrt_port_in_u8`], for a write.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_port_out_u8(port: i64, value: i64) {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port as u16,
            in("al") value as u8,
            options(nostack, preserves_flags),
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = (port, value);
        unsupported("port_out_u8 requires x86");
    }
}

/// # Safety
///
/// As [`lkrt_port_in_u8`], for a 16-bit write.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_port_out_u16(port: i64, value: i64) {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") port as u16,
            in("ax") value as u16,
            options(nostack, preserves_flags),
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = (port, value);
        unsupported("port_out_u16 requires x86");
    }
}

/// # Safety
///
/// As [`lkrt_port_in_u8`], for a 32-bit write.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_port_out_u32(port: i64, value: i64) {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port as u16,
            in("eax") value as u32,
            options(nostack, preserves_flags),
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = (port, value);
        unsupported("port_out_u32 requires x86");
    }
}
