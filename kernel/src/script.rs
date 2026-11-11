//! Fixed-buffer task language. See `docs/TASK.md`.
//!
//! The interpreter does not allocate. Scripts, lines, names, and variables live
//! in stack arrays. This file stays in safe Rust.
//! COM1 is polled for the `script` build. After init, only IRQ0 (timer) and
//! IRQ1 (keyboard) are unmasked. The `shell` build calls `run_inline`.

#[cfg(any(feature = "script", feature = "kbd-script"))]
use crate::qemu_exit;
#[cfg(any(feature = "script", feature = "kbd-script"))]
use crate::serial;

const MAX_CMDS: usize = 24;
const MAX_LINE: usize = 72;
const MAX_VARS: usize = 8;
const MAX_NAME: usize = 8;
const MAX_TEXT: usize = 40;
const MAX_REPEAT: i64 = 16;
/// Physical lines read from COM1, including blanks, comments, and `run`.
const MAX_INPUT_LINES: u32 = 48;
/// Polls of the receive-buffer bit before a missing byte is `reason=timeout`.
#[cfg(feature = "script")]
const BYTE_WAIT_SPINS: u32 = 20_000_000;

const OP_SET: u8 = 1;
const OP_ADD: u8 = 2;
const OP_SUB: u8 = 3;
const OP_SAY: u8 = 4;
const OP_PRINT: u8 = 5;
const OP_REPEAT: u8 = 6;
const OP_END: u8 = 7;
const OP_HALT: u8 = 8;

#[derive(Clone, Copy)]
struct Cmd {
    op: u8,
    line: u32,
    name: [u8; MAX_NAME],
    name_len: u8,
    op_var: [u8; MAX_NAME],
    op_var_len: u8,
    op_is_var: bool,
    imm: i64,
    text: [u8; MAX_TEXT],
    text_len: u8,
}

