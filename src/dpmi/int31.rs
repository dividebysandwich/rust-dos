//! INT 31h: the DPMI host's services to its clients in protected mode, as
//! DPMI 0.9 has them: LDT descriptors (00xxh), DOS memory (01xxh),
//! interrupt vectors (02xxh), calls to real-mode code and callbacks from it
//! (03xxh), the version (0400h), memory blocks (05xxh), page locking and
//! physical mappings (06xxh-08xxh) and the virtual interrupt flag (09xxh).
//! A failed call returns CF set and a DPMI 1.0 error code in AX.

use super::{
    BIG, CALLBACKS, Callback, Client, Context, DATA3, EAX, EBX, ECX, EDI, EDX, ESI, ESP, F_TRANSLATE, Frame,
    HOST_CODE3, LDT_CLIENT, LDT_FIXED, LDT_FREE, RAW_TO_PM, RAW_TO_RM, RESERVED_LDT, ROM_SEG, RM_RETURN, SAVE_PM,
    SAVE_RM, allocate, client, descriptor, enter_rm, free_entry, keep_lpms, push_real, read_desc, real_mode_handler,
    resume, selector_base, set_vector, write_desc,
};
use crate::cpu::{Cpu, CpuFlags, CpuResult, Descriptor, Seg};

/// DPMI 1.0 error codes.
const UNSUPPORTED: u16 = 0x8001;
const DESCRIPTOR_UNAVAILABLE: u16 = 0x8011;
const PHYSICAL_MEMORY_UNAVAILABLE: u16 = 0x8013;
const CALLBACK_UNAVAILABLE: u16 = 0x8015;
const HANDLE_UNAVAILABLE: u16 = 0x8016;
const INVALID_VALUE: u16 = 0x8021;
const INVALID_SELECTOR: u16 = 0x8022;
const INVALID_HANDLE: u16 = 0x8023;
const INVALID_CALLBACK: u16 = 0x8024;
/// DOS's "insufficient memory".
const DOS_NO_MEMORY: u16 = 0x0008;

/// The size of the buffer the state save and restore entry points want
/// (INT 31h AX=0305h). They save nothing, but a client may not call them
/// with a size of 0.
const STATE_SIZE: u16 = 4;

type Result = std::result::Result<(), u16>;

/// INT 31h, with the client's context `ctx`: its general registers there,
/// the others (DS, ES) as the processor has them.
pub(super) fn call(cpu: &mut Cpu, mut ctx: Context) {
    let function = ctx.reg16(EAX);
    if (0x0300..=0x0302).contains(&function) {
        return call_real_mode(cpu, ctx, function);
    }
    match service(cpu, &mut ctx, function) {
        Ok(()) => ctx.set_cf(false),
        Err(code) => {
            if code == UNSUPPORTED {
                cpu.bus.log_string(&format!("[DPMI] Unsupported INT 31h AX={:04X}", function));
            }
            ctx.set_reg16(EAX, code);
            ctx.set_cf(true);
        }
    }
    resume(cpu, &ctx);
}

/// An offset in the register `reg`: 32 bits for a 32-bit client, 16
/// otherwise.
fn offset(ctx: &Context, reg: usize, bits32: bool) -> u32 {
    if bits32 { ctx.gpr[reg] } else { ctx.gpr[reg] & 0xFFFF }
}

/// A 32-bit value in two 16-bit registers, high:low.
fn pair(ctx: &Context, high: usize, low: usize) -> u32 {
    (ctx.reg16(high) as u32) << 16 | ctx.reg16(low) as u32
}

fn set_pair(ctx: &mut Context, high: usize, low: usize, value: u32) {
    ctx.set_reg16(high, (value >> 16) as u16);
    ctx.set_reg16(low, value as u16);
}

