//! DPMI (INT 31h) for the tasks, as Windows' DPMI server gives it to
//! Windows programs: selectors of the Win16 process's LDT, DOS memory,
//! memory blocks, the vectors, real-mode interrupts the BIOS and DOS
//! run, and the functions Windows has no need of (locking pages, the
//! virtual interrupt flag) succeeding.

use super::ldt;
use super::thunk::{EAX, EBX, ECX, EDI, EDX, ESI};
use crate::cpu::{ATTR_DB, Cpu, CpuFlags, Seg};
use crate::dpmi::Context;
use crate::win32::memory::{PAGE, PRIVATE, SHARED, USER, WRITABLE};

const CF: u32 = 0x0001;

/// DPMI's errors.
const UNSUPPORTED: u16 = 0x8001;
const NO_DESCRIPTORS: u16 = 0x8011;
const NO_MEMORY: u16 = 0x8013;
const BAD_SELECTOR: u16 = 0x8022;
const BAD_VALUE: u16 = 0x8021;
const BAD_HANDLE: u16 = 0x8023;
/// DOS's errors, which the DOS memory functions give: not enough memory,
/// a block that isn't one.
const DOS_NO_MEMORY: u16 = 8;
const DOS_BAD_BLOCK: u16 = 9;

fn w(ctx: &Context, reg: usize) -> u16 {
    ctx.gpr[reg] as u16
}

fn set_w(ctx: &mut Context, reg: usize, value: u16) {
    ctx.gpr[reg] = ctx.gpr[reg] & 0xFFFF_0000 | value as u32;
}

fn pair(ctx: &Context, high: usize, low: usize) -> u32 {
    (w(ctx, high) as u32) << 16 | w(ctx, low) as u32
}

fn set_pair(ctx: &mut Context, high: usize, low: usize, value: u32) {
    set_w(ctx, high, (value >> 16) as u16);
    set_w(ctx, low, value as u16);
}

fn system(cpu: &mut Cpu) -> u32 {
    super::win16(cpu).system
}

/// Whether `sel` is a selector the task may change: one given out.
fn owned(cpu: &Cpu, sel: u16) -> bool {
    cpu.bus.win32.win16.ldt.is_allocated(sel)
}

/// Change the descriptor of `sel` with `change`.
fn with_desc(cpu: &mut Cpu, sel: u16, change: impl FnOnce(u64) -> u64) -> bool {
    if !owned(cpu, sel) {
        return false;
    }
    let system = system(cpu);
    let desc = ldt::read(cpu, system, sel);
    ldt::write(cpu, system, sel, change(desc));
    true
}

/// AllocSelector's and function 0000h's selectors: `count` of them, data
/// segments of 0 bytes at 0 (or, with `copy`, as that one is).
pub fn alloc_selectors(cpu: &mut Cpu, count: u16, copy: u16) -> u16 {
    let Some(first) = cpu.bus.win32.win16.ldt.alloc(count) else {
        return 0;
    };
    let system = system(cpu);
    let template = if copy != 0 {
        ldt::read(cpu, system, copy)
    } else {
        ldt::segment(0, 0, ldt::ACCESS_DATA)
    };
    for i in 0..count.max(1) {
        ldt::write(cpu, system, first + 8 * i, template);
    }
    first
}

/// FreeSelector and function 0001h.
pub fn free_selector(cpu: &mut Cpu, sel: u16) -> bool {
    if !owned(cpu, sel) {
        return false;
    }
    let system = system(cpu);
    ldt::write(cpu, system, sel, 0);
    cpu.bus.win32.win16.ldt.free(sel, 1);
    true
}

/// The base of `sel`, or 0.
pub fn base(cpu: &Cpu, sel: u16) -> u32 {
    super::selector_base(cpu, sel).map_or(0, |(b, _, _)| b)
}

pub fn set_base(cpu: &mut Cpu, sel: u16, base: u32) -> bool {
    with_desc(cpu, sel, |d| {
        let limit = ldt::limit_of(d);
        let access = (d >> 40) as u8;
        let flags = ((d >> 52) & 0xF) as u8;
        crate::dpmi::descriptor(base, limit, access, flags & 0x4)
    })
}

/// The limit of `sel`, or 0.
pub fn limit(cpu: &Cpu, sel: u16) -> u32 {
    super::selector_base(cpu, sel).map_or(0, |(_, l, _)| l)
}

pub fn set_limit(cpu: &mut Cpu, sel: u16, limit: u32) -> bool {
    with_desc(cpu, sel, |d| {
        let access = (d >> 40) as u8;
        let flags = ((d >> 52) & 0xF) as u8;
        crate::dpmi::descriptor(ldt::base_of(d), limit, access, flags & 0x4)
    })
}

