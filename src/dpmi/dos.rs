//! INT 21h in protected mode: the DOS API translation Windows and HDPMI
//! give their clients, which INT 2Fh AX=168Ah's "MS-DOS" extensions stand
//! for. Borland's RTM relies on it, and extenders that do their own
//! (DOS/4GW, PMODE/W) don't pass these functions down to the host.
//!
//! A client calls DOS with selectors in its segment registers. The host
//! copies what DOS is to read into the transfer buffer at the start of the
//! client's private data (below its real-mode stack), calls DOS in real
//! mode on it (`call_real`), and copies what DOS wrote back afterwards
//! (`returned`): reads and writes larger than the buffer in pieces, or
//! directly when the client's buffer is in conventional memory. Segments
//! DOS returns become selectors (INT 31h AX=0002h's). Interrupt vectors,
//! the DTA and memory blocks are the host's: protected-mode vectors, a DTA
//! the client addresses by selector, and extended memory with selectors
//! for it. Functions without pointers go to DOS as they are, as do those
//! the host doesn't translate.

use super::int31::{
    can_retile, dos_block, dos_free, dos_resize, offset, resize_memory, retile, segment_selector, set_offset, set_tiles,
    tiles,
};
use super::{
    BIG, Client, Context, DATA3, EAX, EBX, ECX, EDI, EDX, ESI, ESP, F_DOS, Frame, LDT_FIXED, RESULT_FLAGS, allocate,
    call_real, client, descriptor, free_entry, real_mode_handler, reflect, resume, selector_base, set_vector, write_desc,
};
use crate::bus::Bus;
use crate::cpu::{Cpu, Seg};

/// The transfer buffer: the first 8 KB of the client's private data.
const TLB_SIZE: u32 = 0x2000;
/// Where in it a second name or buffer goes.
const SECOND: u32 = 0x100;
/// The longest name copied.
const NAME_MAX: u32 = 0x100;
/// EXEC's parameter block for DOS, and the command line and FCBs it points
/// to.
const EXEC_BLOCK: u32 = 0x200;
const EXEC_CMDLINE: u32 = 0x220;
const EXEC_FCB1: u32 = 0x2A0;
const EXEC_FCB2: u32 = 0x2C0;
const FCB_SIZE: u32 = 0x25;
/// DOS's DTA during a search for a client with a DTA of its own, and the
/// part of a DTA a search uses.
const TLB_DTA: u32 = TLB_SIZE - 0x80;
const FIND_SIZE: u32 = 0x2B;

/// `Frame::sel` of a DOS call: a read or write DOS does on the client's
/// buffer itself, and a search in the transfer buffer's DTA.
pub(super) const DIRECT: u16 = 1;
pub(super) const SWAPPED_DTA: u16 = 2;

/// DOS's "invalid memory block address" and "insufficient memory".
const BAD_BLOCK: u16 = 0x0009;
const NO_MEMORY: u16 = 0x0008;

/// INT 21h from the running client, in its context `ctx`.
pub(super) fn int21(cpu: &mut Cpu, mut ctx: Context) {
    let bits32 = client(cpu).bits32;
    let [ah, al] = function(&ctx);
    match ah {
        // Set the DTA to DS:(E)DX.
        0x1A => client(cpu).dta = Some((ctx.sel(Seg::DS), offset(&ctx, EDX, bits32))),
        // The DTA in ES:(E)BX.
        0x2F => {
            let (sel, off) = dta(cpu);
            ctx.set_sel(Seg::ES, sel);
            set_offset(&mut ctx, EBX, bits32, off);
        }
        // Set the protected-mode vector AL to DS:(E)DX, and get it in
        // ES:(E)BX.
        0x25 => set_vector(cpu, al, (ctx.sel(Seg::DS), offset(&ctx, EDX, bits32))),
        0x35 => {
            let (sel, off) = client(cpu).vectors[al as usize];
            ctx.set_sel(Seg::ES, sel);
            set_offset(&mut ctx, EBX, bits32, off);
        }
        0x48..=0x4A => {
            match memory(cpu, &mut ctx, ah) {
                Ok(()) => ctx.set_cf(false),
                Err(code) => {
                    ctx.set_reg16(EAX, code);
                    ctx.set_cf(true);
                }
            }
        }
        _ => {
            let mut frame = Frame { kind: F_DOS, ctx, vector: 0x21, ..Frame::default() };
            match before(cpu, &mut frame) {
                Some(rm) => call_real(cpu, frame, real_mode_handler(cpu, 0x21), rm),
                None => reflect(cpu, 0x21, ctx),
            }
            return;
        }
    }
    resume(cpu, &ctx);
}

