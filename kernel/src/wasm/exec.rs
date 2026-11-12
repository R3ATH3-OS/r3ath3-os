//! The interpreter loop. Wasm calls use arena frames, never Rust recursion.
//! Every stack, memory, and fuel failure is a `Trap`.

use super::arena::{
    Arena, Frame, Label, EMPTY_FUNC, FRAME_CAP, KIND_LOOP, LABEL_CAP, TY_I32, VALUE_CAP,
};
use super::decode::{self, find_side, KIND_FUNC};
use super::Trap;

const HOST_RETURN: u32 = u32::MAX;
const PAGE: usize = 65_536;

struct Exec<'a> {
    module: &'a [u8],
    arena: &'a mut Arena,
    pc: usize,
    sp: usize,
    frame_sp: usize,
    label_sp: usize,
    fuel: u32,
    done: bool,
    result: Option<i64>,
}

pub fn call(
    module: &[u8],
    arena: &mut Arena,
    export: &str,
    args: &[i64],
    fuel: u32,
) -> Result<Option<i64>, Trap> {
    let Some(func) = exported_func(arena, export) else {
        return Err(Trap::Dead);
    };
    let info = arena.funcs[func];
    if args.len() != info.param_count as usize {
        return Err(Trap::Stack);
    }
    let mut exec = Exec {
        module,
        arena,
        pc: 0,
        sp: 0,
        frame_sp: 0,
        label_sp: 0,
        fuel,
        done: false,
        result: None,
    };
    let type_index = info.type_index as usize;
    for (index, arg) in args.iter().enumerate() {
        let value = if decode::param_is_i64(&exec.arena.types[type_index], index) {
            *arg
        } else {
            *arg as i32 as i64
        };
        exec.push(value)?;
    }
    exec.enter(func, HOST_RETURN)?;
    exec.run()
}

fn exported_func(arena: &Arena, name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    for index in 0..arena.export_count as usize {
        let export = &arena.exports[index];
        if export.kind == KIND_FUNC
            && export.name_len as usize == bytes.len()
            && &export.name[..bytes.len()] == bytes
        {
            return Some(export.index as usize);
        }
    }
    None
}

impl<'a> Exec<'a> {
    fn run(&mut self) -> Result<Option<i64>, Trap> {
        loop {
            if self.done {
                return Ok(self.result);
            }
            if self.fuel == 0 {
                return Err(Trap::Fuel);
            }
            self.fuel -= 1;
            let pc = self.pc;
            let Some(op) = self.module.get(pc).copied() else {
                return Err(Trap::Unreachable);
            };
            self.pc = pc + 1;
            self.step(op, pc as u32)?;
        }
    }

