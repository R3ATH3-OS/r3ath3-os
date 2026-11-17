//! 32-bit entry, identity map, and jump to 64-bit Rust.
//!
//! # Invariants
//! QEMU `-kernel` enters `_start` in ring 0, 32-bit protected mode, with
//! paging off and a flat code/data segment. The image publishes a Xen PVH
//! `PHYS32_ENTRY` note. That path is unchanged: `_start` stores `%ebx` in
//! `pvh_start_info` and builds the same page tables it always has.
//!
//! A Multiboot2 header sits at the start of `.text.boot` so GRUB can load
//! the same ELF. `_start_mb2` checks `%eax == 0x36D76289`, stores `%ebx` in
//! `mb2_info` and `%eax` in `mb2_magic`, then shares the page-table and
//! long-mode code. The header asks for a 1024×768×32 framebuffer and names
//! `_start_mb2` as the entry. GRUB loads the ELF at its physical addresses
//! (1 MiB). The framebuffer tag is optional, so a BIOS machine may stay in
//! text mode.
//!
//! The stub builds a 1 GiB identity map with 2 MiB pages, enables long mode,
//! and calls `kernel_main` on a 16 KiB stack. This stub leaves IF clear.
//! One entry is not write-back: the 2 MiB at `0x3000_0000` is uncached
//! (PWT and PCD). That page is the NVMe MMIO window. With `-m 64M` it is
//! not RAM. A fourth page, `pd_hi`, is a spare page directory. `paging`
//! points one PDPT slot at it to map a framebuffer above 1 GiB.
//!
//! QEMU's PVH entry passes the physical address of the `hvm_start_info`
//! struct in `%ebx`. Nothing else in the stub uses or changes it.
//! This stub's GDT has no TSS. `exceptions::init` loads the runtime GDT, TSS,
//! and IDT, remaps the 8259 PIC, and runs `sti`, with only IRQ0 (timer) and
//! IRQ1 (keyboard) unmasked. A fault before that init is a QEMU exit without
//! a fault line. If `kernel_main` returns, `boot_halt_return` runs. The
//! `hw` build does not write port `0xF4`.

use core::arch::global_asm;

global_asm!(
    r#"
    .section .note.Xen, "a", @note
    .align 4
    .long 4
    .long 4
    .long 18
    .asciz "Xen"
    .align 4
    .long _start

    .section .text.boot, "ax"
    .code32
    .align 8
    mb2_header:
        .long 0xE85250D6
        .long 0
        .long mb2_header_end - mb2_header
        .long 0x100000000 - (0xE85250D6 + (mb2_header_end - mb2_header))
    mb2_entry_tag:
        .short 3
        .short 0
        .long 12
        .long _start_mb2
        .long 0
    mb2_fb_tag:
        .short 5
        .short 1
        .long 20
        .long 1024
        .long 768
        .long 32
        .long 0
    mb2_end_tag:
        .short 0
        .short 0
        .long 8
    mb2_header_end:

    .global _start
    _start:
        cli
        movl %ebx, pvh_start_info
        jmp common_start

    .global _start_mb2
    _start_mb2:
        cli
        cmpl $0x36D76289, %eax
        jne mb2_reject
        movl %ebx, mb2_info
        movl %eax, mb2_magic
        jmp common_start
    mb2_reject:
        movl $0, mb2_info
        movl %eax, mb2_magic

    common_start:
        cld
        movw %ds, %ax
        movw %ax, %es
        movw %ax, %ss
        movl $stack_top, %esp

        movl $page_tables, %edi
        movl $(4 * 4096 / 4), %ecx
        xorl %eax, %eax
        rep stosl

        movl $pd, %edi
        movl $512, %ecx
        movl $0x83, %eax
    fill_pd:
        movl %eax, (%edi)
        movl $0, 4(%edi)
        addl $0x200000, %eax
        addl $8, %edi
        decl %ecx
        jnz fill_pd

        movl $0x3000009B, pd + 3072

        movl $pd, %eax
        orl $3, %eax
        movl %eax, pdpt
        movl $0, pdpt+4

        movl $pdpt, %eax
        orl $3, %eax
        movl %eax, pml4
        movl $0, pml4+4

        movl $pml4, %eax
        movl %eax, %cr3

        movl %cr4, %eax
        orl $0x20, %eax
        movl %eax, %cr4

        movl $0xC0000080, %ecx
        rdmsr
        orl $0x100, %eax
        wrmsr

        movl %cr0, %eax
        orl $0x80000001, %eax
        movl %eax, %cr0

        lgdt gdt_desc
        .byte 0xEA
        .long long_mode_start
        .word 0x08

    .code64
    long_mode_start:
        movw $0x10, %ax
        movw %ax, %ds
        movw %ax, %es
        movw %ax, %ss
        xorw %ax, %ax
        movw %ax, %fs
        movw %ax, %gs
        movq $stack_top, %rsp
        cld
        call kernel_main
        call boot_halt_return
    hang_after_return:
        hlt
        jmp hang_after_return

    .section .rodata.boot, "a"
    .align 8
    gdt64:
        .quad 0
        .quad 0x00209A0000000000
        .quad 0x0000920000000000
    gdt64_end:
    .align 4
    gdt_desc:
        .word gdt64_end - gdt64 - 1
        .long gdt64

    .section .bss.boot, "aw", @nobits
    .align 4096
    page_tables:
    pml4:
        .skip 4096
    .global pdpt
    pdpt:
        .skip 4096
    pd:
        .skip 4096
    .global pd_hi
    pd_hi:
        .skip 4096
    .align 16
    stack_bottom:
        .skip 16384
    stack_top:
    .align 4
    .global pvh_start_info
    pvh_start_info:
        .skip 4
    .global mb2_info
    mb2_info:
        .skip 4
    .global mb2_magic
    mb2_magic:
        .skip 4
    "#,
    options(att_syntax)
);

/// `kernel_main` returned. The `hw` build halts without touching port `0xF4`.
#[no_mangle]
pub extern "C" fn boot_halt_return() -> ! {
    #[cfg(not(feature = "hw"))]
    unsafe {
        // SAFETY: port 0xF4 is QEMU's isa-debug-exit device. This function is
        // compiled with the `out` only when the `hw` feature is off. One CPU.
        crate::port::outl(0xF4, 0x11);
    }
    crate::qemu_exit::halt_forever()
}
