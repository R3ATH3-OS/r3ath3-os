//! Line-editing shell. See `docs/SHELL.md`, `docs/GATE.md`, and `docs/RECORDER.md`.
//!
//! Safe code only. The shell, `run`, the gate, and the flight recorder make no
//! heap allocation. The keyboard is the human input. COM1 RX is the agent
//! channel. Agent bytes never enter the keyboard ring, the line editor, or
//! `dispatch`. The model stays outside the kernel. The gate in this task
//! decides what an agent may do. Only `Decision::Run` and `Approved::Go` start
//! an agent action. Every gate outcome and every finished human command is
//! appended to the flight recorder, a module-private record in kernel memory.
//! That record is RAM-only and is lost on reboot.
//!
//! When the key ring is empty the shell calls `sched::yield_now`. With no
//! other ready task that returns at once and the shell polls again, so a key
//! is noticed within one tick (10 ms). With six `spin` tasks the shell runs
//! once per round of quanta, at most 70 ms.

use core::fmt::Write;

use core::sync::atomic::{AtomicU64, Ordering};

use crate::context::IrqCell;
use crate::frames;
use crate::gate::{
    self, Action, Approved, BadLine, DecideError, Decision, Demo, Gate, GrantError, Name, Pending,
    Perm, Refusal, RevokeError,
};
use crate::heap;
use crate::irq::{self, Key};
use crate::qemu_exit;
use crate::recorder::{self, Recorder};
use crate::sched::{self, SpawnError, WaitError};
use crate::script::{self, ScriptError, Sink};
use crate::selftest;
use crate::serial;
use crate::vga;

const MAX_LINE: usize = 76;
const PROMPT: &[u8] = b"r3> ";
const GATE_TIMEOUT_TICKS: u64 = 6000;
const LOG_CAP: usize = 128;
const AGENT_RESERVE: usize = 8;
const FULL_NOTICE: &[u8] = b"log: flight recorder full; later entries are counted as lost\n";

static LOG: IrqCell<Recorder<LOG_CAP>> = IrqCell::new(Recorder::new());

const HELP: [&str; 19] = [
    "help        list commands",
    "echo TEXT   print TEXT",
    "clear       clear the screen",
    "uptime      timer ticks and seconds since boot",
    "mem         heap and frame use",
    "ps          list tasks",
    "spawn DEMO  start demo task spin or nap",
    "wait ID     wait for a task to exit",
    "badges      list AI badges and the waiting request",
    "grant A P   give agent A permission P",
    "revoke A P  take permission P (or all) from agent A",
    "approve N   let gate request N run",
    "deny N      refuse gate request N",
    "log         newest flight-recorder entries and totals",
    "replay [N]  check the record and print it, or entry N",
    "selftest    run the feature checks and print a report card",
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

enum Incoming {
    Line([u8; 72], usize),
    TooLong,
}

struct AgentIn {
    buf: [u8; 72],
    len: usize,
    too_long: bool,
    ready: bool,
}

impl AgentIn {
    fn new() -> Self {
        Self {
            buf: [0; 72],
            len: 0,
            too_long: false,
            ready: false,
        }
    }

    /// Read COM1 until it is empty or one line is buffered. `\r` is ignored.
    fn pump(&mut self) {
        if self.ready {
            return;
        }
        while let Some(byte) = serial::try_recv() {
            if byte == b'\r' {
                continue;
            }
            if byte == b'\n' {
                self.ready = true;
                return;
            }
            if self.too_long || self.len == 72 {
                self.too_long = true;
                continue;
            }
            self.buf[self.len] = byte;
            self.len += 1;
        }
    }

    fn take_line(&mut self) -> Option<Incoming> {
        if !self.ready {
            return None;
        }
        let incoming = if self.too_long {
            Incoming::TooLong
        } else {
            let mut copy = [0u8; 72];
            copy[..self.len].copy_from_slice(&self.buf[..self.len]);
            Incoming::Line(copy, self.len)
        };
        self.len = 0;
        self.too_long = false;
        self.ready = false;
        Some(incoming)
    }
}

enum GateEvent<'a> {
    Asked {
        rq: u32,
        agent: &'a Name,
        seq: u32,
        verb: &'static str,
        explain: &'static str,
        action: Action,
    },
    Refused {
        agent: &'a Name,
        seq: u32,
        verb: &'static str,
        refusal: Refusal,
        action: Action,
    },
    Bad {
        reason: &'static str,
        agent: Option<&'a Name>,
        seq: Option<u32>,
    },
    Ran {
        agent: &'a Name,
        seq: u32,
        action: &'a Action,
    },
    Approved {
        pending: &'a Pending,
        id: Option<usize>,
    },
    ApproveFailed {
        pending: &'a Pending,
    },
    Denied {
        pending: &'a Pending,
    },
    Cancelled {
        pending: &'a Pending,
    },
    Expired {
        pending: &'a Pending,
    },
    Granted {
        agent: &'a Name,
        perm: &'static str,
    },
    Revoked {
        agent: &'a Name,
        what: &'static str,
    },
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
    let mut gate = Gate::new(GATE_TIMEOUT_TICKS);
    let mut agent = AgentIn::new();

    loop {
        if let Some(key) = irq::pop_key() {
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
                        record_human(&mut out, trim_spaces(&buf[..len]), last);
                    } else {
                        let line = trim_spaces(&buf[..len]);
                        if !line.is_empty() {
                            dispatch(&mut out, line, &mut last, &mut gate);
                            record_human(&mut out, line, last);
                        }
                    }
                    len = 0;
                    overflow = false;
                    out.both(PROMPT);
                }
            }
            continue;
        }
        agent.pump();
        if len == 0 && !overflow {
            // Expiry before the buffered agent line, so a flood of lines
            // cannot starve the timeout reply.
            if let Some(pending) = gate.poll(irq::ticks()) {
                out.both(b"\n");
                report(&mut out, &GateEvent::Expired { pending: &pending });
                out.both(PROMPT);
            } else if let Some(incoming) = agent.take_line() {
                out.both(b"\n");
                handle_agent_line(&mut out, &mut gate, incoming);
                out.both(PROMPT);
            }
        }
        sched::yield_now();
    }
}