    fn step(&mut self, op: u8, op_pc: u32) -> Result<(), Trap> {
        match op {
            0x00 => Err(Trap::Unreachable),
            0x01 => Ok(()),
            0x02 | 0x03 => {
                let results = self.block_results()?;
                let end_pc = find_side(self.arena, op_pc).ok_or(Trap::Unreachable)?;
                let redo = self.pc as u32;
                let kind = if op == 0x03 { KIND_LOOP } else { 0 };
                self.push_label(kind, results, end_pc, redo)
            }
            0x04 => {
                let results = self.block_results()?;
                let else_or_end = find_side(self.arena, op_pc).ok_or(Trap::Unreachable)?;
                let end_pc = if self.module.get(else_or_end as usize) == Some(&0x05) {
                    find_side(self.arena, else_or_end).ok_or(Trap::Unreachable)?
                } else {
                    else_or_end
                };
                let cond = self.pop()?;
                if cond != 0 {
                    self.push_label(2, results, end_pc, 0)
                } else if self.module.get(else_or_end as usize) == Some(&0x05) {
                    self.push_label(2, results, end_pc, 0)?;
                    self.pc = else_or_end as usize + 1;
                    Ok(())
                } else {
                    self.pc = end_pc as usize + 1;
                    Ok(())
                }
            }
            0x05 => {
                let else_pc = op_pc;
                let end_pc = find_side(self.arena, else_pc).ok_or(Trap::Unreachable)?;
                self.pop_label_unwind()?;
                self.pc = end_pc as usize + 1;
                Ok(())
            }
            0x0B => {
                let base = self.frame_label_base()?;
                if self.label_sp == base {
                    self.do_return()
                } else {
                    self.pop_label_unwind()
                }
            }
            0x0C => {
                let depth = self.read_u32()?;
                self.branch(depth)
            }
            0x0D => {
                let depth = self.read_u32()?;
                let cond = self.pop()?;
                if cond != 0 {
                    self.branch(depth)
                } else {
                    Ok(())
                }
            }
            0x0E => self.br_table(),
            0x0F => self.do_return(),
            0x10 => {
                let index = self.read_u32()? as usize;
                if index >= self.arena.func_count as usize {
                    return Err(Trap::Unreachable);
                }
                self.enter(index, self.pc as u32)
            }
            0x11 => self.call_indirect(),
            0x1A => {
                self.pop()?;
                Ok(())
            }
            0x1B => {
                let cond = self.pop()?;
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(if cond != 0 { a } else { b })
            }
            0x20 => {
                let index = self.read_u32()? as usize;
                let value = self.local(index)?;
                self.push(value)
            }
            0x21 => {
                let index = self.read_u32()? as usize;
                let value = self.pop()?;
                self.set_local(index, value)
            }
            0x22 => {
                let index = self.read_u32()? as usize;
                let value = self.peek()?;
                self.set_local(index, value)
            }
            0x23 => {
                let index = self.read_u32()? as usize;
                let value = *self.arena.globals.get(index).ok_or(Trap::Unreachable)?;
                self.push(value)
            }
            0x24 => {
                let index = self.read_u32()? as usize;
                if self.arena.global_mut.get(index).copied().unwrap_or(0) == 0 {
                    return Err(Trap::Unreachable);
                }
                let value = self.pop()?;
                if let Some(slot) = self.arena.globals.get_mut(index) {
                    *slot = value;
                    Ok(())
                } else {
                    Err(Trap::Unreachable)
                }
            }
            0x28 => self.load_mem(4, Sign::I32),
            0x29 => self.load_mem(8, Sign::I64),
            0x2C => self.load_mem(1, Sign::I32S8),
            0x2D => self.load_mem(1, Sign::I32U8),
            0x2E => self.load_mem(2, Sign::I32S16),
            0x2F => self.load_mem(2, Sign::I32U16),
            0x30 => self.load_mem(1, Sign::I64S8),
            0x31 => self.load_mem(1, Sign::I64U8),
            0x32 => self.load_mem(2, Sign::I64S16),
            0x33 => self.load_mem(2, Sign::I64U16),
            0x34 => self.load_mem(4, Sign::I64S32),
            0x35 => self.load_mem(4, Sign::I64U32),
            0x36 => self.store_mem(4),
            0x37 => self.store_mem(8),
            0x3A | 0x3C => self.store_mem(1),
            0x3B | 0x3D => self.store_mem(2),
            0x3E => self.store_mem(4),
            0x3F => {
                self.mem_index_byte()?;
                let pages = self.arena.mem_pages as i32;
                self.push_i32(pages)
            }
            0x40 => self.memory_grow(),
            0x41 => {
                let value = self.read_i32()?;
                self.push_i32(value)
            }
            0x42 => {
                let value = self.read_i64()?;
                self.push(value)
            }
            0x45 => {
                let value = self.pop_i32()?;
                self.push_i32(if value == 0 { 1 } else { 0 })
            }
            0x46 => self.cmp_i32(|a, b| a == b),
            0x47 => self.cmp_i32(|a, b| a != b),
            0x48 => self.cmp_i32(|a, b| a < b),
            0x49 => self.cmp_i32_u(|a, b| a < b),
            0x4A => self.cmp_i32(|a, b| a > b),
            0x4B => self.cmp_i32_u(|a, b| a > b),
            0x4C => self.cmp_i32(|a, b| a <= b),
            0x4D => self.cmp_i32_u(|a, b| a <= b),
            0x4E => self.cmp_i32(|a, b| a >= b),
            0x4F => self.cmp_i32_u(|a, b| a >= b),
            0x50 => {
                let value = self.pop()?;
                self.push_i32(if value == 0 { 1 } else { 0 })
            }
            0x51 => self.cmp_i64(|a, b| a == b),
            0x52 => self.cmp_i64(|a, b| a != b),
            0x53 => self.cmp_i64(|a, b| a < b),
            0x54 => self.cmp_i64_u(|a, b| a < b),
            0x55 => self.cmp_i64(|a, b| a > b),
            0x56 => self.cmp_i64_u(|a, b| a > b),
            0x57 => self.cmp_i64(|a, b| a <= b),
            0x58 => self.cmp_i64_u(|a, b| a <= b),
            0x59 => self.cmp_i64(|a, b| a >= b),
            0x5A => self.cmp_i64_u(|a, b| a >= b),
            0x67 => {
                let value = self.pop_i32()?;
                self.push_i32(value.leading_zeros() as i32)
            }
            0x68 => {
                let value = self.pop_i32()?;
                self.push_i32(value.trailing_zeros() as i32)
            }
            0x69 => {
                let value = self.pop_i32()?;
                self.push_i32(value.count_ones() as i32)
            }
            0x6A => self.bin_i32(|a, b| a.wrapping_add(b)),
            0x6B => self.bin_i32(|a, b| a.wrapping_sub(b)),
            0x6C => self.bin_i32(|a, b| a.wrapping_mul(b)),
            0x6D => self.bin_i32_trap(|a, b| div_s_i32(a, b)),
            0x6E => self.bin_i32_trap(|a, b| div_u_i32(a, b)),
            0x6F => self.bin_i32_trap(|a, b| rem_s_i32(a, b)),
            0x70 => self.bin_i32_trap(|a, b| rem_u_i32(a, b)),
            0x71 => self.bin_i32(|a, b| a & b),
            0x72 => self.bin_i32(|a, b| a | b),
            0x73 => self.bin_i32(|a, b| a ^ b),
            0x74 => self.bin_i32(|a, b| a.wrapping_shl(b as u32)),
            0x75 => self.bin_i32(|a, b| a.wrapping_shr(b as u32)),
            0x76 => self.bin_i32(|a, b| (a as u32).wrapping_shr(b as u32) as i32),
            0x77 => self.bin_i32(|a, b| a.rotate_left(b as u32)),
            0x78 => self.bin_i32(|a, b| a.rotate_right(b as u32)),
            0x79 => {
                let value = self.pop()?;
                self.push_i32(value.leading_zeros() as i32)
            }
            0x7A => {
                let value = self.pop()?;
                self.push_i32(value.trailing_zeros() as i32)
            }
            0x7B => {
                let value = self.pop()?;
                self.push_i32(value.count_ones() as i32)
            }
            0x7C => self.bin_i64(|a, b| a.wrapping_add(b)),
            0x7D => self.bin_i64(|a, b| a.wrapping_sub(b)),
            0x7E => self.bin_i64(|a, b| a.wrapping_mul(b)),
            0x7F => self.bin_i64_trap(|a, b| div_s_i64(a, b)),
            0x80 => self.bin_i64_trap(|a, b| div_u_i64(a, b)),
            0x81 => self.bin_i64_trap(|a, b| rem_s_i64(a, b)),
            0x82 => self.bin_i64_trap(|a, b| rem_u_i64(a, b)),
            0x83 => self.bin_i64(|a, b| a & b),
            0x84 => self.bin_i64(|a, b| a | b),
            0x85 => self.bin_i64(|a, b| a ^ b),
            0x86 => self.bin_i64(|a, b| a.wrapping_shl(b as u32)),
            0x87 => self.bin_i64(|a, b| a.wrapping_shr(b as u32)),
            0x88 => self.bin_i64(|a, b| (a as u64).wrapping_shr(b as u32) as i64),
            0x89 => self.bin_i64(|a, b| a.rotate_left(b as u32)),
            0x8A => self.bin_i64(|a, b| a.rotate_right(b as u32)),
            0xA7 => {
                let value = self.pop()? as i32;
                self.push_i32(value)
            }
            0xAC => {
                let value = self.pop_i32()? as i64;
                self.push(value)
            }
            0xAD => {
                let value = self.pop_i32()? as u32 as i64;
                self.push(value)
            }
            0xC0 => {
                let value = self.pop_i32()? as i8 as i32;
                self.push_i32(value)
            }
            0xC1 => {
                let value = self.pop_i32()? as i16 as i32;
                self.push_i32(value)
            }
            0xC2 => {
                let value = self.pop()? as i8 as i64;
                self.push(value)
            }
            0xC3 => {
                let value = self.pop()? as i16 as i64;
                self.push(value)
            }
            0xC4 => {
                let value = self.pop()? as i32 as i64;
                self.push(value)
            }
            _ => Err(Trap::Unreachable),
        }
    }

