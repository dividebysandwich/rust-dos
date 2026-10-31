//! The processor's side of a Win32 process: its descriptor tables, the
//! switches between the program's protected mode and DOS's real mode, and
//! what comes through the IDT: hardware interrupts, which their real-mode
//! handlers (the BIOS's, DOS's, resident programs') get as they would
//! under Windows 95, and exceptions.
//!
//! The tables are in a block of the process's memory mapped at the start
//! of the system arena (`SYSTEM_BLOCK`). Each of the IDT's gates leads to
//! a handler in the block that jumps to the vector's service trap in the
//! ROM, run at level 0 on the TSS's stack.
//!
//! As on Windows 95 and 98, the program can read and write the block:
//! there the system arena's pages are open to level 3, and programs use
//! that. Copy protections such as SafeDisc's find the IDT with SIDT,
//! write a few instructions over the handler of an exception's gate and
//! raise the exception to run their own code at level 0, then put the
//! handler back. So each gate leads to a handler of its own in the block,
//! a jump to the vector's trap, which a program can change and put back
//! as it could the system's handler. The handlers are 32 bytes apart,
//! room for code like SafeDisc's (which clears five debug registers), so
//! that it doesn't run into the next vectors' handlers while it is
//! there: the timer's (vector 8) is a few after the exceptions'.

use super::memory::{Memory, PAGE, SYSTEM, USER, WRITABLE};
use super::{Context, ESP, ID_IDT, IDT_TRAPS, ROM_BASE, realmode};
use crate::cpu::{CR0_PE, CR0_PG, CR4_OSFXSR, Cpu, CpuFlags, DescTable, Seg, SegCache};
use crate::dpmi::descriptor;

/// The system block's parts, as offsets from where it is mapped.
const GDT: u32 = 0x0000;
const IDT: u32 = 0x1000;
const TSS: u32 = 0x2000;
/// The TSS's I/O permission bitmap: every port open, as Windows 95 leaves
/// those no driver of its own claims; a byte of ones ends it.
const IOPB: u32 = TSS + 0x68;
const IOPB_END: u32 = IOPB + 0x2000;
const LDT: u32 = 0x5000;
/// The LDT's offset in the system block, for the Win16 selectors there.
pub const LDT_OFFSET: u32 = LDT;
pub const LDT_ENTRIES: u32 = 0x2000;
const RING0_STACK: u32 = LDT + 8 * LDT_ENTRIES;
const RING0_STACK_TOP: u32 = RING0_STACK + 2 * PAGE;
/// The handlers the IDT's gates lead to, `HANDLER_SIZE` bytes each: a
/// jump to the vector's trap in the ROM, and room for a program's own
/// code in its place.
const HANDLERS: u32 = RING0_STACK_TOP;
const HANDLER_SIZE: u32 = 32;
pub const SYSTEM_BLOCK_PAGES: u32 = (HANDLERS + 256 * HANDLER_SIZE) / PAGE;

/// The GDT's selectors: level 0 code and data covering everything, the
/// TSS and the LDT.
const CODE0: u16 = 0x08;
const DATA0: u16 = 0x10;
const TSS_SEL: u16 = 0x18;
const LDT_SEL: u16 = 0x20;
const GDT_ENTRIES: u32 = 5;

/// The program's selectors, in the LDT where Windows 95 has its flat code
/// and data segments (0137h and 013Fh), and the thread's TIB after them.
pub const CODE3: u16 = 0x0137;
pub const DATA3: u16 = 0x013F;
pub const TIB3: u16 = 0x0147;

/// Descriptor access bytes and the flags (G, D/B) in `SegCache::attr`'s
/// form.
const ACCESS_CODE3: u8 = 0xFB;
const ACCESS_DATA3: u8 = 0xF3;
const FLAT: u8 = 0xC;
const BIG: u8 = 0x4;