/// AH and AL.
fn function(ctx: &Context) -> [u8; 2] {
    [(ctx.gpr[EAX] >> 8) as u8, ctx.gpr[EAX] as u8]
}

/// The linear address of `seg`:(E)`reg` in the client's context `ctx`, if
/// the selector is one.
fn pointer(cpu: &Cpu, ctx: &Context, seg: Seg, reg: usize, bits32: bool) -> Option<u32> {
    Some(selector_base(cpu, ctx.sel(seg))?.wrapping_add(offset(ctx, reg, bits32)))
}

/// The client's DTA: the one it set, or else DOS's.
fn dta(cpu: &mut Cpu) -> (u16, u32) {
    if let Some(dta) = client(cpu).dta {
        return dta;
    }
    let (segment, off) = (cpu.bus.dta_segment, cpu.bus.dta_offset);
    (segment_selector(cpu, segment).unwrap_or(0), off as u32)
}

fn copy(bus: &mut Bus, from: u32, to: u32, len: u32) {
    for i in 0..len {
        let byte = bus.read_8(from.wrapping_add(i) as usize);
        bus.write_8(to.wrapping_add(i) as usize, byte);
    }
}

/// Copy the bytes at `from` up to and with `end`, at most `max` of them
/// (the last then `end`).
fn copy_until(bus: &mut Bus, from: u32, to: u32, max: u32, end: u8) {
    for i in 0..max {
        let byte = if i == max - 1 { end } else { bus.read_8(from.wrapping_add(i) as usize) };
        bus.write_8(to.wrapping_add(i) as usize, byte);
        if byte == end {
            return;
        }
    }
}

/// Where real-mode code finds the `len` bytes at the linear address
/// `linear`, as segment and offset, when they are in conventional memory.
fn conventional(linear: u32, len: u32) -> Option<(u16, u32)> {
    let end = linear.checked_add(len)?;
    (end <= 0x10_0000 && (linear & 0xF) + len <= 0x1_0000).then_some(((linear >> 4) as u16, linear & 0xF))
}