/// Set an offset register: all of it for a 32-bit client, the low half
/// otherwise.
fn set_offset(ctx: &mut Context, reg: usize, bits32: bool, value: u32) {
    if bits32 {
        ctx.gpr[reg] = value;
    } else {
        ctx.set_reg16(reg, value as u16);
    }
}

fn set_al(ctx: &mut Context, value: u8) {
    ctx.gpr[EAX] = (ctx.gpr[EAX] & !0xFF) | value as u32;
}

/// The LDT index of `selector` if it is the running client's, with its use.
fn own(cpu: &mut Cpu, selector: u16) -> std::result::Result<(usize, u8), u16> {
    let client = client(cpu);
    let index = client.index(selector).ok_or(INVALID_SELECTOR)?;
    Ok((index, client.ldt[index]))
}

fn with_desc(cpu: &mut Cpu, selector: u16, change: impl FnOnce(u64) -> u64) -> Result {
    let (index, _) = own(cpu, selector)?;
    let ldt = client(cpu).block;
    let desc = read_desc(&cpu.bus, ldt, index);
    write_desc(&mut cpu.bus, ldt, index, change(desc));
    Ok(())
}

/// A descriptor's base address bits.
const BASE_BITS: u64 = 0xFF00_00FF_FFFF_0000;
/// A descriptor's limit bits and G.
const LIMIT_BITS: u64 = 0x008F_0000_0000_FFFF;

/// Whether a client may put `desc` in its LDT: a code or data segment at
/// its level (present or not).
fn allowed(desc: u64) -> bool {
    let access = (desc >> 40) as u8;
    access & 0x10 != 0 && access & 0x60 == 0x60
}

