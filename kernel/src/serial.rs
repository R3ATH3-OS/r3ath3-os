//! Polling driver for the QEMU emulated COM1 UART at `0x3F8`.
//!
//! # Invariants
//! `init` runs once, before any other serial write, with interrupts disabled.
//! Bytes are written only after the transmitter-holding-register bit is set.
//! `try_recv` polls the data-ready bit and never waits. There is no
//! interrupt-driven RX/TX and no second UART.

use core::fmt;

use crate::port;

const COM1: u16 = 0x3F8;

pub fn init() {
    unsafe {
        port::outb(COM1 + 1, 0x00);
        port::outb(COM1 + 3, 0x80);
        port::outb(COM1 + 0, 0x01);
        port::outb(COM1 + 1, 0x00);
        port::outb(COM1 + 3, 0x03);
        port::outb(COM1 + 2, 0xC7);
        port::outb(COM1 + 4, 0x0B);
    }
}

pub fn write_str(text: &str) {
    write_bytes(text.as_bytes());
}

pub fn write_bytes(bytes: &[u8]) {
    for &byte in bytes {
        write_byte(byte);
    }
}

/// One received byte, or `None` when the receive buffer is empty.
#[cfg(feature = "script")]
pub fn try_recv() -> Option<u8> {
    let ready = unsafe { port::inb(COM1 + 5) } & 0x01 != 0;
    if ready {
        Some(unsafe { port::inb(COM1) })
    } else {
        None
    }
}

pub struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        write_str(text);
        Ok(())
    }
}

fn write_byte(byte: u8) {
    let line_status = COM1 + 5;
    while unsafe { port::inb(line_status) } & 0x20 == 0 {}
    unsafe { port::outb(COM1, byte) };
}
