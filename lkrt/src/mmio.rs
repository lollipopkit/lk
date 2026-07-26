//! Volatile memory-mapped I/O.
//!
//! These exist as *calls* rather than inline loads and stores for one reason:
//! Cranelift has no volatile flag. Its `MemFlags` can say `notrap`, `aligned`,
//! `readonly`, `can_move` and an alias region, and none of those means "this
//! access must happen exactly as written". `can_move` only forbids moving an
//! access; it does not stop the egraph pass proving two loads of one address
//! equal and keeping a single one.
//!
//! That was measured, not assumed. Lowering `volatile_read_u32(p)` twice to
//! inline loads produced one `mov (%rdi),%esi` followed by `lea (%rsi,%rsi,1)`
//! — the second read gone and `a + b` folded to `a * 2`. For a device register,
//! whose two reads can legitimately differ and whose reads can have side
//! effects, that is a miscompile.
//!
//! An opaque call cannot be collapsed that way, and the bodies below use
//! `read_volatile`/`write_volatile`, where Rust guarantees the access happens.
//! The cost is one call per access — negligible beside the tens to hundreds of
//! nanoseconds an MMIO access takes in hardware.
//!
//! All entries are `WritesHost` in the ABI table, including the reads: the
//! effect annotation drives CSE, and a read that may change device state is not
//! pure no matter what it returns.

/// # Safety
///
/// `addr` must be a valid, mapped address for an aligned `u8` access. Nothing
/// here can check that — it is the claim the caller makes by writing `unsafe`
/// in LK.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_read_u8(addr: i64) -> i64 {
    unsafe { core::ptr::read_volatile(addr as usize as *const u8) as i64 }
}

/// # Safety
///
/// As [`lkrt_mmio_read_u8`], for a `u16` access.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_read_u16(addr: i64) -> i64 {
    unsafe { core::ptr::read_volatile(addr as usize as *const u16) as i64 }
}

/// # Safety
///
/// As [`lkrt_mmio_read_u8`], for a `u32` access.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_read_u32(addr: i64) -> i64 {
    unsafe { core::ptr::read_volatile(addr as usize as *const u32) as i64 }
}

/// # Safety
///
/// As [`lkrt_mmio_read_u8`], for a `u64` access.
///
/// The result is reinterpreted rather than range-checked: a 64-bit register
/// with its top bit set reads back as a negative `i64`, which is the same bit
/// pattern the VM's carrier holds.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_read_u64(addr: i64) -> i64 {
    unsafe { core::ptr::read_volatile(addr as usize as *const u64) as i64 }
}

/// # Safety
///
/// `addr` must be a valid, mapped, writable address for an aligned `u8` access.
///
/// `value` is truncated to the access width, matching what the LK type checker
/// already required of the argument.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_write_u8(addr: i64, value: i64) {
    unsafe { core::ptr::write_volatile(addr as usize as *mut u8, value as u8) }
}

/// # Safety
///
/// As [`lkrt_mmio_write_u8`], for a `u16` access.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_write_u16(addr: i64, value: i64) {
    unsafe { core::ptr::write_volatile(addr as usize as *mut u16, value as u16) }
}

/// # Safety
///
/// As [`lkrt_mmio_write_u8`], for a `u32` access.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_write_u32(addr: i64, value: i64) {
    unsafe { core::ptr::write_volatile(addr as usize as *mut u32, value as u32) }
}

/// # Safety
///
/// As [`lkrt_mmio_write_u8`], for a `u64` access.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_mmio_write_u64(addr: i64, value: i64) {
    unsafe { core::ptr::write_volatile(addr as usize as *mut u64, value as u64) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A round trip through real memory: the address is a local's, so the
    /// access is valid and the value must survive it.
    #[test]
    fn volatile_round_trips_through_memory() {
        let mut cell: u32 = 0;
        let addr = (&raw mut cell) as usize as i64;
        lkrt_mmio_write_u32(addr, 0xDEAD_BEEF_u32 as i64);
        assert_eq!(cell, 0xDEAD_BEEF);
        assert_eq!(lkrt_mmio_read_u32(addr), 0xDEAD_BEEF_i64);
    }

    #[test]
    fn narrower_widths_touch_only_their_own_bytes() {
        let mut cell: u64 = 0;
        let addr = (&raw mut cell) as usize as i64;
        lkrt_mmio_write_u8(addr, 0xFF);
        // Only the low byte, whichever end the target calls low.
        assert_eq!(cell & 0xFF, 0xFF);
        assert_eq!(cell & !0xFF, 0);
        assert_eq!(lkrt_mmio_read_u8(addr), 0xFF);
    }

    /// Writes truncate to the access width rather than spilling into the
    /// neighbouring bytes.
    #[test]
    fn writes_truncate_to_the_access_width() {
        let mut cell: u64 = 0;
        let addr = (&raw mut cell) as usize as i64;
        lkrt_mmio_write_u8(addr, 0x1FF);
        assert_eq!(cell & 0xFF, 0xFF);
        assert_eq!(cell & !0xFF, 0);
    }

    /// A full-width read is a bit pattern, not a magnitude: the top bit comes
    /// back as a negative `i64`, matching the VM's carrier.
    #[test]
    fn full_width_reads_reinterpret_rather_than_saturate() {
        let mut cell: u64 = u64::MAX;
        let addr = (&raw mut cell) as usize as i64;
        assert_eq!(lkrt_mmio_read_u64(addr), -1);
    }
}