fn attr(access: u8, flags: u8) -> u16 {
    access as u16 | (flags as u16) << 12
}

fn write_descriptor(cpu: &mut Cpu, at: u32, desc: u64) {
    cpu.bus.write_32(at as usize, desc as u32);
    cpu.bus.write_32(at as usize + 4, (desc >> 32) as u32);
}

/// Make the system block (`pages` in a row at `phys`) and map it, open to
/// level 3 as Windows 95's system arena is: the GDT, the IDT and the
/// handlers its gates lead to, the TSS with the level 0 stack and the LDT
/// with the program's selectors, the TIB's at `tib`.
pub fn build_tables(cpu: &mut Cpu, memory: &mut Memory, phys: u32, tib: u32) -> Option<()> {
    for page in 0..SYSTEM_BLOCK_PAGES {
        memory.map(
            &mut cpu.bus,
            SYSTEM + page * PAGE,
            phys + page * PAGE,
            WRITABLE | USER,
        )?;
    }
    let gdt = [
        (CODE0, descriptor(0, 0xFFFF_FFFF, 0x9B, FLAT)),
        (DATA0, descriptor(0, 0xFFFF_FFFF, 0x93, FLAT)),
        (TSS_SEL, descriptor(SYSTEM + TSS, IOPB_END - TSS, 0x8B, 0)),
        (
            LDT_SEL,
            descriptor(SYSTEM + LDT, 8 * LDT_ENTRIES - 1, 0x82, 0),
        ),
    ];
    for (selector, desc) in gdt {
        write_descriptor(cpu, phys + GDT + selector as u32, desc);
    }
    // Every vector a 32-bit interrupt gate to its handler, at level 0,
    // which jumps to its trap in the ROM. INT 3 and INTO may be used at
    // level 3.
    for vector in 0..256u32 {
        let offset = SYSTEM + HANDLERS + HANDLER_SIZE * vector;
        let trap = ROM_BASE as u32 + IDT_TRAPS as u32 + 4 * vector;
        let mut handler = [0x90u8; HANDLER_SIZE as usize];
        handler[0] = 0xE9;
        handler[1..5].copy_from_slice(&trap.wrapping_sub(offset + 5).to_le_bytes());
        let at = phys + HANDLERS + HANDLER_SIZE * vector;
        cpu.bus.load_bytes(at as usize, &handler);
        let dpl3 = matches!(vector, 3 | 4);
        let access: u64 = if dpl3 { 0xEE } else { 0x8E };
        let gate = (offset as u64 & 0xFFFF)
            | (CODE0 as u64) << 16
            | access << 40
            | ((offset as u64) >> 16) << 48;
        write_descriptor(cpu, phys + IDT + 8 * vector, gate);
    }
    let tss = (phys + TSS) as usize;
    cpu.bus.write_32(tss + 4, SYSTEM + RING0_STACK_TOP);
    cpu.bus.write_32(tss + 8, DATA0 as u32);
    cpu.bus.write_16(tss + 0x66, (IOPB - TSS) as u16);
    cpu.bus.write_8((phys + IOPB_END) as usize, 0xFF);
    let ldt = phys + LDT;
    let ldt_desc = |selector: u16| ldt + (selector & !7) as u32;
    write_descriptor(
        cpu,
        ldt_desc(CODE3),
        descriptor(0, 0xFFFF_FFFF, ACCESS_CODE3, FLAT),
    );
    write_descriptor(
        cpu,
        ldt_desc(DATA3),
        descriptor(0, 0xFFFF_FFFF, ACCESS_DATA3, FLAT),
    );
    write_descriptor(
        cpu,
        ldt_desc(TIB3),
        descriptor(tib, PAGE - 1, ACCESS_DATA3, BIG),
    );
    Some(())
}