    fn enter(&mut self, func: usize, return_pc: u32) -> Result<(), Trap> {
        let info = *self.arena.funcs.get(func).ok_or(Trap::Unreachable)?;
        let params = info.param_count as usize;
        let locals = info.local_count as usize;
        // A wasm caller is still the top frame, so this is that caller's floor.
        // The host entry has no frame and the floor is 0. A missing argument
        // must not be taken from a local or from a caller's slot.
        if self.sp < self.operand_floor() + params {
            return Err(Trap::Stack);
        }
        let base = self.sp - params;
        if base + locals > VALUE_CAP {
            return Err(Trap::Stack);
        }
        for slot in &mut self.arena.values[base + params..base + locals] {
            *slot = 0;
        }
        if self.frame_sp >= FRAME_CAP {
            return Err(Trap::Stack);
        }
        self.sp = base + locals;
        self.arena.frames[self.frame_sp] = Frame {
            return_pc,
            locals_base: base as u32,
            label_base: self.label_sp as u32,
            func: func as u16,
            results: info.results,
            _pad: 0,
        };
        self.frame_sp += 1;
        self.pc = info.body as usize;
        Ok(())
    }

    fn do_return(&mut self) -> Result<(), Trap> {
        if self.frame_sp == 0 {
            return Err(Trap::Stack);
        }
        let frame = self.arena.frames[self.frame_sp - 1];
        let result = if frame.results == 1 {
            let raw = self.pop()?;
            let ty = self
                .arena
                .funcs
                .get(frame.func as usize)
                .map(|info| info.result_ty)
                .unwrap_or(0);
            let value = if ty == TY_I32 { raw as i32 as i64 } else { raw };
            Some(value)
        } else {
            None
        };
        self.frame_sp -= 1;
        self.sp = frame.locals_base as usize;
        self.label_sp = frame.label_base as usize;
        if frame.return_pc == HOST_RETURN {
            self.done = true;
            self.result = result;
            Ok(())
        } else {
            if let Some(value) = result {
                self.push(value)?;
            }
            self.pc = frame.return_pc as usize;
            Ok(())
        }
    }

