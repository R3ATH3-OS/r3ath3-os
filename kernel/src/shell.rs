//! Line-editing shell. See `docs/SHELL.md`.
//!
//! Safe code only. The shell and `run` make no heap allocation. The keyboard
//! is the only input. There is no model, agent, network, or action gate.
//!
//! When the key ring is empty the shell calls `irq::idle_hlt`. A key that
//! arrives after the empty check and before `hlt` is picked up by the next
//! timer tick at the latest, which is 10 ms at 100 Hz.

use core::fmt::Write;

use crate::frames;
use crate::heap;
use crate::irq::{self, Key};
use crate::qemu_exit;
use crate::script::{self, ScriptError, Sink};
use crate::serial;
use crate::vga;

const MAX_LINE: usize = 76;
const PROMPT: &[u8] = b"r3> ";

const HELP: [&str; 8] = [
    "help        list commands",
    "echo TEXT   print TEXT",
    "clear       clear the screen",
    "uptime      timer ticks and seconds since boot",
    "mem         heap and frame use",
    "run SCRIPT  run a task script, lines separated by ;",
    "exit        leave the shell with the last status",
    "halt        stop the machine",
];

#[derive(Clone, Copy)]
enum Last {
    Ok,
    Cmd,
    Overflow,
    Script,
}

struct Out;

impl Out {
    fn both(&mut self, bytes: &[u8]) {
        serial::write_bytes(bytes);
        for &byte in bytes {
            vga::console_put(byte);
        }
    }
}

impl Write for Out {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.both(text.as_bytes());
        Ok(())
    }
}

impl Sink for Out {
    fn line(&mut self, text: &[u8]) {
        self.both(text);
        self.both(b"\n");
    }
}

pub fn run() -> ! {
    serial::write_str("R3ATH3_SHELL_READY\n");
    vga::console_clear();
    let mut out = Out;
    out.both(b"R3@TH3 shell. Type help for commands.\n");
    out.both(PROMPT);

    let mut buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    let mut overflow = false;
    let mut last = Last::Ok;

    loop {
        let key = loop {
            if let Some(key) = irq::pop_key() {
                break key;
            }
            irq::idle_hlt();
        };
        match key {
            Key::Char(ch) => {
                if len < MAX_LINE {
                    buf[len] = ch;
                    len += 1;
                    out.both(&[ch]);
                } else {
                    overflow = true;
                }
            }
            Key::Backspace => {
                if len > 0 {
                    len -= 1;
                    serial::write_bytes(b"\x08 \x08");
                    vga::console_backspace();
                }
            }
            Key::Enter => {
                out.both(b"\n");
                if overflow {
                    out.both(b"error: line too long (max 76)\n");
                    last = Last::Overflow;
                } else {
                    let line = trim_spaces(&buf[..len]);
                    if !line.is_empty() {
                        dispatch(&mut out, line, &mut last);
                    }
                }
                len = 0;
                overflow = false;
                out.both(PROMPT);
            }
        }
    }
}

