//! Minimal 64-bit exception entry.
//!
//! # Invariants
//! `init` runs once on the boot processor, after the 1 GiB identity map is on
//! and after COM1 is initialized. It loads the exception and IRQ gates, remaps
//! the 8259 PIC, and runs `sti`, with only IRQ0 (timer) and IRQ1 (keyboard)
//! unmasked.
//!
//! The runtime GDT replaces the boot stub's GDT and adds a 64-bit TSS.
//! `#DF` (vector 8) uses IST1, a dedicated 16 KiB stack inside the identity
//! map. That stack has no guard page. Other exceptions use the interrupted
//! stack. Handlers do not return.
//!
//! `unsafe` in this module covers every privileged step: stores into the GDT,
//! TSS, and IDT; `lgdt`, `ltr`, and `lidt`; the exception-frame read, the CR2
//! read, and the IST stack-pointer check; plus the smoke-only `ud2`, unmapped
//! read, and not-present gate edits.

use core::arch::{asm, global_asm};
use core::ptr;

use crate::qemu_exit;
use crate::serial;

#[cfg_attr(not(feature = "fault-pf"), allow(dead_code))]
const UNMAPPED: u64 = 0x4000_0000;
const KERNEL_CODE: u16 = 0x08;
const KERNEL_DATA_TSS_LIMIT: u16 = 5 * 8 - 1;
const IDT_LIMIT: u16 = 48 * 16 - 1;
const TSS_SELECTOR: u16 = 0x18;

global_asm!(
    r#"
    .section .text.exceptions, "ax"
    .code64
    .global load_gdt_and_tss
    load_gdt_and_tss:
        lgdt (%rdi)
        pushq $0x08
        leaq .Lreload_cs(%rip), %rax
        pushq %rax
        lretq
    .Lreload_cs:
        movw $0x10, %ax
        movw %ax, %ds
        movw %ax, %es
        movw %ax, %ss
        xorw %ax, %ax
        movw %ax, %fs
        movw %ax, %gs
        ltr %si
        ret

    .global exception_common
    exception_common:
        cld
        movq %rsp, %rdi
        subq $8, %rsp
        call rust_exception_entry
        ud2

    # Each stub is global so a later codegen-units change can still link it.
    .global isr_0
    .global isr_1
    .global isr_2
    .global isr_3
    .global isr_4
    .global isr_5
    .global isr_6
    .global isr_7
    .global isr_8
    .global isr_9
    .global isr_10
    .global isr_11
    .global isr_12
    .global isr_13
    .global isr_14
    .global isr_15
    .global isr_16
    .global isr_17
    .global isr_18
    .global isr_19
    .global isr_20
    .global isr_21
    .global isr_22
    .global isr_23
    .global isr_24
    .global isr_25
    .global isr_26
    .global isr_27
    .global isr_28
    .global isr_29
    .global isr_30
    .global isr_31

    isr_0:  pushq $0; pushq $0;  jmp exception_common
    isr_1:  pushq $0; pushq $1;  jmp exception_common
    isr_2:  pushq $0; pushq $2;  jmp exception_common
    isr_3:  pushq $0; pushq $3;  jmp exception_common
    isr_4:  pushq $0; pushq $4;  jmp exception_common
    isr_5:  pushq $0; pushq $5;  jmp exception_common
    isr_6:  pushq $0; pushq $6;  jmp exception_common
    isr_7:  pushq $0; pushq $7;  jmp exception_common
    isr_8:  pushq $8; jmp exception_common
    isr_9:  pushq $0; pushq $9;  jmp exception_common
    isr_10: pushq $10; jmp exception_common
    isr_11: pushq $11; jmp exception_common
    isr_12: pushq $12; jmp exception_common
    isr_13: pushq $13; jmp exception_common
    isr_14: pushq $14; jmp exception_common
    isr_15: pushq $0; pushq $15; jmp exception_common
    isr_16: pushq $0; pushq $16; jmp exception_common
    isr_17: pushq $17; jmp exception_common
    isr_18: pushq $0; pushq $18; jmp exception_common
    isr_19: pushq $0; pushq $19; jmp exception_common
    isr_20: pushq $0; pushq $20; jmp exception_common
    isr_21: pushq $21; jmp exception_common
    isr_22: pushq $0; pushq $22; jmp exception_common
    isr_23: pushq $0; pushq $23; jmp exception_common
    isr_24: pushq $0; pushq $24; jmp exception_common
    isr_25: pushq $0; pushq $25; jmp exception_common
    isr_26: pushq $0; pushq $26; jmp exception_common
    isr_27: pushq $0; pushq $27; jmp exception_common
    isr_28: pushq $0; pushq $28; jmp exception_common
    # AMD-only. #VC (29) and #SX (30) push a real error code; do not push 0.
    isr_29: pushq $29; jmp exception_common
    isr_30: pushq $30; jmp exception_common
    isr_31: pushq $0; pushq $31; jmp exception_common
    "#,
    options(att_syntax)
);