/// Get the call in `frame.ctx` ready for DOS: what DOS reads copied into the
/// transfer buffer, and the real-mode registers that point there. None
/// when the host doesn't translate it.
fn before(cpu: &mut Cpu, frame: &mut Frame) -> Option<Context> {
    let ctx = frame.ctx;
    let c = client(cpu);
    let (bits32, seg) = (c.bits32, c.rm_stack);
    let tlb = (seg as u32) << 4;
    let [ah, al] = function(&ctx);
    let ptr = |cpu: &Cpu, s: Seg, reg: usize| pointer(cpu, &ctx, s, reg, bits32);
    let mut rm = ctx;
    rm.seg = [seg, 0, seg, seg, 0, 0];
    match ah {
        // Write the string at DS:(E)DX, up to its '$'.
        0x09 => {
            let from = ptr(cpu, Seg::DS, EDX)?;
            copy_until(&mut cpu.bus, from, tlb, TLB_SIZE, b'$');
            rm.gpr[EDX] = 0;
        }
        // Buffered input into DS:(E)DX, alone or after flushing the
        // keyboard buffer.
        0x0A | 0x0C if ah == 0x0A || al == 0x0A => {
            let from = ptr(cpu, Seg::DS, EDX)?;
            let len = cpu.bus.read_8(from as usize) as u32 + 2;
            copy(&mut cpu.bus, from, tlb, len);
            rm.gpr[EDX] = 0;
        }
        // Functions that return segments, which become selectors.
        0x1B | 0x1C | 0x1F | 0x32 | 0x34 | 0x51 | 0x52 | 0x62 => {}
        0x5D if al == 0x06 || al == 0x0B => {}
        0x63 if al == 0x00 => {}
        // Parse the file name at DS:(E)SI into the FCB at ES:(E)DI.
        0x29 => {
            let (from, fcb) = (ptr(cpu, Seg::DS, ESI)?, ptr(cpu, Seg::ES, EDI)?);
            copy(&mut cpu.bus, from, tlb, 0x80);
            copy(&mut cpu.bus, fcb, tlb + SECOND, FCB_SIZE);
            rm.gpr[ESI] = 0;
            rm.gpr[EDI] = SECOND;
        }
        // The country's information into DS:(E)DX (DX=FFFFh sets it).
        0x38 if ctx.reg16(EDX) != 0xFFFF => {
            ptr(cpu, Seg::DS, EDX)?;
            rm.gpr[EDX] = 0;
        }
        // Functions with the name of a file or directory at DS:(E)DX.
        0x39..=0x3D | 0x41 | 0x43 | 0x4E | 0x5A | 0x5B => {
            let from = ptr(cpu, Seg::DS, EDX)?;
            copy_until(&mut cpu.bus, from, tlb, NAME_MAX, 0);
            rm.gpr[EDX] = 0;
            if ah == 0x4E {
                swap_dta(cpu, frame, false);
            }
        }
        0x4F => swap_dta(cpu, frame, true),
        // Extended open, of the file named at DS:(E)SI.
        0x6C => {
            let from = ptr(cpu, Seg::DS, ESI)?;
            copy_until(&mut cpu.bus, from, tlb, NAME_MAX, 0);
            rm.gpr[ESI] = 0;
        }
        0x3F | 0x40 => {
            let buffer = ptr(cpu, Seg::DS, EDX)?;
            let (count, done) = (offset(&ctx, ECX, bits32), frame.off);
            if done == 0
                && count <= 0xFFFF
                && let Some((segment, off)) = conventional(buffer, count)
            {
                frame.sel = DIRECT;
                rm.set_sel(Seg::DS, segment);
                rm.gpr[EDX] = off;
                return Some(rm);
            }
            let chunk = (count - done).min(TLB_SIZE);
            if ah == 0x40 {
                copy(&mut cpu.bus, buffer.wrapping_add(done), tlb, chunk);
            }
            rm.gpr[EDX] = 0;
            rm.gpr[ECX] = chunk;
        }
        // IOCTL: read (02h, 04h) and write (03h, 05h) CX bytes of control
        // data at DS:(E)DX.
        0x44 if (0x02..=0x05).contains(&al) => {
            let buffer = ptr(cpu, Seg::DS, EDX)?;
            let count = ctx.reg16(ECX) as u32;
            if count > TLB_SIZE {
                return None;
            }
            if al & 1 != 0 {
                copy(&mut cpu.bus, buffer, tlb, count);
            }
            rm.gpr[EDX] = 0;
        }
        // The current directory into the 64 bytes at DS:(E)SI.
        0x47 => {
            ptr(cpu, Seg::DS, ESI)?;
            rm.gpr[ESI] = 0;
        }
        0x4B if al == 0x00 || al == 0x03 => exec(cpu, &ctx, &mut rm, bits32, seg)?,
        // Set the PSP: a selector for it becomes its segment.
        0x50 => {
            let base = selector_base(cpu, ctx.reg16(EBX))?;
            if base >= 0x10_0000 || base & 0xF != 0 {
                return None;
            }
            rm.set_reg16(EBX, (base >> 4) as u16);
        }
        // Rename the file named at DS:(E)DX to the name at ES:(E)DI.
        0x56 => {
            let (from, to) = (ptr(cpu, Seg::DS, EDX)?, ptr(cpu, Seg::ES, EDI)?);
            copy_until(&mut cpu.bus, from, tlb, NAME_MAX, 0);
            copy_until(&mut cpu.bus, to, tlb + SECOND, NAME_MAX, 0);
            rm.gpr[EDX] = 0;
            rm.gpr[EDI] = SECOND;
        }
        // The canonical name of the name at DS:(E)SI into the 128 bytes at
        // ES:(E)DI.
        0x60 => {
            let from = ptr(cpu, Seg::DS, ESI)?;
            ptr(cpu, Seg::ES, EDI)?;
            copy_until(&mut cpu.bus, from, tlb, NAME_MAX, 0);
            rm.gpr[ESI] = 0;
            rm.gpr[EDI] = SECOND;
        }
        // Extended country information into CX bytes at ES:(E)DI.
        0x65 if (0x01..=0x07).contains(&al) && ctx.reg16(ECX) as u32 <= TLB_SIZE => {
            ptr(cpu, Seg::ES, EDI)?;
            rm.gpr[EDI] = 0;
        }
        _ => return None,
    }
    Some(rm)
}

