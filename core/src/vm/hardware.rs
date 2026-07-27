//! The hardware-touching intrinsics: volatile MMIO and CPU control.
//!
//! This is the one file under `vm/` allowed to contain `unsafe`, and the
//! migration guard names it explicitly. The rule it is excepted from — "VM and
//! value code stays safe Rust" — exists because a memory error in the
//! interpreter is unfindable. That reasoning does not reach here: touching a
//! device register *is* the operation, and no safe spelling of it exists. Given
//! the exception has to exist, it is better as one small file with a name that
//! says what it holds than as `unsafe` scattered through the executor.
//!
//! # Why these are feature-gated rather than always refused
//!
//! Whether an address means anything depends on where the interpreter itself is
//! running. Hosted, a raw address belongs to another allocation or to nothing,
//! and touching it is a bug — so these raise. On bare metal the interpreter is
//! what runs on the hardware, the address space is real, and the access is
//! performed.
//!
//! That distinction is load-bearing: the bare-metal image runs the VM. Refusing
//! unconditionally would mean LK could *describe* a driver but never run one on
//! the only backend that reaches the hardware.

use crate::val::RuntimeVal;
use crate::vm::NativeArgs;
use anyhow::{Result, anyhow};

/// The address operand as a `usize`. A pointer is carried as an `Int`.
fn address(args: &NativeArgs<'_>, name: &str) -> Result<usize> {
    match args.get(0) {
        Some(RuntimeVal::Int(addr)) => Ok(*addr as usize),
        _ => Err(anyhow!("{name} expects a pointer as its first argument")),
    }
}

fn value_operand(args: &NativeArgs<'_>, name: &str) -> Result<i64> {
    match args.get(1) {
        Some(RuntimeVal::Int(value)) => Ok(*value),
        _ => Err(anyhow!("{name} expects an integer value")),
    }
}

#[cfg(feature = "std")]
fn hosted_refusal(name: &str) -> anyhow::Error {
    anyhow!(
        "{name} requires bare-metal execution: a hosted process has no device at a raw address. \
         Build for a no_std target, or compile with the AOT backend."
    )
}

macro_rules! volatile_access {
    ($($read:ident, $write:ident, $ty:ty;)+) => {
        $(
            pub(super) fn $read(args: NativeArgs<'_>) -> Result<RuntimeVal> {
                let _addr = address(&args, stringify!($read))?;
                #[cfg(feature = "std")]
                {
                    Err(hosted_refusal(stringify!($read)))
                }
                #[cfg(not(feature = "std"))]
                {
                    // That the address is mapped and aligned is the caller's
                    // claim, made by writing `unsafe` in LK. Nothing here can
                    // check it.
                    let value = unsafe { core::ptr::read_volatile(_addr as *const $ty) };
                    Ok(RuntimeVal::Int(value as i64))
                }
            }

            pub(super) fn $write(args: NativeArgs<'_>) -> Result<RuntimeVal> {
                let _addr = address(&args, stringify!($write))?;
                let _value = value_operand(&args, stringify!($write))?;
                #[cfg(feature = "std")]
                {
                    Err(hosted_refusal(stringify!($write)))
                }
                #[cfg(not(feature = "std"))]
                {
                    unsafe { core::ptr::write_volatile(_addr as *mut $ty, _value as $ty) };
                    Ok(RuntimeVal::Nil)
                }
            }
        )+
    };
}

volatile_access! {
    volatile_read_u8, volatile_write_u8, u8;
    volatile_read_u16, volatile_write_u16, u16;
    volatile_read_u32, volatile_write_u32, u32;
    volatile_read_u64, volatile_write_u64, u64;
}

/// x86 port I/O.
///
/// A second address space, reached by `in`/`out` rather than by a load or a
/// store. Gated on both bare metal *and* x86: other architectures have no such
/// instructions at all, so unlike the volatile intrinsics there is nothing to
/// perform even on a board — a program using these is x86 code.
macro_rules! port_access {
    ($($read:ident, $write:ident, $ty:ty, $reg:tt;)+) => {
        $(
            pub(super) fn $read(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
                #[cfg(all(not(feature = "std"), any(target_arch = "x86_64", target_arch = "x86")))]
                {
                    let port = port_operand(&_args, stringify!($read))?;
                    let value: $ty;
                    // What device answers at this port, and what reading it
                    // does, is the caller's claim — made by writing `unsafe`.
                    unsafe {
                        core::arch::asm!(
                            concat!("in ", $reg, ", dx"),
                            out($reg) value,
                            in("dx") port,
                            options(nomem, nostack, preserves_flags),
                        );
                    }
                    return Ok(RuntimeVal::Int(value as i64));
                }
                #[allow(unreachable_code)]
                Err(port_refusal(stringify!($read)))
            }

            pub(super) fn $write(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
                #[cfg(all(not(feature = "std"), any(target_arch = "x86_64", target_arch = "x86")))]
                {
                    let port = port_operand(&_args, stringify!($write))?;
                    let value = value_operand(&_args, stringify!($write))? as $ty;
                    unsafe {
                        core::arch::asm!(
                            concat!("out dx, ", $reg),
                            in("dx") port,
                            in($reg) value,
                            options(nomem, nostack, preserves_flags),
                        );
                    }
                    return Ok(RuntimeVal::Nil);
                }
                #[allow(unreachable_code)]
                Err(port_refusal(stringify!($write)))
            }
        )+
    };
}