unsafe extern "C" {
    fn load_gdt_and_tss(gdtr: *const TablePointer, tss_selector: u16);
    fn isr_0();
    fn isr_1();
    fn isr_2();
    fn isr_3();
    fn isr_4();
    fn isr_5();
    fn isr_6();
    fn isr_7();
    fn isr_8();
    fn isr_9();
    fn isr_10();
    fn isr_11();
    fn isr_12();
    fn isr_13();
    fn isr_14();
    fn isr_15();
    fn isr_16();
    fn isr_17();
    fn isr_18();
    fn isr_19();
    fn isr_20();
    fn isr_21();
    fn isr_22();
    fn isr_23();
    fn isr_24();
    fn isr_25();
    fn isr_26();
    fn isr_27();
    fn isr_28();
    fn isr_29();
    fn isr_30();
    fn isr_31();
}

#[repr(C)]
struct ExceptionFrame {
    vector: u64,
    error: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const NULL: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn gate(handler: u64, ist: u8) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE,
            ist: ist & 0x7,
            type_attr: 0x8E,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, align(16))]
struct Idt {
    entries: [IdtEntry; 48],
}

#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    reserved2: u64,
    reserved3: u16,
    iopb: u16,
}

#[repr(C, packed)]
struct TablePointer {
    limit: u16,
    base: u64,
}

#[repr(C, align(16))]
struct AlignedTss(Tss);

#[repr(C, align(16))]
struct AlignedStack([u8; 16384]);

static mut IDT: Idt = Idt {
    entries: [IdtEntry::NULL; 48],
};
static mut TSS: AlignedTss = unsafe { core::mem::zeroed() };
static mut GDT: [u64; 5] = [0; 5];
static mut DF_STACK: AlignedStack = AlignedStack([0; 16384]);

pub fn init() {
    unsafe {
        let df_top = ptr::addr_of!(DF_STACK) as u64 + 16384;
        ptr::addr_of_mut!(TSS.0).write(Tss {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist1: df_top,
            ist2: 0,
            ist3: 0,
            ist4: 0,
            ist5: 0,
            ist6: 0,
            ist7: 0,
            reserved2: 0,
            reserved3: 0,
            iopb: core::mem::size_of::<Tss>() as u16,
        });

        let gdt = ptr::addr_of_mut!(GDT) as *mut u64;
        gdt.write(0);
        gdt.add(1).write(0x0020_9A00_0000_0000);
        gdt.add(2).write(0x0000_9200_0000_0000);
        let (tss_low, tss_high) = tss_descriptor(ptr::addr_of!(TSS.0) as u64);
        gdt.add(3).write(tss_low);
        gdt.add(4).write(tss_high);

        let gdtr = TablePointer {
            limit: KERNEL_DATA_TSS_LIMIT,
            base: ptr::addr_of!(GDT) as u64,
        };
        load_gdt_and_tss(ptr::addr_of!(gdtr), TSS_SELECTOR);

        let idt = ptr::addr_of_mut!(IDT.entries) as *mut IdtEntry;
        for vector in 0..32 {
            let ist = if vector == 8 { 1 } else { 0 };
            idt.add(vector)
                .write(IdtEntry::gate(stub_addr(vector), ist));
        }
        for vector in 32..48 {
            idt.add(vector)
                .write(IdtEntry::gate(irq_stub_addr(vector), 0));
        }
        let idtr = TablePointer {
            limit: IDT_LIMIT,
            base: ptr::addr_of!(IDT) as u64,
        };
        asm!(
            "lidt [rax]",
            in("rax") ptr::addr_of!(idtr),
            options(nostack, preserves_flags)
        );
    }
    crate::irq::enable();
}

#[cfg(feature = "fault-ud")]
pub fn trigger_ud() -> ! {
    unsafe {
        asm!("ud2", options(noreturn));
    }
}

#[cfg(feature = "fault-pf")]
pub fn trigger_pf() -> ! {
    unsafe {
        asm!(
            "mov al, [rax]",
            "ud2",
            in("rax") UNMAPPED,
            options(noreturn)
        );
    }
}

#[cfg(feature = "fault-df")]
pub fn trigger_df() -> ! {
    // Keep a valid interrupt-gate type and clear Present on #UD and #NP.
    // `ud2` cannot be delivered (#NP, IDT index 6). That #NP gate is also
    // not present, so the second contributory fault is #DF on IST1.
    // An unmapped RSP aborts QEMU 8.2 instead of reporting #DF.
    unsafe {
        let idt = ptr::addr_of_mut!(IDT.entries) as *mut IdtEntry;
        for vector in [6usize, 11usize] {
            let entry = idt.add(vector);
            let mut gate = entry.read();
            gate.type_attr = 0x0E;
            entry.write(gate);
        }
        asm!("ud2", options(noreturn));
    }
}

