//! Block devices. See `docs/STORAGE.md`.
//!
//! Safe code only. A block is 512 bytes, addressed by an LBA from 0.
//! This module does no I/O of its own. The RAM disk implements `BlockDev`.

#![allow(dead_code)]

pub const BLOCK: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IoError {
    /// `lba >= blocks()`. The device was not touched.
    Range,
    /// Status code type and status code reported by the device.
    Device(u8, u8),
    /// No completion inside the driver's bound.
    Timeout,
}

pub trait BlockDev {
    fn blocks(&self) -> u64;
    fn read(&mut self, lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError>;
    fn write(&mut self, lba: u64, buf: &[u8; BLOCK]) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}
