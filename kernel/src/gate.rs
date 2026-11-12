//! Human gate. See `docs/GATE.md`.
//!
//! Safe code only. No I/O, no statics, and no allocation.
//! Time is the `now: u64` argument. This module decides. The shell is the
//! only code that prints a gate line or starts an approved action.
//!
//! Agent names are self-asserted: they are whatever bytes the request line
//! carries. A revocation does not stop a task an earlier approval already
//! started, and it does not undo output. Badges do not survive a reboot.
//! `next_rq` starts at 1 and uses wrapping addition. Wrapping is unreachable
//! in a real session.
#![allow(dead_code)]
// each gate feature uses a subset of this API

pub const MAX_BADGES: usize = 4;

pub const WORK_ORDERS: [&str; 3] = [
    "WO-1 check spindle oil level on mill 2",
    "WO-2 replace air filter on compressor A",
    "WO-3 log hours for weekly maintenance",
];

const READ_BIT: u8 = 1;
const SAY_BIT: u8 = 2;
const SPAWN_BIT: u8 = 4;
const HALT_BIT: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    Read,
    Say,
    Spawn,
    Halt,
}

impl Perm {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"read" => Some(Self::Read),
            b"say" => Some(Self::Say),
            b"spawn" => Some(Self::Spawn),
            b"halt" => Some(Self::Halt),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Say => "say",
            Self::Spawn => "spawn",
            Self::Halt => "halt",
        }
    }

    pub fn privileged(self) -> bool {
        matches!(self, Self::Spawn | Self::Halt)
    }

    pub fn bit(self) -> u8 {
        match self {
            Self::Read => READ_BIT,
            Self::Say => SAY_BIT,
            Self::Spawn => SPAWN_BIT,
            Self::Halt => HALT_BIT,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Demo {
    Spin,
    Nap,
}

impl Demo {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spin => "spin",
            Self::Nap => "nap",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Read,
    Say { text: [u8; 40], len: u8 },
    Spawn(Demo),
    Halt,
}

impl Action {
    pub fn perm(self) -> Perm {
        match self {
            Self::Read => Perm::Read,
            Self::Say { .. } => Perm::Say,
            Self::Spawn(_) => Perm::Spawn,
            Self::Halt => Perm::Halt,
        }
    }

    pub fn explain(self) -> &'static str {
        match self {
            Self::Spawn(Demo::Nap) => "start demo task nap (sleeps 0.2 s, then exits)",
            Self::Spawn(Demo::Spin) => "start demo task spin (runs forever; no way to stop it)",
            Self::Halt => "stop the machine (ends this session)",
            Self::Read | Self::Say { .. } => "",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; 8],
    len: u8,
}

impl Name {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > 8 {
            return None;
        }
        if !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        {
            return None;
        }
        let mut stored = [0u8; 8];
        stored[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            bytes: stored,
            len: bytes.len() as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub agent: Name,
    pub seq: u32,
    pub action: Action,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BadLine {
    Syntax,
    TooLong,
    Verb,
    Arg,
}

impl BadLine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::TooLong => "too-long",
            Self::Verb => "verb",
            Self::Arg => "arg",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NoBadge,
    NoPermission(Perm),
    Busy,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoBadge => "no-badge",
            Self::NoPermission(_) => "no-permission",
            Self::Busy => "busy",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Refused(Refusal),
    Run(Action),
    Pending { rq: u32 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    pub rq: u32,
    pub agent: Name,
    pub seq: u32,
    pub action: Action,
    pub deadline: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GrantError {
    BadgeTableFull,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RevokeError {
    NoBadge,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DecideError {
    NoPending,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Approved {
    Go(Pending),
    Cancelled(Pending),
}

#[derive(Clone, Copy)]
struct Badge {
    name: Name,
    perms: u8,
}

pub struct Gate {
    badges: [Option<Badge>; MAX_BADGES],
    pending: Option<Pending>,
    next_rq: u32,
    timeout: u64,
}

impl Gate {
    pub const fn new(timeout: u64) -> Self {
        Self {
            badges: [None; MAX_BADGES],
            pending: None,
            next_rq: 1,
            timeout,
        }
    }

    pub fn grant(&mut self, agent: Name, perm: Perm) -> Result<(), GrantError> {
        if let Some(slot) = self.slot_of(&agent) {
            if let Some(badge) = self.badges[slot].as_mut() {
                badge.perms |= perm.bit();
            }
            return Ok(());
        }
        let Some(slot) = self.badges.iter().position(|badge| badge.is_none()) else {
            return Err(GrantError::BadgeTableFull);
        };
        self.badges[slot] = Some(Badge {
            name: agent,
            perms: perm.bit(),
        });
        Ok(())
    }

    /// `perm == None` removes the badge. A pending request that no longer
    /// has its permission is cancelled in this call and returned.
    pub fn revoke(
        &mut self,
        agent: &Name,
        perm: Option<Perm>,
    ) -> Result<Option<Pending>, RevokeError> {
        let Some(slot) = self.slot_of(agent) else {
            return Err(RevokeError::NoBadge);
        };
        match perm {
            None => self.badges[slot] = None,
            Some(perm) => {
                if let Some(badge) = self.badges[slot].as_mut() {
                    badge.perms &= !perm.bit();
                }
            }
        }
        Ok(self.cancel_if_unauthorized(agent))
    }

    pub fn ask(&mut self, req: Request, now: u64) -> Decision {
        let perm = req.action.perm();
        if self.slot_of(&req.agent).is_none() {
            return Decision::Refused(Refusal::NoBadge);
        }
        if !self.holds(&req.agent, perm) {
            return Decision::Refused(Refusal::NoPermission(perm));
        }
        if perm.privileged() {
            if self.pending.is_some() {
                return Decision::Refused(Refusal::Busy);
            }
            let rq = self.next_rq;
            self.next_rq = self.next_rq.wrapping_add(1);
            self.pending = Some(Pending {
                rq,
                agent: req.agent,
                seq: req.seq,
                action: req.action,
                deadline: now.saturating_add(self.timeout),
            });
            return Decision::Pending { rq };
        }
        Decision::Run(req.action)
    }

    pub fn approve(&mut self, rq: u32) -> Result<Approved, DecideError> {
        match self.pending.as_ref() {
            Some(pending) if pending.rq == rq => {}
            _ => return Err(DecideError::NoPending),
        }
        let pending = self.pending.take().ok_or(DecideError::NoPending)?;
        if self.holds(&pending.agent, pending.action.perm()) {
            Ok(Approved::Go(pending))
        } else {
            Ok(Approved::Cancelled(pending))
        }
    }

    pub fn deny(&mut self, rq: u32) -> Result<Pending, DecideError> {
        match self.pending.as_ref() {
            Some(pending) if pending.rq == rq => {}
            _ => return Err(DecideError::NoPending),
        }
        self.pending.take().ok_or(DecideError::NoPending)
    }

    pub fn poll(&mut self, now: u64) -> Option<Pending> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            self.pending.take()
        } else {
            None
        }
    }

    pub fn badges(&self) -> [Option<(Name, u8)>; MAX_BADGES] {
        let mut out = [None; MAX_BADGES];
        for (index, badge) in self.badges.iter().enumerate() {
            if let Some(badge) = badge {
                out[index] = Some((badge.name, badge.perms));
            }
        }
        out
    }

    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// Clears one bit and does not cancel a pending request. The shell
    /// build does not compile this. `gate-core` uses it to reach the
    /// approve re-check.
    #[cfg(feature = "gate-core")]
    pub fn force_clear_for_test(&mut self, agent: &Name, perm: Perm) {
        if let Some(slot) = self.slot_of(agent) {
            if let Some(badge) = self.badges[slot].as_mut() {
                badge.perms &= !perm.bit();
            }
        }
    }

    fn slot_of(&self, agent: &Name) -> Option<usize> {
        self.badges
            .iter()
            .position(|badge| badge.as_ref().is_some_and(|badge| &badge.name == agent))
    }

    fn holds(&self, agent: &Name, perm: Perm) -> bool {
        self.badges
            .iter()
            .flatten()
            .any(|badge| &badge.name == agent && badge.perms & perm.bit() != 0)
    }

    fn cancel_if_unauthorized(&mut self, agent: &Name) -> Option<Pending> {
        let perm = self.pending.as_ref().and_then(|pending| {
            if &pending.agent == agent {
                Some(pending.action.perm())
            } else {
                None
            }
        });
        let Some(perm) = perm else {
            return None;
        };
        if self.holds(agent, perm) {
            None
        } else {
            self.pending.take()
        }
    }
}

const ASK_PREFIX: &[u8] = b"R3GATE ASK ";

pub fn parse_request(line: &[u8]) -> Result<Request, (BadLine, Option<(Name, u32)>)> {
    if line.len() > 72 {
        return Err((BadLine::TooLong, None));
    }
    let Some(rest) = line.strip_prefix(ASK_PREFIX) else {
        return Err((BadLine::Syntax, None));
    };
    let Some((agent_bytes, after_agent)) = split_token(rest) else {
        return Err((BadLine::Syntax, None));
    };
    let Some(agent) = Name::parse(agent_bytes) else {
        return Err((BadLine::Syntax, None));
    };
    let Some((seq_bytes, after_seq)) = split_token(after_agent) else {
        return Err((BadLine::Syntax, None));
    };
    let Some(seq) = parse_seq(seq_bytes) else {
        return Err((BadLine::Syntax, None));
    };
    let parsed = Some((agent, seq));
    if after_seq.is_empty() {
        return Err((BadLine::Syntax, parsed));
    }
    let Some((verb, arg)) = split_token_optional(after_seq) else {
        return Err((BadLine::Syntax, parsed));
    };
    if verb.is_empty() {
        return Err((BadLine::Syntax, parsed));
    }
    match verb {
        b"read" => finish_bare(agent, seq, arg, Action::Read, parsed),
        b"halt" => finish_bare(agent, seq, arg, Action::Halt, parsed),
        b"spawn" => {
            let demo = match arg {
                b"spin" => Demo::Spin,
                b"nap" => Demo::Nap,
                _ => return Err((BadLine::Arg, parsed)),
            };
            Ok(Request {
                agent,
                seq,
                action: Action::Spawn(demo),
            })
        }
        b"say" => {
            if !(1..=40).contains(&arg.len())
                || !arg.iter().all(|byte| (0x20..=0x7E).contains(byte))
            {
                return Err((BadLine::Arg, parsed));
            }
            let mut text = [0u8; 40];
            text[..arg.len()].copy_from_slice(arg);
            Ok(Request {
                agent,
                seq,
                action: Action::Say {
                    text,
                    len: arg.len() as u8,
                },
            })
        }
        _ => Err((BadLine::Verb, parsed)),
    }
}

fn finish_bare(
    agent: Name,
    seq: u32,
    arg: &[u8],
    action: Action,
    parsed: Option<(Name, u32)>,
) -> Result<Request, (BadLine, Option<(Name, u32)>)> {
    if !arg.is_empty() {
        return Err((BadLine::Arg, parsed));
    }
    Ok(Request { agent, seq, action })
}

/// Split on one space. The token must be non-empty, and a space must exist.
fn split_token(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes.iter().position(|byte| *byte == b' ')?;
    if end == 0 {
        return None;
    }
    Some((&bytes[..end], &bytes[end + 1..]))
}

/// Verb plus the bytes after one separating space.
///
/// No space means no argument. A space with nothing after it is a trailing
/// space on an empty argument, which is a syntax error (`None`).
fn split_token_optional(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    match bytes.iter().position(|byte| *byte == b' ') {
        Some(0) => Some((b"", bytes)),
        Some(end) if end + 1 == bytes.len() => None,
        Some(end) => Some((&bytes[..end], &bytes[end + 1..])),
        None => Some((bytes, b"")),
    }
}

fn parse_seq(bytes: &[u8]) -> Option<u32> {
    if !(1..=6).contains(&bytes.len()) {
        return None;
    }
    let mut value: u32 = 0;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
    }
    if value == 0 {
        None
    } else {
        Some(value)
    }
}
