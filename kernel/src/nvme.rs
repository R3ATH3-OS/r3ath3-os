//! Polled NVMe driver for QEMU's emulated controller. See `docs/STORAGE.md`.
//!
//! # Invariants
//! One controller, namespace 1, 512-byte blocks, one admin queue pair and one
//! I/O queue pair of 16 entries, one command at a time. No interrupts. BAR0
//! is the 2 MiB uncached window at `0x3000_0000`. Five frames from
//! `frames::alloc` hold the queues and the data buffer; they are not freed.
//! The CPU does not hold IF clear while it waits for the device.
//! This is QEMU's emulator and a host file. It is not a real disk.

use core::sync::atomic::{fence, Ordering};

use crate::block::{BlockDev, IoError, BLOCK};
use crate::context::IrqCell;
use crate::frames;
use crate::irq;
use crate::memmap;
use crate::pci::{self, PciError};
use crate::sched;

pub const MMIO_BASE: u64 = 0x3000_0000;
const WINDOW: u64 = 2 * 1024 * 1024;
const NVME_TIMEOUT_TICKS: u64 = 200;
const QDEPTH: u16 = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProbeError {
    NoDevice,
    Bar,
    Window,
    Css,
    PageSize,
    Queue,
    Timeout,
    Fatal,
    Identify,
    LbaSize,
}

impl ProbeError {
    pub fn word(self) -> &'static str {
        match self {
            ProbeError::NoDevice => "absent",
            ProbeError::Bar => "bar",
            ProbeError::Window => "window",
            ProbeError::Css => "css",
            ProbeError::PageSize => "page-size",
            ProbeError::Queue => "queue",
            ProbeError::Timeout => "timeout",
            ProbeError::Fatal => "fatal",
            ProbeError::Identify => "identify",
            ProbeError::LbaSize => "lba-size",
        }
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)] // the shell matches `Ok`/`Err`; `nvme-core` reads the fields
pub struct Info {
    pub blocks: u64,
    pub class_code: u32,
    pub bar0: u32,
    pub lbads: u8,
}

struct Controller {
    bar_size: u64,
    stride: u64,
    blocks: u64,
    admin_sq: u64,
    admin_cq: u64,
    io_sq: u64,
    io_cq: u64,
    data: u64,
    admin_sq_tail: u16,
    admin_cq_head: u16,
    admin_phase: u16,
    io_sq_tail: u16,
    io_cq_head: u16,
    io_phase: u16,
    next_cid: u16,
}

static NVME: IrqCell<Option<Controller>> = IrqCell::new(None);

pub struct NvmeDisk;

pub fn probe() -> Result<Info, ProbeError> {
    let loc = pci::find_class().ok_or(ProbeError::NoDevice)?;
    let bar_size = match pci::size_bar0(&loc) {
        Ok(size) => size,
        Err(PciError::Bar) => return Err(ProbeError::Bar),
    };
    let bar0 = match pci::place_bar0(&loc) {
        Ok(addr) => addr,
        Err(PciError::Bar) => return Err(ProbeError::Bar),
    };
    pci::enable(&loc);
    if ram_overlaps_window() {
        return Err(ProbeError::Window);
    }
    let cap = mmio_read64(0x00, bar_size);
    if (cap >> 37) & 1 == 0 {
        return Err(ProbeError::Css);
    }
    if (cap >> 48) & 0xF != 0 {
        return Err(ProbeError::PageSize);
    }
    let mqes = (cap & 0xFFFF) as u16;
    if mqes < 15 {
        return Err(ProbeError::Queue);
    }
    let dstrd = (cap >> 32) & 0xF;
    let stride = 4u64 << dstrd;
    let doorbell_end = 0x1000 + 3 * stride + 4;
    if doorbell_end > bar_size {
        return Err(ProbeError::Queue);
    }
    let cc = mmio_read32(0x14, bar_size);
    if cc & 1 != 0 {
        mmio_write32(0x14, bar_size, cc & !1);
        wait_rdy(bar_size, false)?;
    }
    let frames = alloc_queue_frames()?;
    mmio_write32(0x24, bar_size, (15 << 16) | 15);
    mmio_write64(0x28, bar_size, frames[0]);
    mmio_write64(0x30, bar_size, frames[1]);
    mmio_write32(0x14, bar_size, 0x0046_0001);
    wait_rdy(bar_size, true)?;
    if mmio_read32(0x1C, bar_size) & 2 != 0 {
        return Err(ProbeError::Fatal);
    }
    mmio_write32(0x0C, bar_size, 0xFFFF_FFFF);
    NVME.with(|slot| {
        *slot = Some(Controller {
            bar_size,
            stride,
            blocks: 0,
            admin_sq: frames[0],
            admin_cq: frames[1],
            io_sq: frames[2],
            io_cq: frames[3],
            data: frames[4],
            admin_sq_tail: 0,
            admin_cq_head: 0,
            admin_phase: 1,
            io_sq_tail: 0,
            io_cq_head: 0,
            io_phase: 1,
            next_cid: 0,
        });
    });
    let (nsze, lbads) = match identify(frames[4]) {
        Ok(value) => value,
        Err(error) => {
            drop_controller();
            return Err(error);
        }
    };
    if lbads != 9 {
        drop_controller();
        return Err(ProbeError::LbaSize);
    }
    if nsze == 0 {
        drop_controller();
        return Err(ProbeError::Identify);
    }
    if let Err(error) = create_io_queues(frames[2], frames[3]) {
        drop_controller();
        return Err(error);
    }
    NVME.with(|slot| {
        if let Some(ctl) = slot.as_mut() {
            ctl.blocks = nsze;
        }
    });
    Ok(Info {
        blocks: nsze,
        class_code: loc.class_code,
        bar0,
        lbads,
    })
}

