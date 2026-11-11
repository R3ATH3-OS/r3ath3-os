//! Context switch, task stacks, and `IrqCell`.
//!
//! This is a driver. Every new scheduler `unsafe` lives here: the switch and
//! trampoline assembly, the stack pool and canaries, the saved-RSP table, the
//! initial-frame writes, and `IrqCell`.
//!
//! # Stacks
//! Slots 1–7 each get a 16 KiB stack from a static pool in `.bss` (112 KiB).
//! The pool is not on the heap, so `heap_used` does not move, and the frame
//! allocator never hands the pages out because they sit inside the kernel
//! image. Slot 0 keeps the 16 KiB boot stack and has no canary.
//!
//! The lowest 8 bytes of each pool stack are the canary
//! `0x5233_4154_4833_5354`, written by `prepare` and checked before every
//! switch away from that slot. A mismatch panics `task stack overflow id=<n>`
//! (QEMU status 35).
//!
//! # Switch
//! `switch` saves and restores `rbp rbx r12 r13 r14 r15` and RSP. It does not
//! save IF. Callers run it with IF clear (`debug_assert` in debug builds).
//! A cooperative caller is inside `irq::without_interrupts`, which restores IF
//! when that guard returns on resume. A preemptive caller is the IRQ stub,
//! and `iretq` restores IF.

use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;

use crate::irq;

const STACK_BYTES: usize = 16 * 1024;
const CANARY: u64 = 0x5233_4154_4833_5354;
/// `task_trampoline` plus the six callee-saved slots.
const FRAME_BYTES: usize = 56;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct TaskStack {
    bytes: [u8; STACK_BYTES],
}

struct StackPool(UnsafeCell<[TaskStack; 7]>);

// SAFETY: one CPU. `prepare` and `check_canary` touch a slot only while IF is
// clear, and each task runs on its own slot. The bytes start zeroed in `.bss`.
unsafe impl Sync for StackPool {}

static STACKS: StackPool = StackPool(UnsafeCell::new(
    [TaskStack {
        bytes: [0; STACK_BYTES],
    }; 7],
));

struct SavedRsp(UnsafeCell<[u64; 8]>);

// SAFETY: one CPU. The table is read and written only from `prepare` and
// `switch`, and both run with IF clear.
unsafe impl Sync for SavedRsp {}

static SAVED_RSP: SavedRsp = SavedRsp(UnsafeCell::new([0; 8]));

global_asm!(
    r#"
    .section .text.context, "ax"
    .code64
    .global switch_context
    .global task_trampoline
    switch_context:
        pushq %rbp
        pushq %rbx
        pushq %r12
        pushq %r13
        pushq %r14
        pushq %r15
        movq %rsp, (%rdi)
        movq %rsi, %rsp
        popq %r15
        popq %r14
        popq %r13
        popq %r12
        popq %rbx
        popq %rbp
        retq
    task_trampoline:
        sti
        cld
        movq %r13, %rdi
        callq *%r12
        callq sched_task_exit
        ud2
    "#,
    options(att_syntax)
);

extern "C" {
    fn switch_context(old: *mut u64, new: u64);
    fn task_trampoline();
}

/// A cell that safe code can mutate only with IF clear.
///
/// The `&mut` handed to the closure does not outlive that closure.
pub struct IrqCell<T>(UnsafeCell<T>);

// SAFETY: one CPU. Every access goes through `with`, which runs with IF clear,
// so the IRQ0 hook and task code never overlap.
unsafe impl<T> Sync for IrqCell<T> {}

impl<T> IrqCell<T> {
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        irq::without_interrupts(|| {
            // SAFETY: `without_interrupts` has cleared IF, and there is one
            // CPU, so this exclusive reference is the only live access. It is
            // not returned from the closure.
            f(unsafe { &mut *self.0.get() })
        })
    }
}

/// Write the canary and the initial frame for `slot` (1–7). Caller holds IF clear.
pub fn prepare(slot: usize, entry: fn(usize), arg: usize) {
    debug_assert!((1..8).contains(&slot));
    debug_assert!(!interrupts_enabled());
    // SAFETY: `slot` is 1–7, so the index is inside the pool. IF is clear.
    // The frame sits at the top of that stack: saved RSP = top-56, the six
    // popped registers are r15, r14, r13=arg, r12=entry, rbx, rbp, and the
    // return slot is `task_trampoline`. `top` is 16-byte aligned, which is
    // what the SysV ABI requires of RSP at the trampoline's `call *%r12`.
    unsafe {
        let base = stack_base(slot);
        (base as *mut u64).write(CANARY);
        let top = base.add(STACK_BYTES);
        debug_assert_eq!(top as usize % 16, 0);
        let sp = top.sub(FRAME_BYTES);
        let frame = sp as *mut u64;
        frame.add(0).write(0);
        frame.add(1).write(0);
        frame.add(2).write(arg as u64);
        frame.add(3).write(entry as usize as u64);
        frame.add(4).write(0);
        frame.add(5).write(0);
        frame.add(6).write(task_trampoline as usize as u64);
        (*SAVED_RSP.0.get())[slot] = sp as u64;
    }
}

/// Panic if slot 1–7 has lost its canary. Slot 0 has none.
pub fn check_canary(slot: usize) {
    if slot == 0 || slot >= 8 {
        return;
    }
    // SAFETY: `slot` is 1–7. The read is the first 8 bytes of that stack.
    let value = unsafe { (stack_base(slot) as *const u64).read() };
    if value != CANARY {
        panic!("task stack overflow id={slot}");
    }
}

/// Switch from `from` to `to`. Caller holds IF clear. Both indexes are < 8.
pub fn switch(from: usize, to: usize) {
    debug_assert!(!interrupts_enabled(), "context switch with IF set");
    debug_assert!(from < 8 && to < 8 && from != to);
    // SAFETY: caller holds IF clear and the indexes are inside the saved-RSP
    // table. `switch_context` writes the current RSP through `old` and then
    // resumes the stack stored in `new`.
    unsafe {
        let table = &mut *SAVED_RSP.0.get();
        let new = table[to];
        let old = &mut table[from] as *mut u64;
        switch_context(old, new);
    }
}

fn interrupts_enabled() -> bool {
    let flags: u64;
    // SAFETY: pushfq/pop copy RFLAGS into `flags`. The pair is balanced and
    // does not write Rust memory.
    unsafe {
        asm!(
            "pushfq",
            "pop {flags}",
            flags = out(reg) flags,
            options(nomem, preserves_flags),
        );
    }
    flags & 0x200 != 0
}

/// # Safety
/// `slot` is 1–7.
unsafe fn stack_base(slot: usize) -> *mut u8 {
    let pools = &mut *STACKS.0.get();
    pools[slot - 1].bytes.as_mut_ptr()
}
