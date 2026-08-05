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
                            options(nostack, preserves_flags),
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
                            options(nostack, preserves_flags),
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

/// The system-control instructions: descriptor tables, CR2/CR3, the TLB.
///
/// Gated exactly like port I/O, and for the same reason stated at the top of
/// this file: the bare-metal x86 kernel *hosts this interpreter*, and a program
/// it loads off a disk reaches the same builtins the compiled kernel does.
/// Answering "unsupported" on the one architecture the machine actually is
/// would mean LK could describe a kernel but never run one on the only backend
/// that reaches the hardware.
///
/// These duplicate the bodies in `lkrt/src/system.rs`, as `cpu_irq_save` here
/// already duplicates `lkrt/src/cpu.rs`. The two crates cannot share them:
/// `lkrt` must not depend on `lk-core`, and `lk-core` depending on `lkrt` would
/// close the loop the other way. What keeps the copies honest is that they are
/// each three lines of assembly with the instruction named in the function name.
#[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
mod system {
    /// The operand `lidt`/`lgdt` take: a limit and a base, packed. Built here
    /// rather than by the caller — the layout is `#[repr(packed)]`, which no LK
    /// type describes, and the CPU reads it only during the instruction.
    #[repr(C, packed)]
    pub(super) struct PseudoDescriptor {
        pub(super) limit: u16,
        pub(super) base: u64,
    }
}

/// One operand as a machine word. Gated with the instructions that read it —
/// on a hosted build every caller is compiled out, and CI builds with
/// `-D warnings`.
#[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
fn word_operand(args: &NativeArgs<'_>, index: usize, name: &str) -> Result<i64> {
    match args.get(index) {
        Some(RuntimeVal::Int(value)) => Ok(*value),
        _ => Err(anyhow!("{name} expects an integer as argument {}", index + 1)),
    }
}

fn system_refusal(name: &str) -> anyhow::Error {
    anyhow!("{name} requires bare-metal execution on x86-64: no other target has this instruction")
}