impl Cmd {
    const fn empty() -> Self {
        Self {
            op: 0,
            line: 0,
            name: [0; MAX_NAME],
            name_len: 0,
            op_var: [0; MAX_NAME],
            op_var_len: 0,
            op_is_var: false,
            imm: 0,
            text: [0; MAX_TEXT],
            text_len: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Var {
    name: [u8; MAX_NAME],
    name_len: u8,
    value: i64,
    used: bool,
}

/// A parse or execute failure. `line` is 1-based. `reason` is one word.
pub struct ScriptError {
    pub line: u32,
    pub reason: &'static str,
}

/// Where `say` and `print` send one line of text, without a prefix or newline.
pub trait Sink {
    fn line(&mut self, text: &[u8]);
}

#[cfg(any(feature = "script", feature = "kbd-script"))]
pub fn run() -> ! {
    let mut prog = [Cmd::empty(); MAX_CMDS];
    #[cfg(feature = "kbd-script")]
    let count = {
        serial::write_str("R3ATH3_KBD_READY\n");
        read_program_kbd(&mut prog)
    };
    #[cfg(feature = "script")]
    let count = {
        serial::write_str("R3ATH3_SCRIPT_READY\n");
        read_program(&mut prog)
    };
    if let Err(error) = execute(&prog[..count], &mut SerialSink) {
        fail(error.line, error.reason);
    }
    serial::write_str("R3ATH3_SCRIPT_OK lines=");
    write_u32(count as u32);
    serial::write_str("\n");
    #[cfg(feature = "kbd-script")]
    qemu_exit::kbd_script_ok();
    #[cfg(feature = "script")]
    qemu_exit::script_ok();
}

#[cfg(feature = "script")]
fn read_program(prog: &mut [Cmd]) -> usize {
    let mut count = 0usize;
    let mut depth = 0u8;
    let mut line_no = 1u32;
    let mut buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    loop {
        let byte = match read_byte() {
            Some(b) => b,
            None => fail(line_no, "timeout"),
        };
        if byte == b'\r' {
            continue;
        }
        if byte != b'\n' {
            if len == MAX_LINE {
                fail(line_no, "limit");
            }
            buf[len] = byte;
            len += 1;
            continue;
        }
        match commit_line(&buf[..len], line_no, prog, &mut count, &mut depth, true) {
            Ok(true) => return count,
            Ok(false) => {}
            Err(error) => fail(error.line, error.reason),
        }
        line_no = line_no.saturating_add(1);
        len = 0;
    }
}

#[cfg(feature = "kbd-script")]
fn read_program_kbd(prog: &mut [Cmd]) -> usize {
    let mut count = 0usize;
    let mut depth = 0u8;
    let mut line_no = 1u32;
    let mut buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    loop {
        let event = loop {
            if let Some(event) = crate::irq::pop_key() {
                break event;
            }
            crate::irq::idle_hlt();
        };
        match event {
            crate::irq::Key::Char(ch) => {
                if len == MAX_LINE {
                    fail(line_no, "limit");
                }
                buf[len] = ch;
                len += 1;
                echo_line(&buf[..len]);
            }
            crate::irq::Key::Backspace => {
                if len > 0 {
                    len -= 1;
                }
                echo_line(&buf[..len]);
            }
            crate::irq::Key::Enter => {
                match commit_line(&buf[..len], line_no, prog, &mut count, &mut depth, true) {
                    Ok(true) => return count,
                    Ok(false) => {}
                    Err(error) => fail(error.line, error.reason),
                }
                line_no = line_no.saturating_add(1);
                len = 0;
            }
        }
    }
}

/// Run one shell line as a task script. `;` separates segments outside
/// double quotes. There is no `run` terminator: a segment that is `run` is
/// `reason=syntax`. An open `repeat` at the end is `reason=unclosed` on the
/// line after the last segment. `halt` stops the script and returns `Ok`.
#[cfg(feature = "shell")]
pub fn run_inline(src: &[u8], sink: &mut dyn Sink) -> Result<(), ScriptError> {
    let mut prog = [Cmd::empty(); MAX_CMDS];
    let mut count = 0usize;
    let mut depth = 0u8;
    let mut line_no = 1u32;
    let mut start = 0usize;
    let mut quoted = false;
    let mut i = 0usize;
    loop {
        let at_end = i == src.len();
        let split = !at_end && !quoted && src[i] == b';';
        if !at_end && src[i] == b'"' {
            quoted = !quoted;
        }
        if split || at_end {
            let segment = &src[start..i];
            if segment.len() > MAX_LINE {
                return Err(ScriptError {
                    line: line_no,
                    reason: "limit",
                });
            }
            commit_line(segment, line_no, &mut prog, &mut count, &mut depth, false)?;
            line_no = line_no.saturating_add(1);
            if at_end {
                break;
            }
            start = i + 1;
        }
        i += 1;
    }
    if depth != 0 {
        return Err(ScriptError {
            line: line_no,
            reason: "unclosed",
        });
    }
    execute(&prog[..count], sink)
}

fn commit_line(
    raw: &[u8],
    line_no: u32,
    prog: &mut [Cmd],
    count: &mut usize,
    depth: &mut u8,
    run_terminates: bool,
) -> Result<bool, ScriptError> {
    if line_no > MAX_INPUT_LINES {
        return Err(ScriptError {
            line: line_no,
            reason: "limit",
        });
    }
    let kept = strip_comment(raw);
    let trimmed = trim(&raw[..kept]);
    if trimmed == b"run" {
        if !run_terminates {
            return Err(ScriptError {
                line: line_no,
                reason: "syntax",
            });
        }
        if *depth != 0 {
            return Err(ScriptError {
                line: line_no,
                reason: "unclosed",
            });
        }
        return Ok(true);
    }
    if !trimmed.is_empty() {
        if *count == MAX_CMDS {
            return Err(ScriptError {
                line: line_no,
                reason: "limit",
            });
        }
        prog[*count] = parse_command(trimmed, line_no)?;
        match prog[*count].op {
            OP_REPEAT => {
                if *depth == 1 {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "nested",
                    });
                }
                *depth = 1;
            }
            OP_END => {
                if *depth == 0 {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "syntax",
                    });
                }
                *depth = 0;
            }
            _ => {}
        }
        *count += 1;
    }
    Ok(false)
}

#[cfg(feature = "kbd-script")]
fn echo_line(text: &[u8]) {
    serial::write_str("R3ATH3_ECHO ");
    serial::write_bytes(text);
    serial::write_str("\n");
    crate::vga::write_line(text);
}

