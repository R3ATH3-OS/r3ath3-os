//! R3@TH3 OS boot milestone.
//!
//! Vision, not an implemented cooperative system: "An authentic Human–AI co-op OS".
//! This binary only boots, writes a fixed serial marker, and exits QEMU.
//! No model, agent, or capability check runs in ring 0.

#![no_std]
#![no_main]

mod boot;
mod port;
mod qemu_exit;
mod serial;
mod vga;

use core::fmt::Write;

#[cfg(all(feature = "fault-panic", feature = "fault-hang"))]
compile_error!("enable only one of fault-panic or fault-hang");

#[cfg(all(
    feature = "capture",
    any(feature = "fault-panic", feature = "fault-hang")
))]
compile_error!("capture cannot be combined with a fault feature");

/// Deterministic pass line. The smoke test matches this exact text.
pub const BOOT_MARKER: &str = "R3ATH3_BOOT_OK arch=x86_64 serial=com1 milestone=boot-foundation\n";

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    serial::init();
    serial::write_str("R3ATH3_STAGE rust_main\n");

    #[cfg(feature = "fault-panic")]
    panic!("injected boot fault");

    #[cfg(feature = "fault-hang")]
    {
        serial::write_str("R3ATH3_HANG\n");
        qemu_exit::halt_forever();
    }

    #[cfg(not(any(feature = "fault-panic", feature = "fault-hang")))]
    boot_ok();
}

#[cfg(not(any(feature = "fault-panic", feature = "fault-hang")))]
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