pub(super) fn cpu_load_idt(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let descriptor = system::PseudoDescriptor {
            base: word_operand(&_args, 0, "cpu_load_idt")? as u64,
            limit: word_operand(&_args, 1, "cpu_load_idt")? as u16,
        };
        unsafe {
            core::arch::asm!("lidt [{}]", in(reg) &descriptor, options(preserves_flags));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_load_idt"))
}

pub(super) fn cpu_load_gdt(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let descriptor = system::PseudoDescriptor {
            base: word_operand(&_args, 0, "cpu_load_gdt")? as u64,
            limit: word_operand(&_args, 1, "cpu_load_gdt")? as u16,
        };
        unsafe {
            core::arch::asm!("lgdt [{}]", in(reg) &descriptor, options(preserves_flags));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_load_gdt"))
}

/// Reloads CS and the data segments — the half of a GDT load that `lgdt` does
/// not do, because the segment registers hold cached descriptors.
pub(super) fn cpu_reload_segments(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let code = word_operand(&_args, 0, "cpu_reload_segments")? as u64;
        let data = word_operand(&_args, 1, "cpu_reload_segments")? as u64;
        unsafe {
            // A far return, because CS cannot be written by `mov`: push the
            // selector and the address to continue at, and `retfq` loads both.
            // FS and GS are left alone — writing either zeroes its base.
            core::arch::asm!(
                "push {code}",
                "lea {tmp}, [rip + 2f]",
                "push {tmp}",
                "retfq",
                "2:",
                "mov ds, {data:x}",
                "mov es, {data:x}",
                "mov ss, {data:x}",
                code = in(reg) code,
                data = in(reg) data,
                tmp = lateout(reg) _,
            );
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_reload_segments"))
}

pub(super) fn cpu_load_task_register(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let selector = word_operand(&_args, 0, "cpu_load_task_register")? as u16;
        unsafe {
            core::arch::asm!("ltr {0:x}", in(reg) selector, options(nostack, preserves_flags));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_load_task_register"))
}

/// The address whose access caused the last page fault. Only the CPU writes it.
pub(super) fn cpu_read_cr2(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let value: u64;
        unsafe {
            core::arch::asm!("mov {}, cr2", out(reg) value, options(nostack, preserves_flags));
        }
        return Ok(RuntimeVal::Int(value as i64));
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_read_cr2"))
}

pub(super) fn cpu_read_cr3(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let value: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) value, options(nostack, preserves_flags));
        }
        return Ok(RuntimeVal::Int(value as i64));
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_read_cr3"))
}

/// Raises a software interrupt, whatever its number is.
///
/// The one x86 instruction whose operand a program cannot supply: `int` takes
/// its vector as an immediate, so a kernel that wants to raise a vector it
/// computed has nowhere to put it. The runtime answers that with a table of 256
/// stubs — see `lkrt/src/isr.rs`, which does the same thing for the entry side —
/// and this is the interpreter reaching the same table.
///
/// Without it a kernel written in this language cannot raise its own syscall or
/// reschedule vector, which is not a small gap: it is the difference between
/// defining an interrupt and merely handling one.
/// The symbol above, for a **test** binary.
///
/// `lk-core`'s `no_std` face declares `lkrt_cpu_raise_interrupt` and does not
/// depend on the crate that defines it — sound in the bare-metal image, where
/// both are linked together, and unlinkable in a host test binary, where only
/// one of them is. `cargo test -p lk-core --no-default-features` therefore
/// could not link on x86_64 at all:
///
/// ```text
/// rust-lld: error: undefined symbol: lkrt_cpu_raise_interrupt
/// ```
///
/// That is a CI step (`check.yml`, "lk-core builds and *tests* as no_std") and
/// a documented gate. `cargo build` with the same flags is green, because a
/// library has no link step — which is why running the build in its place hid
/// this.
///
/// A stub rather than a `cfg(test)` arm inside the function: the shipped code
/// then stays the code the tests compile. Raising an interrupt from a host test
/// process is not a thing to do, so it does nothing.
#[cfg(all(test, not(feature = "std"), target_arch = "x86_64"))]
#[unsafe(no_mangle)]
extern "C" fn lkrt_cpu_raise_interrupt(_vector: i64) {}

pub(super) fn cpu_raise_interrupt(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        // Declared, not depended on. `lk-core` must not have `lkrt` as a crate
        // dependency — that boundary is what keeps the runtime free of the
        // parser and the compiler — but on the one target where this means
        // anything, both are linked into the same image and the symbol is simply
        // there. A link-time reference is not an architectural edge.
        unsafe extern "C" {
            fn lkrt_cpu_raise_interrupt(vector: i64);
        }
        let vector = word_operand(&_args, 0, "cpu_raise_interrupt")?;
        // SAFETY: the vector is bounds-checked inside, and a vector with no gate
        // faults exactly as it would if a device had raised it.
        unsafe { lkrt_cpu_raise_interrupt(vector as i64) };
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_raise_interrupt"))
}

/// Switches address spaces, flushing the TLB in doing so. The code after it
/// must be mapped in the new space at the same address — which is why a kernel
/// is mapped into every one.
pub(super) fn cpu_write_cr3(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let value = word_operand(&_args, 0, "cpu_write_cr3")? as u64;
        // No `nomem`: this invalidates every cached translation, so it orders
        // against essentially all memory.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_write_cr3"))
}

/// Drops one page's cached translation. The page table is not what the CPU
/// consults — the TLB is, and it does not notice a write behind it.
pub(super) fn cpu_invalidate_page(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let address = word_operand(&_args, 0, "cpu_invalidate_page")? as u64;
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) address, options(preserves_flags));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(system_refusal("cpu_invalidate_page"))
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
            core::arch::asm!("mrs {}, primask", "cpsid i", out(reg) primask, options(nostack));
        }
        // PRIMASK bit 0 set means interrupts are *masked*.
        return Ok(RuntimeVal::Int(i64::from(primask & 1 == 0)));
    }
    #[cfg(all(not(feature = "std"), target_arch = "aarch64"))]
    {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif, options(nostack));
        }
        // DAIF bit 7 (I) set means masked.
        return Ok(RuntimeVal::Int(i64::from(daif & (1 << 7) == 0)));
    }
    // x86 keeps the flag in RFLAGS, which is only reachable through the stack:
    // there is no "read interrupt flag" instruction. Needed here because the
    // x86-64 kernel now *hosts* this VM — a program it runs off a disk reaches
    // the same builtins the compiled kernel does, and answering "unsupported
    // architecture" on the one architecture the machine is would be absurd.
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let flags: u64;
        unsafe {
            // No `nostack`: `pushfq` and `pop` are exactly a write to and a
            // read from the stack. A refactor that adds it "for consistency"
            // with the ARM arms below is telling the compiler something false
            // about a sequence it may then schedule a red-zone access into.
            core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags);
        }
        // IF is bit 9, and set means *enabled* — the opposite sense from ARM's
        // mask bits, which is why each architecture computes the answer rather
        // than sharing one expression.
        return Ok(RuntimeVal::Int(i64::from(flags & (1 << 9) != 0)));
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
                core::arch::asm!("cpsie i", options(nostack));
            }
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("msr daifclr, #2", options(nostack));
            }
        }
        return Ok(RuntimeVal::Nil);
    }
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        let was_enabled = matches!(_args.get(0), Some(RuntimeVal::Int(value)) if *value != 0);
        if was_enabled {
            unsafe {
                core::arch::asm!("sti", options(nostack));
            }
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(anyhow!(
        "interrupt masking requires bare-metal execution on a supported architecture"
    ))
}

/// The address of an `#[export]`ed function, as an integer.
///
/// A driver table is an array of these. A kernel dispatches through one for
/// interrupt vectors, device operations, per-window repaint — and the
/// alternative in a language without function pointers is a chain of `if`s that
/// has to be edited every time a device is added.
///
/// The VM refuses rather than inventing an answer: an interpreter has no code
/// addresses to give out, and returning a fake one would produce a program that
/// runs under the VM and jumps into nothing when compiled. Refusing is the same
/// choice `port_in_u8` makes, for the same reason.
pub(super) fn symbol_address(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    Err(anyhow!(
        "symbol_address requires native compilation: the VM has no code addresses to hand out"
    ))
}

/// Calls through an address, with two integer arguments.
///
/// The other half of a driver table. Two arguments because that is what the
/// callers here need and every argument count is a separate signature at the
/// machine level; more can be added when something wants them.
pub(super) fn call_address_2(_args: NativeArgs<'_>) -> Result<RuntimeVal> {
    Err(anyhow!(
        "call_address_2 requires native compilation: the VM cannot call through an address"
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
            core::arch::asm!("rdtsc", out("eax") low, out("edx") high, options(nostack));
        }
        return Ok(RuntimeVal::Int(((u64::from(high) << 32) | u64::from(low)) as i64));
    }
    #[cfg(all(not(feature = "std"), target_arch = "aarch64"))]
    {
        let count: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) count, options(nostack));
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
            core::arch::asm!("wfi", options(nostack));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
    {
        unsafe {
            core::arch::asm!("hlt", options(nostack));
        }
        return Ok(RuntimeVal::Nil);
    }
    #[allow(unreachable_code)]
    Err(anyhow!(
        "wait-for-interrupt requires bare-metal execution on a supported architecture"
    ))
}
