//! Fixed 8-slot round-robin scheduler. Safe code only.
//!
//! Each feature calls a subset of the API, so unused-item warnings are off.
#![allow(dead_code)]
//!
//! Slot 0 is the boot context (`main`, or `shell` when that feature is on).
//! Slot 1 is `idle`. Slots 2–7 are spawnable. The table sits in an
//! `IrqCell`, so every borrow runs with IF clear. A context switch is done
//! only after that borrow ends, and still inside the same
//! `irq::without_interrupts` guard, so IF stays clear across the switch and
//! two tasks never hold `&mut` to the table at once.
//!
//! An exited task keeps its slot until `wait` reaps it. Slots 0 and 1 never
//! exit. See `docs/SCHEDULER.md`.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::context::{self, IrqCell};
use crate::irq;

pub const MAX_TASKS: usize = 8;
const IDLE_SLOT: usize = 1;

static PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static IDLE_RUNS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Free,
    Ready,
    Running,
    Sleeping,
    Blocked,
    Exited,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Free => "free",
            State::Ready => "ready",
            State::Running => "running",
            State::Sleeping => "sleeping",
            State::Blocked => "blocked",
            State::Exited => "exited",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitError {
    NoTask,
}

#[derive(Clone, Copy)]
pub struct TaskSnap {
    pub id: usize,
    pub name: &'static str,
    pub state: State,
}

#[derive(Clone, Copy)]
struct Task {
    state: State,
    name: &'static str,
    wake_at: u64,
    waiting_on: Option<usize>,
    #[allow(dead_code)]
    entry: fn(usize),
    #[allow(dead_code)]
    arg: usize,
}

impl Task {
    const fn vacant() -> Self {
        Self {
            state: State::Free,
            name: "",
            wake_at: 0,
            waiting_on: None,
            entry: dormant,
            arg: 0,
        }
    }
}

struct Table {
    tasks: [Task; MAX_TASKS],
    current: usize,
    preempt: bool,
    /// Set at the end of `init`. The PIT is already running before `init`.
    live: bool,
}

impl Table {
    const fn empty() -> Self {
        Self {
            tasks: [Task::vacant(); MAX_TASKS],
            current: 0,
            preempt: false,
            live: false,
        }
    }
}

static TABLE: IrqCell<Table> = IrqCell::new(Table::empty());

fn dormant(_: usize) {}

fn idle_main(_: usize) {
    loop {
        IDLE_RUNS.fetch_add(1, Ordering::Relaxed);
        irq::idle_hlt();
        yield_now();
    }
}

/// Adopt the boot context as task 0 and create idle in slot 1. Prints nothing.
pub fn init(preempt: bool) {
    irq::without_interrupts(|| {
        TABLE.with(|table| {
            table.preempt = preempt;
            table.current = 0;
            table.tasks[0] = Task {
                state: State::Running,
                name: if cfg!(feature = "shell") {
                    "shell"
                } else {
                    "main"
                },
                wake_at: 0,
                waiting_on: None,
                entry: dormant,
                arg: 0,
            };
            context::prepare(IDLE_SLOT, idle_main, 0);
            table.tasks[IDLE_SLOT] = Task {
                state: State::Ready,
                name: "idle",
                wake_at: 0,
                waiting_on: None,
                entry: idle_main,
                arg: 0,
            };
            for slot in 2..MAX_TASKS {
                table.tasks[slot] = Task::vacant();
            }
            table.live = true;
        });
    });
}

/// Lowest free slot in 2–7, or `Full`. Does not switch.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<usize, SpawnError> {
    irq::without_interrupts(|| {
        TABLE.with(|table| {
            let Some(slot) = (2..MAX_TASKS).find(|&id| table.tasks[id].state == State::Free) else {
                return Err(SpawnError::Full);
            };
            context::prepare(slot, entry, arg);
            table.tasks[slot] = Task {
                state: State::Ready,
                name,
                wake_at: 0,
                waiting_on: None,
                entry,
                arg,
            };
            Ok(slot)
        })
    })
}

pub fn yield_now() {
    irq::without_interrupts(|| {
        let switch = TABLE.with(|table| {
            let cur = table.current;
            table.tasks[cur].state = State::Ready;
            let next = pick_next(table, cur);
            arm(table, next)
        });
        resume(switch);
    });
}

/// Sleep until the absolute tick `deadline`. A deadline that has already
/// passed returns at once.
pub fn sleep_until(deadline: u64) {
    irq::without_interrupts(|| {
        let switch = TABLE.with(|table| {
            if deadline <= irq::ticks() {
                return None;
            }
            let cur = table.current;
            table.tasks[cur].state = State::Sleeping;
            table.tasks[cur].wake_at = deadline;
            let next = pick_next(table, cur);
            arm(table, next)
        });
        resume(switch);
    });
}

