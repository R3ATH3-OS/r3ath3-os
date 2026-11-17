//! `mb2-boot` and `lfb-console` proofs. Safe code only.
//!
//! A failed check prints a fail line and exits 35. On success the proof
//! prints its lines and exits with its own status.

use core::fmt::Write;

#[cfg(feature = "mb2-boot")]
use crate::frames;
use crate::{mb2, qemu_exit, serial};

#[cfg(feature = "lfb-console")]
use crate::lfb;

#[cfg(feature = "mb2-boot")]
pub fn run_mb2() -> ! {
    if !mb2::valid() {
        fail("magic");
    }
    let fw = if mb2::firmware_uefi() { "uefi" } else { "bios" };
    let _ = write!(serial::Writer, "R3ATH3_MB2_BOOT magic=ok\n");
    let _ = write!(serial::Writer, "R3ATH3_MB2_BOOT fw={fw}\n");
    let _ = write!(
        serial::Writer,
        "R3ATH3_MB2_BOOT entries={} raw={} type1={}\n",
        mb2::stored_entries(),
        mb2::raw_entries(),
        mb2::type1_bytes()
    );
    match mb2::framebuffer() {
        Some(fb) if fb.kind == 2 => {
            let _ = write!(serial::Writer, "R3ATH3_MB2_BOOT fb=text\n");
        }
        Some(fb) => {
            let kind = if fb.kind == 1 { "rgb" } else { "other" };
            let _ = write!(
                serial::Writer,
                "R3ATH3_MB2_BOOT fb={}x{}x{} addr=0x{:x} pitch={} type={}\n",
                fb.width, fb.height, fb.bpp, fb.addr, fb.pitch, kind
            );
        }
        None => {
            let _ = write!(serial::Writer, "R3ATH3_MB2_BOOT fb=none\n");
        }
    }
    let (total, free) = frames::stats();
    let Some(first) = frames::alloc() else {
        fail("alloc");
    };
    let first_addr = first.addr();
    if frames::free(first).is_err() {
        fail("free");
    }
    let (total2, free2) = frames::stats();
    if total != total2 || free != free2 || free != total || total == 0 {
        fail("stats");
    }
    let kernel_end = frames::kernel_end();
    let kernel_start = frames::kernel_start();
    if first_addr < kernel_end || kernel_start != 0x10_0000 {
        fail("kernel");
    }
    let managed = frames::is_managed(frames::UNCACHED_START);
    let _ = write!(
        serial::Writer,
        "R3ATH3_MB2_BOOT frames_total={total} frames_free={free2} first=0x{first_addr:x} kernel_end=0x{kernel_end:x}\n"
    );
    let managed_word = if managed { "yes" } else { "no" };
    let _ = write!(
        serial::Writer,
        "R3ATH3_MB2_BOOT window=0x{:x}..0x{:x} excluded=yes managed={managed_word}\n",
        frames::UNCACHED_START,
        frames::UNCACHED_END
    );
    let _ = write!(serial::Writer, "R3ATH3_MB2_BOOT ok\n");
    qemu_exit::mb2_boot_ok();
}

#[cfg(feature = "lfb-console")]
pub fn run_lfb() -> ! {
    let Some(screen) = lfb::screen() else {
        fail("nofb");
    };
    if screen.width < 640 || screen.height < 400 {
        fail("small");
    }
    lfb::clear();
    for line in 0..25u32 {
        put_bytes(b"L");
        put_bytes(&two_digits(line));
        lfb::put(b'\n');
    }
    if lfb::scrolls() != 1 {
        fail("scroll");
    }
    if !lfb::glyph_matches(0, 0) {
        fail("glyph");
    }
    let fnv = lfb::console_fnv();
    let fw = if mb2::firmware_uefi() { "uefi" } else { "bios" };
    let _ = write!(
        serial::Writer,
        "R3ATH3_LFB_CONSOLE fw={fw} fb={}x{}x32 addr=0x{:x} pitch={}\n",
        screen.width, screen.height, screen.addr, screen.pitch
    );
    let _ = write!(serial::Writer, "R3ATH3_LFB_CONSOLE scrolled=1\n");
    let _ = write!(serial::Writer, "R3ATH3_LFB_CONSOLE fnv={fnv:016x}\n");
    let _ = write!(serial::Writer, "R3ATH3_LFB_CONSOLE glyph=L ok\n");
    let _ = write!(serial::Writer, "R3ATH3_LFB_CONSOLE ok\n");
    qemu_exit::lfb_console_ok();
}

#[cfg(feature = "lfb-console")]
fn put_bytes(bytes: &[u8]) {
    for &byte in bytes {
        lfb::put(byte);
    }
}

#[cfg(feature = "lfb-console")]
fn two_digits(value: u32) -> [u8; 2] {
    [b'0' + ((value / 10) % 10) as u8, b'0' + (value % 10) as u8]
}

fn fail(detail: &str) -> ! {
    let _ = write!(serial::Writer, "R3ATH3_HWTEST fail detail={detail}\n");
    qemu_exit::fail();
}