fn service(cpu: &mut Cpu, ctx: &mut Context, function: u16) -> Result {
    let bits32 = client(cpu).bits32;
    let end = cpu.bus.ram().len() as u32;
    match function {
        // Allocate CX descriptors: data segments at 0 with a limit of 0.
        0x0000 => {
            let count = ctx.reg16(ECX) as usize;
            let desc = descriptor(0, 0, DATA3, if bits32 { BIG } else { 0 });
            let first = allocate(cpu, count, desc, LDT_CLIENT).ok_or(DESCRIPTOR_UNAVAILABLE)?;
            ctx.set_reg16(EAX, Client::selector(first));
        }
        // Free the descriptor BX.
        0x0001 => match own(cpu, ctx.reg16(EBX))? {
            (index, LDT_CLIENT) => free_entry(cpu, index),
            _ => return Err(INVALID_SELECTOR),
        },
        // A descriptor for the real-mode segment BX, the same one each time.
        0x0002 => {
            let segment = ctx.reg16(EBX);
            let known = client(cpu).segments.iter().find(|&&(s, _)| s == segment).map(|&(_, sel)| sel);
            let selector = match known {
                Some(selector) => selector,
                None => {
                    let desc = descriptor((segment as u32) << 4, 0xFFFF, DATA3, 0);
                    let index = allocate(cpu, 1, desc, LDT_FIXED).ok_or(DESCRIPTOR_UNAVAILABLE)?;
                    let selector = Client::selector(index);
                    client(cpu).segments.push((segment, selector));
                    selector
                }
            };
            ctx.set_reg16(EAX, selector);
        }
        // The selector increment.
        0x0003 => ctx.set_reg16(EAX, 8),
        // Lock and unlock a selector's memory: it is always there.
        0x0004 | 0x0005 => {}
        // The base address of BX in CX:DX.
        0x0006 => {
            let base = selector_base(cpu, ctx.reg16(EBX)).ok_or(INVALID_SELECTOR)?;
            set_pair(ctx, ECX, EDX, base);
        }
        // Set the base address of BX to CX:DX.
        0x0007 => {
            let base = pair(ctx, ECX, EDX);
            with_desc(cpu, ctx.reg16(EBX), |d| (d & !BASE_BITS) | (descriptor(base, 0, 0, 0) & BASE_BITS))?;
        }
        // Set the limit of BX to CX:DX: above 1 MB, in whole pages.
        0x0008 => {
            let limit = pair(ctx, ECX, EDX);
            if limit > 0xF_FFFF && limit & 0xFFF != 0xFFF {
                return Err(INVALID_VALUE);
            }
            with_desc(cpu, ctx.reg16(EBX), |d| (d & !LIMIT_BITS) | (descriptor(0, limit, 0, 0) & LIMIT_BITS))?;
        }
        // Set the access rights of BX: CL, and G, D/B and AVL from CH.
        0x0009 => {
            let (cl, ch) = (ctx.reg16(ECX) as u8, (ctx.reg16(ECX) >> 8) as u8);
            let rights = (cl as u64) << 40 | ((ch & 0xD0) as u64) << 48;
            if !allowed(rights) || ch & 0x20 != 0 {
                return Err(INVALID_VALUE);
            }
            with_desc(cpu, ctx.reg16(EBX), |d| (d & !0x00D0_FF00_0000_0000) | rights)?;
        }
        // A data descriptor for the code segment BX.
        0x000A => {
            let (index, _) = own(cpu, ctx.reg16(EBX))?;
            let ldt = client(cpu).block;
            let desc = read_desc(&cpu.bus, ldt, index);
            if !Descriptor(desc).is_code() {
                return Err(INVALID_SELECTOR);
            }
            let alias = (desc & !(0xFu64 << 40)) | 0x2u64 << 40;
            let index = allocate(cpu, 1, alias, LDT_CLIENT).ok_or(DESCRIPTOR_UNAVAILABLE)?;
            ctx.set_reg16(EAX, Client::selector(index));
        }
        // The descriptor BX, into the 8 bytes at ES:(E)DI.
        0x000B => {
            let (index, _) = own(cpu, ctx.reg16(EBX))?;
            let ldt = client(cpu).block;
            let desc = read_desc(&cpu.bus, ldt, index);
            let at = offset(ctx, EDI, bits32);
            write_buffer(cpu, at, &desc.to_le_bytes()).map_err(|_| INVALID_VALUE)?;
        }
        // Set the descriptor BX from the 8 bytes at ES:(E)DI.
        0x000C => {
            let (index, _) = own(cpu, ctx.reg16(EBX))?;
            let at = offset(ctx, EDI, bits32);
            let mut bytes = [0u8; 8];
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = cpu.read_u8(Seg::ES, at.wrapping_add(i as u32)).map_err(|_| INVALID_VALUE)?;
            }
            let desc = u64::from_le_bytes(bytes);
            if !allowed(desc) {
                return Err(INVALID_VALUE);
            }
            let ldt = client(cpu).block;
            write_desc(&mut cpu.bus, ldt, index, desc);
        }
        // Allocate the descriptor BX, one of the first 16.
        0x000D => {
            let selector = ctx.reg16(EBX);
            let index = (selector >> 3) as usize;
            let c = client(cpu);
            if selector & 4 == 0 || index >= RESERVED_LDT {
                return Err(INVALID_SELECTOR);
            }
            if c.ldt[index] != LDT_FREE {
                return Err(DESCRIPTOR_UNAVAILABLE);
            }
            c.ldt[index] = LDT_CLIENT;
            let ldt = c.block;
            write_desc(&mut cpu.bus, ldt, index, descriptor(0, 0, DATA3, if bits32 { BIG } else { 0 }));
        }
        0x0100 => dos_allocate(cpu, ctx)?,
        0x0101 => dos_free(cpu, ctx)?,
        0x0102 => dos_resize(cpu, ctx)?,
        // The real-mode vector BL in CX:DX.
        0x0200 => {
            let vector = super::read_ivt(&cpu.bus, ctx.gpr[EBX] as u8);
            set_pair(ctx, ECX, EDX, vector);
        }
        // Set the real-mode vector BL to CX:DX.
        0x0201 => super::write_ivt(&mut cpu.bus, ctx.gpr[EBX] as u8, pair(ctx, ECX, EDX)),
        // The exception handler BL in CX:(E)DX, and set it.
        0x0202 | 0x0203 => {
            let exception = ctx.gpr[EBX] as u8 as usize;
            if exception >= super::EXCEPTIONS {
                return Err(INVALID_VALUE);
            }
            if function == 0x0202 {
                let (sel, off) = client(cpu).exceptions[exception];
                ctx.set_reg16(ECX, sel);
                set_offset(ctx, EDX, bits32, off);
            } else {
                client(cpu).exceptions[exception] = (ctx.reg16(ECX), offset(ctx, EDX, bits32));
            }
        }
        // The protected-mode interrupt vector BL in CX:(E)DX, and set it.
        0x0204 => {
            let (sel, off) = client(cpu).vectors[ctx.gpr[EBX] as u8 as usize];
            ctx.set_reg16(ECX, sel);
            set_offset(ctx, EDX, bits32, off);
        }
        0x0205 => set_vector(cpu, ctx.gpr[EBX] as u8, (ctx.reg16(ECX), offset(ctx, EDX, bits32))),
        // A real-mode callback to DS:(E)SI with the call structure at
        // ES:(E)DI, its address in CX:DX.
        0x0303 => {
            let id = client(cpu).id;
            let slot = cpu.bus.dpmi.callbacks.iter().position(Option::is_none).ok_or(CALLBACK_UNAVAILABLE)?;
            cpu.bus.dpmi.callbacks[slot] = Some(Callback {
                client: id,
                proc_sel: ctx.sel(Seg::DS),
                proc_off: offset(ctx, ESI, bits32),
                struct_sel: ctx.sel(Seg::ES),
                struct_off: offset(ctx, EDI, bits32),
            });
            ctx.set_reg16(ECX, ROM_SEG);
            ctx.set_reg16(EDX, CALLBACKS + 4 * slot as u16);
        }
        // Free the callback CX:DX.
        0x0304 => {
            let id = client(cpu).id;
            let slot = (ctx.reg16(EDX).wrapping_sub(CALLBACKS) / 4) as usize;
            let dpmi = &mut cpu.bus.dpmi;
            let valid = ctx.reg16(ECX) == ROM_SEG
                && ctx.reg16(EDX) >= CALLBACKS
                && ctx.reg16(EDX) % 4 == 0
                && dpmi.callbacks.get(slot).copied().flatten().is_some_and(|cb| cb.client == id);
            if !valid {
                return Err(INVALID_CALLBACK);
            }
            dpmi.callbacks[slot] = None;
        }
        // The state save and restore entry points.
        0x0305 => {
            ctx.set_reg16(EAX, STATE_SIZE);
            set_pair(ctx, EBX, ECX, (ROM_SEG as u32) << 16 | SAVE_RM as u32);
            ctx.set_reg16(ESI, HOST_CODE3);
            set_offset(ctx, EDI, bits32, SAVE_PM as u32);
        }
        // The raw mode switch entry points.
        0x0306 => {
            set_pair(ctx, EBX, ECX, (ROM_SEG as u32) << 16 | RAW_TO_PM as u32);
            ctx.set_reg16(ESI, HOST_CODE3);
            set_offset(ctx, EDI, bits32, RAW_TO_RM as u32);
        }
        // Version 0.90 of a 32-bit host that reflects interrupts to real
        // mode (not virtual-8086 mode), without virtual memory; the
        // processor and the PICs' vector bases.
        0x0400 => {
            ctx.set_reg16(EAX, 0x005A);
            ctx.set_reg16(EBX, 0x0003);
            let processor = match cpu.model {
                crate::cpu::CpuModel::I386 => 3,
                crate::cpu::CpuModel::I486 => 4,
                crate::cpu::CpuModel::Pentium => 5,
            };
            ctx.set_reg16(ECX, (ctx.reg16(ECX) & 0xFF00) | processor);
            ctx.set_reg16(EDX, (cpu.bus.pic.vector(0) as u16) << 8 | cpu.bus.pic.vector(8) as u16);
        }
        // Free memory, into the 30h bytes at ES:(E)DI.
        0x0500 => {
            let (largest, total) = cpu.bus.xms.dpmi_free(end);
            let pages = end / 0x1000;
            let info = [
                largest,
                largest / 0x1000,
                largest / 0x1000,
                pages,
                total / 0x1000,
                total / 0x1000,
                pages,
                total / 0x1000,
                u32::MAX,
                u32::MAX,
                u32::MAX,
                u32::MAX,
            ];
            let bytes: Vec<u8> = info.iter().flat_map(|v| v.to_le_bytes()).collect();
            write_buffer(cpu, offset(ctx, EDI, bits32), &bytes).map_err(|_| INVALID_VALUE)?;
        }
        // Allocate a memory block of BX:CX bytes: its address in BX:CX,
        // its handle in SI:DI.
        0x0501 => {
            let size = pair(ctx, EBX, ECX);
            if size == 0 {
                return Err(INVALID_VALUE);
            }
            let base = cpu.bus.xms.take_dpmi(size, end).ok_or(PHYSICAL_MEMORY_UNAVAILABLE)?;
            let len = size.div_ceil(0x1000) * 0x1000;
            cpu.bus.fill_ram(base as usize..(base + len) as usize, 0);
            let handle = cpu.bus.dpmi.next_handle;
            cpu.bus.dpmi.next_handle = handle.wrapping_add(1).max(1);
            client(cpu).memory.push((handle, base, len));
            set_pair(ctx, EBX, ECX, base);
            set_pair(ctx, ESI, EDI, handle);
        }
        // Free the memory block SI:DI.
        0x0502 => {
            let handle = pair(ctx, ESI, EDI);
            let c = client(cpu);
            let i = c.memory.iter().position(|&(h, _, _)| h == handle).ok_or(INVALID_HANDLE)?;
            let (_, base, _) = c.memory.remove(i);
            cpu.bus.xms.release_dpmi(base);
        }
        // Resize the memory block SI:DI to BX:CX bytes, where it is or
        // elsewhere: its address in BX:CX, its handle in SI:DI.
        0x0503 => {
            let (size, handle) = (pair(ctx, EBX, ECX), pair(ctx, ESI, EDI));
            if size == 0 {
                return Err(INVALID_VALUE);
            }
            let i = client(cpu).memory.iter().position(|&(h, _, _)| h == handle).ok_or(INVALID_HANDLE)?;
            let (_, base, old) = client(cpu).memory[i];
            let len = size.div_ceil(0x1000) * 0x1000;
            let base = if cpu.bus.xms.resize_dpmi(base, len, end) {
                if len > old {
                    cpu.bus.fill_ram((base + old) as usize..(base + len) as usize, 0);
                }
                base
            } else {
                let new = cpu.bus.xms.take_dpmi(len, end).ok_or(PHYSICAL_MEMORY_UNAVAILABLE)?;
                cpu.bus.fill_ram(new as usize..(new + len) as usize, 0);
                cpu.bus.copy_ram(base as usize, new as usize, old.min(len) as usize);
                cpu.bus.xms.release_dpmi(base);
                new
            };
            client(cpu).memory[i] = (handle, base, len);
            set_pair(ctx, EBX, ECX, base);
        }
        // Locking and unlocking memory, and demand paging: the memory is
        // always there.
        0x0600..=0x0603 | 0x0702 | 0x0703 => {}
        // The page size.
        0x0604 => set_pair(ctx, EBX, ECX, 0x1000),
        // Map the physical address BX:CX (SI:DI bytes): without paging,
        // it is its own linear address.
        0x0800 => {
            if pair(ctx, ESI, EDI) == 0 {
                return Err(INVALID_VALUE);
            }
        }
        0x0801 => {}
        // The virtual interrupt flag: get it in AL, and clear or set it.
        0x0900..=0x0902 => {
            set_al(ctx, (ctx.eflags & CpuFlags::IF.bits() != 0) as u8);
            match function {
                0x0900 => ctx.eflags &= !CpuFlags::IF.bits(),
                0x0901 => ctx.eflags |= CpuFlags::IF.bits(),
                _ => {}
            }
        }
        // Debug watchpoints: none to set, so none to clear.
        0x0B00 => return Err(HANDLE_UNAVAILABLE),
        0x0B01..=0x0B03 => return Err(INVALID_HANDLE),
        _ => return Err(UNSUPPORTED),
    }
    Ok(())
}