    fn call_indirect(&mut self) -> Result<(), Trap> {
        let type_index = self.read_u32()? as usize;
        let table = self.read_u32()?;
        if table != 0 || self.arena.has_table == 0 {
            return Err(Trap::Unreachable);
        }
        let index = self.pop()? as i32 as u32;
        if index as usize >= self.arena.table_size as usize {
            return Err(Trap::Table);
        }
        let func = *self.arena.table.get(index as usize).ok_or(Trap::Table)?;
        if func == EMPTY_FUNC || func as usize >= self.arena.func_count as usize {
            return Err(Trap::Indirect);
        }
        let callee = self.arena.funcs[func as usize].type_index as usize;
        if type_index >= self.arena.type_count as usize || !sig_eq(self.arena, type_index, callee) {
            return Err(Trap::Indirect);
        }
        self.enter(func as usize, self.pc as u32)
    }

    fn br_table(&mut self) -> Result<(), Trap> {
        let n = self.read_u32()?;
        let index = self.pop()? as i32 as u32;
        let mut chosen = 0u32;
        let mut matched = false;
        for slot in 0..n {
            let depth = self.read_u32()?;
            if slot == index {
                chosen = depth;
                matched = true;
            }
        }
        let default = self.read_u32()?;
        self.branch(if matched { chosen } else { default })
    }

