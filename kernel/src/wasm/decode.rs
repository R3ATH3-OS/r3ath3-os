//! One-pass decoder and structural validator. No allocation.
//!
//! The validator checks section shapes, indices, block nesting, branch depths,
//! `br_table` lengths, local and global indices and mutability, and memory and
//! table limits. It does not prove operand-stack types. A mismatch it does not
//! check traps at run time.

use super::arena::{
    Arena, ExportInfo, FuncInfo, SideEntry, TypeInfo, EMPTY_FUNC, EXPORT_CAP, FUNC_CAP,
    GLOBAL_CAP, MEM_BYTES, SIDE_CAP, TABLE_CAP, TYPE_CAP, TY_I32, TY_I64,
};
use super::{LoadError, MAX_MODULE};

pub const KIND_FUNC: u8 = 0;
pub const KIND_TABLE: u8 = 1;
pub const KIND_MEM: u8 = 2;
pub const KIND_GLOBAL: u8 = 3;

const MAX_LOCALS: u32 = 256;
const MAX_PAGES: u32 = 4;
const PAGE: u32 = 65_536;

pub fn param_is_i64(ty: &TypeInfo, index: usize) -> bool {
    let byte = ty.params.get(index / 8).copied().unwrap_or(0);
    (byte >> (index % 8)) & 1 == 1
}

pub fn load(module: &[u8], arena: &mut Arena) -> Result<(), LoadError> {
    if module.len() > MAX_MODULE {
        return Err(LoadError::TooBig);
    }
    reset(arena);
    if module.len() < 4 {
        return Err(LoadError::Truncated);
    }
    if module[0..4] != [0x00, 0x61, 0x73, 0x6D] {
        return Err(LoadError::Magic);
    }
    if module.len() < 8 {
        return Err(LoadError::Truncated);
    }
    if module[4..8] != [0x01, 0x00, 0x00, 0x00] {
        return Err(LoadError::Version);
    }

    let mut pos = 8usize;
    let mut rank = 0u8;
    let mut data_count: Option<u32> = None;
    let mut saw_data = false;
    let mut code_count = 0u32;

    while pos < module.len() {
        let id = module[pos];
        pos += 1;
        let (size, next) = read_u32(module, pos, module.len())?;
        pos = next;
        let size = size as usize;
        if size > module.len() - pos {
            return Err(LoadError::Truncated);
        }
        let end = pos + size;
        if id == 0 {
            pos = end;
            continue;
        }
        let next_rank = section_rank(id)?;
        if next_rank <= rank {
            return Err(LoadError::Section);
        }
        rank = next_rank;
        match id {
            1 => parse_types(module, pos, end, arena)?,
            2 => parse_imports(module, pos, end)?,
            3 => parse_funcs(module, pos, end, arena)?,
            4 => parse_table(module, pos, end, arena)?,
            5 => parse_memory(module, pos, end, arena)?,
            6 => parse_globals(module, pos, end, arena)?,
            7 => parse_exports(module, pos, end, arena)?,
            8 => return Err(LoadError::Section),
            9 => parse_elements(module, pos, end, arena)?,
            10 => code_count = parse_code(module, pos, end, arena)?,
            11 => {
                parse_data(module, pos, end, arena)?;
                saw_data = true;
            }
            12 => data_count = Some(parse_data_count(module, pos, end)?),
            _ => return Err(LoadError::Section),
        }
        pos = end;
    }

    if arena.func_count as u32 != code_count {
        return Err(LoadError::Section);
    }
    if let Some(expected) = data_count {
        let actual = if saw_data { arena_data_count(arena) } else { 0 };
        if expected != actual {
            return Err(LoadError::Section);
        }
    }
    Ok(())
}

fn arena_data_count(arena: &Arena) -> u32 {
    arena._pad[0] as u32 | ((arena._pad[1] as u32) << 8)
}