/// Write `bytes` at ES:`at`, through the client's ES.
fn write_buffer(cpu: &mut Cpu, at: u32, bytes: &[u8]) -> CpuResult {
    for (i, &byte) in bytes.iter().enumerate() {
        cpu.write_u8(Seg::ES, at.wrapping_add(i as u32), byte)?;
    }
    Ok(())
}

/// Allocate BX paragraphs of DOS memory: its segment in AX and a selector
/// for it in DX, several in a row for more than 64 KB. BX says how much
/// there is when there isn't enough.
fn dos_allocate(cpu: &mut Cpu, ctx: &mut Context) -> Result {
    let paras = ctx.reg16(EBX);
    let owner = cpu.current_psp;
    let strategy = cpu.alloc_strategy;
    let segment = match crate::mcb::alloc_strategy(&mut cpu.bus, owner, paras, strategy) {
        Ok(segment) => segment,
        Err(largest) => {
            ctx.set_reg16(EBX, largest);
            return Err(DOS_NO_MEMORY);
        }
    };
    let count = dos_selectors(paras);
    let Some(first) = allocate(cpu, count, 0, LDT_FIXED) else {
        let _ = crate::mcb::free(&mut cpu.bus, segment);
        return Err(DESCRIPTOR_UNAVAILABLE);
    };
    set_dos_descriptors(cpu, first, segment, paras);
    client(cpu).dos_blocks.push((segment, Client::selector(first), count as u16));
    ctx.set_reg16(EAX, segment);
    ctx.set_reg16(EDX, Client::selector(first));
    Ok(())
}