/// A search (AH=4Eh, and 4Fh when `next`) for a client with a DTA of its
/// own: DOS searches with a copy of it in the transfer buffer.
fn swap_dta(cpu: &mut Cpu, frame: &mut Frame, next: bool) {
    let Some((sel, off)) = client(cpu).dta else { return };
    let Some(base) = selector_base(cpu, sel) else { return };
    let seg = client(cpu).rm_stack;
    if next {
        copy(&mut cpu.bus, base.wrapping_add(off), ((seg as u32) << 4) + TLB_DTA, FIND_SIZE);
    }
    frame.sel = SWAPPED_DTA;
    frame.base = (cpu.bus.dta_segment as u32) << 16 | cpu.bus.dta_offset as u32;
    cpu.bus.dta_segment = seg;
    cpu.bus.dta_offset = TLB_DTA as u16;
}

/// EXEC (AL=00h, and 03h for an overlay): the program named at DS:(E)DX,
/// the parameter block at ES:(E)BX. A 16-bit client's has its environment's
/// selector and far pointers to the command line and FCBs, as DOS's; a
/// 32-bit client's (Windows 95's and HDPMI's) has 48-bit pointers in eight
/// bytes each, and no environment: the program gets its parent's.
fn exec(cpu: &mut Cpu, ctx: &Context, rm: &mut Context, bits32: bool, seg: u16) -> Option<()> {
    let tlb = (seg as u32) << 4;
    let name = pointer(cpu, ctx, Seg::DS, EDX, bits32)?;
    let block = pointer(cpu, ctx, Seg::ES, EBX, bits32)?;
    copy_until(&mut cpu.bus, name, tlb, NAME_MAX, 0);
    rm.gpr[EDX] = 0;
    rm.gpr[EBX] = EXEC_BLOCK;
    if ctx.gpr[EAX] as u8 == 0x03 {
        copy(&mut cpu.bus, block, tlb + EXEC_BLOCK, 4);
        return Some(());
    }
    let read16 = |cpu: &Cpu, at: u32| cpu.bus.read_16(block.wrapping_add(at) as usize);
    let far = |cpu: &Cpu, at: u32| -> Option<u32> {
        let (off, sel) = if bits32 {
            (cpu.bus.read_32(block.wrapping_add(at) as usize), read16(cpu, at + 4))
        } else {
            (read16(cpu, at) as u32, read16(cpu, at + 2))
        };
        Some(selector_base(cpu, sel)?.wrapping_add(off))
    };
    let (env, cmdline, fcbs) = if bits32 {
        (0, far(cpu, 0), [far(cpu, 8), far(cpu, 16)])
    } else {
        let env = read16(cpu, 0);
        let env = match selector_base(cpu, env) {
            Some(base) if base < 0x10_0000 && base & 0xF == 0 => (base >> 4) as u16,
            _ => 0,
        };
        (env, far(cpu, 2), [far(cpu, 6), far(cpu, 10)])
    };
    match cmdline {
        Some(from) => {
            let len = (cpu.bus.read_8(from as usize) as u32).min(0x7E);
            copy(&mut cpu.bus, from, tlb + EXEC_CMDLINE, len + 2);
        }
        None => {
            cpu.bus.write_8((tlb + EXEC_CMDLINE) as usize, 0);
            cpu.bus.write_8((tlb + EXEC_CMDLINE + 1) as usize, 0x0D);
        }
    }
    for (fcb, at) in fcbs.into_iter().zip([EXEC_FCB1, EXEC_FCB2]) {
        match fcb {
            Some(from) => copy(&mut cpu.bus, from, tlb + at, 0x14),
            None => {
                cpu.bus.write_8((tlb + at) as usize, 0);
                for i in 1..12 {
                    cpu.bus.write_8((tlb + at + i) as usize, b' ');
                }
            }
        }
    }
    let words = [env, EXEC_CMDLINE as u16, seg, EXEC_FCB1 as u16, seg, EXEC_FCB2 as u16, seg];
    for (i, word) in words.into_iter().enumerate() {
        cpu.bus.write_16((tlb + EXEC_BLOCK) as usize + 2 * i, word);
    }
    Some(())
}