#[no_mangle]
extern "C" fn rust_exception_entry(frame: *const ExceptionFrame) -> ! {
    let frame = unsafe { &*frame };
    if frame.vector == 8 && !on_double_fault_ist() {
        qemu_exit::fail();
    }
    write_fault(frame);
    qemu_exit::cpu_fault(frame.vector);
}

fn on_double_fault_ist() -> bool {
    let rsp: u64;
    unsafe {
        asm!(
            "mov {}, rsp",
            out(reg) rsp,
            options(nomem, nostack, preserves_flags)
        );
    }
    let ist1 = unsafe {
        ptr::addr_of!(TSS.0)
            .cast::<u8>()
            .add(0x24)
            .cast::<u64>()
            .read_unaligned()
    };
    let bottom = ist1.saturating_sub(16384);
    rsp >= bottom && rsp < ist1
}

fn write_fault(frame: &ExceptionFrame) {
    serial::write_str("R3ATH3_FAULT vector=");
    write_dec(frame.vector);
    serial::write_str(" name=");
    serial::write_str(name(frame.vector));
    serial::write_str(" error=");
    if has_error_code(frame.vector) {
        write_hex(frame.error);
    } else {
        serial::write_str("none");
    }
    serial::write_str(" rip=");
    // For #DF the saved RIP is architecturally undefined. The line still
    // prints the value the CPU pushed; the smoke pattern only checks its shape.
    write_hex(frame.rip);
    if frame.vector == 14 {
        serial::write_str(" cr2=");
        write_hex(cr2());
    }
    serial::write_str("\n");
}

fn name(vector: u64) -> &'static str {
    match vector {
        0 => "de",
        6 => "ud",
        8 => "df",
        13 => "gp",
        14 => "pf",
        29 => "vc",
        30 => "sx",
        _ => "other",
    }
}

fn has_error_code(vector: u64) -> bool {
    // 29 (#VC) and 30 (#SX) are AMD-only and do push an error code.
    matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
}

fn cr2() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn write_dec(value: u64) {
    let mut buf = [0u8; 20];
    let mut n = value;
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    serial::write_str(core::str::from_utf8(&buf[i..]).unwrap_or("?"));
}

fn write_hex(value: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 18];
    let mut n = value;
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = HEX[(n & 0xf) as usize];
        n >>= 4;
        if n == 0 {
            break;
        }
    }
    i -= 2;
    buf[i] = b'0';
    buf[i + 1] = b'x';
    serial::write_str(core::str::from_utf8(&buf[i..]).unwrap_or("0x?"));
}

fn tss_descriptor(base: u64) -> (u64, u64) {
    let limit = (core::mem::size_of::<Tss>() - 1) as u64;
    let mut low = limit & 0xFFFF;
    low |= (base & 0x00FF_FFFF) << 16;
    low |= 0x89 << 40;
    low |= ((limit >> 16) & 0xF) << 48;
    low |= ((base >> 24) & 0xFF) << 56;
    (low, base >> 32)
}

unsafe extern "C" {
    fn isr_32();
    fn isr_33();
    fn isr_34();
    fn isr_35();
    fn isr_36();
    fn isr_37();
    fn isr_38();
    fn isr_39();
    fn isr_40();
    fn isr_41();
    fn isr_42();
    fn isr_43();
    fn isr_44();
    fn isr_45();
    fn isr_46();
    fn isr_47();
}

fn irq_stub_addr(vector: usize) -> u64 {
    let stub: unsafe extern "C" fn() = match vector {
        32 => isr_32,
        33 => isr_33,
        34 => isr_34,
        35 => isr_35,
        36 => isr_36,
        37 => isr_37,
        38 => isr_38,
        39 => isr_39,
        40 => isr_40,
        41 => isr_41,
        42 => isr_42,
        43 => isr_43,
        44 => isr_44,
        45 => isr_45,
        46 => isr_46,
        _ => isr_47,
    };
    stub as usize as u64
}

fn stub_addr(vector: usize) -> u64 {
    let stub: unsafe extern "C" fn() = match vector {
        0 => isr_0,
        1 => isr_1,
        2 => isr_2,
        3 => isr_3,
        4 => isr_4,
        5 => isr_5,
        6 => isr_6,
        7 => isr_7,
        8 => isr_8,
        9 => isr_9,
        10 => isr_10,
        11 => isr_11,
        12 => isr_12,
        13 => isr_13,
        14 => isr_14,
        15 => isr_15,
        16 => isr_16,
        17 => isr_17,
        18 => isr_18,
        19 => isr_19,
        20 => isr_20,
        21 => isr_21,
        22 => isr_22,
        23 => isr_23,
        24 => isr_24,
        25 => isr_25,
        26 => isr_26,
        27 => isr_27,
        28 => isr_28,
        29 => isr_29,
        30 => isr_30,
        _ => isr_31,
    };
    stub as u64
}
