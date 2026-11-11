//! Legacy PIC, PIT, and PS/2 keyboard.
//!
//! # Invariants
//! `enable` runs once, after the IDT has gates for vectors 32–47, on the boot
//! processor. IRQ0 and IRQ1 are the only unmasked lines. Handlers run with IF
//! clear and do not re-enable interrupts. The keyboard ring is single-producer
//! (the IRQ1 handler) and single-consumer (main). A full ring drops the event
//! and increments a counter.
//!
//! The PIT uses divisor 11932. The hardware clock is 1_193_182 Hz, so the
//! nominal rate is 100 Hz (1193182/11932 is about 99.995 Hz).

use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::port;
#[cfg(any(feature = "ticks", feature = "keys"))]
use crate::qemu_exit;
#[cfg(any(feature = "ticks", feature = "keys"))]
use crate::serial;

const PIC_M_CMD: u16 = 0x20;
const PIC_M_DATA: u16 = 0x21;
const PIC_S_CMD: u16 = 0xA0;
const PIC_S_DATA: u16 = 0xA1;
const PIT_CH0: u16 = 0x40;
const PIT_CMD: u16 = 0x43;
const PS2_DATA: u16 = 0x60;
const PS2_STATUS: u16 = 0x64;
#[cfg(feature = "ticks")]
const CMOS_INDEX: u16 = 0x70;
#[cfg(feature = "ticks")]
const CMOS_DATA: u16 = 0x71;

const PIT_DIVISOR: u16 = 11932;
#[cfg(any(feature = "ticks", feature = "shell"))]
pub const HZ: u32 = 100;
#[cfg(feature = "ticks")]
const TICK_WAIT: u64 = 300;
const KB_CAP: usize = 128;

static TICKS: AtomicU64 = AtomicU64::new(0);
static SHIFT_L: AtomicBool = AtomicBool::new(false);
static SHIFT_R: AtomicBool = AtomicBool::new(false);
static KB_HEAD: AtomicUsize = AtomicUsize::new(0);
static KB_TAIL: AtomicUsize = AtomicUsize::new(0);
static KB_DROPS: AtomicU32 = AtomicU32::new(0);
static KB_DATA: KbData = KbData(UnsafeCell::new([0; KB_CAP]));

struct KbData(UnsafeCell<[u8; KB_CAP]>);
unsafe impl Sync for KbData {}

#[cfg(any(feature = "keys", feature = "kbd-script", feature = "shell"))]
#[derive(Clone, Copy)]
pub enum Key {
    Char(u8),
    Backspace,
    Enter,
}

global_asm!(
    r#"
    .section .text.irq, "ax"
    .code64
    .global isr_32
    .global isr_33
    .global isr_34
    .global isr_35
    .global isr_36
    .global isr_37
    .global isr_38
    .global isr_39
    .global isr_40
    .global isr_41
    .global isr_42
    .global isr_43
    .global isr_44
    .global isr_45
    .global isr_46
    .global isr_47
    isr_32: pushq $0; pushq $32; jmp irq_common
    isr_33: pushq $0; pushq $33; jmp irq_common
    isr_34: pushq $0; pushq $34; jmp irq_common
    isr_35: pushq $0; pushq $35; jmp irq_common
    isr_36: pushq $0; pushq $36; jmp irq_common
    isr_37: pushq $0; pushq $37; jmp irq_common
    isr_38: pushq $0; pushq $38; jmp irq_common
    isr_39: pushq $0; pushq $39; jmp irq_common
    isr_40: pushq $0; pushq $40; jmp irq_common
    isr_41: pushq $0; pushq $41; jmp irq_common
    isr_42: pushq $0; pushq $42; jmp irq_common
    isr_43: pushq $0; pushq $43; jmp irq_common
    isr_44: pushq $0; pushq $44; jmp irq_common
    isr_45: pushq $0; pushq $45; jmp irq_common
    isr_46: pushq $0; pushq $46; jmp irq_common
    isr_47: pushq $0; pushq $47; jmp irq_common
    .global irq_common
    irq_common:
        cld
        pushq %rax
        pushq %rcx
        pushq %rdx
        pushq %rsi
        pushq %rdi
        pushq %r8
        pushq %r9
        pushq %r10
        pushq %r11
        movq 72(%rsp), %rdi
        call rust_irq_entry
        popq %r11
        popq %r10
        popq %r9
        popq %r8
        popq %rdi
        popq %rsi
        popq %rdx
        popq %rcx
        popq %rax
        addq $16, %rsp
        iretq
    "#,
    options(att_syntax)
);