/// DOS returned from the call of `frame` with the registers `rm`: what it
/// wrote copied to the client, and the client goes on with DOS's registers
/// and flags, but for those that pointed to the transfer buffer; or the
/// next piece of a read or write.
pub(super) fn returned(cpu: &mut Cpu, frame: Frame, rm: &Context) {
    let caller = frame.ctx;
    let bits32 = client(cpu).bits32;
    let tlb = (client(cpu).rm_stack as u32) << 4;
    let [ah, al] = function(&caller);
    let ptr = |cpu: &Cpu, s: Seg, reg: usize| pointer(cpu, &caller, s, reg, bits32);
    let failed = rm.eflags & 1 != 0;
    let mut ctx = caller;
    ctx.gpr = rm.gpr;
    ctx.gpr[ESP] = caller.gpr[ESP];
    ctx.eflags = (caller.eflags & !RESULT_FLAGS) | (rm.eflags & RESULT_FLAGS);
    let restore = |ctx: &mut Context, regs: &[usize]| {
        for &reg in regs {
            ctx.gpr[reg] = caller.gpr[reg];
        }
    };
    let selector = |cpu: &mut Cpu, segment: u16| segment_selector(cpu, segment).unwrap_or(0);
    match ah {
        0x09 | 0x39..=0x3D | 0x41 | 0x43 | 0x5B => restore(&mut ctx, &[EDX]),
        0x0A | 0x0C => {
            restore(&mut ctx, &[EDX]);
            if let Some(to) = ptr(cpu, Seg::DS, EDX) {
                let len = cpu.bus.read_8(tlb as usize) as u32 + 2;
                copy(&mut cpu.bus, tlb, to, len);
            }
        }
        0x1B | 0x1C | 0x1F | 0x32 | 0x5D | 0x63 => {
            if ah == 0x5D || ah == 0x63 || al != 0xFF {
                let sel = selector(cpu, rm.sel(Seg::DS));
                ctx.set_sel(Seg::DS, sel);
            }
        }
        0x34 | 0x52 => {
            let sel = selector(cpu, rm.sel(Seg::ES));
            ctx.set_sel(Seg::ES, sel);
        }
        0x51 | 0x62 => {
            let sel = selector(cpu, rm.reg16(EBX));
            ctx.set_reg16(EBX, sel);
        }
        0x29 => {
            restore(&mut ctx, &[ESI, EDI]);
            set_offset(&mut ctx, ESI, bits32, offset(&caller, ESI, bits32).wrapping_add(rm.reg16(ESI) as u32));
            if let Some(to) = ptr(cpu, Seg::ES, EDI) {
                copy(&mut cpu.bus, tlb + SECOND, to, FCB_SIZE);
            }
        }
        0x38 => {
            restore(&mut ctx, &[EDX]);
            if let Some(to) = ptr(cpu, Seg::DS, EDX).filter(|_| !failed) {
                copy(&mut cpu.bus, tlb, to, 0x22);
            }
        }
        0x3F | 0x40 => return transferred(cpu, frame, rm, ctx),
        0x44 => {
            restore(&mut ctx, &[EDX]);
            if let Some(to) = ptr(cpu, Seg::DS, EDX).filter(|_| !failed && al & 1 == 0) {
                copy(&mut cpu.bus, tlb, to, (rm.reg16(EAX) as u32).min(TLB_SIZE));
            }
        }
        0x47 => {
            restore(&mut ctx, &[ESI]);
            if let Some(to) = ptr(cpu, Seg::DS, ESI).filter(|_| !failed) {
                copy(&mut cpu.bus, tlb, to, 0x40);
            }
        }
        0x4B => restore(&mut ctx, &[EDX, EBX]),
        0x4E | 0x4F => {
            restore(&mut ctx, &[EDX]);
            if frame.sel == SWAPPED_DTA {
                cpu.bus.dta_segment = (frame.base >> 16) as u16;
                cpu.bus.dta_offset = frame.base as u16;
                let dta = client(cpu).dta.and_then(|(sel, off)| Some(selector_base(cpu, sel)?.wrapping_add(off)));
                if let Some(to) = dta {
                    copy(&mut cpu.bus, tlb + TLB_DTA, to, FIND_SIZE);
                }
            }
        }
        0x50 => restore(&mut ctx, &[EBX]),
        0x56 => restore(&mut ctx, &[EDX, EDI]),
        0x5A => {
            restore(&mut ctx, &[EDX]);
            if let Some(to) = ptr(cpu, Seg::DS, EDX).filter(|_| !failed) {
                copy_until(&mut cpu.bus, tlb, to, NAME_MAX, 0);
            }
        }
        0x60 => {
            restore(&mut ctx, &[ESI, EDI]);
            if let Some(to) = ptr(cpu, Seg::ES, EDI).filter(|_| !failed) {
                copy(&mut cpu.bus, tlb + SECOND, to, 0x80);
            }
        }
        0x65 => {
            restore(&mut ctx, &[EDI]);
            if let Some(to) = ptr(cpu, Seg::ES, EDI).filter(|_| !failed) {
                if al == 0x01 {
                    copy(&mut cpu.bus, tlb, to, (rm.reg16(ECX) as u32).min(TLB_SIZE));
                } else {
                    // An ID and a far pointer to a table of DOS's.
                    copy(&mut cpu.bus, tlb, to, 3);
                    let segment = cpu.bus.read_16(tlb as usize + 3);
                    let sel = selector(cpu, segment);
                    cpu.bus.write_16(to.wrapping_add(3) as usize, sel);
                }
            }
        }
        0x6C => restore(&mut ctx, &[ESI]),
        _ => {}
    }
    resume(cpu, &ctx);
}