/// Set the access byte (and the D bit from `extended`) of `sel`: a
/// code or data segment's of privilege 3, as DPMI 0.9 allows.
fn set_rights(cpu: &mut Cpu, sel: u16, access: u8, extended: u8) -> bool {
    if access & 0x70 != 0x70 {
        return false;
    }
    with_desc(cpu, sel, |d| {
        crate::dpmi::descriptor(
            ldt::base_of(d),
            ldt::limit_of(d),
            access | 0x60,
            extended & 0x4,
        )
    })
}

/// A selector for the segment `sel` is, of the other kind: data for
/// code, code for data (AllocCStoDSAlias, AllocDStoCSAlias, function
/// 000Ah), of the same size, as the D/B bit says.
pub fn alias(cpu: &mut Cpu, sel: u16, code: bool) -> u16 {
    let Some((base, limit, attr)) = super::selector_base(cpu, sel) else {
        return 0;
    };
    let Some(new) = cpu.bus.win32.win16.ldt.alloc(1) else {
        return 0;
    };
    let access = if code {
        ldt::ACCESS_CODE
    } else {
        ldt::ACCESS_DATA
    };
    let system = system(cpu);
    let big = attr & ATTR_DB != 0;
    ldt::write(cpu, system, new, ldt::segment_sized(base, limit, access, big));
    new
}

/// PrestoChangoSelector: make `dest` an alias of the other kind of
/// `source`, of the same size.
pub fn presto_chango(cpu: &mut Cpu, source: u16, dest: u16) -> u16 {
    let Some((base, limit, attr)) = super::selector_base(cpu, source) else {
        return 0;
    };
    let code = attr & 0x08 != 0;
    let access = if code {
        ldt::ACCESS_DATA
    } else {
        ldt::ACCESS_CODE
    };
    let big = attr & ATTR_DB != 0;
    if !with_desc(cpu, dest, |_| ldt::segment_sized(base, limit, access, big)) {
        return 0;
    }
    dest
}

/// INT 31h.
pub fn int31(cpu: &mut Cpu, ctx: &mut Context) {
    let result = function(cpu, ctx);
    match result {
        Ok(()) => ctx.eflags &= !CF,
        Err(code) => {
            set_w(ctx, EAX, code);
            ctx.eflags |= CF;
        }
    }
}

