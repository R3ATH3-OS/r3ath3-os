//! Store proof. Safe code only. A failed internal check panics (status 35).
//! Every tick is a literal. The disk is the RAM disk. Nothing here is kept
//! past the QEMU exit.

use core::fmt::Write;

use crate::block::{BlockDev, IoError, BLOCK};
use crate::qemu_exit;
use crate::ramdisk::{self, RamDisk};
use crate::recorder::{self, AppendError, Recorder};
use crate::serial;
use crate::store::{self, Device, MountError};

const HEAD1: u64 = 0xebb9_5e48_118d_0282;
const HEAD2: u64 = 0x4d9b_4cc9_2b89_320a;
const HEAD6: u64 = 0x6871_2f55_9755_9a1a;

struct Limit {
    blocks: u64,
}

impl BlockDev for Limit {
    fn blocks(&self) -> u64 {
        self.blocks
    }

    fn read(&mut self, lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks {
            return Err(IoError::Range);
        }
        RamDisk.read(lba, buf)
    }

    fn write(&mut self, lba: u64, buf: &[u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks {
            return Err(IoError::Range);
        }
        RamDisk.write(lba, buf)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        RamDisk.flush()
    }
}

struct FailWrites {
    armed: bool,
    attempts: u32,
}

impl BlockDev for FailWrites {
    fn blocks(&self) -> u64 {
        RamDisk.blocks()
    }

    fn read(&mut self, lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks() {
            return Err(IoError::Range);
        }
        RamDisk.read(lba, buf)
    }