/// A piece of a read (AH=3Fh) or write (40h) of (E)CX bytes at DS:(E)DX
/// is done: the next, or the client goes on with how many there were in
/// (E)AX. An error ends it.
fn transferred(cpu: &mut Cpu, mut frame: Frame, rm: &Context, mut ctx: Context) {
    let caller = frame.ctx;
    let bits32 = client(cpu).bits32;
    let tlb = (client(cpu).rm_stack as u32) << 4;
    ctx.gpr[EDX] = caller.gpr[EDX];
    ctx.gpr[ECX] = caller.gpr[ECX];
    if rm.eflags & 1 != 0 {
        return resume(cpu, &ctx);
    }
    let moved = rm.reg16(EAX) as u32;
    let mut done = moved;
    if frame.sel != DIRECT {
        let (count, earlier) = (offset(&caller, ECX, bits32), frame.off);
        let chunk = (count - earlier).min(TLB_SIZE);
        let buffer = pointer(cpu, &caller, Seg::DS, EDX, bits32);
        if caller.gpr[EAX] >> 8 & 0xFF == 0x3F
            && let Some(buffer) = buffer
        {
            copy(&mut cpu.bus, tlb, buffer.wrapping_add(earlier), moved.min(chunk));
        }
        done = earlier + moved.min(chunk);
        if moved == chunk && done < count {
            frame.off = done;
            if let Some(rm) = before(cpu, &mut frame) {
                return call_real(cpu, frame, real_mode_handler(cpu, 0x21), rm);
            }
        }
    }
    if bits32 {
        ctx.gpr[EAX] = done;
    } else {
        ctx.set_reg16(EAX, done as u16);
    }
    resume(cpu, &ctx);
}

/// AH=48h-4Ah: memory blocks for the client. They are extended memory with
/// selectors for all of it: one for a 32-bit client, one for each 64 KB
/// for a 16-bit one, as HDPMI has them. A DOS block of INT 31h AX=0100h
/// can be freed and resized too. A failure has the paragraphs there are
/// in (E)BX.
fn memory(cpu: &mut Cpu, ctx: &mut Context, ah: u8) -> Result<(), u16> {
    let bits32 = client(cpu).bits32;
    let paras = offset(ctx, EBX, bits32);
    let selector = ctx.sel(Seg::ES);
    let result = match ah {
        0x48 => allocate_block(cpu, paras, bits32).map(|selector| ctx.set_reg16(EAX, selector)),
        0x49 => free_block(cpu, selector),
        _ if block(cpu, selector).is_none() && dos_block(cpu, selector).is_some() => {
            return dos_resize(cpu, selector, paras as u16).map_err(|(_, largest)| {
                ctx.set_reg16(EBX, largest);
                NO_MEMORY
            });
        }
        _ => resize_block(cpu, selector, paras, bits32),
    };
    if result == Err(NO_MEMORY) {
        let end = cpu.bus.ram().len() as u32;
        let free = cpu.bus.xms.dpmi_free(end).0 / 16;
        set_offset(ctx, EBX, bits32, if bits32 { free } else { free.min(0xFFFE) });
    }
    result
}

