//! Port I/O for this boot milestone.
//!
//! # Invariants
//! Callers may use COM1 (`0x3F8..=0x3FD`), the legacy PIC (`0x20`, `0x21`,
//! `0xA0`, `0xA1`), the PIT (`0x40`, `0x43`), the PS/2 controller (`0x60`,
//! `0x64`), the CMOS RTC (`0x70`, `0x71`), the VGA CRTC (`0x3D4`, `0x3D5`),
//! and the QEMU `isa-debug-exit` port `0xF4`. Only the boot processor runs.
//! `0xF4` is not a hardware power-off port.

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