    fn branch(&mut self, depth: u32) -> Result<(), Trap> {
        let base = self.frame_label_base()?;
        if self.label_sp <= base {
            return Err(Trap::Stack);
        }
        let available = self.label_sp - base;
        if depth as usize >= available {
            return Err(Trap::Stack);
        }
        let idx = self.label_sp - 1 - depth as usize;
        let label = self.arena.labels[idx];
        if label.kind == KIND_LOOP {
            self.sp = label.stack_base as usize;
            self.label_sp = idx + 1;
            self.pc = label.redo_pc as usize;
            Ok(())
        } else {
            let result = if label.results == 1 {
                Some(self.pop()?)
            } else {
                None
            };
            self.sp = label.stack_base as usize;
            self.label_sp = idx;
            if let Some(value) = result {
                self.push(value)?;
            }
            self.pc = label.end_pc as usize + 1;
            Ok(())
        }
    }

    fn push_label(&mut self, kind: u8, results: u8, end_pc: u32, redo_pc: u32) -> Result<(), Trap> {
        if self.label_sp >= LABEL_CAP {
            return Err(Trap::Stack);
        }
        self.arena.labels[self.label_sp] = Label {
            kind,
            results,
            _pad: 0,
            end_pc,
            redo_pc,
            stack_base: self.sp as u32,
        };
        self.label_sp += 1;
        Ok(())
    }

    fn pop_label_unwind(&mut self) -> Result<(), Trap> {
        if self.label_sp == 0 {
            return Err(Trap::Stack);
        }
        let label = self.arena.labels[self.label_sp - 1];
        let result = if label.results == 1 {
            Some(self.pop()?)
        } else {
            None
        };
        self.label_sp -= 1;
        self.sp = label.stack_base as usize;
        if let Some(value) = result {
            self.push(value)?;
        }
        Ok(())
    }

    fn frame_label_base(&self) -> Result<usize, Trap> {
        self.arena
            .frames
            .get(self.frame_sp.wrapping_sub(1))
            .map(|frame| frame.label_base as usize)
            .ok_or(Trap::Stack)
    }

    fn block_results(&mut self) -> Result<u8, Trap> {
        let kind = self.read_blocktype()?;
        Ok(if kind == 0 { 0 } else { 1 })
    }

    fn local(&self, index: usize) -> Result<i64, Trap> {
        let frame = self.arena.frames.get(self.frame_sp.wrapping_sub(1)).ok_or(Trap::Stack)?;
        let locals = self.arena.funcs.get(frame.func as usize).map(|info| info.local_count as usize).unwrap_or(0);
        if index >= locals {
            return Err(Trap::Stack);
        }
        self.arena
            .values
            .get(frame.locals_base as usize + index)
            .copied()
            .ok_or(Trap::Stack)
    }

