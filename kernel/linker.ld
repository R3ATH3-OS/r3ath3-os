ENTRY(_start)

SECTIONS
{
    . = 1M;
    __kernel_start = .;

    .text ALIGN(4K) : {
        *(.text.boot)
        *(.text .text.*)
        *(.rodata .rodata.*)
        *(.got .got.*)
        *(.data.rel.ro .data.rel.ro.*)
    }

    .note ALIGN(4) : {
        KEEP(*(.note.Xen))
    }

    .data ALIGN(4K) : {
        *(.data .data.*)
    }

    .bss ALIGN(4K) : {
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4K);
    }
    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment)
        *(.note.GNU-stack)
        *(.llvm_addrsig)
    }
}
