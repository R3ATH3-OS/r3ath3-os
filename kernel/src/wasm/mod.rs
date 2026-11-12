//! WebAssembly integer interpreter. See `docs/WASM.md`.
//!
//! Safe code except `arena.rs`. One instance lives in a static arena. The
//! module bytes stay in the kernel image. Nothing here allocates. A bad byte
//! or a bad stack becomes `LoadError` or `Trap`; this module does not panic
//! on module input.

#![allow(dead_code)]

mod arena;
mod decode;
mod exec;

pub use arena::with_arena;

use arena::Arena;

/// Instructions charged to one `call`. `call_with_fuel` can pass a smaller budget.
pub const FUEL_PER_CALL: u32 = 1_000_000;

/// Module larger than this is `LoadError::TooBig`.
pub const MAX_MODULE: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    Magic,
    Version,
    Truncated,
    Section,
    Opcode,
    Float,
    Import,
    Limits,
    Export,
    TooBig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    Unreachable,
    Memory,
    DivZero,
    Overflow,
    Indirect,
    Table,
    Stack,
    Fuel,
    Dead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
}

impl LoadError {
    pub fn word(self) -> &'static str {
        match self {
            Self::Magic => "magic",
            Self::Version => "version",
            Self::Truncated => "truncated",
            Self::Section => "section",
            Self::Opcode => "opcode",
            Self::Float => "float",
            Self::Import => "import",
            Self::Limits => "limits",
            Self::Export => "export",
            Self::TooBig => "too-big",
        }
    }
}

impl Trap {
    pub fn word(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Memory => "memory",
            Self::DivZero => "div-zero",
            Self::Overflow => "overflow",
            Self::Indirect => "indirect",
            Self::Table => "table",
            Self::Stack => "stack",
            Self::Fuel => "fuel",
            Self::Dead => "dead",
        }
    }
}

pub struct Instance<'m, 'a> {
    module: &'m [u8],
    arena: &'a mut Arena,
    dead: bool,
}

impl<'m, 'a> Instance<'m, 'a> {
    pub fn call(&mut self, export: &str, args: &[i64]) -> Result<Option<i64>, Trap> {
        self.call_with_fuel(export, args, FUEL_PER_CALL)
    }

    pub fn call_with_fuel(
        &mut self,
        export: &str,
        args: &[i64],
        fuel: u32,
    ) -> Result<Option<i64>, Trap> {
        if self.dead {
            return Err(Trap::Dead);
        }
        match exec::call(self.module, self.arena, export, args, fuel) {
            Ok(value) => Ok(value),
            Err(trap) => {
                self.dead = true;
                Err(trap)
            }
        }
    }

    pub fn memory(&self) -> &[u8] {
        let pages = self.arena.mem_pages as usize;
        let len = pages.saturating_mul(65_536).min(self.arena.memory.len());
        &self.arena.memory[..len]
    }

    pub fn has_export(&self, name: &str) -> bool {
        export_index(self.arena, name).is_some()
    }

    pub fn export_is_memory(&self, name: &str) -> bool {
        match export_index(self.arena, name) {
            Some(index) => self.arena.exports[index].kind == decode::KIND_MEM,
            None => false,
        }
    }

    pub fn func_matches(&self, name: &str, params: &[ValType], result: Option<ValType>) -> bool {
        let Some(index) = export_index(self.arena, name) else {
            return false;
        };
        let export = &self.arena.exports[index];
        if export.kind != decode::KIND_FUNC {
            return false;
        }
        let func_index = export.index as usize;
        if func_index >= self.arena.func_count as usize {
            return false;
        }
        let type_index = self.arena.funcs[func_index].type_index as usize;
        if type_index >= self.arena.type_count as usize {
            return false;
        }
        let ty = &self.arena.types[type_index];
        if ty.param_count as usize != params.len() {
            return false;
        }
        for (i, param) in params.iter().enumerate() {
            let is_i64 = decode::param_is_i64(ty, i);
            let want_i64 = *param == ValType::I64;
            if is_i64 != want_i64 {
                return false;
            }
        }
        match result {
            None => ty.results == 0,
            Some(ValType::I32) => ty.results == 1 && ty.result_ty == arena::TY_I32,
            Some(ValType::I64) => ty.results == 1 && ty.result_ty == arena::TY_I64,
        }
    }
}

fn export_index(arena: &Arena, name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    for index in 0..arena.export_count as usize {
        let export = &arena.exports[index];
        if export.name_len as usize == bytes.len() && &export.name[..bytes.len()] == bytes {
            return Some(index);
        }
    }
    None
}

/// `None` when the arena is already taken. That does not happen in this
/// milestone: one shell, one app. A load failure is `Some(Err(_))`.
pub fn with_instance<R>(
    module: &[u8],
    f: impl FnOnce(Result<Instance<'_, '_>, LoadError>) -> R,
) -> Option<R> {
    with_arena(|arena| {
        let loaded = match decode::load(module, arena) {
            Ok(()) => Ok(Instance {
                module,
                arena,
                dead: false,
            }),
            Err(error) => Err(error),
        };
        f(loaded)
    })
}
