//! The system-control instructions: descriptor tables, control registers, and
//! the TLB.
//!
//! Split from `cpu.rs` rather than added to it because the two files answer to
//! different rules. `cpu.rs` holds operations every architecture has some
//! spelling of — a barrier, an interrupt mask, a wait — and each is implemented
//! per architecture. Nothing here has a portable meaning at all: an IDT is an
//! x86 concept, and aarch64's equivalent is a vector *base* register with a
//! fixed layout, not a table of gates. So these are gated like `port.rs` is,
//! and raise elsewhere rather than pretending.
//!
//! # Why these are named instructions and not an `asm!` construct
//!
//! `cpu.rs` says the line is drawn at operations that are "a fixed instruction
//! sequence with no operands", and that reading and writing arbitrary system
//! registers is out because the register name is a *compile-time* operand — the
//! one thing that shape cannot express. That line is intact here, and it is
//! worth restating because these look at first glance like the thing it
//! excluded. They are not. Each entry below names one register or one table:
//! `cpu_read_cr3` is as specific an operation as `cpu_irq_save`, with a
//! signature a type checker can state. What remains excluded is
//! `read_system_register(name)`, where the operand decides which instruction is
//! emitted — that still needs an assembler in the build, and still is not here.
//!
//! # What each one exists for
//!
//! Nothing here is speculative; each has a caller in the bare-metal kernel that
//! is otherwise forced to be Rust:
//!
//! | intrinsic | the thing it unblocks |
//! | --- | --- |
//! | `cpu_load_idt` | LK building its own interrupt table |
//! | `cpu_load_gdt` + `cpu_reload_segments` | LK building its own GDT |
//! | `cpu_load_task_register` | the TSS, and with it ring 3 |
//! | `cpu_read_cr2` | which address faulted, in a page-fault handler |
//! | `cpu_read_cr3` / `cpu_write_cr3` | switching address spaces |
//! | `cpu_invalidate_page` | changing a mapping that is already live |
//!
//! Deliberately absent: `rdmsr`/`wrmsr`, and CR0/CR4. Paging and protection are
//! already on by the time any LK code runs, and no caller wants an MSR yet.
//! They are one entry each to add on the day something does.
//!
//! All entries are `WritesHost` in the ABI table, the reads included. `cr2`
//! changes behind the code's back — that is its entire purpose — so collapsing
//! two reads of it would report the first fault's address for the second.

#[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
fn unsupported(name: &str) -> ! {
    crate::panic::raise_str(name);
}

/// The operand `lidt` and `lgdt` take: a limit and a base, packed with no
/// padding between them.
///
/// Built here rather than by the caller for two reasons. The layout is
/// `#[repr(packed)]` — a 16-bit field immediately followed by a 64-bit one,
/// which no LK type describes — and the CPU reads it *during* the instruction
/// and never again, so the natural place for it is a stack temporary whose
/// lifetime is exactly the call. A caller-supplied address would be a lifetime
/// nothing checks, in service of hiding nothing.
#[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
#[repr(C, packed)]
struct PseudoDescriptor {
    limit: u16,
    base: u64,
}

/// Points the CPU at an interrupt descriptor table.
///
/// `limit` is the table's size in bytes *minus one*, which is what the hardware
/// field holds: a limit of 0 means one addressable byte, so a 256-gate table is
/// `256 * 16 - 1`. Passed as the caller states it rather than as a gate count,
/// because that is the number the manual talks about and translating it here
/// would make one of the two spellings wrong at every call site.
///
/// # Safety
///
/// `base` must point at a correctly formed table that stays alive for as long
/// as it is loaded. A malformed gate is not a fault the kernel can report — the
/// CPU triple-faults trying to report it, and the machine resets.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_load_idt(base: i64, limit: i64) {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        let descriptor = PseudoDescriptor {
            limit: limit as u16,
            base: base as u64,
        };
        // No `nostack`: the operand *is* a stack temporary, and the address of
        // it is what the instruction reads. No `nomem` for the same reason —
        // with it the compiler would be free to leave the two fields
        // unwritten, since nothing else reads them.
        unsafe {
            core::arch::asm!("lidt [{}]", in(reg) &descriptor, options(preserves_flags));
        }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = (base, limit);
        unsupported("cpu_load_idt requires x86");
    }
}

/// Points the CPU at a global descriptor table.
///
/// **Not sufficient on its own.** The segment registers hold *cached*
/// descriptors, loaded when each was last written; `lgdt` changes the table
/// they came from and nothing else. Until [`lkrt_cpu_reload_segments`] runs,
/// the CPU is still using the descriptors from the old table — which is fine
/// while the entries agree and silently wrong the moment they do not.
///
/// # Safety
///
/// As [`lkrt_cpu_load_idt`]. Additionally, entry 0 must be null and the
/// currently cached selectors must remain valid in the new table until they are
/// reloaded.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_load_gdt(base: i64, limit: i64) {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        let descriptor = PseudoDescriptor {
            limit: limit as u16,
            base: base as u64,
        };
        unsafe {
            core::arch::asm!("lgdt [{}]", in(reg) &descriptor, options(preserves_flags));
        }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        let _ = (base, limit);
        unsupported("cpu_load_gdt requires x86");
    }
}