/// Let level 3 use INT n for the vectors from 10h on, in the IDT of the
/// system block at `phys`: the Win16 process's, whose tasks call DOS,
/// DPMI and the BIOS so (src/win16/dos.rs). The exceptions' vectors below
/// stay the system's.
pub fn open_soft_ints(cpu: &mut Cpu, phys: u32) {
    for vector in 0x10..256u32 {
        let at = (phys + IDT + 8 * vector + 5) as usize;
        let access = cpu.bus.read_8(at);
        cpu.bus.write_8(at, access | 0x60);
    }
}

/// Write the descriptor of a thread's TIB, at `tib`, with the selector
/// `selector`, in the LDT of the system block at `phys`.
pub fn set_tib(cpu: &mut Cpu, phys: u32, selector: u16, tib: u32) {
    write_descriptor(
        cpu,
        phys + LDT + (selector & !7) as u32,
        descriptor(tib, PAGE - 1, ACCESS_DATA3, BIG),
    );
}

/// Put the processor in the program's context `ctx`, from wherever it is:
/// protected mode with paging, the process's tables, level 3.
pub fn enter_user(cpu: &mut Cpu, ctx: &Context) {
    let process = super::running(cpu).expect("a Win32 process");
    let cr3 = process.memory.cr3;
    // The threads' TIB selectors.
    let tibs: Vec<(u16, u32)> = process
        .threads
        .iter()
        .map(|t| (t.selector, t.tib))
        .collect();
    let system = process.system;
    if !cpu.pe() {
        cpu.note_mode_switch(true);
    }
    cpu.cr3 = cr3;
    cpu.cr0 |= CR0_PE | CR0_PG;
    if cpu.model.has_sse() {
        cpu.cr4 |= CR4_OSFXSR;
    }
    cpu.tlb.flush();
    cpu.gdtr = DescTable {
        base: SYSTEM + GDT,
        limit: (GDT_ENTRIES * 8 - 1) as u16,
    };
    cpu.idtr = DescTable {
        base: SYSTEM + IDT,
        limit: 0x7FF,
    };
    cpu.ldtr = SegCache::from_descriptor(LDT_SEL, SYSTEM + LDT, 8 * LDT_ENTRIES - 1, 0x82);
    cpu.tr = SegCache::from_descriptor(TSS_SEL, SYSTEM + TSS, IOPB_END - TSS, 0x8B);
    cpu.cpl = 3;
    load_segments(cpu, ctx, &tibs, system);
    load_registers(cpu, ctx);
}

/// Put the program's context `ctx` in the processor when it runs the
/// process's code already, at level 3: as `enter_user` without loading
/// the tables and flushing the TLB, for the switches between a Win16
/// task's 16-bit code and the 32-bit code it calls.
pub fn load_context(cpu: &mut Cpu, ctx: &Context) {
    let process = super::running(cpu).expect("a Win32 process");
    let tibs: Vec<(u16, u32)> = process
        .threads
        .iter()
        .map(|t| (t.selector, t.tib))
        .collect();
    let system = process.system;
    // From the system's handlers too (a Win16 task's INT 21h), which run
    // at level 0.
    cpu.cpl = 3;
    load_segments(cpu, ctx, &tibs, system);
    load_registers(cpu, ctx);
}

/// Load the segment registers of `ctx`: the flat selectors, the threads'
/// TIB selectors (`tibs`), and the other selectors of the LDT of the
/// system block at `system` (a Win16 task's) as their descriptors say.
fn load_segments(cpu: &mut Cpu, ctx: &Context, tibs: &[(u16, u32)], system: u32) {
    let code = SegCache::from_descriptor(CODE3, 0, 0xFFFF_FFFF, attr(ACCESS_CODE3, FLAT));
    let data = SegCache::from_descriptor(DATA3, 0, 0xFFFF_FFFF, attr(ACCESS_DATA3, FLAT));
    for seg in Seg::ALL {
        let cache = match ctx.seg[seg as usize] {
            CODE3 => code,
            DATA3 => data,
            selector => match tibs.iter().find(|&&(s, _)| s == selector) {
                Some(&(_, tib)) => {
                    SegCache::from_descriptor(selector, tib, PAGE - 1, attr(ACCESS_DATA3, BIG))
                }
                None => ldt_segment(cpu, system, seg, selector),
            },
        };
        cpu.set_seg_cache(seg, cache);
    }
}