pub fn enable() {
    unsafe {
        outb(PIC_M_DATA, 0xFF);
        outb(PIC_S_DATA, 0xFF);
        outb(PIC_M_CMD, 0x11);
        outb(PIC_S_CMD, 0x11);
        outb(PIC_M_DATA, 32);
        outb(PIC_S_DATA, 40);
        outb(PIC_M_DATA, 0x04);
        outb(PIC_S_DATA, 0x02);
        outb(PIC_M_DATA, 0x01);
        outb(PIC_S_DATA, 0x01);
        outb(PIC_M_DATA, 0xFF);
        outb(PIC_S_DATA, 0xFF);

        outb(PIT_CMD, 0x34);
        outb(PIT_CH0, PIT_DIVISOR as u8);
        outb(PIT_CH0, (PIT_DIVISOR >> 8) as u8);

        init_keyboard();
        outb(PIC_M_DATA, 0xFC);
        outb(PIC_S_DATA, 0xFF);
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(any(feature = "ticks", feature = "shell"))]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Acquire)
}

#[cfg(feature = "ticks")]
pub fn wait_ticks(n: u64) -> u64 {
    let start = ticks();
    loop {
        unsafe {
            asm!("cli", options(nomem, nostack, preserves_flags));
        }
        let got = ticks().wrapping_sub(start);
        if got >= n {
            return got;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    }
}

#[cfg(any(feature = "keys", feature = "kbd-script", feature = "shell"))]
pub fn pop_key() -> Option<Key> {
    let head = KB_HEAD.load(Ordering::Relaxed);
    if head == KB_TAIL.load(Ordering::Acquire) {
        return None;
    }
    let byte = unsafe { (*KB_DATA.0.get())[head] };
    KB_HEAD.store((head + 1) % KB_CAP, Ordering::Release);
    match byte {
        0x08 => Some(Key::Backspace),
        0x0A => Some(Key::Enter),
        ch => Some(Key::Char(ch)),
    }
}

/// Wait until the next interrupt. `irq::enable` has already run `sti`.
///
/// A key that arrives after `pop_key` returns `None` and before this `hlt`
/// retires is taken on the next interrupt. The PIT is 100 Hz, so that wait is
/// at most one tick, about 10 ms.
#[cfg(any(feature = "kbd-script", feature = "shell"))]
pub fn idle_hlt() {
    // SAFETY: `hlt` pauses this CPU until an interrupt. Interrupts are enabled.
    // The instruction does not read or write Rust memory and preserves flags.
    unsafe {
        asm!("hlt", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(feature = "ticks")]
pub fn ticks_demo() -> ! {
    let start_sec = rtc_seconds();
    let got = wait_ticks(TICK_WAIT);
    let end_sec = rtc_seconds();
    let delta = (u16::from(end_sec) + 60 - u16::from(start_sec)) % 60;
    serial::write_str("R3ATH3_TICKS ok count=");
    write_u64(got);
    serial::write_str(" hz=");
    write_u64(u64::from(HZ));
    serial::write_str(" rtc=");
    write_u64(u64::from(delta));
    serial::write_str("\n");
    qemu_exit::ticks_ok();
}

#[cfg(feature = "keys")]
pub fn keys_demo() -> ! {
    serial::write_str("R3ATH3_KEYS_READY\n");
    let mut text = [0u8; 32];
    let mut len = 0usize;
    loop {
        let Some(key) = pop_key() else {
            unsafe {
                asm!("hlt", options(nomem, nostack, preserves_flags));
            }
            continue;
        };
        match key {
            Key::Char(ch) => {
                if len < text.len() {
                    text[len] = ch;
                    len += 1;
                }
                serial::write_str("R3ATH3_KEY ");
                serial::write_bytes(&[ch]);
                serial::write_str("\n");
            }
            Key::Backspace => {
                if len > 0 {
                    len -= 1;
                }
                serial::write_str("R3ATH3_KEY bs\n");
            }
            Key::Enter => {
                serial::write_str("R3ATH3_KEY enter\n");
                serial::write_str("R3ATH3_KEYS ok text=");
                serial::write_bytes(&text[..len]);
                serial::write_str(" drops=");
                write_u64(u64::from(KB_DROPS.load(Ordering::Relaxed)));
                serial::write_str("\n");
                qemu_exit::keys_ok();
            }
        }
    }
}

#[no_mangle]
extern "C" fn rust_irq_entry(vector: u64) {
    match vector {
        32 => {
            TICKS.fetch_add(1, Ordering::Release);
            eoi(0);
        }
        33 => {
            on_keyboard_irq();
            eoi(1);
        }
        39 => {
            if master_isr() & 0x80 == 0 {
                return;
            }
            eoi(7);
        }
        47 => {
            if slave_isr() & 0x80 == 0 {
                eoi_master();
                return;
            }
            eoi(15);
        }
        34..=46 => eoi((vector - 32) as u8),
        _ => {}
    }
}

fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC_S_CMD, 0x20);
        }
        outb(PIC_M_CMD, 0x20);
    }
}

fn eoi_master() {
    unsafe { outb(PIC_M_CMD, 0x20) }
}

fn master_isr() -> u8 {
    unsafe {
        outb(PIC_M_CMD, 0x0B);
        inb(PIC_M_CMD)
    }
}

fn slave_isr() -> u8 {
    unsafe {
        outb(PIC_S_CMD, 0x0B);
        inb(PIC_S_CMD)
    }
}