fn set_data_count(arena: &mut Arena, count: u32) {
    arena._pad[0] = count as u8;
    arena._pad[1] = (count >> 8) as u8;
}

fn reset(arena: &mut Arena) {
    let pages = arena.mem_pages as usize;
    let bytes = pages.saturating_mul(PAGE as usize).min(arena.memory.len());
    for byte in &mut arena.memory[..bytes] {
        *byte = 0;
    }
    let slots = arena.table_size as usize;
    for slot in &mut arena.table[..slots.min(TABLE_CAP)] {
        *slot = EMPTY_FUNC;
    }
    arena.mem_pages = 0;
    arena.mem_max = 0;
    arena.table_size = 0;
    arena.global_count = 0;
    arena.func_count = 0;
    arena.type_count = 0;
    arena.export_count = 0;
    arena.side_count = 0;
    arena.has_memory = 0;
    arena.has_table = 0;
    arena._pad = [0; 2];
}

fn section_rank(id: u8) -> Result<u8, LoadError> {
    match id {
        1 => Ok(1),
        2 => Ok(2),
        3 => Ok(3),
        4 => Ok(4),
        5 => Ok(5),
        6 => Ok(6),
        7 => Ok(7),
        8 => Ok(8),
        9 => Ok(9),
        12 => Ok(10),
        10 => Ok(11),
        11 => Ok(12),
        _ => Err(LoadError::Section),
    }
}

