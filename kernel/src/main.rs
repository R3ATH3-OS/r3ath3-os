//! R3@TH3 OS boot milestone.
//!
//! Suggested wording, not a certification and not a claim that this image is that system: "An authentic Human–AI co-op OS".
//! This binary boots, reports CPU exceptions on serial, sets up a bump-allocated
//! kernel heap and a physical frame allocator, and exits QEMU. The `shell`
//! feature reads the keyboard instead of taking that exit.
//! No model, agent, or capability check runs in ring 0.

#![no_std]
#![no_main]

extern crate alloc;

mod boot;
mod exceptions;
mod frames;
mod heap;
mod irq;
mod mem;
mod memmap;
#[cfg(any(
    feature = "heap",
    feature = "heap-oom",
    feature = "heap-oom-panic",
    feature = "frames",
    feature = "frames-oom"
))]
mod memtest;
mod port;
mod qemu_exit;
#[cfg(any(feature = "script", feature = "kbd-script", feature = "shell"))]
mod script;
mod serial;
#[cfg(feature = "shell")]
mod shell;
mod vga;

use core::fmt::Write;

const _: () = {
    let modes = cfg!(feature = "fault-panic") as u8
        + cfg!(feature = "fault-hang") as u8
        + cfg!(feature = "capture") as u8
        + cfg!(feature = "fault-ud") as u8
        + cfg!(feature = "fault-pf") as u8
        + cfg!(feature = "fault-df") as u8
        + cfg!(feature = "script") as u8
        + cfg!(feature = "kbd-script") as u8
        + cfg!(feature = "ticks") as u8
        + cfg!(feature = "keys") as u8
        + cfg!(feature = "heap") as u8
        + cfg!(feature = "heap-oom") as u8
        + cfg!(feature = "heap-oom-panic") as u8
        + cfg!(feature = "frames") as u8
        + cfg!(feature = "frames-oom") as u8
        + cfg!(feature = "mem-badinfo") as u8
        + cfg!(feature = "shell") as u8;
    assert!(modes <= 1, "enable at most one test feature");
};

/// Deterministic pass line. The smoke test matches this exact text.
pub const BOOT_MARKER: &str = "R3ATH3_BOOT_OK arch=x86_64 serial=com1 milestone=boot-foundation\n";

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    serial::init();
    serial::write_str("R3ATH3_STAGE rust_main\n");
    exceptions::init();

    #[cfg(feature = "mem-badinfo")]
    memmap::corrupt_start_info_for_test();

    if let Err(error) = mem::init() {
        serial::write_str("R3ATH3_MEM_ERROR reason=");
        serial::write_str(error.reason());
        serial::write_str("\n");
        qemu_exit::mem_error();
    }

    #[cfg(feature = "mem-badinfo")]
    panic!("memory init accepted a zeroed start-info pointer");

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

    #[cfg(feature = "kbd-script")]
    script::run();

    #[cfg(feature = "ticks")]
    irq::ticks_demo();

    #[cfg(feature = "keys")]
    irq::keys_demo();

    #[cfg(feature = "heap")]
    memtest::run_heap();

    #[cfg(feature = "heap-oom")]
    memtest::run_heap_oom();

    #[cfg(feature = "heap-oom-panic")]
    memtest::run_heap_oom_panic();

    #[cfg(feature = "frames")]
    memtest::run_frames();

    #[cfg(feature = "frames-oom")]
    memtest::run_frames_oom();

    #[cfg(feature = "shell")]
    shell::run();

    #[cfg(not(any(
        feature = "fault-panic",
        feature = "fault-hang",
        feature = "fault-ud",
        feature = "fault-pf",
        feature = "fault-df",
        feature = "script",
        feature = "kbd-script",
        feature = "ticks",
        feature = "keys",
        feature = "heap",
        feature = "heap-oom",
        feature = "heap-oom-panic",
        feature = "frames",
        feature = "frames-oom",
        feature = "mem-badinfo",
        feature = "shell"
    )))]
    boot_ok();
}

#[cfg(not(any(
    feature = "fault-panic",
    feature = "fault-hang",
    feature = "fault-ud",
    feature = "fault-pf",
    feature = "fault-df",
    feature = "script",
    feature = "kbd-script",
    feature = "ticks",
    feature = "keys",
    feature = "heap",
    feature = "heap-oom",
    feature = "heap-oom-panic",
    feature = "frames",
    feature = "frames-oom",
    feature = "mem-badinfo",
    feature = "shell"
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