    fn write(&mut self, lba: u64, buf: &[u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks() {
            return Err(IoError::Range);
        }
        if self.armed {
            self.attempts = self.attempts.saturating_add(1);
            return Err(IoError::Timeout);
        }
        RamDisk.write(lba, buf)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        RamDisk.flush()
    }
}

pub fn run_core() -> ! {
    ramdisk::wipe_for_test();
    let mounted = must_mount(store::mount(&mut RamDisk, Device::Ram), 1);
    if !mounted.formatted || mounted.blocks != 512 || mounted.slots != 15 || mounted.this != Some(1)
    {
        panic!("step 1");
    }
    serial::write_str(
        "R3ATH3_STORE_CORE step=1 blank formatted=yes blocks=512 slots=15 this=1\n",
    );

    let mut rec = Recorder::<6>::new();
    let mut mounted = mounted;
    let samples: [(u64, &[u8], &[u8]); 6] = [
        (10, b"human last=ok", b"help"),
        (20, b"agent ai1", b"say seq=1 done text=hi"),
        (30, b"gate", b"grant ai1 say"),
        (40, b"gate", b"revoke ai1 all"),
        (50, b"human last=cmd", b"approve 9"),
        (60, b"selftest", b"ok=7 fail=0"),
    ];
    for (tick, tag, body) in samples {
        let n = must_append(rec.append(tick, tag, body), 2);
        must_io(
            store::persist_entry(&mut RamDisk, &mut mounted, n, |i| rec.get(i)),
            2,
        );
    }
    match rec.append(70, b"human last=ok", b"log") {
        Err(AppendError::Full) => {}
        _ => panic!("step 2 full"),
    }
    if rec.lost() != 1 {
        panic!("step 2 lost");
    }
    must_io(store::persist_lost(&mut RamDisk, &mut mounted, rec.lost()), 2);
    let scan = must_scan(store::scan_boot(&mut RamDisk, &mounted, 1), 2);
    if !scan.header_ok
        || scan.broken_at.is_some()
        || scan.entries != 6
        || scan.lost != 1
        || scan.head != HEAD6
        || rec.head() != HEAD6
    {
        panic!("step 2 scan");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_STORE_CORE step=2 persisted=6 boot1 entries=6 lost=1 head={HEAD6:016x} chain=ok\n"
    );

    let mounted = must_mount(store::mount(&mut RamDisk, Device::Ram), 3);
    if mounted.formatted || mounted.this != Some(2) || mounted.used != 2 {
        panic!("step 3 mount");
    }
    let boot2 = must_scan(store::scan_boot(&mut RamDisk, &mounted, 2), 3);
    let prev = must_prev(store::prev_word(&mut RamDisk, &mounted, 2, &boot2), 3);
    if prev != "ok" {
        panic!("step 3 prev");
    }
    serial::write_str("R3ATH3_STORE_CORE step=3 remount formatted=no this=2 used=2 prev=ok\n");

    must_text(store::save_text(&mut RamDisk, b"note", b"hello"), 4);
    must_text(store::save_text(&mut RamDisk, b"n2", b"x"), 4);
    must_text(store::forget_text(&mut RamDisk, b"n2"), 4);
    let census = must_saved(store::read_saved(&mut RamDisk), 4);
    let note = match census.slots[0] {
        store::Slot::Used {
            text,
            text_len,
            name,
            name_len,
        } if &name[..name_len as usize] == b"note" && &text[..text_len as usize] == b"hello" => true,
        _ => false,
    };
    let n2_gone = !census.slots.iter().any(|slot| match slot {
        store::Slot::Used { name, name_len, .. } => &name[..*name_len as usize] == b"n2",
        _ => false,
    });
    if census.used != 1 || !note || !n2_gone {
        panic!("step 4 saved");
    }
    serial::write_str("R3ATH3_STORE_CORE step=4 saved=1 note=hello n2=gone\n");

    let mut block = must_read(9, 5);
    let saved_block = block;
    block[306] ^= 1;
    must_write(9, &block, 5);
    let tampered = must_scan(store::scan_boot(&mut RamDisk, &mounted, 1), 5);
    let boot2 = must_scan(store::scan_boot(&mut RamDisk, &mounted, 2), 5);
    let prev = must_prev(store::prev_word(&mut RamDisk, &mounted, 2, &boot2), 5);
    if tampered.entries != 2 || tampered.head != HEAD2 || tampered.broken_at != Some(3) || prev != "bad"
    {
        panic!("step 5 tamper");
    }
    must_write(9, &saved_block, 5);
    let restored = must_scan(store::scan_boot(&mut RamDisk, &mounted, 1), 5);
    let boot2 = must_scan(store::scan_boot(&mut RamDisk, &mounted, 2), 5);
    let prev = must_prev(store::prev_word(&mut RamDisk, &mounted, 2, &boot2), 5);
    if restored.broken_at.is_some() || restored.entries != 6 || restored.head != HEAD6 || prev != "ok" {
        panic!("step 5 restore");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_STORE_CORE step=5 tamper boot1 entries=2 head={HEAD2:016x} broken=3 boot2 prev=bad restored=ok\n"
    );

    let original = must_read(0, 6);
    let mut flipped = original;
    flipped[8] ^= 1;
    must_write(0, &flipped, 6);
    let bad_check = word(store::read_superblock(&mut RamDisk));
    must_write(0, &original, 6);
    let mut versioned = original;
    versioned[8..12].copy_from_slice(&2u32.to_le_bytes());
    let check = fnv(&versioned[0..48]);
    versioned[48..56].copy_from_slice(&check.to_le_bytes());
    must_write(0, &versioned, 6);
    let bad_version = word(store::read_superblock(&mut RamDisk));
    must_write(0, &original, 6);
    let restored = word(store::read_superblock(&mut RamDisk));
    if bad_check != "superblock" || bad_version != "version" || restored != "ok" {
        panic!("step 6");
    }
    serial::write_str(
        "R3ATH3_STORE_CORE step=6 badcheck=superblock badversion=version restored=ok\n",
    );

    let short = word(store::read_superblock(&mut Limit { blocks: 256 }));
    ramdisk::wipe_for_test();
    let small = word(store::mount(&mut Limit { blocks: 40 }, Device::Ram));
    ramdisk::wipe_for_test();
    let mut metadata = must_read(5, 7);
    metadata[0] = 1;
    must_write(5, &metadata, 7);
    let not_blank = word(store::mount(&mut RamDisk, Device::Ram));
    let lba0 = must_read(0, 7);
    let lba0_zero = if lba0.iter().all(|byte| *byte == 0) {
        "zero"
    } else {
        "dirty"
    };
    if short != "size" || small != "small" || not_blank != "not-blank" || lba0_zero != "zero" {
        panic!("step 7");
    }
    serial::write_str(
        "R3ATH3_STORE_CORE step=7 short=size small=small notblank=not-blank lba0=zero\n",
    );

    ramdisk::wipe_for_test();
    for n in 1..=15u32 {
        let opened = must_mount(store::mount(&mut RamDisk, Device::Ram), 8);
        if opened.this != Some(n) {
            panic!("step 8 open");
        }
    }
    let sixteenth = must_mount(store::mount(&mut RamDisk, Device::Ram), 8);
    if sixteenth.this.is_some() || sixteenth.used != 15 {
        panic!("step 8 full");
    }
    serial::write_str("R3ATH3_STORE_CORE step=8 opened=15 sixteenth=none used=15\n");

    ramdisk::wipe_for_test();
    let mut fail = FailWrites {
        armed: false,
        attempts: 0,
    };
    let mut mounted = must_mount(store::mount(&mut fail, Device::Ram), 9);
    let mut local = Recorder::<4>::new();
    let n = must_append(local.append(10, b"human last=ok", b"help"), 9);
    must_io(
        store::persist_entry(&mut fail, &mut mounted, n, |i| local.get(i)),
        9,
    );
    fail.armed = true;
    let n = must_append(local.append(20, b"agent ai1", b"x"), 9);
    if store::persist_entry(&mut fail, &mut mounted, n, |i| local.get(i)).is_ok() || !mounted.failed
    {
        panic!("step 9 fail");
    }
    let n = must_append(local.append(30, b"gate", b"y"), 9);
    if store::persist_entry(&mut fail, &mut mounted, n, |i| local.get(i)).is_err() {
        panic!("step 9 again");
    }
    let writes_after = fail.attempts.saturating_sub(1);
    if mounted.saved_upto != 1 || !mounted.failed || writes_after != 0 {
        panic!("step 9 totals");
    }
    serial::write_str(
        "R3ATH3_STORE_CORE step=9 writefail saved_upto=1 failed=yes writes_after=0\n",
    );

    let entry = rec.get(1).unwrap_or_else(|| panic!("step 10 missing"));
    let encoded = store::encode_entry(&entry);
    let decoded = store::decode_entry(&encoded).unwrap_or_else(|| panic!("step 10 decode"));
    let link = recorder::link(
        recorder::BASIS,
        decoded.n(),
        decoded.tick(),
        decoded.tag(),
        decoded.body(),
    );
    if encoded.len() != 128
        || decoded.n() != 1
        || decoded.tick() != 10
        || decoded.tag() != b"human last=ok"
        || decoded.body() != b"help"
        || link != HEAD1
        || decoded.link() != HEAD1
    {
        panic!("step 10");
    }
    let _ = write!(
        serial::Writer,
        "R3ATH3_STORE_CORE step=10 encode=128 decode=1,10,human last=ok,help link={HEAD1:016x}\n"
    );

    serial::write_str("R3ATH3_STORE_CORE ok steps=10\n");
    qemu_exit::store_core_ok();
}

fn fnv(data: &[u8]) -> u64 {
    let mut hash = recorder::BASIS;
    for &byte in data {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn word(result: Result<impl Sized, MountError>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(error) => error.word(),
    }
}

fn must_mount(result: Result<store::Mounted, MountError>, step: u32) -> store::Mounted {
    match result {
        Ok(mounted) => mounted,
        Err(_) => panic!("step {step} mount"),
    }
}

fn must_append(result: Result<u32, AppendError>, step: u32) -> u32 {
    match result {
        Ok(n) => n,
        Err(_) => panic!("step {step} append"),
    }
}

fn must_io(result: Result<(), IoError>, step: u32) {
    if result.is_err() {
        panic!("step {step} io");
    }
}

fn must_scan(result: Result<store::BootScan, IoError>, step: u32) -> store::BootScan {
    match result {
        Ok(scan) => scan,
        Err(_) => panic!("step {step} scan"),
    }
}

fn must_prev(result: Result<&'static str, IoError>, step: u32) -> &'static str {
    match result {
        Ok(word) => word,
        Err(_) => panic!("step {step} prev"),
    }
}

fn must_text(result: Result<(), store::TextError>, step: u32) {
    if result.is_err() {
        panic!("step {step} text");
    }
}

fn must_saved(result: Result<store::Census, IoError>, step: u32) -> store::Census {
    match result {
        Ok(census) => census,
        Err(_) => panic!("step {step} saved"),
    }
}

fn must_read(lba: u64, step: u32) -> [u8; BLOCK] {
    let mut buf = [0u8; BLOCK];
    if RamDisk.read(lba, &mut buf).is_err() {
        panic!("step {step} read");
    }
    buf
}

fn must_write(lba: u64, buf: &[u8; BLOCK], step: u32) {
    if RamDisk.write(lba, buf).is_err() {
        panic!("step {step} write");
    }
}