fn dispatch(out: &mut Out, line: &[u8], last: &mut Last, gate: &mut Gate) {
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
        b"badges" => badges_cmd(out, rest, last, gate),
        b"grant" => grant_cmd(out, rest, last, gate),
        b"revoke" => revoke_cmd(out, rest, last, gate),
        b"approve" => approve_cmd(out, rest, last, gate),
        b"deny" => deny_cmd(out, rest, last, gate),
        b"log" => log_cmd(out, rest, last),
        b"replay" => replay_cmd(out, rest, last),
        b"selftest" => selftest_cmd(out, rest, last),
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

fn handle_agent_line(out: &mut Out, gate: &mut Gate, incoming: Incoming) {
    if !LOG.with(|log| log.admits_agent(AGENT_RESERVE)) {
        refuse_full(out, incoming);
        return;
    }
    let parsed = match incoming {
        Incoming::TooLong => Err((BadLine::TooLong, None)),
        Incoming::Line(buf, len) => gate::parse_request(&buf[..len]),
    };
    match parsed {
        Err((bad, id)) => {
            let (agent, seq) = match id.as_ref() {
                Some((name, seq)) => (Some(name), Some(*seq)),
                None => (None, None),
            };
            report(
                out,
                &GateEvent::Bad {
                    reason: bad.as_str(),
                    agent,
                    seq,
                },
            );
        }
        Ok(req) => {
            let agent = req.agent;
            let seq = req.seq;
            let verb = req.action.perm().as_str();
            match gate.ask(req, irq::ticks()) {
                Decision::Refused(refusal) => report(
                    out,
                    &GateEvent::Refused {
                        agent: &agent,
                        seq,
                        verb,
                        refusal,
                        action: req.action,
                    },
                ),
                Decision::Run(action) => report(
                    out,
                    &GateEvent::Ran {
                        agent: &agent,
                        seq,
                        action: &action,
                    },
                ),
                Decision::Pending { rq } => {
                    let explain = gate
                        .pending()
                        .map(|pending| pending.action.explain())
                        .unwrap_or("");
                    report(
                        out,
                        &GateEvent::Asked {
                            rq,
                            agent: &agent,
                            seq,
                            verb,
                            explain,
                            action: req.action,
                        },
                    );
                }
            }
        }
    }
}

fn badges_cmd(out: &mut Out, rest: &[u8], last: &mut Last, gate: &Gate) {
    if has_extra(rest) {
        usage(out, b"badges");
        *last = Last::Cmd;
        return;
    }
    let mut any = false;
    for slot in gate.badges().into_iter().flatten() {
        any = true;
        out.both(b"badge ");
        out.both(slot.0.as_bytes());
        out.both(b" may=");
        write_perms(out, slot.1);
        out.both(b"\n");
    }
    if !any {
        out.both(b"no badges\n");
    }
    match gate.pending() {
        Some(pending) => {
            let _ = write!(out, "pending rq={} agent=", pending.rq);
            out.both(pending.agent.as_bytes());
            out.both(b" action=");
            write_pending_action(out, pending.action);
            out.both(b"\n");
        }
        None => out.both(b"no pending request\n"),
    }
    *last = Last::Ok;
}

fn grant_cmd(out: &mut Out, rest: &[u8], last: &mut Last, gate: &mut Gate) {
    let Some((agent_bytes, perm_bytes)) = two_words(rest) else {
        usage(out, b"grant AGENT PERM");
        *last = Last::Cmd;
        return;
    };
    let Some(agent) = Name::parse(agent_bytes) else {
        bad_agent(out, agent_bytes);
        *last = Last::Cmd;
        return;
    };
    let Some(perm) = Perm::parse(perm_bytes) else {
        unknown_perm(out, perm_bytes);
        *last = Last::Cmd;
        return;
    };
    match gate.grant(agent, perm) {
        Ok(()) => {
            report(
                out,
                &GateEvent::Granted {
                    agent: &agent,
                    perm: perm.as_str(),
                },
            );
            *last = Last::Ok;
        }
        Err(GrantError::BadgeTableFull) => {
            out.both(b"error: badge table full\n");
            *last = Last::Cmd;
        }
    }
}

fn revoke_cmd(out: &mut Out, rest: &[u8], last: &mut Last, gate: &mut Gate) {
    let Some((agent_bytes, perm_bytes)) = two_words(rest) else {
        usage(out, b"revoke AGENT PERM");
        *last = Last::Cmd;
        return;
    };
    let Some(agent) = Name::parse(agent_bytes) else {
        bad_agent(out, agent_bytes);
        *last = Last::Cmd;
        return;
    };
    let (perm, what) = if perm_bytes == b"all" {
        (None, "all")
    } else {
        match Perm::parse(perm_bytes) {
            Some(perm) => (Some(perm), perm.as_str()),
            None => {
                unknown_perm(out, perm_bytes);
                *last = Last::Cmd;
                return;
            }
        }
    };
    match gate.revoke(&agent, perm) {
        Err(RevokeError::NoBadge) => {
            out.both(b"error: no badge ");
            out.both(agent_bytes);
            out.both(b"\n");
            *last = Last::Cmd;
        }
        Ok(cancelled) => {
            report(
                out,
                &GateEvent::Revoked {
                    agent: &agent,
                    what,
                },
            );
            if let Some(pending) = cancelled {
                report(out, &GateEvent::Cancelled { pending: &pending });
            }
            *last = Last::Ok;
        }
    }
}

fn approve_cmd(out: &mut Out, rest: &[u8], last: &mut Last, gate: &mut Gate) {
    let Some(word) = single_word(rest) else {
        usage(out, b"approve N");
        *last = Last::Cmd;
        return;
    };
    note_expiry(out, gate);
    if LOG.with(|log| log.is_full()) {
        out.both(b"error: flight recorder full: approve refused\n");
        *last = Last::Cmd;
        return;
    }
    let Some(rq) = parse_u32(word) else {
        no_pending(out, word);
        *last = Last::Cmd;
        return;
    };
    match gate.approve(rq) {
        Err(DecideError::NoPending) => {
            no_pending(out, word);
            *last = Last::Cmd;
        }
        Ok(Approved::Cancelled(pending)) => {
            report(out, &GateEvent::Cancelled { pending: &pending });
            *last = Last::Ok;
        }
        Ok(Approved::Go(pending)) => execute_go(out, pending, last),
    }
}

fn deny_cmd(out: &mut Out, rest: &[u8], last: &mut Last, gate: &mut Gate) {
    let Some(word) = single_word(rest) else {
        usage(out, b"deny N");
        *last = Last::Cmd;
        return;
    };
    note_expiry(out, gate);
    let Some(rq) = parse_u32(word) else {
        no_pending(out, word);
        *last = Last::Cmd;
        return;
    };
    match gate.deny(rq) {
        Err(DecideError::NoPending) => {
            no_pending(out, word);
            *last = Last::Cmd;
        }
        Ok(pending) => {
            report(out, &GateEvent::Denied { pending: &pending });
            *last = Last::Ok;
        }
    }
}

fn note_expiry(out: &mut Out, gate: &mut Gate) {
    if let Some(pending) = gate.poll(irq::ticks()) {
        report(out, &GateEvent::Expired { pending: &pending });
    }
}

/// `Approved::Go` is the only path that starts a privileged agent action.
fn execute_go(out: &mut Out, pending: Pending, last: &mut Last) {
    match pending.action {
        Action::Spawn(demo) => {
            let (name, entry): (&'static str, fn(usize)) = match demo {
                Demo::Spin => ("spin", spin_demo),
                Demo::Nap => ("nap", nap_demo),
            };
            match sched::spawn(name, entry, 0) {
                Ok(id) => {
                    report(
                        out,
                        &GateEvent::Approved {
                            pending: &pending,
                            id: Some(id),
                        },
                    );
                    *last = Last::Ok;
                }
                Err(SpawnError::Full) => {
                    report(out, &GateEvent::ApproveFailed { pending: &pending });
                    *last = Last::Cmd;
                }
            }
        }
        Action::Halt => {
            report(
                out,
                &GateEvent::Approved {
                    pending: &pending,
                    id: None,
                },
            );
            serial::write_str("R3ATH3_SHELL_HALT\n");
            qemu_exit::shell_halt();
        }
        Action::Read | Action::Say { .. } => panic!("approved a free verb"),
    }
}

fn report(out: &mut Out, ev: &GateEvent<'_>) {
    match ev {
        GateEvent::Asked {
            rq,
            agent,
            seq,
            verb,
            explain,
            action,
        } => {
            let _ = write!(out, "gate: request {rq} from ");
            out.both(agent.as_bytes());
            out.both(b" wants to:\n");
            out.both(b"gate:   ");
            out.both(explain.as_bytes());
            out.both(b"\n");
            let _ = write!(out, "gate: type approve {rq} or deny {rq} within 60 s\n");
            reply(
                Some(agent),
                Some(*seq),
                Some(verb),
                "pending",
                Some(*rq),
                None,
                None,
            );
            record_agent(
                out,
                Some(agent),
                recorder::event_body(
                    None,
                    action_word(*action),
                    Some(*seq),
                    Some(*rq),
                    "pending",
                    None,
                    None,
                    None,
                ),
            );
        }
        GateEvent::Refused {
            agent,
            seq,
            verb,
            refusal,
            action,
        } => {
            out.both(b"gate: refused ");
            out.both(agent.as_bytes());
            match refusal {
                Refusal::NoBadge => out.both(b": no badge\n"),
                Refusal::NoPermission(perm) => {
                    out.both(b": no permission to ");
                    out.both(perm.as_str().as_bytes());
                    out.both(b"\n");
                }
                Refusal::Busy => out.both(b": another request is waiting\n"),
            }
            reply(
                Some(agent),
                Some(*seq),
                Some(verb),
                "denied",
                None,
                None,
                Some(refusal.as_str()),
            );
            record_agent(
                out,
                Some(agent),
                recorder::event_body(
                    None,
                    action_word(*action),
                    Some(*seq),
                    None,
                    "denied",
                    None,
                    Some(refusal.as_str()),
                    None,
                ),
            );
        }
        GateEvent::Bad { reason, agent, seq } => {
            out.both(b"gate: ignored a bad request line (");
            out.both(reason.as_bytes());
            out.both(b")\n");
            reply(*agent, *seq, None, "bad", None, None, Some(reason));
            record_agent(
                out,
                *agent,
                recorder::event_body(None, "-", *seq, None, "bad", None, Some(reason), None),
            );
        }
        GateEvent::Ran { agent, seq, action } => match action {
            Action::Read => {
                out.both(b"gate: ");
                out.both(agent.as_bytes());
                out.both(b" read the work orders\n");
                for (index, text) in gate::WORK_ORDERS.iter().enumerate() {
                    let n = index + 1;
                    let _ = write!(serial::Writer, "R3GATE data seq={seq} n={n} text={text}\n");
                }
                reply(
                    Some(agent),
                    Some(*seq),
                    Some("read"),
                    "done",
                    None,
                    None,
                    None,
                );
                record_agent(
                    out,
                    Some(agent),
                    recorder::event_body(None, "read", Some(*seq), None, "done", None, None, None),
                );
            }
            Action::Say { text, len } => {
                out.both(agent.as_bytes());
                out.both(b" says: ");
                out.both(&text[..*len as usize]);
                out.both(b"\n");
                reply(
                    Some(agent),
                    Some(*seq),
                    Some("say"),
                    "done",
                    None,
                    None,
                    None,
                );
                record_agent(
                    out,
                    Some(agent),
                    recorder::event_body(
                        None,
                        "say",
                        Some(*seq),
                        None,
                        "done",
                        None,
                        None,
                        Some(&text[..*len as usize]),
                    ),
                );
            }
            Action::Spawn(_) | Action::Halt => {}
        },
        GateEvent::Approved { pending, id } => match pending.action {
            Action::Spawn(demo) => {
                if let Some(id) = id {
                    let _ = write!(
                        out,
                        "gate: request {} approved: started task {id} ({})\n",
                        pending.rq,
                        demo.as_str()
                    );
                    reply_pending(pending, "done", Some(*id), None);
                    record_gate(
                        out,
                        gate_event_body(pending, "done", Some(*id), None),
                    );
                }
            }
            Action::Halt => {
                let _ = write!(
                    out,
                    "gate: request {} approved: stopping the machine\n",
                    pending.rq
                );
                reply_pending(pending, "done", None, None);
                record_gate(out, gate_event_body(pending, "done", None, None));
            }
            Action::Read | Action::Say { .. } => {}
        },
        GateEvent::ApproveFailed { pending } => {
            let _ = write!(
                out,
                "gate: request {} approved, but the task table is full\n",
                pending.rq
            );
            reply_pending(pending, "failed", None, Some("full"));
            record_gate(out, gate_event_body(pending, "failed", None, Some("full")));
        }
        GateEvent::Denied { pending } => {
            let _ = write!(out, "gate: request {} denied\n", pending.rq);
            reply_pending(pending, "denied", None, Some("human"));
            record_gate(out, gate_event_body(pending, "denied", None, Some("human")));
        }
        GateEvent::Cancelled { pending } => {
            let _ = write!(out, "gate: request {} cancelled: ", pending.rq);
            out.both(pending.agent.as_bytes());
            out.both(b" may no longer ");
            out.both(pending.action.perm().as_str().as_bytes());
            out.both(b"\n");
            reply_pending(pending, "denied", None, Some("revoked"));
            record_gate(
                out,
                gate_event_body(pending, "denied", None, Some("revoked")),
            );
        }
        GateEvent::Expired { pending } => {
            let _ = write!(
                out,
                "gate: request {} expired after 60 s: denied\n",
                pending.rq
            );
            reply_pending(pending, "denied", None, Some("timeout"));
            record_gate(
                out,
                gate_event_body(pending, "denied", None, Some("timeout")),
            );
        }
        GateEvent::Granted { agent, perm } => {
            out.both(b"granted ");
            out.both(agent.as_bytes());
            out.both(b" ");
            out.both(perm.as_bytes());
            out.both(b"\n");
            record_words(out, b"grant ", agent.as_bytes(), perm.as_bytes());
        }
        GateEvent::Revoked { agent, what } => {
            out.both(b"revoked ");
            out.both(agent.as_bytes());
            out.both(b" ");
            out.both(what.as_bytes());
            out.both(b"\n");
            record_words(out, b"revoke ", agent.as_bytes(), what.as_bytes());
        }
    }
}

fn reply_pending(pending: &Pending, result: &str, id: Option<usize>, reason: Option<&str>) {
    reply(
        Some(&pending.agent),
        Some(pending.seq),
        Some(pending.action.perm().as_str()),
        result,
        Some(pending.rq),
        id,
        reason,
    );
}

fn reply(
    agent: Option<&Name>,
    seq: Option<u32>,
    verb: Option<&str>,
    result: &str,
    rq: Option<u32>,
    id: Option<usize>,
    reason: Option<&str>,
) {
    serial::write_str("R3GATE reply agent=");
    match agent {
        Some(name) => serial::write_bytes(name.as_bytes()),
        None => serial::write_str("-"),
    }
    serial::write_str(" seq=");
    match seq {
        Some(n) => {
            let _ = write!(serial::Writer, "{n}");
        }
        None => serial::write_str("-"),
    }
    serial::write_str(" verb=");
    serial::write_str(verb.unwrap_or("-"));
    serial::write_str(" result=");
    serial::write_str(result);
    if let Some(rq) = rq {
        let _ = write!(serial::Writer, " rq={rq}");
    }
    if let Some(id) = id {
        let _ = write!(serial::Writer, " id={id}");
    }
    if let Some(reason) = reason {
        serial::write_str(" reason=");
        serial::write_str(reason);
    }
    serial::write_str("\n");
}

fn write_perms(out: &mut Out, bits: u8) {
    if bits == 0 {
        out.both(b"none");
        return;
    }
    let mut first = true;
    for perm in [Perm::Read, Perm::Say, Perm::Spawn, Perm::Halt] {
        if bits & perm.bit() != 0 {
            if !first {
                out.both(b",");
            }
            first = false;
            out.both(perm.as_str().as_bytes());
        }
    }
}

fn write_pending_action(out: &mut Out, action: Action) {
    match action {
        Action::Spawn(demo) => {
            out.both(b"spawn ");
            out.both(demo.as_str().as_bytes());
        }
        Action::Halt => out.both(b"halt"),
        Action::Read => out.both(b"read"),
        Action::Say { .. } => out.both(b"say"),
    }
}

fn bad_agent(out: &mut Out, word: &[u8]) {
    out.both(b"error: bad agent name: ");
    out.both(word);
    out.both(b"\n");
}

fn unknown_perm(out: &mut Out, word: &[u8]) {
    out.both(b"error: unknown permission: ");
    out.both(word);
    out.both(b"\n");
}

fn no_pending(out: &mut Out, word: &[u8]) {
    out.both(b"error: no pending request ");
    out.both(word);
    out.both(b"\n");
}

fn last_word(last: Last) -> &'static str {
    match last {
        Last::Ok => "ok",
        Last::Cmd => "cmd",
        Last::Overflow => "overflow",
        Last::Script => "script",
    }
}

