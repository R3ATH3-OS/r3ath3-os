//! Memory initialisation. Safe code only.
//!
//! # Invariants
//! `init` runs once, on the boot processor, after `exceptions::init` and
//! before anything uses `frames`. It parses the PVH memory map and then builds
//! the frame allocator from it. It prints nothing on success. The heap does
//! not depend on it. Spec: `docs/MEMORY.md`.

use crate::{frames, memmap};

/// Why memory initialisation failed. `reason` is the word printed on the
/// `R3ATH3_MEM_ERROR` line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemError {
    StartInfo,
    MemMap,
    NoUsable,
}

impl MemError {
    pub fn reason(self) -> &'static str {
        match self {
            MemError::StartInfo => "startinfo",
            MemError::MemMap => "memmap",
            MemError::NoUsable => "nousable",
        }
    }
}

pub fn init() -> Result<(), MemError> {
    memmap::init()?;
    frames::init()
}