/// The selectors a DOS block of `paras` paragraphs has: one per 64 KB.
fn dos_selectors(paras: u16) -> usize {
    (paras as usize * 16).div_ceil(0x10000).max(1)
}

/// The descriptors of a DOS block at `segment`, from LDT index `first`:
/// each 64 KB of it, or what is left.
fn set_dos_descriptors(cpu: &mut Cpu, first: usize, segment: u16, paras: u16) {
    let ldt = client(cpu).block;
    let bytes = paras as u32 * 16;
    for i in 0..dos_selectors(paras) {
        let offset = i as u32 * 0x10000;
        let limit = (bytes - offset).min(0x10000).max(1) - 1;
        write_desc(&mut cpu.bus, ldt, first + i, descriptor(((segment as u32) << 4) + offset, limit, DATA3, 0));
    }
}

/// The DOS block with the selector DX: its place in `dos_blocks`.
fn dos_block(cpu: &mut Cpu, ctx: &Context) -> std::result::Result<usize, u16> {
    let selector = ctx.reg16(EDX) | 3;
    client(cpu).dos_blocks.iter().position(|&(_, sel, _)| sel == selector).ok_or(INVALID_SELECTOR)
}

/// Free the DOS block with the selector DX, and its selectors.
fn dos_free(cpu: &mut Cpu, ctx: &mut Context) -> Result {
    let i = dos_block(cpu, ctx)?;
    let (segment, selector, count) = client(cpu).dos_blocks[i];
    crate::mcb::free(&mut cpu.bus, segment).map_err(|e| e as u16)?;
    client(cpu).dos_blocks.remove(i);
    let first = (selector >> 3) as usize;
    for index in first..first + count as usize {
        free_entry(cpu, index);
    }
    Ok(())
}