fn init_keyboard() {
    unsafe {
        if !wait_in() {
            return;
        }
        outb(PS2_STATUS, 0xAD);
        while inb(PS2_STATUS) & 0x01 != 0 {
            let _ = inb(PS2_DATA);
        }
        if !wait_in() {
            return;
        }
        outb(PS2_STATUS, 0x20);
        if !wait_out() {
            return;
        }
        let mut cfg = inb(PS2_DATA);
        cfg = (cfg | 0x01 | 0x20 | 0x40) & !0x02 & !0x10;
        if !wait_in() {
            return;
        }
        outb(PS2_STATUS, 0x60);
        if !wait_in() {
            return;
        }
        outb(PS2_DATA, cfg);
        if !wait_in() {
            return;
        }
        outb(PS2_STATUS, 0xAE);
        while inb(PS2_STATUS) & 0x01 != 0 {
            let _ = inb(PS2_DATA);
        }
    }
}

fn on_keyboard_irq() {
    unsafe {
        let status = inb(PS2_STATUS);
        if status & 0x01 == 0 {
            return;
        }
        if status & 0x20 != 0 {
            let _ = inb(PS2_DATA);
            return;
        }
        decode(inb(PS2_DATA));
    }
}

fn decode(code: u8) {
    if code == 0xE0 {
        return;
    }
    if code & 0x80 != 0 {
        match code & 0x7F {
            0x2A => SHIFT_L.store(false, Ordering::Relaxed),
            0x36 => SHIFT_R.store(false, Ordering::Relaxed),
            _ => {}
        }
        return;
    }
    match code {
        0x2A => SHIFT_L.store(true, Ordering::Relaxed),
        0x36 => SHIFT_R.store(true, Ordering::Relaxed),
        0x0E => push_key(0x08),
        0x1C => push_key(0x0A),
        other => {
            if let Some(ch) = translate(other, shift_down()) {
                push_key(ch);
            }
        }
    }
}

fn shift_down() -> bool {
    SHIFT_L.load(Ordering::Relaxed) || SHIFT_R.load(Ordering::Relaxed)
}

fn translate(code: u8, shift: bool) -> Option<u8> {
    let ch = match code {
        0x02..=0x0B => {
            let digits = b"1234567890";
            let shifted = b"!@#$%^&*()";
            let i = (code - 0x02) as usize;
            if shift {
                shifted[i]
            } else {
                digits[i]
            }
        }
        0x10..=0x19 => fold_row(code, 0x10, b"qwertyuiop", shift),
        0x1E..=0x26 => fold_row(code, 0x1E, b"asdfghjkl", shift),
        0x2C..=0x32 => fold_row(code, 0x2C, b"zxcvbnm", shift),
        0x39 => b' ',
        0x0C => {
            if shift {
                b'_'
            } else {
                b'-'
            }
        }
        0x0D => {
            if shift {
                b'+'
            } else {
                b'='
            }
        }
        0x27 => {
            if shift {
                b':'
            } else {
                b';'
            }
        }
        0x28 => {
            if shift {
                b'"'
            } else {
                b'\''
            }
        }
        // Scan codes this table does not list produce no key and are ignored.
        _ => return None,
    };
    Some(ch)
}

fn fold_row(code: u8, start: u8, chars: &[u8], shift: bool) -> u8 {
    let lower = chars[(code - start) as usize];
    if shift {
        lower.to_ascii_uppercase()
    } else {
        lower
    }
}

fn push_key(byte: u8) {
    let tail = KB_TAIL.load(Ordering::Relaxed);
    let next = (tail + 1) % KB_CAP;
    if next == KB_HEAD.load(Ordering::Acquire) {
        KB_DROPS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    unsafe {
        (*KB_DATA.0.get())[tail] = byte;
    }
    KB_TAIL.store(next, Ordering::Release);
}

#[cfg(feature = "ticks")]
fn rtc_seconds() -> u8 {
    unsafe {
        for _ in 0..100_000 {
            if cmos(0x0A) & 0x80 == 0 {
                break;
            }
        }
        let binary = cmos(0x0B) & 0x04 != 0;
        let raw = cmos(0x00);
        if binary {
            raw
        } else {
            (raw & 0x0F) + ((raw >> 4) * 10)
        }
    }
}

#[cfg(feature = "ticks")]
unsafe fn cmos(index: u8) -> u8 {
    outb(CMOS_INDEX, index | 0x80);
    inb(CMOS_DATA)
}

fn wait_in() -> bool {
    for _ in 0..100_000 {
        unsafe {
            if inb(PS2_STATUS) & 0x02 == 0 {
                return true;
            }
        }
    }
    false
}

fn wait_out() -> bool {
    for _ in 0..100_000 {
        unsafe {
            if inb(PS2_STATUS) & 0x01 != 0 {
                return true;
            }
        }
    }
    false
}

unsafe fn outb(port: u16, value: u8) {
    port::outb(port, value);
}

unsafe fn inb(port: u16) -> u8 {
    port::inb(port)
}

#[cfg(any(feature = "ticks", feature = "keys"))]
fn write_u64(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    serial::write_bytes(&buf[i..]);
}