fn parse_command(line: &[u8], line_no: u32) -> Result<Cmd, ScriptError> {
    let mut cmd = Cmd::empty();
    cmd.line = line_no;
    let mut words = Words::new(line);
    let op = match words.next() {
        Some(op) => op,
        None => {
            return Err(ScriptError {
                line: line_no,
                reason: "syntax",
            })
        }
    };
    if op == b"say" {
        cmd.op = OP_SAY;
        cmd.text_len = parse_say(words.rest(), line_no, &mut cmd.text)?;
        return Ok(cmd);
    }
    match op {
        b"set" => cmd.op = OP_SET,
        b"add" => cmd.op = OP_ADD,
        b"sub" => cmd.op = OP_SUB,
        b"print" => cmd.op = OP_PRINT,
        b"repeat" => cmd.op = OP_REPEAT,
        b"end" => cmd.op = OP_END,
        b"halt" => cmd.op = OP_HALT,
        _ => {
            return Err(ScriptError {
                line: line_no,
                reason: "unknown",
            })
        }
    }
    match cmd.op {
        OP_END | OP_HALT => {
            if words.next().is_some() {
                return Err(ScriptError {
                    line: line_no,
                    reason: "syntax",
                });
            }
        }
        OP_PRINT => {
            let name = match words.next() {
                Some(name) => name,
                None => {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "syntax",
                    })
                }
            };
            if words.next().is_some() {
                return Err(ScriptError {
                    line: line_no,
                    reason: "syntax",
                });
            }
            store_name(name, line_no, &mut cmd.name, &mut cmd.name_len)?;
        }
        OP_REPEAT => {
            let n = match words.next() {
                Some(n) => n,
                None => {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "syntax",
                    })
                }
            };
            if words.next().is_some() {
                return Err(ScriptError {
                    line: line_no,
                    reason: "syntax",
                });
            }
            cmd.imm = parse_i64(n, line_no)?;
            if cmd.imm < 0 {
                return Err(ScriptError {
                    line: line_no,
                    reason: "syntax",
                });
            }
            if cmd.imm > MAX_REPEAT {
                return Err(ScriptError {
                    line: line_no,
                    reason: "repeat",
                });
            }
        }
        OP_SET => {
            let name = match words.next() {
                Some(name) => name,
                None => {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "syntax",
                    })
                }
            };
            let value = match words.next() {
                Some(value) => value,
                None => {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "syntax",
                    })
                }
            };
            if words.next().is_some() {
                return Err(ScriptError {
                    line: line_no,
                    reason: "syntax",
                });
            }
            store_name(name, line_no, &mut cmd.name, &mut cmd.name_len)?;
            cmd.imm = parse_i64(value, line_no)?;
        }
        OP_ADD | OP_SUB => {
            let name = match words.next() {
                Some(name) => name,
                None => {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "syntax",
                    })
                }
            };
            let operand = match words.next() {
                Some(operand) => operand,
                None => {
                    return Err(ScriptError {
                        line: line_no,
                        reason: "syntax",
                    })
                }
            };
            if words.next().is_some() {
                return Err(ScriptError {
                    line: line_no,
                    reason: "syntax",
                });
            }
            store_name(name, line_no, &mut cmd.name, &mut cmd.name_len)?;
            if is_name(operand) {
                cmd.op_is_var = true;
                store_name(operand, line_no, &mut cmd.op_var, &mut cmd.op_var_len)?;
            } else {
                cmd.imm = parse_i64(operand, line_no)?;
            }
        }
        _ => {
            return Err(ScriptError {
                line: line_no,
                reason: "syntax",
            })
        }
    }
    Ok(cmd)
}