/// Resize the DOS block with the selector DX to BX paragraphs, with the
/// selectors it then needs (after its first, which don't have to be free
/// when it shrinks).
fn dos_resize(cpu: &mut Cpu, ctx: &mut Context) -> Result {
    let i = dos_block(cpu, ctx)?;
    let paras = ctx.reg16(EBX);
    let (segment, selector, count) = client(cpu).dos_blocks[i];
    let (first, count) = ((selector >> 3) as usize, count as usize);
    let needed = dos_selectors(paras);
    let c = client(cpu);
    if needed > count && (first + count..first + needed).any(|index| c.ldt.get(index) != Some(&LDT_FREE)) {
        return Err(DESCRIPTOR_UNAVAILABLE);
    }
    if let Err(largest) = crate::mcb::resize(&mut cpu.bus, segment, paras) {
        ctx.set_reg16(EBX, largest);
        return Err(DOS_NO_MEMORY);
    }
    let c = client(cpu);
    for index in first + count..first + needed {
        c.ldt[index] = LDT_FIXED;
    }
    c.dos_blocks[i].2 = needed as u16;
    for index in first + needed..first + count {
        free_entry(cpu, index);
    }
    set_dos_descriptors(cpu, first, segment, paras);
    Ok(())
}

/// The call structure of INT 31h AX=0300h-0302h and the callbacks, at
/// `seg`:`at` through the processor's segment register: the real-mode
/// registers it holds.
pub(super) fn read_call_structure(cpu: &mut Cpu, seg: Seg, at: u32) -> CpuResult<Context> {
    let mut bytes = [0u8; 0x32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = cpu.read_u8(seg, at.wrapping_add(i as u32))?;
    }
    let dword = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let word = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let mut ctx = Context::default();
    // EDI, ESI, EBP, (ESP), EBX, EDX, ECX, EAX, in that order.
    for (i, reg) in [7, 6, 5, 4, 3, 2, 1, 0].into_iter().enumerate() {
        ctx.gpr[reg] = dword(4 * i);
    }
    ctx.gpr[ESP] = word(0x2E) as u32;
    ctx.eflags = word(0x20) as u32;
    ctx.eip = word(0x2A) as u32;
    ctx.seg = [word(0x22), word(0x2C), word(0x30), word(0x24), word(0x26), word(0x28)];
    Ok(ctx)
}

