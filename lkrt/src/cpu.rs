//! CPU-level operations that have no expression in LK: memory barriers,
//! interrupt masking, and waiting for an interrupt.
//!
//! These are intrinsics rather than an `asm!` construct in the language.
//! Everything a driver or kernel actually needs from assembly is a *fixed
//! instruction sequence with no operands* — a barrier, a mask, a wait. Naming
//! each one gives it a checkable signature and a portable meaning, where a
//! general `asm!` would need a template parser, operand constraints, register
//! allocation and an assembler in the build. The bodies below are ordinary
//! Rust, so where assembly is genuinely required it is written here, once per
//! architecture, and reviewed like any other code.
//!
//! What is deliberately *not* here: reading and writing arbitrary system
//! registers. Those take a register name as a compile-time operand, which is
//! the one thing this shape cannot express, and they are the point at which a
//! real `asm!` would start to earn its cost.

use core::sync::atomic::{Ordering, compiler_fence, fence};

/// A full memory barrier: no access may be reordered across this point, by the
/// compiler or by the CPU.
///
/// This is what belongs between a device write and a subsequent read that
/// depends on it. `volatile_*` alone does not order accesses against
/// *non*-volatile ones, and it says nothing about the CPU's store buffer.
///
/// No `asm!` needed: `fence(SeqCst)` already lowers to `mfence` on x86-64 and
/// `dmb ish` on aarch64. Writing the instruction by hand would be less
/// portable and no more correct.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_barrier() {
    fence(Ordering::SeqCst);
}

/// A compiler-only barrier: forbids the compiler from moving accesses across
/// this point, but emits no instruction.
///
/// Correct on a uniprocessor where the ordering only has to hold against
/// interrupt handlers on the same core, and cheaper than a full barrier there.
/// Wrong for ordering against another core or a bus master.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_compiler_barrier() {
    compiler_fence(Ordering::SeqCst);
}

/// Masks interrupts on the current core, returning the previous state so a
/// nested critical section can restore rather than blindly re-enable.
///
/// Returns 1 if interrupts were previously enabled, 0 if already masked.
///
/// # Safety
///
/// Requires privilege (ring 0 / EL1). Under a hosted OS this traps; it is
/// meaningful only in a kernel or on bare metal.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_irq_save() -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        let flags: u64;
        unsafe {
            core::arch::asm!("pushfq", "pop {}", "cli", out(reg) flags, options(nomem, preserves_flags));
        }
        // IF is bit 9 of RFLAGS.
        i64::from(flags & (1 << 9) != 0)
    }
    #[cfg(target_arch = "aarch64")]
    {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif, options(nomem, nostack));
        }
        // I is bit 7 of DAIF; set means *masked*, so enabled is the inverse.
        i64::from(daif & (1 << 7) == 0)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        // No portable spelling. Reporting "were enabled" would be a lie a
        // later restore would then act on.
        unimplemented!("interrupt masking is not implemented for this architecture")
    }
}

/// Restores the interrupt state returned by [`lkrt_cpu_irq_save`].
///
/// # Safety
///
/// As [`lkrt_cpu_irq_save`]. Passing a value that did not come from it will
/// enable or mask interrupts against the caller's intent.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_irq_restore(was_enabled: i64) {
    if was_enabled == 0 {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        unimplemented!("interrupt masking is not implemented for this architecture")
    }
}

/// Parks the core until an interrupt arrives.
///
/// An idle loop should call this rather than spinning: spinning burns power
/// and, on a hyperthreaded or virtualised core, starves the sibling.
///
/// # Safety
///
/// Requires privilege. With interrupts masked and no pending interrupt this
/// never returns, which is a hang rather than a crash.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_wait_for_interrupt() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        unimplemented!("wait-for-interrupt is not implemented for this architecture")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The barriers are callable and emit no observable effect on their own.
    /// There is little to assert beyond that — a barrier's whole content is
    /// what it forbids, which shows up in *other* code's ordering.
    #[test]
    fn barriers_are_callable() {
        lkrt_cpu_barrier();
        lkrt_cpu_compiler_barrier();
    }

    /// A restore of "was masked" must not enable interrupts. This is the case
    /// that makes nesting safe, and the one a naive `irq_enable()` gets wrong.
    #[test]
    fn restoring_a_masked_state_is_a_no_op() {
        // Safe to call under a hosted OS precisely because it does nothing.
        lkrt_cpu_irq_restore(0);
    }
}

/// A monotonically increasing count of core cycles.
///
/// `WritesHost` in the ABI table, like the MMIO reads: two reads of a clock
/// legitimately differ, so collapsing them would turn a measurement into zero.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_timestamp() -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        let low: u32;
        let high: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack));
        }
        ((u64::from(high) << 32) | u64::from(low)) as i64
    }
    #[cfg(target_arch = "aarch64")]
    {
        let count: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) count, options(nomem, nostack));
        }
        count as i64
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        unimplemented!("cpu_timestamp is not implemented for this architecture")
    }
}