fn execute(cmds: &[Cmd], sink: &mut dyn Sink) -> Result<(), ScriptError> {
    let mut vars = [Var {
        name: [0; MAX_NAME],
        name_len: 0,
        value: 0,
        used: false,
    }; MAX_VARS];
    let mut ip = 0usize;
    let mut loop_back = 0usize;
    let mut left = 0i64;
    let mut active = false;
    while ip < cmds.len() {
        let cmd = cmds[ip];
        match cmd.op {
            OP_SET => {
                let slot = define(&mut vars, &cmd.name, cmd.name_len, cmd.line)?;
                vars[slot].value = cmd.imm;
                ip += 1;
            }
            OP_ADD | OP_SUB => {
                let slot = lookup(&vars, &cmd.name, cmd.name_len).ok_or(ScriptError {
                    line: cmd.line,
                    reason: "undefined",
                })?;
                let operand = if cmd.op_is_var {
                    let src = lookup(&vars, &cmd.op_var, cmd.op_var_len).ok_or(ScriptError {
                        line: cmd.line,
                        reason: "undefined",
                    })?;
                    vars[src].value
                } else {
                    cmd.imm
                };
                let next = if cmd.op == OP_ADD {
                    vars[slot].value.checked_add(operand)
                } else {
                    vars[slot].value.checked_sub(operand)
                };
                vars[slot].value = next.ok_or(ScriptError {
                    line: cmd.line,
                    reason: "overflow",
                })?;
                ip += 1;
            }
            OP_SAY => {
                sink.line(&cmd.text[..cmd.text_len as usize]);
                ip += 1;
            }
            OP_PRINT => {
                let slot = lookup(&vars, &cmd.name, cmd.name_len).ok_or(ScriptError {
                    line: cmd.line,
                    reason: "undefined",
                })?;
                let mut buf = [0u8; 40];
                let n = format_print(&cmd.name[..cmd.name_len as usize], vars[slot].value, &mut buf);
                sink.line(&buf[..n]);
                ip += 1;
            }
            OP_REPEAT => {
                let end = matching_end(cmds, ip).ok_or(ScriptError {
                    line: cmd.line,
                    reason: "unclosed",
                })?;
                if cmd.imm == 0 {
                    active = false;
                    ip = end + 1;
                    continue;
                }
                active = true;
                loop_back = ip + 1;
                left = cmd.imm;
                ip += 1;
            }
            OP_END => {
                if !active {
                    return Err(ScriptError {
                        line: cmd.line,
                        reason: "syntax",
                    });
                }
                left -= 1;
                if left > 0 {
                    ip = loop_back;
                } else {
                    active = false;
                    ip += 1;
                }
            }
            OP_HALT => return Ok(()),
            _ => {
                return Err(ScriptError {
                    line: cmd.line,
                    reason: "syntax",
                })
            }
        }
    }
    Ok(())
}

fn matching_end(cmds: &[Cmd], repeat_at: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut i = repeat_at + 1;
    while i < cmds.len() {
        match cmds[i].op {
            OP_REPEAT => depth += 1,
            OP_END => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn define(vars: &mut [Var], name: &[u8], name_len: u8, line: u32) -> Result<usize, ScriptError> {
    if let Some(slot) = lookup(vars, name, name_len) {
        return Ok(slot);
    }
    for (i, var) in vars.iter_mut().enumerate() {
        if !var.used {
            var.used = true;
            var.name_len = name_len;
            var.name[..name_len as usize].copy_from_slice(&name[..name_len as usize]);
            var.value = 0;
            return Ok(i);
        }
    }
    Err(ScriptError {
        line,
        reason: "limit",
    })
}

fn lookup(vars: &[Var], name: &[u8], name_len: u8) -> Option<usize> {
    vars.iter().position(|var| {
        var.used
            && var.name_len == name_len
            && var.name[..name_len as usize] == name[..name_len as usize]
    })
}

struct Words<'a> {
    src: &'a [u8],
    at: usize,
}

impl<'a> Words<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self { src, at: 0 }
    }

    fn next(&mut self) -> Option<&'a [u8]> {
        while self.at < self.src.len() && self.src[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
        if self.at >= self.src.len() {
            return None;
        }
        let start = self.at;
        while self.at < self.src.len() && !self.src[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
        Some(&self.src[start..self.at])
    }

    fn rest(&self) -> &'a [u8] {
        let mut at = self.at;
        while at < self.src.len() && self.src[at].is_ascii_whitespace() {
            at += 1;
        }
        &self.src[at..]
    }
}

fn parse_say(rest: &[u8], line: u32, dest: &mut [u8; MAX_TEXT]) -> Result<u8, ScriptError> {
    if rest.first() != Some(&b'"') {
        return Err(ScriptError {
            line,
            reason: "syntax",
        });
    }
    let mut i = 1usize;
    let mut n = 0usize;
    while i < rest.len() && rest[i] != b'"' {
        if n == MAX_TEXT {
            return Err(ScriptError {
                line,
                reason: "limit",
            });
        }
        if !(0x20..=0x7E).contains(&rest[i]) {
            return Err(ScriptError {
                line,
                reason: "char",
            });
        }
        dest[n] = rest[i];
        n += 1;
        i += 1;
    }
    if i >= rest.len() || rest[i] != b'"' {
        return Err(ScriptError {
            line,
            reason: "syntax",
        });
    }
    let mut tail = i + 1;
    while tail < rest.len() {
        if !rest[tail].is_ascii_whitespace() {
            return Err(ScriptError {
                line,
                reason: "syntax",
            });
        }
        tail += 1;
    }
    Ok(n as u8)
}