/// Write the real-mode registers `rm` into the call structure at
/// `selector`:`at`: the general registers, flags and data segments, and
/// with `all` CS:IP and SS:SP.
pub(super) fn write_call_structure(cpu: &mut Cpu, selector: u16, at: u32, rm: &Context, all: bool) {
    let Some(base) = selector_base(cpu, selector) else { return };
    let at = base.wrapping_add(at) as usize;
    for (i, reg) in [7, 6, 5, 4, 3, 2, 1, 0].into_iter().enumerate() {
        if reg != ESP {
            cpu.bus.write_32(at + 4 * i, rm.gpr[reg]);
        }
    }
    cpu.bus.write_16(at + 0x20, rm.eflags as u16);
    for (offset, seg) in [(0x22, Seg::ES), (0x24, Seg::DS), (0x26, Seg::FS), (0x28, Seg::GS)] {
        cpu.bus.write_16(at + offset, rm.sel(seg));
    }
    if all {
        cpu.bus.write_16(at + 0x2A, rm.eip as u16);
        cpu.bus.write_16(at + 0x2C, rm.sel(Seg::CS));
        cpu.bus.write_16(at + 0x2E, rm.gpr[ESP] as u16);
        cpu.bus.write_16(at + 0x30, rm.sel(Seg::SS));
    }
}