/// Reloads CS and the data segments from the current GDT.
///
/// The other half of `lgdt`. CS cannot be written by `mov` — the only ways to
/// change it are a far jump, a far call, a far return, or an interrupt return.
/// A far return is used here because it needs nothing but the stack: push the
/// new selector and the address to continue at, and `retfq` loads both.
///
/// FS and GS are left alone, and that is not an oversight. Writing either
/// zeroes its 64-bit base, which on a kernel that keeps per-CPU state there
/// would silently point every access at address 0. A kernel that wants them
/// reloaded knows it, and can say so.
///
/// # Safety
///
/// `code` must select a present, executable, 64-bit code segment and `data` a
/// present, writable data segment, both in the table currently loaded. A wrong
/// selector faults on the `retfq` itself, at which point CS is already whatever
/// the fault handler's gate says — so this is not a failure a handler can
/// report usefully.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_reload_segments(code: i64, data: i64) {
    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            core::arch::asm!(
                // The far return's frame: the target selector under the target
                // address, popped as CS:RIP.
                "push {code}",
                "lea {tmp}, [rip + 2f]",
                "push {tmp}",
                "retfq",
                "2:",
                // SS last of the three: `mov ss` blocks interrupts for exactly
                // one instruction, which is the window a stack switch needs.
                // Here there is no switch — the stack pointer is unchanged —
                // so the ordering is only about keeping that property true if
                // one is ever added.
                "mov ds, {data:x}",
                "mov es, {data:x}",
                "mov ss, {data:x}",
                code = in(reg) code as u64,
                data = in(reg) data as u64,
                tmp = lateout(reg) _,
            );
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (code, data);
        unsupported("cpu_reload_segments requires x86-64");
    }
}

/// Loads the task register with a TSS selector.
///
/// In long mode the TSS holds no saved registers — hardware task switching is
/// gone — and exists for one field the CPU still reads: the ring-0 stack it
/// switches to when an interrupt arrives from ring 3. Without this, ring 3 is
/// not reachable at all; the first interrupt after entering it would push its
/// frame onto the *user's* stack.
///
/// # Safety
///
/// `selector` must name a present 64-bit TSS descriptor (type 9) in the current
/// GDT, and the segment it names must stay mapped. Loading a busy TSS
/// descriptor faults.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_load_task_register(selector: i64) {
    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            core::arch::asm!("ltr {0:x}", in(reg) selector as u16, options(nostack, preserves_flags));
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = selector;
        unsupported("cpu_load_task_register requires x86-64");
    }
}

/// The address whose access caused the last page fault.
///
/// The CPU writes this on every `#PF` and nothing else does. A handler that
/// wants to say *what* was touched has no other source for it — the faulting
/// instruction's operand is not on the stack.
///
/// # Safety
///
/// Requires ring 0. The value is only meaningful inside a page-fault handler,
/// before the next fault overwrites it.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_read_cr2() -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        let value: u64;
        unsafe {
            core::arch::asm!("mov {}, cr2", out(reg) value, options(nostack, preserves_flags));
        }
        value as i64
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        unsupported("cpu_read_cr2 requires x86-64");
    }
}

/// The physical address of the current address space's top-level page table.
///
/// # Safety
///
/// Requires ring 0.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_read_cr3() -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        let value: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) value, options(nostack, preserves_flags));
        }
        value as i64
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        unsupported("cpu_read_cr3 requires x86-64");
    }
}

/// Switches address spaces, and flushes the TLB in doing so.
///
/// This is the whole difference between a thread and a process, as one
/// instruction: every virtual address means something different afterwards.
///
/// # Safety
///
/// `value` must be the physical address of a valid PML4, and the code that runs
/// after this instruction must be mapped *in the new space at the same address*
/// — including the return path out of here. A kernel mapped into every address
/// space is what makes that true; a kernel mapped into only some of them makes
/// this instruction the last one that runs.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_write_cr3(value: i64) {
    #[cfg(target_arch = "x86_64")]
    {
        // No `nomem`: this invalidates every cached translation, so it orders
        // against essentially all memory. Claiming otherwise would let the
        // compiler hoist a load of the new space's memory above the switch,
        // where it would read the old space.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) value as u64, options(nostack, preserves_flags));
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = value;
        unsupported("cpu_write_cr3 requires x86-64");
    }
}

/// Drops one page's cached translation.
///
/// Needed because a page table is not the thing the CPU consults — the TLB is,
/// and it does not notice a write to the table behind it. Changing a mapping
/// that was already used and *not* calling this leaves the old translation
/// live, for an unbounded time and only on the cores that cached it, which is
/// as hard a bug as this layer produces.
///
/// One page rather than the whole TLB: reloading CR3 also flushes, and is the
/// blunt version. `invlpg` is the one to reach for when a single mapping
/// changed.
///
/// # Safety
///
/// Requires ring 0. `address` is a *virtual* address, and any address in the
/// page selects it.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_cpu_invalidate_page(address: i64) {
    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) address as u64, options(preserves_flags));
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = address;
        unsupported("cpu_invalidate_page requires x86-64");
    }
}

#[cfg(test)]
mod tests {
    /// The reads are the only entries safe to call from a hosted test — they
    /// take no operand and change nothing — and even they need ring 0, which a
    /// test process does not have. So what is asserted here is the thing that
    /// can be: that the pseudo-descriptor the loads build has the layout the
    /// hardware reads, ten bytes with no padding between the limit and the
    /// base.
    ///
    /// Worth a test because the failure is silent in the worst way: with
    /// natural alignment the base would sit at offset 8, the CPU would read six
    /// bytes of padding and the low two bytes of the base as the address, and
    /// load a table from somewhere near zero.
    #[test]
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    fn pseudo_descriptor_is_packed() {
        assert_eq!(core::mem::size_of::<super::PseudoDescriptor>(), 10);
        assert_eq!(core::mem::align_of::<super::PseudoDescriptor>(), 1);
    }
}