/// The segment `selector` selects in the LDT of the system block at
/// `system`, if it is a present segment at level 3; else a null one.
///
/// A selector freed since the context was saved (its descriptor 0)
/// comes back as 0 in a data segment register `seg`, as the DPMI host
/// of Windows zeroes the registers that hold a selector it frees
/// (`win16::ldt::write` does it for the processor's). The context of an
/// INT, of a call into 32-bit code or back to 16-bit code, or of
/// another task was saved before the free, and the selector in it
/// would fault when the program loads it again (`push es` ... `pop es`).
fn ldt_segment(cpu: &Cpu, system: u32, seg: Seg, selector: u16) -> SegCache {
    let index = selector as u32 >> 3;
    if selector & 4 == 0 || index >= LDT_ENTRIES || system == 0 {
        return SegCache::null(selector);
    }
    let at = (system + LDT + 8 * index) as usize;
    let raw = cpu.bus.read_32(at) as u64 | (cpu.bus.read_32(at + 4) as u64) << 32;
    if raw == 0 && matches!(seg, Seg::ES | Seg::DS | Seg::FS | Seg::GS) {
        return SegCache::null(0);
    }
    let desc = crate::cpu::Descriptor(raw);
    let access = desc.attr() as u8;
    if access & 0x80 == 0 || access & 0x60 != 0x60 || !desc.is_segment() {
        return SegCache::null(selector);
    }
    SegCache::from_descriptor(selector, desc.base(), desc.limit(), desc.attr())
}

/// The general registers, EIP and the flags of `ctx`.
fn load_registers(cpu: &mut Cpu, ctx: &Context) {
    let mut snapshot = cpu.snapshot();
    snapshot.gpr = ctx.gpr;
    cpu.restore(&snapshot);
    cpu.set_eip(ctx.eip);
    // IOPL 0: CLI, STI and the like are the system's.
    let flags = ctx.eflags & !(CpuFlags::VM | CpuFlags::NT | CpuFlags::RF | CpuFlags::IOPL).bits();
    cpu.load_eflags(flags | CpuFlags::IF.bits());
}

/// Put the processor in real mode at `cs:ip`, on the stack `ss:sp`, with
/// DS and ES the process's PSP and the flags `flags`.
pub fn enter_real(cpu: &mut Cpu, (cs, ip): (u16, u16), (ss, sp): (u16, u16), flags: u32) {
    let psp = dos_psp(cpu);
    let cr4 = if cpu.bus.win32.psp != 0 {
        cpu.bus.win32.cr4
    } else {
        cpu.cr4
    };
    if cpu.pe() {
        cpu.note_mode_switch(false);
    }
    cpu.cr0 &= !(CR0_PE | CR0_PG);
    cpu.cr4 = cr4;
    cpu.tlb.flush();
    cpu.cpl = 0;
    cpu.idtr = DescTable {
        base: 0,
        limit: 0x3FF,
    };
    for seg in Seg::ALL {
        let selector = match seg {
            Seg::CS => cs,
            Seg::SS => ss,
            Seg::DS | Seg::ES => psp,
            Seg::FS | Seg::GS => 0,
        };
        cpu.set_seg_cache(seg, SegCache::real(selector));
    }
    cpu.set_esp(sp as u32);
    cpu.set_eip(ip as u32);
    cpu.load_eflags(flags & !(CpuFlags::VM | CpuFlags::NT | CpuFlags::IOPL).bits());
}