fn function(cpu: &mut Cpu, ctx: &mut Context) -> Result<(), u16> {
    let ax = w(ctx, EAX);
    let bx = w(ctx, EBX);
    match ax {
        0x0000 => {
            let sel = alloc_selectors(cpu, w(ctx, ECX).max(1), 0);
            if sel == 0 {
                return Err(NO_DESCRIPTORS);
            }
            set_w(ctx, EAX, sel);
        }
        0x0001 => {
            if !free_selector(cpu, bx) {
                return Err(BAD_SELECTOR);
            }
        }
        0x0002 => {
            let sel = super::dos::segment_selector(cpu, bx);
            if sel == 0 {
                return Err(NO_DESCRIPTORS);
            }
            set_w(ctx, EAX, sel);
        }
        0x0003 => set_w(ctx, EAX, ldt::AHINCR),
        0x0006 => {
            if super::selector_base(cpu, bx).is_none() {
                return Err(BAD_SELECTOR);
            }
            let b = base(cpu, bx);
            set_pair(ctx, ECX, EDX, b);
        }
        0x0007 => {
            if !set_base(cpu, bx, pair(ctx, ECX, EDX)) {
                return Err(BAD_SELECTOR);
            }
        }
        0x0008 => {
            if !set_limit(cpu, bx, pair(ctx, ECX, EDX)) {
                return Err(BAD_SELECTOR);
            }
        }
        0x0009 => {
            let cx = w(ctx, ECX);
            if !set_rights(cpu, bx, cx as u8, (cx >> 8) as u8 >> 4) {
                return Err(BAD_VALUE);
            }
        }
        0x000A => {
            let sel = alias(cpu, bx, false);
            if sel == 0 {
                return Err(BAD_SELECTOR);
            }
            set_w(ctx, EAX, sel);
        }
        0x000B => {
            if bx & 4 == 0 {
                return Err(BAD_SELECTOR);
            }
            let system = system(cpu);
            let desc = ldt::read(cpu, system, bx);
            let at = super::lin(cpu, ctx.seg[Seg::ES as usize], w(ctx, EDI));
            super::poke(cpu, at, &desc.to_le_bytes());
        }
        0x000C => {
            let at = super::lin(cpu, ctx.seg[Seg::ES as usize], w(ctx, EDI));
            let bytes = super::peek(cpu, at, 8).ok_or(BAD_VALUE)?;
            let mut desc = u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8]));
            // Level 3 only.
            desc |= 3 << 45;
            if !with_desc(cpu, bx, |_| desc) {
                return Err(BAD_SELECTOR);
            }
        }
        // DOS memory: a block of conventional memory and a selector.
        // A failure gives DOS's error and, for want of memory, the largest
        // block there is in BX, as DPMI 0.9 has them.
        0x0100 => match dos_alloc(cpu, bx as u32 * 16) {
            Ok((seg, sel)) => {
                set_w(ctx, EAX, seg);
                set_w(ctx, EDX, sel);
            }
            Err(largest) => {
                set_w(ctx, EBX, largest);
                return Err(DOS_NO_MEMORY);
            }
        },
        0x0101 => {
            if !dos_free(cpu, w(ctx, EDX)) {
                return Err(BAD_SELECTOR);
            }
        }
        0x0102 => {
            let sel = w(ctx, EDX);
            let b = base(cpu, sel);
            if b == 0 || b >= 0x10_0000 || b & 0xF != 0 {
                return Err(DOS_BAD_BLOCK);
            }
            match crate::mcb::resize(&mut cpu.bus, (b >> 4) as u16, bx) {
                Ok(()) => {
                    let system = system(cpu);
                    let limit = (bx.max(1) as u32) * 16 - 1;
                    ldt::write(cpu, system, sel, ldt::segment(b, limit, ldt::ACCESS_DATA));
                }
                Err(e) => {
                    if let crate::mcb::ResizeError::Insufficient(largest) = e {
                        set_w(ctx, EBX, largest);
                    }
                    return Err(e.code() as u16);
                }
            }
        }
        // The real-mode vectors: the IVT's.
        0x0200 => {
            let v = cpu.bus.read_32(bx as u8 as usize * 4);
            set_pair(ctx, ECX, EDX, v);
        }
        0x0201 => {}
        // The protected-mode vectors: the task's.
        0x0204 => {
            let v = bx as u8;
            let task = super::task(cpu);
            let handler = task.vectors.iter().find(|&&(n, _)| n == v).map(|&(_, h)| h);
            let handler = handler.unwrap_or_else(|| {
                super::builtin::stub_address(cpu, 0, "DefaultVector").unwrap_or(0)
            });
            set_pair(ctx, ECX, EDX, handler);
        }
        0x0205 => {
            let v = bx as u8;
            let handler = pair(ctx, ECX, EDX);
            let task = super::task(cpu);
            task.vectors.retain(|&(n, _)| n != v);
            task.vectors.push((v, handler));
        }
        // Exception handlers: faults end the task, as they do without.
        0x0202 => set_pair(ctx, ECX, EDX, 0),
        0x0203 => {}
        0x0300 => real_mode_int(cpu, ctx)?,
        0x0301 | 0x0302 => return Err(UNSUPPORTED),
        0x0400 => {
            set_w(ctx, EAX, 0x005A);
            // 32-bit hosts, real mode in virtual 8086 mode, virtual memory.
            set_w(ctx, EBX, 0x0007);
            set_w(ctx, ECX, 0x0004);
            set_w(ctx, EDX, 0x0870);
        }
        0x0500 => {
            let free = super::kernel::free_space(cpu);
            let at = super::lin(cpu, ctx.seg[Seg::ES as usize], w(ctx, EDI));
            let mut info = [0xFFu8; 0x30];
            info[0..4].copy_from_slice(&free.to_le_bytes());
            let pages = (free / 0x1000).to_le_bytes();
            for field in [1, 2, 3, 5, 6] {
                info[4 * field..4 * field + 4].copy_from_slice(&pages);
            }
            super::poke(cpu, at, &info);
        }
        // Memory blocks: of the global heap's memory, handle their base.
        // Linear memory: whole pages, as Windows' host gives them.
        0x0501 => {
            let size = pair(ctx, EBX, ECX).max(1);
            let pages = size.checked_next_multiple_of(PAGE).ok_or(NO_MEMORY)?;
            let pid = cpu.bus.win32.win16.pid;
            let base = super::with_win16(cpu, pid, |p, bus| {
                let base = p.memory.reserve(None, pages, PRIVATE, SHARED)?;
                if p.memory.commit(bus, base, pages, USER | WRITABLE).is_none() {
                    p.memory.release_region(bus, base);
                    return None;
                }
                Some(base)
            })
            .flatten()
            .ok_or(NO_MEMORY)?;
            cpu.bus.win32.win16.dpmi_blocks.push(base);
            set_pair(ctx, EBX, ECX, base);
            set_pair(ctx, ESI, EDI, base);
        }
        0x0502 => {
            let handle = pair(ctx, ESI, EDI);
            let blocks = &mut cpu.bus.win32.win16.dpmi_blocks;
            let Some(i) = blocks.iter().position(|&b| b == handle) else {
                return Err(BAD_HANDLE);
            };
            blocks.remove(i);
            let pid = cpu.bus.win32.win16.pid;
            super::with_win16(cpu, pid, |p, bus| p.memory.release_region(bus, handle));
        }
        0x0503 => return Err(NO_MEMORY),
        0x0600..=0x0603 | 0x0702 | 0x0703 => {}
        0x0604 => set_pair(ctx, EBX, ECX, 0x1000),
        // Physical addresses: those of the first megabyte are where they
        // are.
        0x0800 => {
            let phys = pair(ctx, EBX, ECX);
            let size = pair(ctx, ESI, EDI);
            if phys.saturating_add(size) > 0x11_0000 {
                return Err(UNSUPPORTED);
            }
        }
        0x0801 => {}
        // The virtual interrupt flag.
        0x0900..=0x0902 => {
            let on = ctx.eflags & 0x200 != 0;
            ctx.gpr[EAX] = ctx.gpr[EAX] & !0xFF | on as u32;
        }
        // The coprocessor: a 387, there.
        0x0E00 => set_w(ctx, EAX, 0x0045),
        0x0E01 => {}
        _ => {
            cpu.bus
                .log_string(&format!("[W16] DPMI function {:04X}h isn't there", ax));
            return Err(UNSUPPORTED);
        }
    }
    Ok(())
}

