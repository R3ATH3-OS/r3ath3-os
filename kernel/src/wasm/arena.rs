//! The one runtime arena. This file is the only new privileged code in the kernel.
//!
//! The arena is a static `.bss` value, zeroed at boot, never allocated on the
//! heap, and never freed. `TAKEN` is the exclusive-borrow guard: there is one
//! `&mut Arena` at a time, and the flag is not held across a return to the
//! shell prompt. One CPU. The shell task is the only caller.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub const MEM_BYTES: usize = 262_144;
pub const VALUE_CAP: usize = 8_192;
pub const FRAME_CAP: usize = 256;
pub const LABEL_CAP: usize = 2_048;
pub const GLOBAL_CAP: usize = 64;
pub const TABLE_CAP: usize = 512;
pub const SIDE_CAP: usize = 16_384;
pub const FUNC_CAP: usize = 512;
pub const TYPE_CAP: usize = 128;
pub const EXPORT_CAP: usize = 64;
pub const EMPTY_FUNC: u32 = u32::MAX;

pub const KIND_BLOCK: u8 = 0;
pub const KIND_LOOP: u8 = 1;
pub const KIND_IF: u8 = 2;

pub const TY_I32: u8 = 1;
pub const TY_I64: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Frame {
    pub return_pc: u32,
    pub locals_base: u32,
    pub label_base: u32,
    pub func: u16,
    pub results: u8,
    pub _pad: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Label {
    pub kind: u8,
    pub results: u8,
    pub _pad: u16,
    pub end_pc: u32,
    pub redo_pc: u32,
    pub stack_base: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SideEntry {
    pub at: u32,
    pub target: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FuncInfo {
    pub type_index: u16,
    pub param_count: u16,
    pub local_count: u16,
    pub results: u8,
    pub result_ty: u8,
    pub body: u32,
    pub body_end: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeInfo {
    pub param_count: u16,
    pub results: u8,
    pub result_ty: u8,
    pub params: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExportInfo {
    pub name: [u8; 32],
    pub name_len: u8,
    pub kind: u8,
    pub index: u16,
}

#[repr(C)]
pub struct Arena {
    pub memory: [u8; MEM_BYTES],
    pub values: [i64; VALUE_CAP],
    pub frames: [Frame; FRAME_CAP],
    pub labels: [Label; LABEL_CAP],
    pub side: [SideEntry; SIDE_CAP],
    pub globals: [i64; GLOBAL_CAP],
    pub global_mut: [u8; GLOBAL_CAP],
    pub table: [u32; TABLE_CAP],
    pub funcs: [FuncInfo; FUNC_CAP],
    pub types: [TypeInfo; TYPE_CAP],
    pub exports: [ExportInfo; EXPORT_CAP],
    pub mem_pages: u32,
    pub mem_max: u32,
    pub table_size: u32,
    pub global_count: u16,
    pub func_count: u16,
    pub type_count: u16,
    pub export_count: u16,
    pub side_count: u32,
    pub has_memory: u8,
    pub has_table: u8,
    pub _pad: [u8; 2],
}

const _: () = assert!(core::mem::size_of::<Arena>() <= 524_288);

/// Bytes occupied by the arena. Reported by the part A proof notes.
pub const ARENA_BYTES: usize = core::mem::size_of::<Arena>();

struct ArenaCell(UnsafeCell<Arena>);

// SAFETY: one CPU. `with_arena` is the only borrower. The flag is set before
// the reference is created and cleared before this function returns, so the
// reference does not escape and a second borrow cannot start.
unsafe impl Sync for ArenaCell {}

static ARENA: ArenaCell = ArenaCell(UnsafeCell::new(Arena {
    memory: [0; MEM_BYTES],
    values: [0; VALUE_CAP],
    frames: [Frame {
        return_pc: 0,
        locals_base: 0,
        label_base: 0,
        func: 0,
        results: 0,
        _pad: 0,
    }; FRAME_CAP],
    labels: [Label {
        kind: 0,
        results: 0,
        _pad: 0,
        end_pc: 0,
        redo_pc: 0,
        stack_base: 0,
    }; LABEL_CAP],
    side: [SideEntry { at: 0, target: 0 }; SIDE_CAP],
    globals: [0; GLOBAL_CAP],
    global_mut: [0; GLOBAL_CAP],
    table: [0; TABLE_CAP],
    funcs: [FuncInfo {
        type_index: 0,
        param_count: 0,
        local_count: 0,
        results: 0,
        result_ty: 0,
        body: 0,
        body_end: 0,
    }; FUNC_CAP],
    types: [TypeInfo {
        param_count: 0,
        results: 0,
        result_ty: 0,
        params: [0; 32],
    }; TYPE_CAP],
    exports: [ExportInfo {
        name: [0; 32],
        name_len: 0,
        kind: 0,
        index: 0,
    }; EXPORT_CAP],
    mem_pages: 0,
    mem_max: 0,
    table_size: 0,
    global_count: 0,
    func_count: 0,
    type_count: 0,
    export_count: 0,
    side_count: 0,
    has_memory: 0,
    has_table: 0,
    _pad: [0; 2],
}));

static TAKEN: AtomicBool = AtomicBool::new(false);

pub fn with_arena<R>(f: impl FnOnce(&mut Arena) -> R) -> Option<R> {
    if TAKEN.swap(true, Ordering::Acquire) {
        return None;
    }
    // SAFETY: `TAKEN` was false and is now true, so this is the only live
    // reference. The closure returns before the flag is cleared.
    let result = f(unsafe { &mut *ARENA.0.get() });
    TAKEN.store(false, Ordering::Release);
    Some(result)
}
