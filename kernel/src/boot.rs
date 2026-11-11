//! 32-bit entry, identity map, and jump to 64-bit Rust.
//!
//! # Invariants
//! QEMU `-kernel` enters `_start` in ring 0, 32-bit protected mode, with
//! paging off and a flat code/data segment. The image publishes a Xen PVH
//! `PHYS32_ENTRY` note. QEMU 8.2 rejects an ELF64 Multiboot image, so this
//! milestone does not carry a Multiboot header.
//!
//! `_start` builds a 1 GiB identity map with 2 MiB pages, enables long mode,
//! and calls `kernel_main` on a 16 KiB stack. This stub leaves IF clear.
//!
//! QEMU's PVH entry passes the physical address of the `hvm_start_info`
//! struct in `%ebx`. `_start` stores it, as its first action after `cli`, in
//! the 4-byte `.bss.boot` symbol `pvh_start_info`. `memmap::init` reads and
//! validates it. Nothing else in the stub uses or changes it.
//! This stub's GDT has no TSS. `exceptions::init` loads the runtime GDT, TSS,
//! and IDT, remaps the 8259 PIC, and runs `sti`, with only IRQ0 (timer) and
//! IRQ1 (keyboard) unmasked. A fault before that init is a QEMU exit without
//! a fault line.

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
    .global _start
    _start:
        cli
        movl %ebx, pvh_start_info
        cld
        movw %ds, %ax
        movw %ax, %es
        movw %ax, %ss
        movl $stack_top, %esp

        movl $page_tables, %edi
        movl $(3 * 4096 / 4), %ecx
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
        movl $0x11, %eax
        movw $0xF4, %dx
        outl %eax, %dx
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
    pdpt:
        .skip 4096
    pd:
        .skip 4096
    .align 16
    stack_bottom:
        .skip 16384
    stack_top:
    .align 4
    .global pvh_start_info
    pvh_start_info:
        .skip 4
    "#,
    options(att_syntax)
);
