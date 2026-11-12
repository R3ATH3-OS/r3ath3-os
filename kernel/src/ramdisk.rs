//! RAM disk. See `docs/STORAGE.md`.
//!
//! 512 blocks of 512 bytes (256 KiB) in a module-private `IrqCell`. The
//! initializer is all zero, so the disk lives in `.bss` and is inside the
//! kernel image. A reboot erases it. `read` and `write` copy one block with
//! interrupts clear. `flush` returns immediately: the copy is already done.
//! Safe code only.

#![allow(dead_code)]

use crate::block::{BlockDev, IoError, BLOCK};
use crate::context::IrqCell;

pub const RAM_BLOCKS: usize = 512;

static DISK: IrqCell<[[u8; BLOCK]; RAM_BLOCKS]> = IrqCell::new([[0; BLOCK]; RAM_BLOCKS]);

/// Zero-sized handle. The bytes live in `DISK`.
#[derive(Clone, Copy)]
pub struct RamDisk;

impl BlockDev for RamDisk {
    fn blocks(&self) -> u64 {
        RAM_BLOCKS as u64
    }

    fn read(&mut self, lba: u64, buf: &mut [u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks() {
            return Err(IoError::Range);
        }
        let index = lba as usize;
        DISK.with(|disk| {
            buf.copy_from_slice(&disk[index]);
        });
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8; BLOCK]) -> Result<(), IoError> {
        if lba >= self.blocks() {
            return Err(IoError::Range);
        }
        let index = lba as usize;
        DISK.with(|disk| {
            disk[index].copy_from_slice(buf);
        });
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

/// Zero every block. Compiled only for the store proof.
#[cfg(feature = "store-core")]
pub fn wipe_for_test() {
    DISK.with(|disk| {
        for block in disk.iter_mut() {
            block.fill(0);
        }
    });
}