fn leave(last: Last) -> ! {
    let word = last_word(last);
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
        Ok(_) => {
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

fn two_words(rest: &[u8]) -> Option<(&[u8], &[u8])> {
    let text = skip_spaces(rest);
    if text.is_empty() {
        return None;
    }
    let mut end = 0usize;
    while end < text.len() && text[end] != b' ' {
        end += 1;
    }
    let first = &text[..end];
    let rest = skip_spaces(&text[end..]);
    if rest.is_empty() {
        return None;
    }
    let mut second_end = 0usize;
    while second_end < rest.len() && rest[second_end] != b' ' {
        second_end += 1;
    }
    if skip_spaces(&rest[second_end..]).is_empty() {
        Some((first, &rest[..second_end]))
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

fn parse_u32(word: &[u8]) -> Option<u32> {
    if word.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for &byte in word {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
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

fn action_word(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Say { .. } => "say",
        Action::Spawn(Demo::Nap) => "spawn-nap",
        Action::Spawn(Demo::Spin) => "spawn-spin",
        Action::Halt => "halt",
    }
}

fn gate_event_body(
    pending: &Pending,
    result: &str,
    id: Option<usize>,
    reason: Option<&str>,
) -> recorder::Body {
    recorder::event_body(
        Some(pending.agent.as_bytes()),
        action_word(pending.action),
        Some(pending.seq),
        Some(pending.rq),
        result,
        id,
        reason,
        None,
    )
}

/// Append one entry. Samples the tick, then appends inside `LOG.with`.
/// Prints the full notice after the closure when this append filled the record.
/// A body that overflowed, or a tag or body `append` rejects, is counted lost.
fn record(out: &mut Out, tag: &[u8], body: &[u8]) {
    let tick = irq::ticks();
    let notice = LOG.with(|log| match log.append(tick, tag, body) {
        Ok(n) => n as usize == log.capacity(),
        Err(_) => false,
    });
    if notice {
        out.both(FULL_NOTICE);
    }
}

fn record_body(out: &mut Out, tag: &[u8], body: &recorder::Body) {
    if body.overflow() {
        LOG.with(|log| log.note_lost());
        return;
    }
    record(out, tag, body.as_bytes());
}

fn record_agent(out: &mut Out, agent: Option<&Name>, body: recorder::Body) {
    let (tag, len) = agent_tag(agent);
    record_body(out, &tag[..len], &body);
}

fn record_gate(out: &mut Out, body: recorder::Body) {
    record_body(out, b"gate", &body);
}

fn record_words(out: &mut Out, kind: &[u8], agent: &[u8], what: &[u8]) {
    let mut buf = [0u8; 32];
    let mut n = 0usize;
    for &byte in kind.iter().chain(agent).chain(b" ".iter()).chain(what) {
        buf[n] = byte;
        n += 1;
    }
    record(out, b"gate", &buf[..n]);
}

fn record_human(out: &mut Out, line: &[u8], last: Last) {
    let (tag, len) = human_tag(last);
    record(out, &tag[..len], line);
}

fn human_tag(last: Last) -> ([u8; recorder::TAG_MAX], usize) {
    let word = last_word(last).as_bytes();
    let mut tag = [0u8; recorder::TAG_MAX];
    let prefix = b"human last=";
    tag[..prefix.len()].copy_from_slice(prefix);
    tag[prefix.len()..prefix.len() + word.len()].copy_from_slice(word);
    (tag, prefix.len() + word.len())
}

fn agent_tag(agent: Option<&Name>) -> ([u8; recorder::TAG_MAX], usize) {
    let mut tag = [0u8; recorder::TAG_MAX];
    tag[..6].copy_from_slice(b"agent ");
    match agent {
        Some(name) => {
            let bytes = name.as_bytes();
            tag[6..6 + bytes.len()].copy_from_slice(bytes);
            (tag, 6 + bytes.len())
        }
        None => {
            tag[6] = b'-';
            (tag, 7)
        }
    }
}

fn refuse_full(out: &mut Out, incoming: Incoming) {
    let (agent, seq, verb) = match incoming {
        Incoming::TooLong => (None, None, None),
        Incoming::Line(buf, len) => match gate::parse_request(&buf[..len]) {
            Ok(req) => (
                Some(req.agent),
                Some(req.seq),
                Some(req.action.perm().as_str()),
            ),
            Err((_, Some((name, seq)))) => (Some(name), Some(seq), None),
            Err((_, None)) => (None, None, None),
        },
    };
    out.both(b"gate: ignored an agent line: flight recorder full\n");
    reply(
        agent.as_ref(),
        seq,
        verb,
        "denied",
        None,
        None,
        Some("log-full"),
    );
    LOG.with(|log| log.note_lost());
}

fn log_cmd(out: &mut Out, rest: &[u8], last: &mut Last) {
    if has_extra(rest) {
        usage(out, b"log");
        *last = Last::Cmd;
        return;
    }
    let (len, lost, head) = LOG.with(|log| (log.len(), log.lost(), log.head()));
    let _ = write!(out, "log entries={len} of {LOG_CAP} lost={lost} head=");
    write_hex16(out, head);
    out.both(b"\n");
    let start = len.saturating_sub(5);
    for n in (start + 1)..=len {
        if let Some(entry) = LOG.with(|log| log.get(n as u32)) {
            print_entry(out, &entry);
        }
    }
    *last = Last::Ok;
}

fn replay_cmd(out: &mut Out, rest: &[u8], last: &mut Last) {
    let text = skip_spaces(rest);
    if text.is_empty() {
        replay_all(out, last);
        return;
    }
    let mut end = 0usize;
    while end < text.len() && text[end] != b' ' {
        end += 1;
    }
    if !skip_spaces(&text[end..]).is_empty() {
        usage(out, b"replay [N]");
        *last = Last::Cmd;
        return;
    }
    replay_one(out, &text[..end], last);
}

fn replay_all(out: &mut Out, last: &mut Last) {
    match LOG.with(|log| log.verify()) {
        recorder::Verify::Broken { at } => {
            for n in 1..at {
                if let Some(entry) = LOG.with(|log| log.get(n)) {
                    print_entry(out, &entry);
                }
            }
            let _ = write!(out, "replay: chain broken at #{at}\n");
            *last = Last::Cmd;
        }
        recorder::Verify::Ok => {
            let len = LOG.with(|log| log.len());
            for n in 1..=len {
                if let Some(entry) = LOG.with(|log| log.get(n as u32)) {
                    print_entry(out, &entry);
                }
            }
            let (len, lost, head) = LOG.with(|log| (log.len(), log.lost(), log.head()));
            let _ = write!(out, "replay ok entries={len} lost={lost} head=");
            write_hex16(out, head);
            out.both(b"\n");
            *last = Last::Ok;
        }
    }
}

fn replay_one(out: &mut Out, word: &[u8], last: &mut Last) {
    let len = LOG.with(|log| log.len());
    let Some(n) = parse_u32(word) else {
        no_entry(out, word);
        *last = Last::Cmd;
        return;
    };
    if n == 0 || n as usize > len {
        no_entry(out, word);
        *last = Last::Cmd;
        return;
    }
    match LOG.with(|log| log.verify_upto(n)) {
        recorder::Verify::Broken { at } => {
            let _ = write!(out, "replay: chain broken at #{at}\n");
            *last = Last::Cmd;
        }
        recorder::Verify::Ok => {
            if let Some(entry) = LOG.with(|log| log.get(n)) {
                print_entry(out, &entry);
            }
            let len = LOG.with(|log| log.len());
            let _ = write!(out, "replay step {n} of {len} ok\n");
            *last = Last::Ok;
        }
    }
}

fn no_entry(out: &mut Out, word: &[u8]) {
    out.both(b"error: no log entry ");
    out.both(word);
    out.both(b"\n");
}

fn selftest_cmd(out: &mut Out, rest: &[u8], last: &mut Last) {
    if has_extra(rest) {
        usage(out, b"selftest");
        *last = Last::Cmd;
        return;
    }
    let heap_before = heap::used();
    let live_ok = LOG.with(|log| log.verify()) == recorder::Verify::Ok;
    let card = selftest::run(heap_before, live_ok);
    selftest::write_card(out, &card);
    let ok = card.ok_count();
    let fail = card.fail_count();
    let body = selftest_body(ok, fail);
    record(out, b"selftest", &body);
    *last = if fail == 0 { Last::Ok } else { Last::Cmd };
}

fn selftest_body(ok: usize, fail: usize) -> [u8; 11] {
    let mut body = *b"ok=0 fail=0";
    body[3] = b'0' + ok as u8;
    body[10] = b'0' + fail as u8;
    body
}

fn print_entry(out: &mut Out, entry: &recorder::Entry) {
    let _ = write!(out, "#{} t={} ", entry.n(), entry.tick());
    out.both(entry.tag());
    out.both(b" link=");
    write_hex16(out, entry.link());
    out.both(b"\n  ");
    out.both(entry.body());
    out.both(b"\n");
}

fn write_hex16(out: &mut Out, value: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 16];
    for index in 0..16 {
        let shift = (15 - index) * 4;
        buf[index] = HEX[((value >> shift) & 0xf) as usize];
    }
    out.both(&buf);
}