#[cfg(feature = "nvme-core")]
pub fn read_unchecked(lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError> {
    io_read(lba, buf)
}

impl BlockDev for NvmeDisk {
    fn blocks(&self) -> u64 {
        NVME.with(|slot| slot.as_ref().map(|ctl| ctl.blocks).unwrap_or(0))
    }

    fn read(&mut self, lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks() {
            return Err(IoError::Range);
        }
        io_read(lba, buf)
    }

    fn write(&mut self, lba: u64, buf: &[u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks() {
            return Err(IoError::Range);
        }
        io_write(lba, buf)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        io_flush()
    }
}

fn drop_controller() {
    NVME.with(|slot| *slot = None);
}

fn alloc_queue_frames() -> Result<[u64; 5], ProbeError> {
    let mut frames = [0u64; 5];
    for slot in &mut frames {
        match frames::alloc() {
            Some(frame) => *slot = frame.addr(),
            None => return Err(ProbeError::Queue),
        }
    }
    Ok(frames)
}

fn ram_overlaps_window() -> bool {
    let end = MMIO_BASE + WINDOW;
    for region in memmap::regions() {
        if region.kind != memmap::KIND_RAM || region.size == 0 {
            continue;
        }
        let region_end = region.addr.saturating_add(region.size);
        if region.addr < end && region_end > MMIO_BASE {
            return true;
        }
    }
    false
}

fn wait_rdy(bar_size: u64, want: bool) -> Result<(), ProbeError> {
    let start = irq::ticks();
    loop {
        let csts = mmio_read32(0x1C, bar_size);
        if csts & 2 != 0 {
            return Err(ProbeError::Fatal);
        }
        if (csts & 1 != 0) == want {
            return Ok(());
        }
        if irq::ticks().wrapping_sub(start) >= NVME_TIMEOUT_TICKS {
            return Err(ProbeError::Timeout);
        }
        sched::yield_now();
    }
}

fn identify(data: u64) -> Result<(u64, u8), ProbeError> {
    if let Err(error) = admin(0x06, 1, data, 0, 0) {
        return Err(identify_error(error));
    }
    let mut hdr = [0u8; 32];
    dma_read(data, 0, &mut hdr);
    let mut nsze_bytes = [0u8; 8];
    nsze_bytes.copy_from_slice(&hdr[0..8]);
    let nsze = u64::from_le_bytes(nsze_bytes);
    let index = usize::from(hdr[26] & 0x0F);
    let mut lbaf = [0u8; 4];
    dma_read(data, 128 + 4 * index, &mut lbaf);
    Ok((nsze, lbaf[2]))
}

fn identify_error(error: IoError) -> ProbeError {
    let bar_size = NVME.with(|slot| slot.as_ref().map(|ctl| ctl.bar_size).unwrap_or(0));
    if bar_size != 0 && mmio_read32(0x1C, bar_size) & 2 != 0 {
        return ProbeError::Fatal;
    }
    match error {
        IoError::Timeout => ProbeError::Timeout,
        _ => ProbeError::Identify,
    }
}

fn create_io_queues(io_sq: u64, io_cq: u64) -> Result<(), ProbeError> {
    if let Err(error) = admin(0x05, 0, io_cq, (15 << 16) | 1, 1) {
        return Err(queue_error(error));
    }
    if let Err(error) = admin(0x01, 0, io_sq, (15 << 16) | 1, (1 << 16) | 1) {
        return Err(queue_error(error));
    }
    Ok(())
}

fn queue_error(error: IoError) -> ProbeError {
    match error {
        IoError::Timeout => ProbeError::Timeout,
        _ => ProbeError::Queue,
    }
}

fn admin(opcode: u8, nsid: u32, prp: u64, cdw10: u32, cdw11: u32) -> Result<(), IoError> {
    let mut sqe = [0u8; 64];
    sqe[0] = opcode;
    put_u32(&mut sqe, 4, nsid);
    put_u64(&mut sqe, 24, prp);
    put_u32(&mut sqe, 40, cdw10);
    put_u32(&mut sqe, 44, cdw11);
    let cid = submit(false, sqe)?;
    poll(false, cid)
}

fn io_read(lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError> {
    let data = data_frame()?;
    io_cmd(0x02, lba, data)?;
    dma_read(data, 0, buf);
    Ok(())
}

fn io_write(lba: u64, buf: &[u8; BLOCK]) -> Result<(), IoError> {
    let data = data_frame()?;
    dma_write(data, 0, buf);
    io_cmd(0x01, lba, data)
}

fn io_flush() -> Result<(), IoError> {
    io_cmd(0x00, 0, 0)
}

fn io_cmd(opcode: u8, lba: u64, prp: u64) -> Result<(), IoError> {
    let mut sqe = [0u8; 64];
    sqe[0] = opcode;
    put_u32(&mut sqe, 4, 1);
    put_u64(&mut sqe, 24, prp);
    put_u32(&mut sqe, 40, lba as u32);
    put_u32(&mut sqe, 44, (lba >> 32) as u32);
    let cid = submit(true, sqe)?;
    poll(true, cid)
}

fn data_frame() -> Result<u64, IoError> {
    NVME.with(|slot| slot.as_ref().map(|ctl| ctl.data).ok_or(IoError::Timeout))
}

fn submit(io: bool, mut sqe: [u8; 64]) -> Result<u16, IoError> {
    NVME.with(|slot| {
        let Some(ctl) = slot.as_mut() else {
            return Err(IoError::Timeout);
        };
        let cid = ctl.next_cid;
        ctl.next_cid = ctl.next_cid.wrapping_add(1);
        sqe[2] = cid as u8;
        sqe[3] = (cid >> 8) as u8;
        let (base, tail, db, bar) = if io {
            (
                ctl.io_sq,
                ctl.io_sq_tail,
                0x1000 + 2 * ctl.stride,
                ctl.bar_size,
            )
        } else {
            (ctl.admin_sq, ctl.admin_sq_tail, 0x1000, ctl.bar_size)
        };
        dma_write(base, usize::from(tail) * 64, &sqe);
        let new_tail = if tail + 1 == QDEPTH { 0 } else { tail + 1 };
        if io {
            ctl.io_sq_tail = new_tail;
        } else {
            ctl.admin_sq_tail = new_tail;
        }
        fence(Ordering::SeqCst);
        mmio_write32(db, bar, u32::from(new_tail));
        Ok(cid)
    })
}

fn poll(io: bool, cid: u16) -> Result<(), IoError> {
    let start = irq::ticks();
    loop {
        let done = NVME.with(|slot| {
            let Some(ctl) = slot.as_mut() else {
                return Some(Err(IoError::Timeout));
            };
            let (cq, head, phase, db, bar) = if io {
                (
                    ctl.io_cq,
                    ctl.io_cq_head,
                    ctl.io_phase,
                    0x1000 + 3 * ctl.stride,
                    ctl.bar_size,
                )
            } else {
                (
                    ctl.admin_cq,
                    ctl.admin_cq_head,
                    ctl.admin_phase,
                    0x1000 + ctl.stride,
                    ctl.bar_size,
                )
            };
            let mut entry = [0u8; 16];
            dma_read(cq, usize::from(head) * 16, &mut entry);
            let dw3 = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);
            let got_phase = ((dw3 >> 16) & 1) as u16;
            if got_phase != phase {
                return None;
            }
            let got_cid = (dw3 & 0xFFFF) as u16;
            let status = dw3 >> 17;
            let (new_head, new_phase) = if head + 1 == QDEPTH {
                (0, phase ^ 1)
            } else {
                (head + 1, phase)
            };
            if io {
                ctl.io_cq_head = new_head;
                ctl.io_phase = new_phase;
            } else {
                ctl.admin_cq_head = new_head;
                ctl.admin_phase = new_phase;
            }
            mmio_write32(db, bar, u32::from(new_head));
            if got_cid != cid {
                return Some(Err(IoError::Device(0x7, 0xFF)));
            }
            if status == 0 {
                Some(Ok(()))
            } else {
                let sc = (status & 0xFF) as u8;
                let sct = ((status >> 8) & 7) as u8;
                Some(Err(IoError::Device(sct, sc)))
            }
        });
        if let Some(result) = done {
            fence(Ordering::SeqCst);
            return result;
        }
        if irq::ticks().wrapping_sub(start) >= NVME_TIMEOUT_TICKS {
            return Err(IoError::Timeout);
        }
        sched::yield_now();
    }
}

fn put_u32(buf: &mut [u8], off: usize, value: u32) {
    buf[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(buf: &mut [u8], off: usize, value: u64) {
    buf[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

fn mmio_fits(off: u64, size: u64, bar_size: u64) -> bool {
    match off.checked_add(size) {
        Some(end) => off % size == 0 && end <= bar_size && end <= WINDOW,
        None => false,
    }
}

fn mmio_read32(off: u64, bar_size: u64) -> u32 {
    if !mmio_fits(off, 4, bar_size) {
        return 0xFFFF_FFFF;
    }
    unsafe {
        // SAFETY: `off` is 4-byte aligned and `MMIO_BASE + off` is inside the
        // sized BAR and inside the 2 MiB uncached window. The page is mapped.
        // The load is volatile so it reaches the controller.
        ((MMIO_BASE + off) as *const u32).read_volatile()
    }
}

fn mmio_write32(off: u64, bar_size: u64, value: u32) {
    if !mmio_fits(off, 4, bar_size) {
        return;
    }
    unsafe {
        // SAFETY: `off` is 4-byte aligned and `MMIO_BASE + off` is inside the
        // sized BAR and inside the 2 MiB uncached window. The page is mapped
        // writable. The store is volatile so it reaches the controller.
        ((MMIO_BASE + off) as *mut u32).write_volatile(value);
    }
}

fn mmio_read64(off: u64, bar_size: u64) -> u64 {
    if !mmio_fits(off, 8, bar_size) {
        return 0xFFFF_FFFF_FFFF_FFFF;
    }
    unsafe {
        // SAFETY: `off` is 8-byte aligned and both dwords lie in the sized
        // BAR and in the uncached window. The low dword is read first.
        let ptr = (MMIO_BASE + off) as *const u32;
        let lo = ptr.read_volatile();
        let hi = ptr.add(1).read_volatile();
        u64::from(lo) | (u64::from(hi) << 32)
    }
}

fn mmio_write64(off: u64, bar_size: u64, value: u64) {
    if !mmio_fits(off, 8, bar_size) {
        return;
    }
    unsafe {
        // SAFETY: `off` is 8-byte aligned and both dwords lie in the sized
        // BAR and in the uncached window. The low dword is written first,
        // which is what a 64-bit NVMe register requires.
        let ptr = (MMIO_BASE + off) as *mut u32;
        ptr.write_volatile(value as u32);
        ptr.add(1).write_volatile((value >> 32) as u32);
    }
}

fn dma_fits(off: usize, len: usize) -> bool {
    off.checked_add(len).is_some_and(|end| end <= 4096)
}

fn dma_write(frame: u64, off: usize, src: &[u8]) {
    if !dma_fits(off, src.len()) {
        return;
    }
    unsafe {
        // SAFETY: `frame` is a 4 KiB frame taken from `frames::alloc` by
        // `probe` and owned by this driver for the rest of the boot. `off`
        // plus the slice length stays inside that frame. The identity map
        // covers it. Volatile stores are what the controller's DMA reads.
        // Aligned dwords are stored whole so a queue entry is not torn.
        let dst = (frame as *mut u8).add(off);
        if off % 4 == 0 && src.len() % 4 == 0 {
            let words = dst as *mut u32;
            let count = src.len() / 4;
            for index in 0..count {
                let word = u32::from_le_bytes([
                    src[index * 4],
                    src[index * 4 + 1],
                    src[index * 4 + 2],
                    src[index * 4 + 3],
                ]);
                words.add(index).write_volatile(word);
            }
        } else {
            for (index, byte) in src.iter().enumerate() {
                dst.add(index).write_volatile(*byte);
            }
        }
    }
}

fn dma_read(frame: u64, off: usize, dst: &mut [u8]) {
    if !dma_fits(off, dst.len()) {
        return;
    }
    unsafe {
        // SAFETY: `frame` is a 4 KiB frame taken from `frames::alloc` by
        // `probe` and owned by this driver for the rest of the boot. `off`
        // plus the slice length stays inside that frame. The identity map
        // covers it. Volatile loads observe bytes the controller wrote.
        // An aligned dword is one load, so the completion's phase and CID
        // cannot come from two different updates.
        let src = (frame as *const u8).add(off);
        if off % 4 == 0 && dst.len() % 4 == 0 {
            let words = src as *const u32;
            let count = dst.len() / 4;
            for index in 0..count {
                let word = words.add(index).read_volatile().to_le_bytes();
                dst[index * 4..index * 4 + 4].copy_from_slice(&word);
            }
        } else {
            for (index, byte) in dst.iter_mut().enumerate() {
                *byte = src.add(index).read_volatile();
            }
        }
    }
}
