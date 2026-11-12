//! On-store format `R3ATH3ST` version 1. See `docs/STORAGE.md`.
//!
//! Safe code only. No statics and no allocation. Every read and write goes
//! through a `&mut dyn BlockDev`. The same layout is used on the RAM disk.
//! Nothing stored here survives a reboot of the RAM disk: the bytes live in
//! guest RAM and come back as zeros.

#![allow(dead_code)]

use crate::block::{BlockDev, IoError, BLOCK};
use crate::recorder::{self, Entry};

const SUPER_MAGIC: &[u8; 8] = b"R3ATH3ST";
const BOOT_MAGIC: &[u8; 8] = b"R3BOOT01";
const MIN_BLOCKS: u64 = 41;
const SAVED_SLOTS: usize = 16;
const SAVED_LBA: u32 = 1;
const SAVED_BLOCKS: u32 = 4;
const BOOT_LBA: u32 = 8;
const BOOT_BLOCKS: u32 = 33;
const MAX_SLOTS: u32 = 64;
const FNV_PRIME: u64 = 0x100_0000_01b3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MountError {
    Io,
    Small,
    Superblock,
    Version,
    Layout,
    Size,
    NotBlank,
}

impl MountError {
    pub fn word(self) -> &'static str {
        match self {
            MountError::Io => "io",
            MountError::Small => "small",
            MountError::Superblock => "superblock",
            MountError::Version => "version",
            MountError::Layout => "layout",
            MountError::Size => "size",
            MountError::NotBlank => "not-blank",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Ram,
    Nvme,
}

impl Device {
    pub fn word(self) -> &'static str {
        match self {
            Device::Ram => "ram",
            Device::Nvme => "nvme",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Mounted {
    pub blocks: u64,
    pub slots: u32,
    pub used: u32,
    pub this: Option<u32>,
    pub formatted: bool,
    pub saved_lost: u32,
    pub saved_upto: u32,
    pub failed: bool,
    pub device: Device,
}

impl Mounted {
    pub const UNMOUNTED: Mounted = Mounted {
        blocks: 0,
        slots: 0,
        used: 0,
        this: None,
        formatted: false,
        saved_lost: 0,
        saved_upto: 0,
        failed: false,
        device: Device::Ram,
    };

    pub fn is_mounted(self) -> bool {
        self.blocks >= MIN_BLOCKS
    }
}

#[derive(Clone, Copy)]
pub struct BootScan {
    pub header_ok: bool,
    pub free: bool,
    pub lost: u32,
    pub prev_head: u64,
    pub prev_entries: u32,
    pub entries: u32,
    pub head: u64,
    pub broken_at: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    Read,
    Write,
    Full,
    Missing,
}

#[derive(Clone, Copy)]
pub enum Slot {
    Empty,
    Bad,
    Used {
        name: [u8; 12],
        name_len: u8,
        text: [u8; 58],
        text_len: u8,
    },
}

pub struct Census {
    pub used: u32,
    pub slots: [Slot; SAVED_SLOTS],
}

fn seal(bytes: &[u8]) -> u64 {
    let mut hash = recorder::BASIS;
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

fn slots_for(blocks: u64) -> u32 {
    if blocks < 8 {
        return 0;
    }
    let fit = (blocks - 8) / 33;
    u32::try_from(fit).unwrap_or(u32::MAX).min(MAX_SLOTS)
}

fn boot_header_lba(k: u32) -> u64 {
    u64::from(BOOT_LBA) + u64::from(BOOT_BLOCKS) * u64::from(k - 1)
}

fn map_io(_error: IoError) -> MountError {
    MountError::Io
}

fn read_block(dev: &mut dyn BlockDev, lba: u64) -> Result<[u8; BLOCK], IoError> {
    let mut buf = [0u8; BLOCK];
    dev.read(lba, &mut buf)?;
    Ok(buf)
}

fn all_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

struct Super {
    blocks: u64,
    slots: u32,
}

fn parse_super(buf: &[u8; BLOCK], device_blocks: u64) -> Result<Super, MountError> {
    if &buf[0..8] != SUPER_MAGIC {
        return Err(MountError::Superblock);
    }
    let stored = u64::from_le_bytes(buf[48..56].try_into().unwrap());
    if stored != seal(&buf[0..48]) {
        return Err(MountError::Superblock);
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if version != 1 {
        return Err(MountError::Version);
    }
    let block_size = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    let blocks = u64::from_le_bytes(buf[16..24].try_into().unwrap());
    let saved_lba = u32::from_le_bytes(buf[24..28].try_into().unwrap());
    let saved_blocks = u32::from_le_bytes(buf[28..32].try_into().unwrap());
    let boot_lba = u32::from_le_bytes(buf[32..36].try_into().unwrap());
    let boot_blocks = u32::from_le_bytes(buf[36..40].try_into().unwrap());
    let slots = u32::from_le_bytes(buf[40..44].try_into().unwrap());
    if block_size != BLOCK as u32
        || saved_lba != SAVED_LBA
        || saved_blocks != SAVED_BLOCKS
        || boot_lba != BOOT_LBA
        || boot_blocks != BOOT_BLOCKS
        || slots != slots_for(blocks)
    {
        return Err(MountError::Layout);
    }
    if blocks > device_blocks {
        return Err(MountError::Size);
    }
    Ok(Super { blocks, slots })
}

fn write_superblock(dev: &mut dyn BlockDev, blocks: u64) -> Result<(), IoError> {
    let mut buf = [0u8; BLOCK];
    buf[0..8].copy_from_slice(SUPER_MAGIC);
    buf[8..12].copy_from_slice(&1u32.to_le_bytes());
    buf[12..16].copy_from_slice(&(BLOCK as u32).to_le_bytes());
    buf[16..24].copy_from_slice(&blocks.to_le_bytes());
    buf[24..28].copy_from_slice(&SAVED_LBA.to_le_bytes());
    buf[28..32].copy_from_slice(&SAVED_BLOCKS.to_le_bytes());
    buf[32..36].copy_from_slice(&BOOT_LBA.to_le_bytes());
    buf[36..40].copy_from_slice(&BOOT_BLOCKS.to_le_bytes());
    buf[40..44].copy_from_slice(&slots_for(blocks).to_le_bytes());
    let check = seal(&buf[0..48]);
    buf[48..56].copy_from_slice(&check.to_le_bytes());
    dev.write(0, &buf)?;
    dev.flush()
}

struct Header {
    lost: u32,
    prev_head: u64,
    prev_entries: u32,
}

fn parse_header(buf: &[u8; BLOCK], k: u32) -> Option<Header> {
    if &buf[0..8] != BOOT_MAGIC {
        return None;
    }
    let boot = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    let lost = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    let prev_head = u64::from_le_bytes(buf[16..24].try_into().unwrap());
    let prev_entries = u32::from_le_bytes(buf[24..28].try_into().unwrap());
    let zero = u32::from_le_bytes(buf[28..32].try_into().unwrap());
    let stored = u64::from_le_bytes(buf[32..40].try_into().unwrap());
    if boot != k || zero != 0 || !all_zero(&buf[40..]) || stored != seal(&buf[0..32]) {
        return None;
    }
    Some(Header {
        lost,
        prev_head,
        prev_entries,
    })
}

fn write_header(
    dev: &mut dyn BlockDev,
    k: u32,
    lost: u32,
    prev_head: u64,
    prev_entries: u32,
) -> Result<(), IoError> {
    let mut buf = [0u8; BLOCK];
    buf[0..8].copy_from_slice(BOOT_MAGIC);
    buf[8..12].copy_from_slice(&k.to_le_bytes());
    buf[12..16].copy_from_slice(&lost.to_le_bytes());
    buf[16..24].copy_from_slice(&prev_head.to_le_bytes());
    buf[24..28].copy_from_slice(&prev_entries.to_le_bytes());
    let check = seal(&buf[0..32]);
    buf[32..40].copy_from_slice(&check.to_le_bytes());
    dev.write(boot_header_lba(k), &buf)
}

fn blank_metadata(dev: &mut dyn BlockDev, slots: u32) -> Result<bool, MountError> {
    for lba in 1..8 {
        let buf = read_block(dev, lba).map_err(map_io)?;
        if !all_zero(&buf) {
            return Ok(false);
        }
    }
    for k in 1..=slots {
        let buf = read_block(dev, boot_header_lba(k)).map_err(map_io)?;
        if !all_zero(&buf) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Steps 1, 2, and 4 of mount. Writes nothing.
pub fn read_superblock(dev: &mut dyn BlockDev) -> Result<(u64, u32), MountError> {
    let blocks = dev.blocks();
    if blocks < MIN_BLOCKS {
        return Err(MountError::Small);
    }
    let buf = read_block(dev, 0).map_err(map_io)?;
    let superblock = parse_super(&buf, blocks)?;
    Ok((superblock.blocks, superblock.slots))
}

pub fn mount(dev: &mut dyn BlockDev, device: Device) -> Result<Mounted, MountError> {
    let blocks = dev.blocks();
    if blocks < MIN_BLOCKS {
        return Err(MountError::Small);
    }
    let lba0 = read_block(dev, 0).map_err(map_io)?;
    let mut formatted = false;
    let slots = if all_zero(&lba0) {
        let fit = slots_for(blocks);
        if !blank_metadata(dev, fit)? {
            return Err(MountError::NotBlank);
        }
        write_superblock(dev, blocks).map_err(map_io)?;
        formatted = true;
        fit
    } else {
        parse_super(&lba0, blocks)?.slots
    };

    let mut used = 0u32;
    let mut this = None;
    for k in 1..=slots {
        let buf = read_block(dev, boot_header_lba(k)).map_err(map_io)?;
        if all_zero(&buf) {
            if this.is_none() {
                this = Some(k);
            }
        } else {
            used += 1;
        }
    }

    let mut mounted = Mounted {
        blocks,
        slots,
        used,
        this,
        formatted,
        saved_lost: 0,
        saved_upto: 0,
        failed: false,
        device,
    };

    if let Some(k) = this {
        let (prev_entries, prev_head) = if k == 1 {
            (0, recorder::BASIS)
        } else {
            let scan = scan_boot(dev, &mounted, k - 1).map_err(map_io)?;
            if scan.header_ok {
                (scan.entries, scan.head)
            } else {
                (0, recorder::BASIS)
            }
        };
        let zero = [0u8; BLOCK];
        let header = boot_header_lba(k);
        for index in 1..=32 {
            dev.write(header + index, &zero).map_err(map_io)?;
        }
        write_header(dev, k, 0, prev_head, prev_entries).map_err(map_io)?;
        dev.flush().map_err(map_io)?;
        mounted.used += 1;
    }
    Ok(mounted)
}

pub fn encode_entry(entry: &Entry) -> [u8; 128] {
    let mut buf = [0u8; 128];
    buf[0..4].copy_from_slice(&entry.n().to_le_bytes());
    buf[8..16].copy_from_slice(&entry.tick().to_le_bytes());
    buf[16..24].copy_from_slice(&entry.link().to_le_bytes());
    let tag = entry.tag();
    let body = entry.body();
    buf[24] = tag.len() as u8;
    buf[25..25 + tag.len()].copy_from_slice(tag);
    buf[49] = body.len() as u8;
    buf[50..50 + body.len()].copy_from_slice(body);
    buf
}

pub fn decode_entry(bytes: &[u8; 128]) -> Option<Entry> {
    let n = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let zero4 = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let tick = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let link = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let tag_len = bytes[24];
    let body_len = bytes[49];
    let zero2 = u16::from_le_bytes(bytes[126..128].try_into().unwrap());
    if n < 1 || zero4 != 0 || zero2 != 0 || tag_len < 1 || tag_len > 24 || body_len > 76 {
        return None;
    }
    let tag_len = tag_len as usize;
    let body_len = body_len as usize;
    if !all_zero(&bytes[25 + tag_len..49]) || !all_zero(&bytes[50 + body_len..126]) {
        return None;
    }
    Entry::from_parts(n, tick, link, &bytes[25..25 + tag_len], &bytes[50..50 + body_len])
}

fn mark_failed(mounted: &mut Mounted, error: IoError) -> IoError {
    mounted.failed = true;
    error
}

/// Write the block that holds entry `n`, then flush. Slots above the live
/// length stay zero. Does nothing when this boot has no record or writing
/// has already failed.
pub fn persist_entry(
    dev: &mut dyn BlockDev,
    mounted: &mut Mounted,
    n: u32,
    mut get: impl FnMut(u32) -> Option<Entry>,
) -> Result<(), IoError> {
    if mounted.this.is_none() || mounted.failed || n == 0 {
        return Ok(());
    }
    let k = mounted.this.unwrap();
    let q = (n - 1) / 4;
    let mut block = [0u8; BLOCK];
    for slot in 0..4u32 {
        let number = q * 4 + 1 + slot;
        if let Some(entry) = get(number) {
            let encoded = encode_entry(&entry);
            let off = slot as usize * 128;
            block[off..off + 128].copy_from_slice(&encoded);
        }
    }
    let lba = boot_header_lba(k) + 1 + u64::from(q);
    if let Err(error) = dev.write(lba, &block) {
        return Err(mark_failed(mounted, error));
    }
    if let Err(error) = dev.flush() {
        return Err(mark_failed(mounted, error));
    }
    mounted.saved_upto = n;
    Ok(())
}

/// Rewrite this boot's header with `lost`, then flush.
pub fn persist_lost(
    dev: &mut dyn BlockDev,
    mounted: &mut Mounted,
    lost: u32,
) -> Result<(), IoError> {
    if mounted.this.is_none() || mounted.failed {
        return Ok(());
    }
    let k = mounted.this.unwrap();
    let lba = boot_header_lba(k);
    let mut buf = match read_block(dev, lba) {
        Ok(buf) => buf,
        Err(error) => return Err(mark_failed(mounted, error)),
    };
    buf[12..16].copy_from_slice(&lost.to_le_bytes());
    let check = seal(&buf[0..32]);
    buf[32..40].copy_from_slice(&check.to_le_bytes());
    if let Err(error) = dev.write(lba, &buf) {
        return Err(mark_failed(mounted, error));
    }
    if let Err(error) = dev.flush() {
        return Err(mark_failed(mounted, error));
    }
    mounted.saved_lost = lost;
    Ok(())
}

fn empty_scan() -> BootScan {
    BootScan {
        header_ok: false,
        free: false,
        lost: 0,
        prev_head: 0,
        prev_entries: 0,
        entries: 0,
        head: recorder::BASIS,
        broken_at: None,
    }
}

pub fn walk_boot(
    dev: &mut dyn BlockDev,
    mounted: &Mounted,
    k: u32,
    f: &mut dyn FnMut(&Entry),
) -> Result<BootScan, IoError> {
    if k == 0 || k > mounted.slots {
        return Err(IoError::Range);
    }
    let header = read_block(dev, boot_header_lba(k))?;
    if all_zero(&header) {
        let mut scan = empty_scan();
        scan.free = true;
        return Ok(scan);
    }
    let Some(parsed) = parse_header(&header, k) else {
        return Ok(empty_scan());
    };
    let mut scan = BootScan {
        header_ok: true,
        free: false,
        lost: parsed.lost,
        prev_head: parsed.prev_head,
        prev_entries: parsed.prev_entries,
        entries: 0,
        head: recorder::BASIS,
        broken_at: None,
    };
    let mut prev = recorder::BASIS;
    let mut zero_at: Option<u32> = None;
    let mut block = [0u8; BLOCK];
    let mut loaded: Option<u32> = None;
    for n in 1..=128u32 {
        let q = (n - 1) / 4;
        if loaded != Some(q) {
            block = read_block(dev, boot_header_lba(k) + 1 + u64::from(q))?;
            loaded = Some(q);
        }
        let off = ((n - 1) % 4) as usize * 128;
        let mut slot = [0u8; 128];
        slot.copy_from_slice(&block[off..off + 128]);
        let is_zero = all_zero(&slot);
        if let Some(at) = zero_at {
            if !is_zero {
                scan.broken_at = Some(at);
                break;
            }
            continue;
        }
        if is_zero {
            zero_at = Some(n);
            continue;
        }
        let Some(entry) = decode_entry(&slot) else {
            scan.broken_at = Some(n);
            break;
        };
        let expect = recorder::link(prev, entry.n(), entry.tick(), entry.tag(), entry.body());
        if entry.n() != n || expect != entry.link() {
            scan.broken_at = Some(n);
            break;
        }
        prev = entry.link();
        scan.entries = n;
        scan.head = entry.link();
        f(&entry);
    }
    Ok(scan)
}

pub fn scan_boot(dev: &mut dyn BlockDev, mounted: &Mounted, k: u32) -> Result<BootScan, IoError> {
    walk_boot(dev, mounted, k, &mut |_| {})
}

/// `ok`, `bad`, or `-` for the cross-boot `prev` check. `k` must have a good header.
pub fn prev_word(dev: &mut dyn BlockDev, mounted: &Mounted, k: u32, scan: &BootScan) -> Result<&'static str, IoError> {
    if k == 1 {
        return Ok("-");
    }
    let earlier = scan_boot(dev, mounted, k - 1)?;
    if !earlier.header_ok {
        return Ok("bad");
    }
    if earlier.entries == scan.prev_entries && earlier.head == scan.prev_head {
        Ok("ok")
    } else {
        Ok("bad")
    }
}

pub fn check_live(
    dev: &mut dyn BlockDev,
    mounted: &Mounted,
    live_len: u32,
    live_head: u64,
) -> Result<(), &'static str> {
    if read_superblock(dev).is_err() {
        return Err("superblock unreadable");
    }
    if mounted.failed {
        return Err("store write failed");
    }
    if let Some(k) = mounted.this {
        match scan_boot(dev, mounted, k) {
            Ok(scan)
                if scan.header_ok
                    && scan.broken_at.is_none()
                    && scan.entries == live_len
                    && scan.head == live_head => {}
            _ => return Err("record copy differs"),
        }
    }
    Ok(())
}

fn name_ok(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= 12
        && name
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn text_ok(text: &[u8]) -> bool {
    !text.is_empty() && text.len() <= 58 && text.iter().all(|byte| (0x20..=0x7e).contains(byte))
}

fn slot_lba(index: usize) -> u64 {
    1 + (index / 4) as u64
}

fn slot_off(index: usize) -> usize {
    (index % 4) * 128
}

fn decode_slot(raw: &[u8; 128]) -> Slot {
    if all_zero(raw) {
        return Slot::Empty;
    }
    if raw[0] != 1 {
        return Slot::Bad;
    }
    let name_len = raw[1];
    let text_len = raw[14];
    if !(1..=12).contains(&name_len) || !(1..=58).contains(&text_len) || !all_zero(&raw[73..120]) {
        return Slot::Bad;
    }
    let name_len = name_len as usize;
    let text_len = text_len as usize;
    if !all_zero(&raw[2 + name_len..14]) || !all_zero(&raw[15 + text_len..73]) {
        return Slot::Bad;
    }
    let stored = u64::from_le_bytes(raw[120..128].try_into().unwrap());
    if stored != seal(&raw[0..120]) {
        return Slot::Bad;
    }
    let name_bytes = &raw[2..2 + name_len];
    let text_bytes = &raw[15..15 + text_len];
    if !name_ok(name_bytes) || !text_ok(text_bytes) {
        return Slot::Bad;
    }
    let mut name = [0u8; 12];
    let mut text = [0u8; 58];
    name[..name_len].copy_from_slice(name_bytes);
    text[..text_len].copy_from_slice(text_bytes);
    Slot::Used {
        name,
        name_len: name_len as u8,
        text,
        text_len: text_len as u8,
    }
}

fn encode_slot(name: &[u8], text: &[u8]) -> [u8; 128] {
    let mut raw = [0u8; 128];
    raw[0] = 1;
    raw[1] = name.len() as u8;
    raw[2..2 + name.len()].copy_from_slice(name);
    raw[14] = text.len() as u8;
    raw[15..15 + text.len()].copy_from_slice(text);
    let check = seal(&raw[0..120]);
    raw[120..128].copy_from_slice(&check.to_le_bytes());
    raw
}

pub fn read_saved(dev: &mut dyn BlockDev) -> Result<Census, IoError> {
    let mut slots = [Slot::Empty; SAVED_SLOTS];
    let mut used = 0u32;
    for block_index in 0..4usize {
        let block = read_block(dev, 1 + block_index as u64)?;
        for inner in 0..4usize {
            let index = block_index * 4 + inner;
            let off = inner * 128;
            let mut raw = [0u8; 128];
            raw.copy_from_slice(&block[off..off + 128]);
            let slot = decode_slot(&raw);
            if matches!(slot, Slot::Used { .. }) {
                used += 1;
            }
            slots[index] = slot;
        }
    }
    Ok(Census { used, slots })
}

fn write_slot(dev: &mut dyn BlockDev, index: usize, raw: &[u8; 128]) -> Result<(), TextError> {
    let lba = slot_lba(index);
    let mut block = read_block(dev, lba).map_err(|_| TextError::Read)?;
    let off = slot_off(index);
    block[off..off + 128].copy_from_slice(raw);
    dev.write(lba, &block).map_err(|_| TextError::Write)?;
    dev.flush().map_err(|_| TextError::Write)?;
    Ok(())
}

fn same_name(slot: &Slot, name: &[u8]) -> bool {
    match slot {
        Slot::Used {
            name: stored,
            name_len,
            ..
        } => &stored[..*name_len as usize] == name,
        _ => false,
    }
}

pub fn save_text(dev: &mut dyn BlockDev, name: &[u8], text: &[u8]) -> Result<(), TextError> {
    if !name_ok(name) || !text_ok(text) {
        return Err(TextError::Missing);
    }
    let census = read_saved(dev).map_err(|_| TextError::Read)?;
    let mut empty = None;
    for (index, slot) in census.slots.iter().enumerate() {
        if same_name(slot, name) {
            return write_slot(dev, index, &encode_slot(name, text));
        }
        if empty.is_none() && matches!(slot, Slot::Empty) {
            empty = Some(index);
        }
    }
    let Some(index) = empty else {
        return Err(TextError::Full);
    };
    write_slot(dev, index, &encode_slot(name, text))
}

pub fn forget_text(dev: &mut dyn BlockDev, name: &[u8]) -> Result<(), TextError> {
    let census = read_saved(dev).map_err(|_| TextError::Read)?;
    for (index, slot) in census.slots.iter().enumerate() {
        if same_name(slot, name) {
            return write_slot(dev, index, &[0u8; 128]);
        }
    }
    Err(TextError::Missing)
}