fn store_name(src: &[u8], line: u32, dest: &mut [u8; MAX_NAME], len: &mut u8) -> Result<(), ScriptError> {
    if src.len() > MAX_NAME {
        return Err(ScriptError {
            line,
            reason: "limit",
        });
    }
    if !is_name(src) {
        return Err(ScriptError {
            line,
            reason: "syntax",
        });
    }
    dest[..src.len()].copy_from_slice(src);
    *len = src.len() as u8;
    Ok(())
}

fn is_name(src: &[u8]) -> bool {
    if src.is_empty() {
        return false;
    }
    let first = src[0];
    if !first.is_ascii_alphabetic() && first != b'_' {
        return false;
    }
    src[1..]
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn parse_i64(src: &[u8], line: u32) -> Result<i64, ScriptError> {
    if src == b"-9223372036854775808" {
        return Ok(i64::MIN);
    }
    let (neg, digits) = if src.first() == Some(&b'-') {
        (true, &src[1..])
    } else {
        (false, src)
    };
    if digits.is_empty() || !digits.iter().all(|c| c.is_ascii_digit()) {
        return Err(ScriptError {
            line,
            reason: "syntax",
        });
    }
    let mut n: i64 = 0;
    for &d in digits {
        n = n.checked_mul(10).ok_or(ScriptError {
            line,
            reason: "overflow",
        })?;
        n = n.checked_add(i64::from(d - b'0')).ok_or(ScriptError {
            line,
            reason: "overflow",
        })?;
    }
    if neg {
        n.checked_neg().ok_or(ScriptError {
            line,
            reason: "overflow",
        })
    } else {
        Ok(n)
    }
}

fn strip_comment(src: &[u8]) -> usize {
    let mut quoted = false;
    for (i, &b) in src.iter().enumerate() {
        if b == b'"' {
            quoted = !quoted;
        } else if b == b'#' && !quoted {
            return i;
        }
    }
    src.len()
}

fn trim(src: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = src.len();
    while start < end && src[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && src[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &src[start..end]
}

#[cfg(feature = "script")]
fn read_byte() -> Option<u8> {
    for _ in 0..BYTE_WAIT_SPINS {
        if let Some(byte) = serial::try_recv() {
            return Some(byte);
        }
    }
    None
}

fn format_print(name: &[u8], value: i64, dest: &mut [u8]) -> usize {
    dest[..name.len()].copy_from_slice(name);
    dest[name.len()] = b'=';
    let wrote = format_i64(value, &mut dest[name.len() + 1..]);
    name.len() + 1 + wrote
}

fn format_i64(value: i64, dest: &mut [u8]) -> usize {
    if value == i64::MIN {
        let text = b"-9223372036854775808";
        dest[..text.len()].copy_from_slice(text);
        return text.len();
    }
    let neg = value < 0;
    let mut n = if neg { (-value) as u64 } else { value as u64 };
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    loop {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut at = 0usize;
    if neg {
        dest[0] = b'-';
        at = 1;
    }
    let digits = &tmp[i..];
    dest[at..at + digits.len()].copy_from_slice(digits);
    at + digits.len()
}

#[cfg(any(feature = "script", feature = "kbd-script"))]
struct SerialSink;

#[cfg(any(feature = "script", feature = "kbd-script"))]
impl Sink for SerialSink {
    fn line(&mut self, text: &[u8]) {
        serial::write_str("R3ATH3_OUT ");
        serial::write_bytes(text);
        serial::write_str("\n");
    }
}

#[cfg(any(feature = "script", feature = "kbd-script"))]
fn fail(line: u32, reason: &str) -> ! {
    serial::write_str("R3ATH3_SCRIPT_ERROR line=");
    write_u32(line);
    serial::write_str(" reason=");
    serial::write_str(reason);
    serial::write_str("\n");
    qemu_exit::script_error();
}

#[cfg(any(feature = "script", feature = "kbd-script"))]
fn write_u32(mut n: u32) {
    let mut buf = [0u8; 10];
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