/// The DOS process the programs run in: its PSP.
fn dos_psp(cpu: &Cpu) -> u16 {
    match cpu.bus.win32.psp {
        0 => cpu.current_psp,
        psp => psp,
    }
}

/// The program's context where it entered the IDT's gate, from the frame
/// on the level 0 stack, and the error code, if one came with it. None if
/// the gate wasn't entered from level 3.
fn gate_context(cpu: &mut Cpu) -> Option<(Context, Option<u32>)> {
    let top = SYSTEM + RING0_STACK_TOP;
    let sp = cpu.esp();
    let (error, frame) = match top.wrapping_sub(sp) {
        20 => (None, sp),
        24 => (Some(cpu.bus.guest_read_32(sp)), sp + 4),
        _ => return None,
    };
    let read = |cpu: &mut Cpu, i: u32| cpu.bus.guest_read_32(frame + 4 * i);
    let mut ctx = Context::of(cpu);
    ctx.eip = read(cpu, 0);
    ctx.seg[Seg::CS as usize] = read(cpu, 1) as u16;
    ctx.eflags = read(cpu, 2);
    ctx.gpr[ESP] = read(cpu, 3);
    ctx.seg[Seg::SS as usize] = read(cpu, 4) as u16;
    Some((ctx, error))
}

/// The vectors of the hardware interrupts, as the PICs are programmed.
fn is_irq(cpu: &Cpu, vector: u8) -> bool {
    (0..16).any(|irq| cpu.bus.pic.vector(irq) == vector)
}

/// A vector's trap in the ROM: an interrupt or exception at level 3.
pub fn idt(cpu: &mut Cpu, vector: u8) {
    let Some((ctx, error)) = gate_context(cpu) else {
        super::fatal(
            cpu,
            &format!("exception {:02X}h in the system", vector),
            None,
        );
        return;
    };
    // A Win16 task's INT n: DOS, DPMI and the BIOS (src/win16/dos.rs).
    if error.is_none() && crate::win16::is_soft_int(cpu, vector, &ctx) {
        crate::win16::dos::interrupt(cpu, vector, ctx);
        return;
    }
    // The first PIC's vectors are exceptions 8-15 too; those push an
    // error code (#DF, #TS, #NP, #SS, #GP, #PF), the interrupts don't.
    let exception = vector < 0x20 && (vector < 8 || error.is_some() || !is_irq(cpu, vector));
    if exception {
        exception_entry(cpu, vector, error, ctx);
    } else if is_irq(cpu, vector) {
        reflect(cpu, vector, ctx);
    } else {
        // INT n the program may not use: #GP, as for a gate of level 0.
        exception_entry(cpu, 13, Some((vector as u32) << 3 | 2), ctx);
    }
}

/// An exception in the program: its handlers get it (seh.rs).
fn exception_entry(cpu: &mut Cpu, vector: u8, error: Option<u32>, ctx: Context) {
    if crate::win16::is_task_context(cpu, &ctx) {
        crate::win16::task::fault(cpu, vector, error, ctx);
        return;
    }
    super::seh::fault(cpu, vector, error, ctx);
}

/// A hardware interrupt that came while the program ran: its real-mode
/// handler runs, on the host's real-mode stack, with DS and ES the DOS
/// process's PSP, and returns to the trap at `realmode::RETURN`, which
/// goes on with the program.
fn reflect(cpu: &mut Cpu, vector: u8, ctx: Context) {
    let psp = dos_psp(cpu);
    let rm = realmode::handler_context(cpu, vector, ctx.gpr, psp, 0x0002);
    // The handler's IRET comes back to the trap, interrupts still off.
    let (seg, off) = realmode::RETURN;
    let frame = realmode::Frame::new(realmode::Kind::Irq, ctx);
    realmode::call(cpu, frame, &rm, &[0x0002, seg, off]);
}

/// The trap ids of the IDT's vectors.
pub fn idt_id(vector: u8) -> u16 {
    ID_IDT | vector as u16
}
