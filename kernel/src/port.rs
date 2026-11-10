//! Port I/O for this boot milestone.
//!
//! # Invariants
//! Callers may use only COM1 (`0x3F8..=0x3FD`) and the QEMU `isa-debug-exit`
//! port `0xF4`. Interrupts stay disabled and only the boot processor runs, so
//! these ports are not shared. This module is QEMU-only: `0xF4` is not a
//! hardware power-off port.

#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline]
pub unsafe fn outl(port: u16, value: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") value,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") value,
        options(nomem, nostack, preserves_flags)
    );
    value
}
