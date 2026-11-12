//! PCI configuration space on bus 0. See `docs/STORAGE.md`.
//!
//! # Invariants
//! This is a driver. Config cycles use port `0xCF8` (address) and port
//! `0xCFC` (data). Only the boot processor runs. The scan looks for class
//! `01:08:02` (NVMe). BAR0 must be a 64-bit memory BAR no larger than 2 MiB,
//! and it is placed at `0x3000_0000`.

use crate::port;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;
const ENABLE: u32 = 0x8000_0000;

pub struct Located {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub class_code: u32,
    pub bar_lo: u32,
    pub bar_hi: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PciError {
    Bar,
}

pub fn config_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let address = config_address(bus, dev, func, offset);
    unsafe {
        // SAFETY: `0xCF8` and `0xCFC` are the PCI configuration address and
        // data ports. Only the boot processor runs. `address` selects bus 0,
        // one device and function, and a 4-byte-aligned register.
        port::outl(CONFIG_ADDRESS, address);
        port::inl(CONFIG_DATA)
    }
}

pub fn config_write32(bus: u8, dev: u8, func: u8, offset: u8, value: u32) {
    let address = config_address(bus, dev, func, offset);
    unsafe {
        // SAFETY: same ports and same single-processor rule as
        // `config_read32`. The write hits one 4-byte config register.
        port::outl(CONFIG_ADDRESS, address);
        port::outl(CONFIG_DATA, value);
    }
}

/// First function on bus 0 whose class is `01:08:02`.
pub fn find_class() -> Option<Located> {
    for dev in 0..32 {
        if let Some(found) = inspect(dev, 0) {
            if is_nvme(found.class_code) {
                return Some(found);
            }
            let header = config_read32(0, dev, 0, 0x0C);
            let header_type = (header >> 16) & 0xFF;
            if header_type & 0x80 == 0 {
                continue;
            }
            for func in 1..8 {
                if let Some(found) = inspect(dev, func) {
                    if is_nvme(found.class_code) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

/// Size of the 64-bit BAR0. The original BAR and command values are restored.
pub fn size_bar0(loc: &Located) -> Result<u64, PciError> {
    if loc.bar_lo & 0x7 != 0x4 {
        return Err(PciError::Bar);
    }
    let command = config_read32(loc.bus, loc.dev, loc.func, 0x04);
    config_write32(loc.bus, loc.dev, loc.func, 0x04, command & !0x6);
    config_write32(loc.bus, loc.dev, loc.func, 0x10, 0xFFFF_FFFF);
    config_write32(loc.bus, loc.dev, loc.func, 0x14, 0xFFFF_FFFF);
    let lo = config_read32(loc.bus, loc.dev, loc.func, 0x10);
    let hi = config_read32(loc.bus, loc.dev, loc.func, 0x14);
    config_write32(loc.bus, loc.dev, loc.func, 0x10, loc.bar_lo);
    config_write32(loc.bus, loc.dev, loc.func, 0x14, loc.bar_hi);
    config_write32(loc.bus, loc.dev, loc.func, 0x04, command);
    let raw = ((hi as u64) << 32) | ((lo as u64) & !0xF);
    let size = (!raw).wrapping_add(1);
    if size == 0 || size > 2 * 1024 * 1024 {
        return Err(PciError::Bar);
    }
    Ok(size)
}

/// Program BAR0 to `0x3000_0000` and read it back. Returns the decoded address.
pub fn place_bar0(loc: &Located) -> Result<u32, PciError> {
    config_write32(loc.bus, loc.dev, loc.func, 0x10, 0x3000_0000);
    config_write32(loc.bus, loc.dev, loc.func, 0x14, 0);
    let lo = config_read32(loc.bus, loc.dev, loc.func, 0x10);
    let hi = config_read32(loc.bus, loc.dev, loc.func, 0x14);
    if (lo & !0xF) != 0x3000_0000 || hi != 0 {
        return Err(PciError::Bar);
    }
    Ok(lo & !0xF)
}

/// Memory space, bus master, and INTx disable.
pub fn enable(loc: &Located) {
    let command = config_read32(loc.bus, loc.dev, loc.func, 0x04);
    let enabled = command | (1 << 1) | (1 << 2) | (1 << 10);
    config_write32(loc.bus, loc.dev, loc.func, 0x04, enabled);
}

fn config_address(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    ENABLE
        | (u32::from(bus) << 16)
        | (u32::from(dev) << 11)
        | (u32::from(func) << 8)
        | (u32::from(offset) & 0xFC)
}

fn inspect(dev: u8, func: u8) -> Option<Located> {
    let id = config_read32(0, dev, func, 0x00);
    let vendor = (id & 0xFFFF) as u16;
    if vendor == 0xFFFF {
        return None;
    }
    let class_word = config_read32(0, dev, func, 0x08);
    let bar_lo = config_read32(0, dev, func, 0x10);
    let bar_hi = config_read32(0, dev, func, 0x14);
    Some(Located {
        bus: 0,
        dev,
        func,
        class_code: (class_word >> 8) & 0x00FF_FFFF,
        bar_lo,
        bar_hi,
    })
}

fn is_nvme(class_code: u32) -> bool {
    class_code == 0x01_08_02
}
