//! Port I/O for this boot milestone.
//!
//! # Invariants
//! Callers may use COM1 (`0x3F8..=0x3FD`), the legacy PIC (`0x20`, `0x21`,
//! `0xA0`, `0xA1`), the PIT channel 0 timer (`0x40`, `0x43`), PIT channel 2
//! and the PC speaker (`0x42`, `0x43` with channel-2 command bytes only,
//! `0x61` speaker bits only), the PS/2 controller (`0x60`, `0x64`), the CMOS
//! RTC (`0x70`, `0x71`), the VGA CRTC (`0x3D4`, `0x3D5`) and the other VGA
//! mode-set ports (`0x3C0`, `0x3C2`, `0x3C4`, `0x3C5`, `0x3C7`, `0x3C8`,
//! `0x3C9`, `0x3CE`, `0x3CF`, `0x3DA`), the PCI configuration ports (`0xCF8`,
//! `0xCFC`), and the QEMU `isa-debug-exit` port `0xF4`. Only the boot
//! processor runs.
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
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx") port,
        out("eax") value,
        options(nomem, nostack, preserves_flags)
    );
    value
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