/// A block of `bytes` of conventional memory, and its selector.
pub fn dos_alloc(cpu: &mut Cpu, bytes: u32) -> Result<(u16, u16), u16> {
    let paras = bytes.div_ceil(16).clamp(1, 0xFFFF) as u16;
    let psp = cpu.bus.win32.psp;
    let seg = crate::mcb::alloc(&mut cpu.bus, psp, paras)?;
    let Some(sel) = cpu.bus.win32.win16.ldt.alloc(1) else {
        let _ = crate::mcb::free(&mut cpu.bus, seg);
        return Err(0);
    };
    let system = system(cpu);
    ldt::write(
        cpu,
        system,
        sel,
        ldt::segment((seg as u32) << 4, paras as u32 * 16 - 1, ldt::ACCESS_DATA),
    );
    Ok((seg, sel))
}

/// Free a block of conventional memory `dos_alloc` gave, by its
/// selector.
pub fn dos_free(cpu: &mut Cpu, sel: u16) -> bool {
    let b = base(cpu, sel);
    if b == 0 || b >= 0x10_0000 || b & 0xF != 0 {
        return false;
    }
    let ok = crate::mcb::free(&mut cpu.bus, (b >> 4) as u16).is_ok();
    free_selector(cpu, sel);
    ok
}

/// Function 0300h: the real-mode interrupt BL with the registers of the
/// structure at ES:(E)DI, which gets the registers it returns.
fn real_mode_int(cpu: &mut Cpu, ctx: &mut Context) -> Result<(), u16> {
    let at = super::lin(cpu, ctx.seg[Seg::ES as usize], w(ctx, EDI));
    let regs = super::peek(cpu, at, 0x32).ok_or(BAD_VALUE)?;
    let d = |i: usize| u32::from_le_bytes([regs[i], regs[i + 1], regs[i + 2], regs[i + 3]]);
    let h = |i: usize| u16::from_le_bytes([regs[i], regs[i + 1]]);
    let vector = w(ctx, EBX) as u8;
    let mut snapshot = cpu.snapshot();
    // EDI ESI EBP (reserved) EBX EDX ECX EAX.
    snapshot.gpr = [
        d(0x1C),
        d(0x18),
        d(0x14),
        d(0x10),
        0,
        d(0x08),
        d(0x04),
        d(0x00),
    ];
    cpu.restore(&snapshot);
    let (ss, sp) = cpu.bus.win32.rm_stack;
    cpu.set_ss(ss);
    cpu.set_sp(sp.wrapping_sub(0x40));
    cpu.set_es(h(0x22));
    cpu.set_ds(h(0x24));
    cpu.set_cpu_flag(CpuFlags::CF, h(0x20) & 1 != 0);
    crate::win32::dos(cpu, |cpu| crate::interrupts::handle_hle(cpu, vector));
    let out = cpu.snapshot().gpr;
    let flags = cpu.eflags_image() as u16;
    let (es, ds) = (cpu.es(), cpu.ds());
    let mut bytes = regs.clone();
    for (i, value) in [
        (0x00, out[7]),
        (0x04, out[6]),
        (0x08, out[5]),
        (0x10, out[3]),
        (0x14, out[2]),
        (0x18, out[1]),
        (0x1C, out[0]),
    ] {
        bytes[i..i + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[0x20..0x22].copy_from_slice(&flags.to_le_bytes());
    bytes[0x22..0x24].copy_from_slice(&es.to_le_bytes());
    bytes[0x24..0x26].copy_from_slice(&ds.to_le_bytes());
    super::poke(cpu, at, &bytes);
    Ok(())
}
