//! Line-editing shell. See `docs/SHELL.md`.
//!
//! Safe code only. The shell and `run` make no heap allocation. The keyboard
//! is the only input. There is no model, agent, network, or action gate.
//!
//! When the key ring is empty the shell calls `sched::yield_now`. With no
//! other ready task that returns at once and the shell polls again, so a key
//! is noticed within one tick (10 ms). With six `spin` tasks the shell runs
//! once per round of quanta, at most 70 ms.

use core::fmt::Write;

use core::sync::atomic::{AtomicU64, Ordering};

use crate::frames;
use crate::heap;
use crate::irq::{self, Key};
use crate::qemu_exit;
use crate::sched::{self, SpawnError, WaitError};
use crate::script::{self, ScriptError, Sink};
use crate::serial;
use crate::vga;

const MAX_LINE: usize = 76;
const PROMPT: &[u8] = b"r3> ";

const HELP: [&str; 11] = [
    "help        list commands",
    "echo TEXT   print TEXT",
    "clear       clear the screen",
    "uptime      timer ticks and seconds since boot",
    "mem         heap and frame use",
    "ps          list tasks",
    "spawn DEMO  start demo task spin or nap",
    "wait ID     wait for a task to exit",
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
            sched::yield_now();
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
                ticks,
                irq::HZ,
                seconds,
                hundredths
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
        b"ps" => {
            if has_extra(rest) {
                usage(out, b"ps");
                *last = Last::Cmd;
                return;
            }
            for slot in sched::snapshot().into_iter().flatten() {
                let _ = write!(
                    out,
                    "task id={} name={} state={}\n",
                    slot.id,
                    slot.name,
                    slot.state.as_str()
                );
            }
            *last = Last::Ok;
        }
        b"spawn" => spawn_demo(out, rest, last),
        b"wait" => wait_task(out, rest, last),
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

fn spawn_demo(out: &mut Out, rest: &[u8], last: &mut Last) {
    let Some(word) = single_word(rest) else {
        usage(out, b"spawn DEMO");
        *last = Last::Cmd;
        return;
    };
    let (name, entry): (&'static str, fn(usize)) = if word == b"spin" {
        ("spin", spin_demo)
    } else if word == b"nap" {
        ("nap", nap_demo)
    } else {
        out.both(b"error: unknown demo: ");
        out.both(word);
        out.both(b"\n");
        *last = Last::Cmd;
        return;
    };
    match sched::spawn(name, entry, 0) {
        Ok(id) => {
            let _ = write!(out, "spawned id={id} name={name}\n");
            *last = Last::Ok;
        }
        Err(SpawnError::Full) => {
            out.both(b"error: task table full\n");
            *last = Last::Cmd;
        }
    }
}

fn wait_task(out: &mut Out, rest: &[u8], last: &mut Last) {
    let Some(word) = single_word(rest) else {
        usage(out, b"wait ID");
        *last = Last::Cmd;
        return;
    };
    let Some(id) = parse_id(word) else {
        no_task(out, word);
        *last = Last::Cmd;
        return;
    };
    match sched::wait(id) {
        Ok(()) => {
            let _ = write!(out, "exited id={id}\n");
            *last = Last::Ok;
        }
        Err(WaitError::NoTask) => {
            no_task(out, word);
            *last = Last::Cmd;
        }
    }
}

fn no_task(out: &mut Out, word: &[u8]) {
    out.both(b"error: no task ");
    out.both(word);
    out.both(b"\n");
}

static SPIN_LOOPS: AtomicU64 = AtomicU64::new(0);

fn spin_demo(_: usize) {
    loop {
        SPIN_LOOPS.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

fn nap_demo(_: usize) {
    sched::sleep_ticks(20);
}

fn single_word(rest: &[u8]) -> Option<&[u8]> {
    let text = skip_spaces(rest);
    if text.is_empty() {
        return None;
    }
    let mut end = 0usize;
    while end < text.len() && text[end] != b' ' {
        end += 1;
    }
    if skip_spaces(&text[end..]).is_empty() {
        Some(&text[..end])
    } else {
        None
    }
}

fn parse_id(word: &[u8]) -> Option<usize> {
    if word.is_empty() {
        return None;
    }
    let mut value: usize = 0;
    for &byte in word {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as usize)?;
    }
    Some(value)
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
