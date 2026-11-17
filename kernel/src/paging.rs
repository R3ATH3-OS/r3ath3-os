//! Map one physical range above 1 GiB with uncached 2 MiB pages.
//!
//! # Invariants
//! The boot stub identity-maps the first 1 GiB and leaves `pd_hi`, one spare
//! page directory, with no PDPT slot pointing at it. `map_uncached_2m` fills
//! that directory and exactly one PDPT entry (not slot 0). The range must
//! lie above 1 GiB and inside a single 1 GiB window. There is no general
//! page-table manager. One CPU. Called only for the linear framebuffer.

use core::ptr;
use core::sync::atomic::{AtomicU16, Ordering};

const GIB: u64 = 1 << 30;
const PAGE_2M: u64 = 2 << 20;
const PD_ENTRIES: usize = 512;
/// Present, writable, PWT, PCD, page-size. The physical address is OR-ed in.
const PTE_UNCACHED_2M: u64 = 0x9B;
const PDPT_PRESENT: u64 = 0x3;
/// No PDPT slot has claimed `pd_hi` yet. A real slot is 1..=511.
const SLOT_NONE: u16 = u16::MAX;

// SAFETY: `pdpt` and `pd_hi` are 4096-byte (512-entry) tables in `.bss.boot`.
// The boot stub zeroes them before `kernel_main`. This declaration only
// names those symbols. The writes are in `map_uncached_2m`.
unsafe extern "C" {
    static mut pdpt: [u64; PD_ENTRIES];
    static mut pd_hi: [u64; PD_ENTRIES];
}

/// Which PDPT slot currently points at `pd_hi`. There is one spare directory,
/// so a second call for a different slot is refused.
static MAPPED_SLOT: AtomicU16 = AtomicU16::new(SLOT_NONE);

/// Map `[phys, phys+len)` as uncached 2 MiB pages. `false` if the range is
/// empty, below 1 GiB, or does not fit in one spare PDPT slot.
pub fn map_uncached_2m(phys: u64, len: u64) -> bool {
    if len == 0 || phys < GIB {
        return false;
    }
    let Some(end) = phys.checked_add(len) else {
        return false;
    };
    if end <= phys {
        return false;
    }
    let slot = phys >> 30;
    if slot == 0 || slot >= PD_ENTRIES as u64 || ((end - 1) >> 30) != slot {
        return false;
    }
    let first = phys & !(PAGE_2M - 1);
    // Rounding the exclusive end up to 2 MiB must not wrap the address space.
    let Some(rounded) = end.checked_add(PAGE_2M - 1) else {
        return false;
    };
    let last = rounded & !(PAGE_2M - 1);
    if last < first {
        return false;
    }
    let slot_u = slot as u16;
    match MAPPED_SLOT.compare_exchange(SLOT_NONE, slot_u, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(prev) if prev == slot_u => {}
        Err(_) => return false,
    }
    // SAFETY: `pd_hi` and `pdpt` are 512-entry tables in `.bss.boot`, inside
    // the identity map. The boot stub zeroes them and writes only `pdpt[0]`.
    // `slot != 0`, so the low 1 GiB map stays. One spare directory exists.
    // A different slot was refused above, so this directory is not also
    // installed in another PDPT entry. One CPU. Each store is one 8-byte
    // entry, and `index` is masked to 0..512. CR3 is reloaded so the new
    // entries are visible. The addresses written are the caller's range
    // rounded out to 2 MiB, and that range was checked to sit in this slot.
    unsafe {
        let pd = ptr::addr_of_mut!(pd_hi).cast::<u64>();
        let mut index = 0;
        while index < PD_ENTRIES {
            pd.add(index).write_volatile(0);
            index += 1;
        }
        let mut addr = first;
        while addr < last {
            let pte = ((addr >> 21) & 0x1FF) as usize;
            pd.add(pte).write_volatile(addr | PTE_UNCACHED_2M);
            let Some(next) = addr.checked_add(PAGE_2M) else {
                return false;
            };
            addr = next;
        }
        let entry = (pd as u64) | PDPT_PRESENT;
        ptr::addr_of_mut!(pdpt)
            .cast::<u64>()
            .add(slot as usize)
            .write_volatile(entry);
        let cr3: u64;
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) cr3,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    }
    true
}