/// After a real-mode call: its registers back into the call structure.
pub(super) fn store_call_structure(cpu: &mut Cpu, selector: u16, at: u32, rm: &Context) {
    write_call_structure(cpu, selector, at, rm, false);
}

/// INT 31h AX=0300h (the real-mode interrupt BL), 0301h (a far call) and
/// 0302h (a call with an IRET frame): the registers from the call structure
/// at ES:(E)DI, CX words from the client's stack on the real-mode stack
/// (the structure's, or the host's when that is 0:0), and back to the
/// client with the registers in the structure once the code returns.
fn call_real_mode(cpu: &mut Cpu, mut ctx: Context, function: u16) {
    let bits32 = client(cpu).bits32;
    let at = offset(&ctx, EDI, bits32);
    let selector = ctx.sel(Seg::ES);
    let mut rm = match read_call_structure(cpu, Seg::ES, at) {
        Ok(rm) => rm,
        Err(_) => {
            ctx.set_reg16(EAX, INVALID_VALUE);
            ctx.set_cf(true);
            return resume(cpu, &ctx);
        }
    };
    let target = match function {
        0x0300 => real_mode_handler(cpu, ctx.gpr[EBX] as u8),
        _ => (rm.sel(Seg::CS) as u32) << 16 | rm.eip & 0xFFFF,
    };
    if target == 0 {
        // No handler: nothing happens, as for a real-mode INT.
        ctx.set_cf(false);
        return resume(cpu, &ctx);
    }
    let c = client(cpu);
    let (ss, mut sp) = match (rm.sel(Seg::SS), rm.gpr[ESP] & 0xFFFF) {
        (0, 0) => (c.rm_stack, c.rm_sp),
        (ss, sp) => (ss, sp),
    };
    let frame = Frame {
        kind: F_TRANSLATE,
        client: c.id,
        ctx,
        sel: selector,
        off: at,
        rm_sp: c.rm_sp,
        lpms: c.lpms_esp,
        ..Frame::default()
    };
    keep_lpms(c, &ctx);
    // The parameters, from the client's stack.
    let words = ctx.reg16(ECX) as u32;
    if words > 0 {
        let stack = client_stack(cpu, &ctx);
        let params: Vec<u16> = (0..words).map(|i| cpu.bus.read_16(stack.wrapping_add(2 * i) as usize)).collect();
        sp = push_real(cpu, ss, sp, &params.iter().rev().copied().collect::<Vec<u16>>());
    }
    let flags = rm.eflags & 0xFFFF;
    let pushed: &[u16] = if function == 0x0301 { &[ROM_SEG, RM_RETURN] } else { &[flags as u16, ROM_SEG, RM_RETURN] };
    sp = push_real(cpu, ss, sp, pushed);
    cpu.bus.dpmi.frames.push(frame);
    rm.gpr[ESP] = sp;
    rm.set_sel(Seg::SS, ss);
    rm.set_sel(Seg::CS, (target >> 16) as u16);
    rm.eip = target & 0xFFFF;
    let cleared = if function == 0x0301 { CpuFlags::TF } else { CpuFlags::IF | CpuFlags::TF };
    rm.eflags = flags & !cleared.bits();
    enter_rm(cpu, &rm);
}

/// The linear address of the top of the client's stack in `ctx`.
fn client_stack(cpu: &Cpu, ctx: &Context) -> u32 {
    let ss = ctx.sel(Seg::SS);
    let big = match super::descriptor_of(cpu, ss) {
        Some(desc) => desc & (1 << 54) != 0,
        None => false,
    };
    let base = selector_base(cpu, ss).unwrap_or(0);
    base.wrapping_add(if big { ctx.gpr[ESP] } else { ctx.gpr[ESP] & 0xFFFF })
}