/// The place in `blocks` of the block whose first selector is `selector`.
fn block(cpu: &mut Cpu, selector: u16) -> Option<usize> {
    client(cpu).blocks.iter().position(|&(first, _, _)| first == selector | 3)
}

/// The bytes of `paras` paragraphs, if a block may have them: not none,
/// and for a 16-bit client not FFFFh, which asks how much there is.
fn block_size(paras: u32, bits32: bool) -> Option<u32> {
    let valid = paras != 0 && paras < 0x1000_0000 && (bits32 || paras != 0xFFFF);
    valid.then_some(paras * 16)
}

fn allocate_block(cpu: &mut Cpu, paras: u32, bits32: bool) -> Result<u16, u16> {
    let bytes = block_size(paras, bits32).ok_or(NO_MEMORY)?;
    let end = cpu.bus.ram().len() as u32;
    let len = bytes.div_ceil(0x1000) * 0x1000;
    let base = cpu.bus.xms.take_dpmi(len, end).ok_or(NO_MEMORY)?;
    let count = if bits32 { 1 } else { tiles(bytes) };
    let Some(first) = allocate(cpu, count, 0, LDT_FIXED) else {
        cpu.bus.xms.release_dpmi(base);
        return Err(NO_MEMORY);
    };
    cpu.bus.fill_ram(base as usize..(base + len) as usize, 0);
    tile(cpu, first, base, bytes, bits32);
    let handle = cpu.bus.dpmi.next_handle;
    cpu.bus.dpmi.next_handle = handle.wrapping_add(1).max(1);
    let c = client(cpu);
    c.memory.push((handle, base, len));
    c.blocks.push((Client::selector(first), count as u16, handle));
    Ok(Client::selector(first))
}

/// The descriptors of a block of `bytes` at `base` from LDT index `first`.
fn tile(cpu: &mut Cpu, first: usize, base: u32, bytes: u32, bits32: bool) {
    if bits32 {
        let ldt = client(cpu).block;
        write_desc(&mut cpu.bus, ldt, first, descriptor(base, bytes - 1, DATA3, BIG));
    } else {
        set_tiles(cpu, first, base, bytes);
    }
}

fn free_block(cpu: &mut Cpu, selector: u16) -> Result<(), u16> {
    let Some(i) = block(cpu, selector) else {
        return match dos_block(cpu, selector) {
            Some(_) => dos_free(cpu, selector).map_err(|_| BAD_BLOCK),
            None => Err(BAD_BLOCK),
        };
    };
    let (first, count, handle) = client(cpu).blocks.remove(i);
    let c = client(cpu);
    if let Some(m) = c.memory.iter().position(|&(h, _, _)| h == handle) {
        let (_, base, _) = c.memory.remove(m);
        cpu.bus.xms.release_dpmi(base);
    }
    let first = (first >> 3) as usize;
    for index in first..first + count as usize {
        free_entry(cpu, index);
    }
    Ok(())
}

fn resize_block(cpu: &mut Cpu, selector: u16, paras: u32, bits32: bool) -> Result<(), u16> {
    let i = block(cpu, selector).ok_or(BAD_BLOCK)?;
    let bytes = block_size(paras, bits32).ok_or(NO_MEMORY)?;
    let (first, count, handle) = client(cpu).blocks[i];
    let (first, count) = ((first >> 3) as usize, count as usize);
    if !bits32 && !can_retile(cpu, first, count, tiles(bytes)) {
        return Err(NO_MEMORY);
    }
    let m = client(cpu).memory.iter().position(|&(h, _, _)| h == handle).ok_or(BAD_BLOCK)?;
    let base = resize_memory(cpu, m, bytes).map_err(|_| NO_MEMORY)?;
    if bits32 {
        tile(cpu, first, base, bytes, true);
    } else {
        client(cpu).blocks[i].1 = retile(cpu, first, count, base, bytes) as u16;
    }
    Ok(())
}