fn dispatch(out: &mut Out, line: &[u8], last: &mut Last) {
    let (cmd, rest) = split_first(line);
    match cmd {
        b"help" => {
            if has_extra(rest) {
                usage(out, b"help");
                *last = Last::Cmd;
                return;
            }
            for line in HELP {
                out.both(line.as_bytes());
                out.both(b"\n");
            }
            *last = Last::Ok;
        }
        b"echo" => {
            echo(out, rest);
            *last = Last::Ok;
        }
        b"clear" => {
            if has_extra(rest) {
                usage(out, b"clear");
                *last = Last::Cmd;
                return;
            }
            serial::write_bytes(b"\x1b[2J\x1b[H");
            vga::console_clear();
            *last = Last::Ok;
        }
        b"uptime" => {
            if has_extra(rest) {
                usage(out, b"uptime");
                *last = Last::Cmd;
                return;
            }
            let ticks = irq::ticks();
            let seconds = ticks / u64::from(irq::HZ);
            let hundredths = ticks % u64::from(irq::HZ);
            let _ = write!(
                out,
                "uptime ticks={} hz={} seconds={}.{:02}\n",
                ticks, irq::HZ, seconds, hundredths
            );
            *last = Last::Ok;
        }
        b"mem" => {
            if has_extra(rest) {
                usage(out, b"mem");
                *last = Last::Cmd;
                return;
            }
            let (total, free) = frames::stats();
            let _ = write!(
                out,
                "mem heap_used={} heap_size={} frames_total={} frames_free={}\n",
                heap::used(),
                heap::SIZE,
                total,
                free
            );
            *last = Last::Ok;
        }
        b"run" => {
            let script = skip_spaces(rest);
            if script.is_empty() {
                usage(out, b"run SCRIPT");
                *last = Last::Cmd;
                return;
            }
            match script::run_inline(script, out) {
                Ok(()) => *last = Last::Ok,
                Err(error) => {
                    script_error(out, &error);
                    *last = Last::Script;
                }
            }
        }
        b"exit" => {
            if has_extra(rest) {
                usage(out, b"exit");
                *last = Last::Cmd;
                return;
            }
            leave(*last);
        }
        b"halt" => {
            if has_extra(rest) {
                usage(out, b"halt");
                *last = Last::Cmd;
                return;
            }
            serial::write_str("R3ATH3_SHELL_HALT\n");
            qemu_exit::shell_halt();
        }
        other => {
            out.both(b"error: unknown command: ");
            out.both(other);
            out.both(b"\n");
            *last = Last::Cmd;
        }
    }
}

fn leave(last: Last) -> ! {
    let word = match last {
        Last::Ok => "ok",
        Last::Cmd => "cmd",
        Last::Overflow => "overflow",
        Last::Script => "script",
    };
    serial::write_str("R3ATH3_SHELL_EXIT last=");
    serial::write_str(word);
    serial::write_str("\n");
    match last {
        Last::Ok => qemu_exit::shell_exit_ok(),
        Last::Cmd => qemu_exit::shell_exit_cmd(),
        Last::Overflow => qemu_exit::shell_exit_overflow(),
        Last::Script => qemu_exit::shell_exit_script(),
    }
}

fn script_error(out: &mut Out, error: &ScriptError) {
    let _ = write!(
        out,
        "error: script line={} reason={}\n",
        error.line, error.reason
    );
}

fn usage(out: &mut Out, text: &[u8]) {
    out.both(b"error: usage: ");
    out.both(text);
    out.both(b"\n");
}

fn echo(out: &mut Out, rest: &[u8]) {
    let mut i = 0usize;
    let mut any = false;
    while i < rest.len() {
        while i < rest.len() && rest[i] == b' ' {
            i += 1;
        }
        if i >= rest.len() {
            break;
        }
        let start = i;
        while i < rest.len() && rest[i] != b' ' {
            i += 1;
        }
        if any {
            out.both(b" ");
        }
        any = true;
        out.both(&rest[start..i]);
    }
    out.both(b"\n");
}

fn split_first(line: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0usize;
    while i < line.len() && line[i] != b' ' {
        i += 1;
    }
    (&line[..i], &line[i..])
}

fn has_extra(rest: &[u8]) -> bool {
    rest.iter().any(|byte| *byte != b' ')
}

fn skip_spaces(src: &[u8]) -> &[u8] {
    let mut i = 0usize;
    while i < src.len() && src[i] == b' ' {
        i += 1;
    }
    &src[i..]
}

fn trim_spaces(src: &[u8]) -> &[u8] {
    let mut start = 0usize;
    let mut end = src.len();
    while start < end && src[start] == b' ' {
        start += 1;
    }
    while end > start && src[end - 1] == b' ' {
        end -= 1;
    }
    &src[start..end]
}