/// The port operand as the 16 bits `in`/`out` actually address.
#[cfg(all(not(feature = "std"), any(target_arch = "x86_64", target_arch = "x86")))]
fn port_operand(args: &NativeArgs<'_>, name: &str) -> Result<u16> {
    match args.get(0) {
        Some(RuntimeVal::Int(port)) => Ok(*port as u16),
        _ => Err(anyhow!("{name} expects a port number as its first argument")),
    }
}

fn port_refusal(name: &str) -> anyhow::Error {
    anyhow!("{name} requires bare-metal execution on x86: no other architecture has port I/O")
}

port_access! {
    port_in_u8, port_out_u8, u8, "al";
    port_in_u16, port_out_u16, u16, "ax";
    port_in_u32, port_out_u32, u32, "eax";
}

/// A full memory barrier.
///
/// `fence(SeqCst)` rather than hand-written assembly: it is `mfence` on x86-64,
/// `dmb ish` on aarch64 and `dmb sy` on Cortex-M, and writing any of those by
/// hand would be less portable without being more correct.
pub(super) fn cpu_barrier(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(feature = "std")]
    {
        Err(anyhow!(
            "cpu_barrier requires bare-metal or native execution; a hosted interpreter has no core \
             to apply it to."
        ))
    }
    #[cfg(not(feature = "std"))]
    {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        Ok(RuntimeVal::Nil)
    }
}

pub(super) fn cpu_compiler_barrier(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(feature = "std")]
    {
        Err(anyhow!(
            "cpu_compiler_barrier requires bare-metal or native execution; a hosted interpreter \
             has no core to apply it to."
        ))
    }
    #[cfg(not(feature = "std"))]
    {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        Ok(RuntimeVal::Nil)
    }
}

/// Masks interrupts, reporting whether they were previously enabled.
///
/// The previous state is returned rather than offering a bare `enable` so that
/// nested critical sections restore instead of unconditionally re-enabling —
/// the case a naive `irq_enable()` gets wrong.
///
/// An architecture whose masking instruction is not known here raises rather
/// than reporting a state a later restore would act on.
pub(super) fn cpu_irq_save(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "arm"))]
    {
        let primask: u32;
        unsafe {
            core::arch::asm!("mrs {}, primask", "cpsid i", out(reg) primask, options(nomem, nostack));
        }
        // PRIMASK bit 0 set means interrupts are *masked*.
        return Ok(RuntimeVal::Int(i64::from(primask & 1 == 0)));
    }
    #[cfg(all(not(feature = "std"), target_arch = "aarch64"))]
    {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif, options(nomem, nostack));
        }
        // DAIF bit 7 (I) set means masked.
        return Ok(RuntimeVal::Int(i64::from(daif & (1 << 7) == 0)));
    }
    #[allow(unreachable_code)]
    Err(anyhow!(
        "interrupt masking requires bare-metal execution on a supported architecture"
    ))
}

pub(super) fn cpu_irq_restore(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), any(target_arch = "arm", target_arch = "aarch64")))]
    {
        let was_enabled = matches!(_args.get(0), Some(RuntimeVal::Int(value)) if *value != 0);
        if was_enabled {
            #[cfg(target_arch = "arm")]
            unsafe {
                core::arch::asm!("cpsie i", options(nomem, nostack));
            }
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
            }
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(anyhow!(
        "interrupt masking requires bare-metal execution on a supported architecture"
    ))
}

/// A monotonically increasing count of core cycles.
///
/// For measuring, which a kernel needs before it can honestly claim anything
/// got faster. The unit is whatever the core counts in — comparable with
/// itself, not across machines, which is exactly what a before/after needs.
pub(super) fn cpu_timestamp(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let low: u32;
        let high: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack));
        }
        return Ok(RuntimeVal::Int(((u64::from(high) << 32) | u64::from(low)) as i64));
    }
    #[cfg(all(not(feature = "std"), target_arch = "aarch64"))]
    {
        let count: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) count, options(nomem, nostack));
        }
        return Ok(RuntimeVal::Int(count as i64));
    }
    #[allow(unreachable_code)]
    Err(anyhow!(
        "cpu_timestamp requires bare-metal execution on a supported architecture"
    ))
}

/// Parks the core until an interrupt arrives.
///
/// An idle loop should call this rather than spinning: spinning burns power
/// and, on a virtualised core, starves the sibling.
pub(super) fn cpu_wait_for_interrupt(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), any(target_arch = "arm", target_arch = "aarch64")))]
    {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(anyhow!(
        "wait-for-interrupt requires bare-metal execution on a supported architecture"
    ))
}