    fn set_local(&mut self, index: usize, value: i64) -> Result<(), Trap> {
        let frame = self.arena.frames.get(self.frame_sp.wrapping_sub(1)).ok_or(Trap::Stack)?;
        let locals = self.arena.funcs.get(frame.func as usize).map(|info| info.local_count as usize).unwrap_or(0);
        if index >= locals {
            return Err(Trap::Stack);
        }
        let slot = frame.locals_base as usize + index;
        if let Some(cell) = self.arena.values.get_mut(slot) {
            *cell = value;
            Ok(())
        } else {
            Err(Trap::Stack)
        }
    }

    fn memory_grow(&mut self) -> Result<(), Trap> {
        self.mem_index_byte()?;
        let delta = self.pop_i32()? as u32;
        let old = self.arena.mem_pages;
        let new = old as u64 + delta as u64;
        if new > self.arena.mem_max as u64 || new > 4 {
            return self.push_i32(-1);
        }
        let old_bytes = old as usize * PAGE;
        let new_bytes = new as usize * PAGE;
        if new_bytes > self.arena.memory.len() {
            return self.push_i32(-1);
        }
        for byte in &mut self.arena.memory[old_bytes..new_bytes] {
            *byte = 0;
        }
        self.arena.mem_pages = new as u32;
        self.push_i32(old as i32)
    }

    fn mem_index_byte(&mut self) -> Result<(), Trap> {
        if self.arena.has_memory == 0 {
            return Err(Trap::Memory);
        }
        let Some(byte) = self.module.get(self.pc).copied() else {
            return Err(Trap::Unreachable);
        };
        self.pc += 1;
        if byte != 0 {
            Err(Trap::Unreachable)
        } else {
            Ok(())
        }
    }

    fn load_mem(&mut self, size: usize, sign: Sign) -> Result<(), Trap> {
        let offset = self.memarg()?;
        let addr = self.pop_i32()? as u32;
        let start = mem_bounds(self.arena, addr, offset, size)?;
        let mut buf = [0u8; 8];
        buf[..size].copy_from_slice(&self.arena.memory[start..start + size]);
        let value = match sign {
            Sign::I32 => i32::from_le_bytes(buf[..4].try_into().unwrap_or([0; 4])) as i64,
            Sign::I64 => i64::from_le_bytes(buf),
            Sign::I32S8 => buf[0] as i8 as i32 as i64,
            Sign::I32U8 => buf[0] as i32 as i64,
            Sign::I32S16 => i16::from_le_bytes([buf[0], buf[1]]) as i32 as i64,
            Sign::I32U16 => u16::from_le_bytes([buf[0], buf[1]]) as i32 as i64,
            Sign::I64S8 => buf[0] as i8 as i64,
            Sign::I64U8 => buf[0] as i64,
            Sign::I64S16 => i16::from_le_bytes([buf[0], buf[1]]) as i64,
            Sign::I64U16 => u16::from_le_bytes([buf[0], buf[1]]) as i64,
            Sign::I64S32 => i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64,
            Sign::I64U32 => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64,
        };
        self.push(value)
    }

    fn store_mem(&mut self, size: usize) -> Result<(), Trap> {
        let offset = self.memarg()?;
        let value = self.pop()?;
        let addr = self.pop_i32()? as u32;
        let start = mem_bounds(self.arena, addr, offset, size)?;
        let bytes = (value as u64).to_le_bytes();
        self.arena.memory[start..start + size].copy_from_slice(&bytes[..size]);
        Ok(())
    }

    fn memarg(&mut self) -> Result<u32, Trap> {
        if self.arena.has_memory == 0 {
            return Err(Trap::Memory);
        }
        let _align = self.read_u32()?;
        self.read_u32()
    }

    #[inline(always)]
    fn push(&mut self, value: i64) -> Result<(), Trap> {
        if self.sp >= VALUE_CAP {
            return Err(Trap::Stack);
        }
        self.arena.values[self.sp] = value;
        self.sp += 1;
        Ok(())
    }