fn parse_types(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count as usize > TYPE_CAP {
        return Err(LoadError::Limits);
    }
    for _ in 0..count {
        if pos >= end {
            return Err(LoadError::Section);
        }
        if module[pos] != 0x60 {
            return Err(LoadError::Section);
        }
        pos += 1;
        let (params, next) = read_u32(module, pos, end)?;
        pos = next;
        if params > MAX_LOCALS {
            return Err(LoadError::Limits);
        }
        let mut info = TypeInfo {
            param_count: params as u16,
            results: 0,
            result_ty: 0,
            params: [0; 32],
        };
        for index in 0..params as usize {
            let (ty, next) = read_valtype(module, pos, end)?;
            pos = next;
            if ty == TY_I64 {
                info.params[index / 8] |= 1 << (index % 8);
            }
        }
        let (results, next) = read_u32(module, pos, end)?;
        pos = next;
        if results > 1 {
            return Err(LoadError::Opcode);
        }
        if results == 1 {
            let (ty, next) = read_valtype(module, pos, end)?;
            pos = next;
            info.results = 1;
            info.result_ty = ty;
        }
        let slot = arena.type_count as usize;
        arena.types[slot] = info;
        arena.type_count += 1;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_imports(module: &[u8], mut pos: usize, end: usize) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count > 0 {
        return Err(LoadError::Import);
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_funcs(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count as usize > FUNC_CAP {
        return Err(LoadError::Limits);
    }
    for _ in 0..count {
        let (type_index, next) = read_u32(module, pos, end)?;
        pos = next;
        if type_index as usize >= arena.type_count as usize {
            return Err(LoadError::Section);
        }
        let ty = &arena.types[type_index as usize];
        let slot = arena.func_count as usize;
        arena.funcs[slot] = FuncInfo {
            type_index: type_index as u16,
            param_count: ty.param_count,
            local_count: ty.param_count,
            results: ty.results,
            result_ty: ty.result_ty,
            body: 0,
            body_end: 0,
        };
        arena.func_count += 1;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_table(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count == 0 {
        return Err(LoadError::Section);
    }
    if count > 1 {
        return Err(LoadError::Limits);
    }
    if pos >= end {
        return Err(LoadError::Section);
    }
    if module[pos] != 0x70 {
        return Err(LoadError::Opcode);
    }
    pos += 1;
    let (min, max, next) = read_limits(module, pos, end, true)?;
    pos = next;
    if max > TABLE_CAP as u32 || min > max {
        return Err(LoadError::Limits);
    }
    arena.table_size = min;
    arena.has_table = 1;
    for slot in &mut arena.table[..min as usize] {
        *slot = EMPTY_FUNC;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_memory(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count == 0 {
        return Err(LoadError::Section);
    }
    if count > 1 {
        return Err(LoadError::Limits);
    }
    let (min, max, next) = read_limits(module, pos, end, true)?;
    pos = next;
    if max > MAX_PAGES || min > max {
        return Err(LoadError::Limits);
    }
    arena.mem_pages = min;
    arena.mem_max = max;
    arena.has_memory = 1;
    let bytes = (min as usize).saturating_mul(PAGE as usize).min(MEM_BYTES);
    for byte in &mut arena.memory[..bytes] {
        *byte = 0;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_globals(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count as usize > GLOBAL_CAP {
        return Err(LoadError::Limits);
    }
    for _ in 0..count {
        let (ty, next) = read_valtype(module, pos, end)?;
        pos = next;
        if pos >= end {
            return Err(LoadError::Section);
        }
        let mutable = module[pos];
        pos += 1;
        if mutable > 1 {
            return Err(LoadError::Section);
        }
        let (value, next) = read_const(module, pos, end, ty)?;
        pos = next;
        let slot = arena.global_count as usize;
        arena.globals[slot] = value;
        arena.global_mut[slot] = mutable;
        arena.global_count += 1;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_exports(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count as usize > EXPORT_CAP {
        return Err(LoadError::Limits);
    }
    for _ in 0..count {
        let (name_len, next) = read_u32(module, pos, end)?;
        pos = next;
        if name_len > 32 {
            return Err(LoadError::Export);
        }
        let name_len = name_len as usize;
        if name_len > end - pos {
            return Err(LoadError::Truncated);
        }
        let mut name = [0u8; 32];
        name[..name_len].copy_from_slice(&module[pos..pos + name_len]);
        pos += name_len;
        for index in 0..arena.export_count as usize {
            let prev = &arena.exports[index];
            if prev.name_len as usize == name_len && prev.name[..name_len] == name[..name_len] {
                return Err(LoadError::Export);
            }
        }
        if pos >= end {
            return Err(LoadError::Section);
        }
        let kind = module[pos];
        pos += 1;
        let (index, next) = read_u32(module, pos, end)?;
        pos = next;
        let limit = match kind {
            KIND_FUNC => arena.func_count as u32,
            KIND_TABLE => arena.has_table as u32,
            KIND_MEM => arena.has_memory as u32,
            KIND_GLOBAL => arena.global_count as u32,
            _ => return Err(LoadError::Section),
        };
        if index >= limit {
            return Err(LoadError::Section);
        }
        let slot = arena.export_count as usize;
        arena.exports[slot] = ExportInfo {
            name,
            name_len: name_len as u8,
            kind,
            index: index as u16,
        };
        arena.export_count += 1;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_elements(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count > 0 && arena.has_table == 0 {
        return Err(LoadError::Section);
    }
    for _ in 0..count {
        let (flags, next) = read_u32(module, pos, end)?;
        pos = next;
        if flags == 2 {
            let (table, next) = read_u32(module, pos, end)?;
            pos = next;
            if table != 0 {
                return Err(LoadError::Opcode);
            }
        } else if flags != 0 {
            return Err(LoadError::Section);
        }
        let (offset, next) = read_const(module, pos, end, TY_I32)?;
        pos = next;
        if flags == 2 {
            if pos >= end || module[pos] != 0x00 {
                return Err(LoadError::Section);
            }
            pos += 1;
        }
        let (n, next) = read_u32(module, pos, end)?;
        pos = next;
        if offset < 0 {
            return Err(LoadError::Limits);
        }
        let start = offset as u32;
        let last = (start as u64).saturating_add(n as u64);
        if last > arena.table_size as u64 {
            return Err(LoadError::Limits);
        }
        for slot in 0..n {
            let (func, next) = read_u32(module, pos, end)?;
            pos = next;
            if func >= arena.func_count as u32 {
                return Err(LoadError::Section);
            }
            arena.table[(start + slot) as usize] = func;
        }
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn parse_data(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<(), LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count > 0 && arena.has_memory == 0 {
        return Err(LoadError::Section);
    }
    if count > 65_535 {
        return Err(LoadError::Limits);
    }
    for _ in 0..count {
        let (flags, next) = read_u32(module, pos, end)?;
        pos = next;
        if flags == 2 {
            let (mem, next) = read_u32(module, pos, end)?;
            pos = next;
            if mem != 0 {
                return Err(LoadError::Opcode);
            }
        } else if flags != 0 {
            return Err(LoadError::Section);
        }
        let (offset, next) = read_const(module, pos, end, TY_I32)?;
        pos = next;
        let (n, next) = read_u32(module, pos, end)?;
        pos = next;
        if n as usize > end - pos {
            return Err(LoadError::Truncated);
        }
        if offset < 0 {
            return Err(LoadError::Limits);
        }
        let start = offset as usize;
        let mem_len = (arena.mem_pages as usize).saturating_mul(PAGE as usize);
        if start.saturating_add(n as usize) > mem_len {
            return Err(LoadError::Limits);
        }
        arena.memory[start..start + n as usize].copy_from_slice(&module[pos..pos + n as usize]);
        pos += n as usize;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    set_data_count(arena, count);
    Ok(())
}

fn parse_data_count(module: &[u8], mut pos: usize, end: usize) -> Result<u32, LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(count)
}

fn parse_code(module: &[u8], mut pos: usize, end: usize, arena: &mut Arena) -> Result<u32, LoadError> {
    let (count, next) = read_u32(module, pos, end)?;
    pos = next;
    if count != arena.func_count as u32 {
        return Err(LoadError::Section);
    }
    for func in 0..count {
        let (body_size, next) = read_u32(module, pos, end)?;
        pos = next;
        if body_size as usize > end - pos {
            return Err(LoadError::Truncated);
        }
        let body_end = pos + body_size as usize;
        let local_extra = read_locals(module, &mut pos, body_end)?;
        let info = &mut arena.funcs[func as usize];
        let total = info.param_count as u32 + local_extra;
        if total > MAX_LOCALS {
            return Err(LoadError::Limits);
        }
        info.local_count = total as u16;
        info.body = pos as u32;
        info.body_end = body_end as u32;
        validate_body(module, pos, body_end, arena, func as usize)?;
        pos = body_end;
    }
    if pos != end {
        return Err(LoadError::Section);
    }
    Ok(count)
}

fn read_locals(module: &[u8], pos: &mut usize, end: usize) -> Result<u32, LoadError> {
    let (groups, next) = read_u32(module, *pos, end)?;
    *pos = next;
    let mut total = 0u32;
    for _ in 0..groups {
        let (n, next) = read_u32(module, *pos, end)?;
        *pos = next;
        if n > MAX_LOCALS || total > MAX_LOCALS - n {
            return Err(LoadError::Limits);
        }
        let (_ty, next) = read_valtype(module, *pos, end)?;
        *pos = next;
        total += n;
    }
    Ok(total)
}

fn validate_body(
    module: &[u8],
    mut pos: usize,
    end: usize,
    arena: &mut Arena,
    func: usize,
) -> Result<(), LoadError> {
    // Control stack lives in `values` for the duration of validation. Low 32
    // bits are the side index of a block, loop, or if. High 32 bits are the
    // side index of its `else`, or `u32::MAX` when there is no else. `else`
    // is not its own label.
    let mut csp = 0usize;
    let mut saw_end = false;
    while pos < end {
        if saw_end {
            return Err(LoadError::Section);
        }
        let op_pc = pos as u32;
        let op = module[pos];
        pos += 1;
        if is_float_opcode(op) {
            return Err(LoadError::Float);
        }
        match op {
            0x00 | 0x01 | 0x0F | 0x1A | 0x1B => {}
            0x02 | 0x03 | 0x04 => {
                let (_results, next) = read_blocktype(module, pos, end)?;
                pos = next;
                let index = append_side(arena, op_pc)?;
                push_ctrl(arena, &mut csp, index, u32::MAX)?;
            }
            0x05 => {
                if csp == 0 {
                    return Err(LoadError::Opcode);
                }
                let (index, else_index) = ctrl_top(arena, csp);
                if else_index != u32::MAX {
                    return Err(LoadError::Opcode);
                }
                if module.get(arena.side[index as usize].at as usize).copied() != Some(0x04) {
                    return Err(LoadError::Opcode);
                }
                let else_side = append_side(arena, op_pc)?;
                arena.side[index as usize].target = op_pc;
                set_ctrl(arena, csp, index, else_side);
            }
            0x0B => {
                if csp == 0 {
                    saw_end = true;
                } else {
                    let (index, else_index) = pop_ctrl(arena, &mut csp);
                    if else_index == u32::MAX {
                        arena.side[index as usize].target = op_pc;
                    } else {
                        arena.side[else_index as usize].target = op_pc;
                    }
                }
            }
            0x0C | 0x0D => {
                let (depth, next) = read_u32(module, pos, end)?;
                pos = next;
                if depth as usize >= csp {
                    return Err(LoadError::Opcode);
                }
            }
            0x0E => {
                let (n, next) = read_u32(module, pos, end)?;
                pos = next;
                if n as usize > end.saturating_sub(pos) {
                    return Err(LoadError::Section);
                }
                for _ in 0..n {
                    let (depth, next) = read_u32(module, pos, end)?;
                    pos = next;
                    if depth as usize >= csp {
                        return Err(LoadError::Opcode);
                    }
                }
                let (depth, next) = read_u32(module, pos, end)?;
                pos = next;
                if depth as usize >= csp {
                    return Err(LoadError::Opcode);
                }
                push_side_closed(arena, op_pc, pos as u32)?;
            }
            0x10 => {
                let (index, next) = read_u32(module, pos, end)?;
                pos = next;
                if index >= arena.func_count as u32 {
                    return Err(LoadError::Opcode);
                }
            }
            0x11 => {
                let (type_index, next) = read_u32(module, pos, end)?;
                pos = next;
                if type_index as usize >= arena.type_count as usize {
                    return Err(LoadError::Opcode);
                }
                if arena.has_table == 0 {
                    return Err(LoadError::Opcode);
                }
                let (table, next) = read_u32(module, pos, end)?;
                pos = next;
                if table != 0 {
                    return Err(LoadError::Opcode);
                }
            }
            0x20 | 0x21 | 0x22 => {
                let (index, next) = read_u32(module, pos, end)?;
                pos = next;
                if index >= arena.funcs[func].local_count as u32 {
                    return Err(LoadError::Opcode);
                }
            }
            0x23 | 0x24 => {
                let (index, next) = read_u32(module, pos, end)?;
                pos = next;
                if index >= arena.global_count as u32 {
                    return Err(LoadError::Opcode);
                }
                if op == 0x24 && arena.global_mut[index as usize] == 0 {
                    return Err(LoadError::Opcode);
                }
            }
            0x28 | 0x29 | 0x2C | 0x2D | 0x2E | 0x2F | 0x30 | 0x31 | 0x32 | 0x33 | 0x34 | 0x35
            | 0x36 | 0x37 | 0x3A | 0x3B | 0x3C | 0x3D | 0x3E => {
                if arena.has_memory == 0 {
                    return Err(LoadError::Opcode);
                }
                let (_align, next) = read_u32(module, pos, end)?;
                pos = next;
                let (_offset, next) = read_u32(module, pos, end)?;
                pos = next;
            }
            0x3F | 0x40 => {
                if arena.has_memory == 0 {
                    return Err(LoadError::Opcode);
                }
                if pos >= end {
                    return Err(LoadError::Section);
                }
                if module[pos] != 0x00 {
                    return Err(LoadError::Opcode);
                }
                pos += 1;
            }
            0x41 => {
                let (_value, next) = read_i32(module, pos, end)?;
                pos = next;
            }
            0x42 => {
                let (_value, next) = read_i64(module, pos, end)?;
                pos = next;
            }
            0x45 | 0x50 | 0x67 | 0x68 | 0x69 | 0x79 | 0x7A | 0x7B | 0xA7 | 0xAC | 0xAD | 0xC0
            | 0xC1 | 0xC2 | 0xC3 | 0xC4 => {}
            0x46..=0x4F | 0x51..=0x5A | 0x6A..=0x78 | 0x7C..=0x8A => {}
            _ => return Err(LoadError::Opcode),
        }
    }
    if !saw_end || csp != 0 {
        return Err(LoadError::Section);
    }
    Ok(())
}

fn append_side(arena: &mut Arena, at: u32) -> Result<u32, LoadError> {
    push_side_closed(arena, at, 0)
}

fn push_ctrl(arena: &mut Arena, csp: &mut usize, index: u32, else_index: u32) -> Result<(), LoadError> {
    if *csp >= arena.values.len() {
        return Err(LoadError::Limits);
    }
    arena.values[*csp] = pack_ctrl(index, else_index);
    *csp += 1;
    Ok(())
}

fn set_ctrl(arena: &mut Arena, csp: usize, index: u32, else_index: u32) {
    arena.values[csp - 1] = pack_ctrl(index, else_index);
}

fn ctrl_top(arena: &Arena, csp: usize) -> (u32, u32) {
    unpack_ctrl(arena.values[csp - 1])
}

fn pop_ctrl(arena: &Arena, csp: &mut usize) -> (u32, u32) {
    *csp -= 1;
    unpack_ctrl(arena.values[*csp])
}

fn pack_ctrl(index: u32, else_index: u32) -> i64 {
    (((else_index as u64) << 32) | index as u64) as i64
}

fn unpack_ctrl(packed: i64) -> (u32, u32) {
    let bits = packed as u64;
    (bits as u32, (bits >> 32) as u32)
}

fn push_side_closed(arena: &mut Arena, at: u32, target: u32) -> Result<u32, LoadError> {
    if arena.side_count as usize >= SIDE_CAP {
        return Err(LoadError::Limits);
    }
    let index = arena.side_count;
    arena.side[index as usize] = SideEntry { at, target };
    arena.side_count += 1;
    Ok(index)
}

fn read_blocktype(module: &[u8], pos: usize, end: usize) -> Result<(u8, usize), LoadError> {
    if pos >= end {
        return Err(if pos >= module.len() {
            LoadError::Truncated
        } else {
            LoadError::Section
        });
    }
    match module[pos] {
        0x40 => Ok((0, pos + 1)),
        0x7F => Ok((1, pos + 1)),
        0x7E => Ok((2, pos + 1)),
        0x7C | 0x7D => Err(LoadError::Float),
        _ => Err(LoadError::Opcode),
    }
}

fn read_const(module: &[u8], mut pos: usize, end: usize, ty: u8) -> Result<(i64, usize), LoadError> {
    if pos >= end {
        return Err(LoadError::Section);
    }
    let op = module[pos];
    pos += 1;
    let (value, next) = if op == 0x41 && ty == TY_I32 {
        let (value, next) = read_i32(module, pos, end)?;
        (value as i64, next)
    } else if op == 0x42 && ty == TY_I64 {
        read_i64(module, pos, end)?
    } else if is_float_opcode(op) || op == 0x7C || op == 0x7D {
        return Err(LoadError::Float);
    } else {
        return Err(LoadError::Opcode);
    };
    pos = next;
    if pos >= end || module[pos] != 0x0B {
        return Err(LoadError::Opcode);
    }
    Ok((value, pos + 1))
}

fn read_valtype(module: &[u8], pos: usize, end: usize) -> Result<(u8, usize), LoadError> {
    if pos >= end {
        return Err(if pos >= module.len() {
            LoadError::Truncated
        } else {
            LoadError::Section
        });
    }
    match module[pos] {
        0x7F => Ok((TY_I32, pos + 1)),
        0x7E => Ok((TY_I64, pos + 1)),
        0x7C | 0x7D => Err(LoadError::Float),
        _ => Err(LoadError::Opcode),
    }
}

fn read_limits(
    module: &[u8],
    mut pos: usize,
    end: usize,
    require_max: bool,
) -> Result<(u32, u32, usize), LoadError> {
    if pos >= end {
        return Err(LoadError::Section);
    }
    let flags = module[pos];
    pos += 1;
    if flags & 0x06 != 0 {
        return Err(LoadError::Opcode);
    }
    let has_max = flags & 0x01 != 0;
    if require_max && !has_max {
        return Err(LoadError::Limits);
    }
    let (min, next) = read_u32(module, pos, end)?;
    pos = next;
    let max = if has_max {
        let (max, next) = read_u32(module, pos, end)?;
        pos = next;
        max
    } else {
        min
    };
    Ok((min, max, pos))
}

fn is_float_opcode(op: u8) -> bool {
    matches!(
        op,
        0x2A | 0x2B | 0x38 | 0x39 | 0x43 | 0x44 | 0x5B..=0x66 | 0x8B..=0xA6 | 0xA8..=0xAB | 0xAE..=0xBF
    )
}

fn read_u32(module: &[u8], mut pos: usize, end: usize) -> Result<(u32, usize), LoadError> {
    let mut result = 0u32;
    let mut shift = 0;
    for _ in 0..5 {
        if pos >= end {
            return Err(if pos >= module.len() {
                LoadError::Truncated
            } else {
                LoadError::Section
            });
        }
        let byte = module[pos];
        pos += 1;
        let piece = (byte & 0x7F) as u32;
        if shift == 28 && piece > 0x0F {
            return Err(LoadError::Section);
        }
        result |= piece << shift;
        if byte & 0x80 == 0 {
            return Ok((result, pos));
        }
        shift += 7;
    }
    Err(LoadError::Section)
}

fn read_i32(module: &[u8], pos: usize, end: usize) -> Result<(i32, usize), LoadError> {
    let (value, next) = read_sleb(module, pos, end, 32)?;
    if value != value as i32 as i64 {
        return Err(LoadError::Section);
    }
    Ok((value as i32, next))
}

fn read_i64(module: &[u8], pos: usize, end: usize) -> Result<(i64, usize), LoadError> {
    read_sleb(module, pos, end, 64)
}

fn read_sleb(module: &[u8], mut pos: usize, end: usize, bits: u32) -> Result<(i64, usize), LoadError> {
    let mut result: i64 = 0;
    let mut shift = 0;
    let mut byte;
    let max_bytes = (bits as usize + 6) / 7;
    for _ in 0..max_bytes {
        if pos >= end {
            return Err(if pos >= module.len() {
                LoadError::Truncated
            } else {
                LoadError::Section
            });
        }
        byte = module[pos];
        pos += 1;
        result |= ((byte & 0x7F) as i64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && byte & 0x40 != 0 {
                result |= !0i64 << shift;
            }
            return Ok((result, pos));
        }
    }
    Err(LoadError::Section)
}

pub fn find_side(arena: &Arena, at: u32) -> Option<u32> {
    let side = &arena.side[..arena.side_count as usize];
    side.binary_search_by_key(&at, |entry| entry.at)
        .ok()
        .map(|index| side[index].target)
}
