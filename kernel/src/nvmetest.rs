//! NVMe proof. Safe code only. A failed check panics (status 35).
//! The disk is QEMU's emulated NVMe controller on a 2 MiB image.
//! Nothing here is kept past the QEMU exit except the bytes in that image.

use core::fmt::Write;

use crate::block::{BlockDev, IoError, BLOCK};
use crate::nvme::{self, NvmeDisk};
use crate::qemu_exit;
use crate::serial;

pub fn run_core() -> ! {
    let info = match nvme::probe() {
        Ok(info) => info,
        Err(error) => panic!("nvme probe {}", error.word()),
    };
    let _ = write!(
        serial::Writer,
        "R3ATH3_NVME_CORE step=1 found class={:06x} bar0={:08x}\n",
        info.class_code, info.bar0
    );
    let lba_bytes = 1u32 << info.lbads;
    let _ = write!(
        serial::Writer,
        "R3ATH3_NVME_CORE step=2 ready lba_bytes={lba_bytes} blocks={}\n",
        info.blocks
    );
    let mut disk = NvmeDisk;
    let mut buf = [0u8; BLOCK];
    must(disk.read(0, &mut buf), "read 0");
    let zero = if buf.iter().all(|byte| *byte == 0) {
        "yes"
    } else {
        "no"
    };
    let _ = write!(serial::Writer, "R3ATH3_NVME_CORE step=3 read lba=0 zero={zero}\n");
    check_roundtrip(&mut disk, 7, 4);
    check_roundtrip(&mut disk, 4095, 5);
    let range = disk.read(4096, &mut buf);
    let raw = nvme::read_unchecked(4096, &mut buf);
    let (sct, sc) = match raw {
        Err(IoError::Device(sct, sc)) => (sct, sc),
        Ok(()) => panic!("lba 4096 read succeeded"),
        Err(IoError::Range) => panic!("unchecked read returned range"),
        Err(IoError::Timeout) => panic!("unchecked read timed out"),
    };
    if range != Err(IoError::Range) {
        panic!("driver did not refuse lba 4096");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_NVME_CORE step=6 lba=4096 driver=range device=sct{sct}-sc{sc:02x}\n"
    );
    must(disk.flush(), "flush");
    serial::write_str("R3ATH3_NVME_CORE step=7 flush=ok\n");
    serial::write_str("R3ATH3_NVME_CORE ok steps=7\n");
    qemu_exit::nvme_core_ok();
}

fn check_roundtrip(disk: &mut NvmeDisk, lba: u64, step: u32) {
    let pattern = pattern(lba);
    must(disk.write(lba, &pattern), "write");
    let mut got = [0u8; BLOCK];
    must(disk.read(lba, &mut got), "readback");
    let same = if got == pattern { "same" } else { "different" };
    let _ = write!(
        serial::Writer,
        "R3ATH3_NVME_CORE step={step} write lba={lba} readback={same}\n"
    );
    if got != pattern {
        panic!("readback mismatch");
    }
}

fn pattern(lba: u64) -> [u8; BLOCK] {
    let mut buf = [0u8; BLOCK];
    let base = lba as usize;
    for (index, byte) in buf.iter_mut().enumerate() {
        *byte = index.wrapping_mul(7).wrapping_add(base) as u8;
    }
    buf
}

fn must(result: Result<(), IoError>, what: &str) {
    if let Err(error) = result {
        match error {
            IoError::Range => panic!("{what} range"),
            IoError::Timeout => panic!("{what} timeout"),
            IoError::Device(sct, sc) => panic!("{what} sct={sct} sc={sc:02x}"),
        }
    }
}