    #[inline(always)]
    fn push_i32(&mut self, value: i32) -> Result<(), Trap> {
        self.push(value as i64)
    }

    /// End of the locals, or the top label's stack base when that is higher.
    /// `pop` and `peek` stop at this index so a short operand stack cannot
    /// read a local or a caller's slot.
    fn operand_floor(&self) -> usize {
        let Some(frame) = self.arena.frames.get(self.frame_sp.wrapping_sub(1)) else {
            return 0;
        };
        let locals = self
            .arena
            .funcs
            .get(frame.func as usize)
            .map(|info| info.local_count as usize)
            .unwrap_or(0);
        let mut floor = frame.locals_base as usize + locals;
        if self.label_sp > frame.label_base as usize {
            let top = self.arena.labels[self.label_sp - 1].stack_base as usize;
            if top > floor {
                floor = top;
            }
        }
        floor
    }

    #[inline(always)]
    fn pop(&mut self) -> Result<i64, Trap> {
        if self.sp <= self.operand_floor() {
            return Err(Trap::Stack);
        }
        self.sp -= 1;
        Ok(self.arena.values[self.sp])
    }

    #[inline(always)]
    fn pop_i32(&mut self) -> Result<i32, Trap> {
        Ok(self.pop()? as i32)
    }

    fn peek(&self) -> Result<i64, Trap> {
        if self.sp <= self.operand_floor() {
            return Err(Trap::Stack);
        }
        Ok(self.arena.values[self.sp - 1])
    }

    fn bin_i32(&mut self, f: impl FnOnce(i32, i32) -> i32) -> Result<(), Trap> {
        let b = self.pop_i32()?;
        let a = self.pop_i32()?;
        self.push_i32(f(a, b))
    }

    fn bin_i32_trap(&mut self, f: impl FnOnce(i32, i32) -> Result<i32, Trap>) -> Result<(), Trap> {
        let b = self.pop_i32()?;
        let a = self.pop_i32()?;
        self.push_i32(f(a, b)?)
    }

    fn bin_i64(&mut self, f: impl FnOnce(i64, i64) -> i64) -> Result<(), Trap> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push(f(a, b))
    }

    fn bin_i64_trap(&mut self, f: impl FnOnce(i64, i64) -> Result<i64, Trap>) -> Result<(), Trap> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push(f(a, b)?)
    }

    fn cmp_i32(&mut self, f: impl FnOnce(i32, i32) -> bool) -> Result<(), Trap> {
        let b = self.pop_i32()?;
        let a = self.pop_i32()?;
        self.push_i32(if f(a, b) { 1 } else { 0 })
    }

    fn cmp_i32_u(&mut self, f: impl FnOnce(u32, u32) -> bool) -> Result<(), Trap> {
        let b = self.pop_i32()? as u32;
        let a = self.pop_i32()? as u32;
        self.push_i32(if f(a, b) { 1 } else { 0 })
    }

    fn cmp_i64(&mut self, f: impl FnOnce(i64, i64) -> bool) -> Result<(), Trap> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push_i32(if f(a, b) { 1 } else { 0 })
    }

    fn cmp_i64_u(&mut self, f: impl FnOnce(u64, u64) -> bool) -> Result<(), Trap> {
        let b = self.pop()? as u64;
        let a = self.pop()? as u64;
        self.push_i32(if f(a, b) { 1 } else { 0 })
    }

    fn read_blocktype(&mut self) -> Result<u8, Trap> {
        let Some(byte) = self.module.get(self.pc).copied() else {
            return Err(Trap::Unreachable);
        };
        self.pc += 1;
        match byte {
            0x40 => Ok(0),
            0x7F | 0x7E => Ok(1),
            _ => Err(Trap::Unreachable),
        }
    }

    fn read_u32(&mut self) -> Result<u32, Trap> {
        let mut result = 0u32;
        let mut shift = 0;
        for _ in 0..5 {
            let Some(byte) = self.module.get(self.pc).copied() else {
                return Err(Trap::Unreachable);
            };
            self.pc += 1;
            let piece = (byte & 0x7F) as u32;
            if shift == 28 && piece > 0x0F {
                return Err(Trap::Unreachable);
            }
            result |= piece << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err(Trap::Unreachable)
    }

    fn read_i32(&mut self) -> Result<i32, Trap> {
        Ok(self.read_sleb(32)? as i32)
    }

    fn read_i64(&mut self) -> Result<i64, Trap> {
        self.read_sleb(64)
    }

    fn read_sleb(&mut self, bits: u32) -> Result<i64, Trap> {
        let mut result: i64 = 0;
        let mut shift = 0;
        let max_bytes = (bits as usize + 6) / 7;
        for _ in 0..max_bytes {
            let Some(byte) = self.module.get(self.pc).copied() else {
                return Err(Trap::Unreachable);
            };
            self.pc += 1;
            result |= ((byte & 0x7F) as i64) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    result |= !0i64 << shift;
                }
                return Ok(result);
            }
        }
        Err(Trap::Unreachable)
    }
}