/// Sleep `n` ticks from the tick count sampled with IF clear.
/// `n == 0` returns at once.
pub fn sleep_ticks(n: u64) {
    irq::without_interrupts(|| {
        let now = irq::ticks();
        if n == 0 {
            return;
        }
        let deadline = now.wrapping_add(n);
        let switch = TABLE.with(|table| {
            if deadline <= irq::ticks() {
                return None;
            }
            let cur = table.current;
            table.tasks[cur].state = State::Sleeping;
            table.tasks[cur].wake_at = deadline;
            let next = pick_next(table, cur);
            arm(table, next)
        });
        resume(switch);
    });
}

/// Block until task `id` has exited, then reap its slot.
pub fn wait(id: usize) -> Result<(), WaitError> {
    irq::without_interrupts(|| {
        let switch = TABLE.with(|table| -> Result<Option<(usize, usize)>, WaitError> {
            if !(2..MAX_TASKS).contains(&id) || table.tasks[id].state == State::Free {
                return Err(WaitError::NoTask);
            }
            if table.tasks[id].state == State::Exited {
                table.tasks[id] = Task::vacant();
                return Ok(None);
            }
            let cur = table.current;
            table.tasks[cur].state = State::Blocked;
            table.tasks[cur].waiting_on = Some(id);
            let next = pick_next(table, cur);
            Ok(arm(table, next))
        })?;
        resume(switch);
        TABLE.with(|table| {
            if table.tasks[id].state == State::Exited {
                table.tasks[id] = Task::vacant();
            }
        });
        Ok(())
    })
}

/// Called by the trampoline when a task entry returns. Does not return.
#[no_mangle]
pub extern "C" fn sched_task_exit() -> ! {
    exit_current()
}

fn exit_current() -> ! {
    irq::without_interrupts(|| {
        let switch = TABLE.with(|table| {
            let cur = table.current;
            table.tasks[cur].state = State::Exited;
            for task in &mut table.tasks {
                if task.state == State::Blocked && task.waiting_on == Some(cur) {
                    task.state = State::Ready;
                    task.waiting_on = None;
                }
            }
            let next = pick_next(table, cur);
            arm(table, next)
        });
        resume(switch);
    });
    panic!("exited task resumed");
}

/// Timer hook. IF is clear and the IRQ has already sent EOI. No allocation,
/// no serial, no VGA. Ticks that arrive before `init` are ignored.
pub fn on_tick() {
    irq::without_interrupts(|| {
        let switch = TABLE.with(|table| {
            if !table.live {
                return None;
            }
            let now = irq::ticks();
            let mut woke = false;
            for task in &mut table.tasks {
                if task.state == State::Sleeping && task.wake_at <= now {
                    task.state = State::Ready;
                    woke = true;
                }
            }
            let cur = table.current;
            if table.preempt && cur != IDLE_SLOT {
                table.tasks[cur].state = State::Ready;
                let next = pick_next(table, cur);
                if next != cur {
                    PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
                }
                arm(table, next)
            } else if cur == IDLE_SLOT && woke {
                table.tasks[cur].state = State::Ready;
                let next = pick_next(table, cur);
                arm(table, next)
            } else {
                None
            }
        });
        resume(switch);
    });
}

pub fn snapshot() -> [Option<TaskSnap>; MAX_TASKS] {
    TABLE.with(|table| {
        let mut out = [None; MAX_TASKS];
        for (id, task) in table.tasks.iter().enumerate() {
            if task.state != State::Free {
                out[id] = Some(TaskSnap {
                    id,
                    name: task.name,
                    state: task.state,
                });
            }
        }
        out
    })
}

/// Free slots in 2–7.
pub fn free_slots() -> usize {
    TABLE.with(|table| {
        (2..MAX_TASKS)
            .filter(|&id| table.tasks[id].state == State::Free)
            .count()
    })
}

pub fn switches() -> u64 {
    SWITCHES.load(Ordering::Relaxed)
}

pub fn preemptions() -> u64 {
    PREEMPTIONS.load(Ordering::Relaxed)
}

pub fn idle_runs() -> u64 {
    IDLE_RUNS.load(Ordering::Relaxed)
}

/// Scan `current+1`, `current+2`, … modulo 8, skipping idle, and take the
/// first `Ready` task. The current task counts when it is still `Ready`.
/// If none is ready, idle runs.
fn pick_next(table: &Table, current: usize) -> usize {
    let mut id = current;
    for _ in 0..MAX_TASKS {
        id = (id + 1) % MAX_TASKS;
        if id == IDLE_SLOT {
            continue;
        }
        if table.tasks[id].state == State::Ready {
            return id;
        }
    }
    IDLE_SLOT
}

/// Commit `next` as running. `None` means the current task stays put.
fn arm(table: &mut Table, next: usize) -> Option<(usize, usize)> {
    let cur = table.current;
    if next == cur {
        table.tasks[cur].state = State::Running;
        None
    } else {
        table.tasks[next].state = State::Running;
        table.current = next;
        Some((cur, next))
    }
}

fn resume(switch: Option<(usize, usize)>) {
    let Some((from, to)) = switch else {
        return;
    };
    if from != 0 {
        context::check_canary(from);
    }
    SWITCHES.fetch_add(1, Ordering::Relaxed);
    context::switch(from, to);
}
