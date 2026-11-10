//! R3@TH3 OS boot milestone.
//!
//! Suggested wording, not a certification and not a claim that this image is that system: "An authentic Human–AI co-op OS".
//! This binary boots, reports CPU exceptions on serial, and exits QEMU.
//! No model, agent, or capability check runs in ring 0.

#![no_std]
#![no_main]

mod boot;
mod exceptions;
mod port;
mod qemu_exit;
#[cfg(feature = "script")]
mod script;
mod serial;
mod vga;

use core::fmt::Write;

const _: () = {
    let modes = cfg!(feature = "fault-panic") as u8
        + cfg!(feature = "fault-hang") as u8
        + cfg!(feature = "capture") as u8
        + cfg!(feature = "fault-ud") as u8
        + cfg!(feature = "fault-pf") as u8
        + cfg!(feature = "fault-df") as u8
        + cfg!(feature = "script") as u8;
    assert!(modes <= 1, "enable at most one test feature");
};

/// Deterministic pass line. The smoke test matches this exact text.
pub const BOOT_MARKER: &str = "R3ATH3_BOOT_OK arch=x86_64 serial=com1 milestone=boot-foundation\n";

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    serial::init();
    serial::write_str("R3ATH3_STAGE rust_main\n");
    exceptions::init();

    #[cfg(feature = "fault-panic")]
    panic!("injected boot fault");

    #[cfg(feature = "fault-hang")]
    {
        serial::write_str("R3ATH3_HANG\n");
        qemu_exit::halt_forever();
    }

    #[cfg(feature = "fault-ud")]
    exceptions::trigger_ud();

    #[cfg(feature = "fault-pf")]
    exceptions::trigger_pf();

    #[cfg(feature = "fault-df")]
    exceptions::trigger_df();

    #[cfg(feature = "script")]
    script::run();

    #[cfg(not(any(
        feature = "fault-panic",
        feature = "fault-hang",
        feature = "fault-ud",
        feature = "fault-pf",
        feature = "fault-df",
        feature = "script"
    )))]
    boot_ok();
}

#[cfg(not(any(
    feature = "fault-panic",
    feature = "fault-hang",
    feature = "fault-ud",
    feature = "fault-pf",
    feature = "fault-df",
    feature = "script"
)))]
fn boot_ok() -> ! {
    serial::write_str(BOOT_MARKER);
    vga::write_marker(BOOT_MARKER);
    #[cfg(feature = "capture")]
    qemu_exit::halt_forever();
    #[cfg(not(feature = "capture"))]
    qemu_exit::success();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    serial::write_str("R3ATH3_PANIC");
    if let Some(location) = info.location() {
        serial::write_str(" location=");
        serial::write_str(location.file());
        serial::write_str(":");
        let _ = write!(serial::Writer, "{}", location.line());
    }
    serial::write_str(" message=");
    let _ = write!(serial::Writer, "{}", info.message());
    serial::write_str("\n");
    qemu_exit::fail();
}