enum Sign {
    I32,
    I64,
    I32S8,
    I32U8,
    I32S16,
    I32U16,
    I64S8,
    I64U8,
    I64S16,
    I64U16,
    I64S32,
    I64U32,
}

fn mem_bounds(arena: &Arena, addr: u32, offset: u32, size: usize) -> Result<usize, Trap> {
    let start = (addr as u64) + (offset as u64);
    let end = start + size as u64;
    let len = (arena.mem_pages as usize).saturating_mul(PAGE) as u64;
    if end > len || start > end {
        return Err(Trap::Memory);
    }
    Ok(start as usize)
}

fn sig_eq(arena: &Arena, a: usize, b: usize) -> bool {
    if a == b {
        return true;
    }
    let Some(left) = arena.types.get(a) else {
        return false;
    };
    let Some(right) = arena.types.get(b) else {
        return false;
    };
    if left.param_count != right.param_count || left.results != right.results || left.result_ty != right.result_ty
    {
        return false;
    }
    let bytes = (left.param_count as usize + 7) / 8;
    left.params.get(..bytes) == right.params.get(..bytes)
}

fn div_s_i32(a: i32, b: i32) -> Result<i32, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    if a == i32::MIN && b == -1 {
        return Err(Trap::Overflow);
    }
    Ok(a.wrapping_div(b))
}

fn div_u_i32(a: i32, b: i32) -> Result<i32, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    Ok((a as u32).wrapping_div(b as u32) as i32)
}

fn rem_s_i32(a: i32, b: i32) -> Result<i32, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    if a == i32::MIN && b == -1 {
        return Ok(0);
    }
    Ok(a.wrapping_rem(b))
}

fn rem_u_i32(a: i32, b: i32) -> Result<i32, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    Ok((a as u32).wrapping_rem(b as u32) as i32)
}

fn div_s_i64(a: i64, b: i64) -> Result<i64, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    if a == i64::MIN && b == -1 {
        return Err(Trap::Overflow);
    }
    Ok(a.wrapping_div(b))
}

fn div_u_i64(a: i64, b: i64) -> Result<i64, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    Ok((a as u64).wrapping_div(b as u64) as i64)
}

fn rem_s_i64(a: i64, b: i64) -> Result<i64, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    if a == i64::MIN && b == -1 {
        return Ok(0);
    }
    Ok(a.wrapping_rem(b))
}

fn rem_u_i64(a: i64, b: i64) -> Result<i64, Trap> {
    if b == 0 {
        return Err(Trap::DivZero);
    }
    Ok((a as u64).wrapping_rem(b as u64) as i64)
}
