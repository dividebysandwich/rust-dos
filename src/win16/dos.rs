//! The INT n of the tasks' 16-bit code: DOS (INT 21h), the multiplex
//! interrupt (INT 2Fh), DPMI (INT 31h), the BIOS, and the floating-point
//! emulator's interrupts that Microsoft's compilers emit (INT 34h-3Dh).
//!
//! DOS runs in Rust as the built-in DOS has it, on real-mode addresses:
//! as KRNL386 does, a call with pointers has what DOS is to read copied
//! into a transfer buffer in conventional memory, below 1 MB, DOS called
//! on it with real-mode segments in the registers, and what DOS wrote
//! copied back to the task's buffer. Reads and writes go in pieces of
//! the buffer's size. Memory, interrupt vectors, the DTA and the PSP are
//! the task's: blocks of the global heap, its own vectors, a DTA it
//! addresses by selector.

use super::thunk::{EAX, EBP, EBX, ECX, EDI, EDX, ESI, ESP};
use super::{global, ldt};
use crate::cpu::{Cpu, CpuFlags, Seg};
use crate::dpmi::Context;

/// The transfer buffer's paragraphs, and its parts: the data of a read
/// or write, two names, a DTA and the rest.
const TB_PARAS: u16 = 0x500;
const CHUNK: u32 = 0x4000;
const NAME1: u32 = 0x4000;
const NAME2: u32 = 0x4100;
const DTA: u32 = 0x4200;
const OTHER: u32 = 0x4300;
/// The longest name copied, and a DTA's bytes a search uses.
const NAME_MAX: u32 = 0x100;
const FIND_SIZE: u32 = 0x2B;

const CF: u32 = 0x0001;
const ZF: u32 = 0x0040;

/// DOS's error codes the tasks get here.
const INVALID_FUNCTION: u16 = 0x01;
const NO_MEMORY: u16 = 0x08;
const BAD_BLOCK: u16 = 0x09;
const BAD_FORMAT: u16 = 0x0B;

fn w(ctx: &Context, reg: usize) -> u16 {
    ctx.gpr[reg] as u16
}

fn set_w(ctx: &mut Context, reg: usize, value: u16) {
    ctx.gpr[reg] = ctx.gpr[reg] & 0xFFFF_0000 | value as u32;
}

fn set_al(ctx: &mut Context, value: u8) {
    ctx.gpr[EAX] = ctx.gpr[EAX] & !0xFF | value as u32;
}

fn sel(ctx: &Context, seg: Seg) -> u16 {
    ctx.seg[seg as usize]
}

fn set_cf(ctx: &mut Context, on: bool) {
    ctx.eflags = if on {
        ctx.eflags | CF
    } else {
        ctx.eflags & !CF
    };
}

/// Fail with DOS's error `code` in AX.
fn fail(ctx: &mut Context, code: u16) {
    set_w(ctx, EAX, code);
    set_cf(ctx, true);
}

/// The linear address of `sel`:`off` of the task.
fn at(cpu: &Cpu, sel: u16, off: u16) -> u32 {
    super::lin(cpu, sel, off)
}

/// The transfer buffer's segment and linear address.
fn tb(cpu: &Cpu) -> (u16, u32) {
    let seg = cpu.bus.win32.win16.tb;
    (seg, (seg as u32) << 4)
}

/// Take the transfer buffer from DOS when the Win16 process is made: a
/// block of conventional memory the DOS process owns, which the program
/// DOS loaded, whose block is mostly all of it, gives up the end of.
pub fn init(cpu: &mut Cpu) -> Result<(), String> {
    let psp = cpu.bus.win32.psp;
    let seg = match crate::mcb::alloc(&mut cpu.bus, psp, TB_PARAS) {
        Ok(seg) => seg,
        Err(_) => {
            let (ss, sp) = cpu.bus.win32.rm_stack;
            let keep = (ss as u32 + (sp as u32 >> 4) + 1).saturating_sub(psp as u32) + 0x10;
            let keep = keep.max(0x20) as u16;
            let _ = crate::mcb::resize(&mut cpu.bus, psp, keep);
            crate::mcb::alloc(&mut cpu.bus, psp, TB_PARAS)
                .map_err(|_| "There isn't enough conventional memory for Windows.".to_string())?
        }
    };
    cpu.bus.win32.win16.tb = seg;
    Ok(())
}

/// The selector of the real-mode segment `seg`, a 64 KB data segment
/// there, made the first time one is asked for.
pub fn segment_selector(cpu: &mut Cpu, seg: u16) -> u16 {
    let known = super::task::real_selector(cpu, seg);
    if known != 0 {
        return known;
    }
    let Some(sel) = cpu.bus.win32.win16.ldt.alloc(1) else {
        return 0;
    };
    let system = super::win16(cpu).system;
    ldt::write(
        cpu,
        system,
        sel,
        ldt::segment((seg as u32) << 4, 0xFFFF, ldt::ACCESS_DATA),
    );
    cpu.bus.win32.win16.real_selectors.push((seg, sel));
    sel
}

/// The task's INT `vector`, in its context `ctx`, which goes on after the
/// INT instruction.
pub fn interrupt(cpu: &mut Cpu, vector: u8, mut ctx: Context) {
    // A vector the task set (INT 21h AH=25h) that isn't one of these is
    // its own handler's: the handler runs as the INT's would.
    if !matches!(vector, 0x21 | 0x2F | 0x31 | 0x34..=0x3D)
        && let Some(handler) = vector_of(cpu, vector)
    {
        call_handler(cpu, ctx, handler);
        return;
    }
    match vector {
        0x21 => {
            if !int21(cpu, &mut ctx) {
                return;
            }
        }
        0x2F => {
            if !int2f(cpu, &mut ctx) {
                return;
            }
        }
        0x31 => super::dpmi::int31(cpu, &mut ctx),
        0x34..=0x3D => fpu_fixup(cpu, vector, &mut ctx),
        0x10 | 0x11 | 0x12 | 0x15 | 0x16 | 0x1A => bios(cpu, vector, &mut ctx),
        0x20 => {
            super::task::exit(cpu, 0);
            return;
        }
        _ => {
            cpu.bus.log_string(&format!(
                "[W16] INT {:02X}h (AX {:04X}) isn't there, at {:04X}:{:04X}",
                vector,
                w(&ctx, EAX),
                sel(&ctx, Seg::CS),
                ctx.eip.wrapping_sub(2) as u16
            ));
        }
    }
    resume(cpu, &ctx);
}

/// Go on with the task's context `ctx`, at level 3.
fn resume(cpu: &mut Cpu, ctx: &Context) {
    crate::win32::system::load_context(cpu, ctx);
}

/// The handler the task set for `vector`, as selector:offset.
fn vector_of(cpu: &Cpu, vector: u8) -> Option<u32> {
    let t = super::current_task(cpu)?;
    cpu.bus.win32.win16.tasks[t]
        .vectors
        .iter()
        .find(|&&(v, _)| v == vector)
        .map(|&(_, h)| h)
}

/// Run the task's handler `handler` for an INT in `ctx`: the flags, CS
/// and IP pushed as an interrupt pushes them.
fn call_handler(cpu: &mut Cpu, mut ctx: Context, handler: u32) {
    let ss = sel(&ctx, Seg::SS);
    let base = super::selector_base(cpu, ss).map_or(0, |(b, _, _)| b);
    let mut sp = ctx.gpr[ESP] as u16;
    for word in [ctx.eflags as u16, sel(&ctx, Seg::CS), ctx.eip as u16] {
        sp = sp.wrapping_sub(2);
        super::write_16(cpu, base.wrapping_add(sp as u32), word);
    }
    set_w(&mut ctx, ESP, sp);
    ctx.seg[Seg::CS as usize] = (handler >> 16) as u16;
    ctx.eip = handler & 0xFFFF;
    ctx.eflags &= !0x0300;
    resume(cpu, &ctx);
}

/// KERNEL's handler of the vectors a task didn't set, which INT 21h
/// AH=35h gives: an IRET.
pub fn default_vector(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    let base = cpu.seg_cache(Seg::SS).base;
    let sp = ctx.gpr[ESP] as u16;
    let read =
        |cpu: &Cpu, i: u16| super::read_16(cpu, base.wrapping_add(sp.wrapping_add(2 * i) as u32));
    let (ip, cs, flags) = (read(cpu, 0), read(cpu, 1), read(cpu, 2));
    set_w(&mut ctx, ESP, sp.wrapping_add(6));
    ctx.eip = ip as u32;
    ctx.seg[Seg::CS as usize] = cs;
    ctx.eflags = ctx.eflags & !0xFFFF | flags as u32 | 0x0202;
    resume(cpu, &ctx);
    None
}

/// Call the built-in DOS (or the BIOS's `vector`) with the registers of
/// `ctx` and the real-mode segments `ds` and `es`, on the DOS process's
/// stack. The registers it returns, the segments too, in a context whose
/// stack and segments are otherwise `ctx`'s.
fn call_real(cpu: &mut Cpu, vector: u8, ctx: &Context, ds: u16, es: u16) -> (Context, u16, u16) {
    let saved = Context::of(cpu);
    let (ss, sp) = cpu.bus.win32.rm_stack;
    let mut snapshot = cpu.snapshot();
    snapshot.gpr = ctx.gpr;
    cpu.restore(&snapshot);
    cpu.set_sp(sp.wrapping_sub(0x40));
    cpu.set_ss(ss);
    cpu.set_ds(ds);
    cpu.set_es(es);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    crate::win32::dos(cpu, |cpu| crate::interrupts::handle_hle(cpu, vector));
    let mut out = *ctx;
    out.gpr = cpu.snapshot().gpr;
    out.gpr[ESP] = ctx.gpr[ESP];
    out.gpr[EBP] = ctx.gpr[EBP];
    let flags = cpu.eflags_image() & (CF | ZF);
    out.eflags = ctx.eflags & !(CF | ZF) | flags;
    let segments = (cpu.ds(), cpu.es());
    crate::win32::system::load_context(cpu, &saved);
    (out, segments.0, segments.1)
}

/// INT 21h for KERNEL's functions: AX, BX, CX and DX, and a name DS:DX
/// points to if there is one. AX and DX that DOS returned, or its error.
pub fn call21(cpu: &mut Cpu, regs: [u16; 4], name: Option<&[u8]>) -> Result<(u16, u16), u16> {
    let (tb_seg, base) = tb(cpu);
    let mut ctx = Context::of(cpu);
    for (reg, value) in [EAX, EBX, ECX, EDX].into_iter().zip(regs) {
        set_w(&mut ctx, reg, value);
    }
    if let Some(name) = name {
        for (i, &b) in name
            .iter()
            .take(NAME_MAX as usize - 1)
            .chain(&[0])
            .enumerate()
        {
            cpu.bus.write_8((base + NAME1) as usize + i, b);
        }
        set_w(&mut ctx, EDX, NAME1 as u16);
    }
    let (out, _, _) = call_real(cpu, 0x21, &ctx, tb_seg, tb_seg);
    if out.eflags & CF != 0 {
        Err(w(&out, EAX))
    } else {
        Ok((w(&out, EAX), w(&out, EDX)))
    }
}

/// The transfer buffer's name, after a call that wrote one there (AH=60h).
pub fn name_returned(cpu: &Cpu, offset: u32) -> Vec<u8> {
    let (_, base) = tb(cpu);
    (0..NAME_MAX)
        .map(|i| cpu.bus.read_8((base + offset + i) as usize))
        .take_while(|&b| b != 0)
        .collect()
}

/// The canonical name of `name` (INT 21h AH=60h), or None.
pub fn full_name(cpu: &mut Cpu, name: &[u8]) -> Option<Vec<u8>> {
    let (tb_seg, base) = tb(cpu);
    for (i, &b) in name
        .iter()
        .take(NAME_MAX as usize - 1)
        .chain(&[0])
        .enumerate()
    {
        cpu.bus.write_8((base + NAME1) as usize + i, b);
    }
    let mut ctx = Context::of(cpu);
    set_w(&mut ctx, EAX, 0x6000);
    set_w(&mut ctx, ESI, NAME1 as u16);
    set_w(&mut ctx, EDI, NAME2 as u16);
    let (out, _, _) = call_real(cpu, 0x21, &ctx, tb_seg, tb_seg);
    (out.eflags & CF == 0).then(|| name_returned(cpu, NAME2))
}

/// Read (or, with `write`, write) `len` bytes of the file `handle` to
/// (from) the linear address `at`, in the transfer buffer's pieces. The
/// bytes moved, or DOS's error.
pub fn transfer(cpu: &mut Cpu, handle: u16, at: u32, len: u32, write: bool) -> Result<u32, u16> {
    let (tb_seg, _) = tb(cpu);
    let mut done = 0u32;
    loop {
        let piece = (len - done).min(CHUNK);
        if write {
            to_tb(cpu, at + done, 0, piece);
        }
        let mut ctx = Context::of(cpu);
        set_w(&mut ctx, EAX, if write { 0x4000 } else { 0x3F00 });
        set_w(&mut ctx, EBX, handle);
        set_w(&mut ctx, ECX, piece as u16);
        set_w(&mut ctx, EDX, 0);
        let (out, _, _) = call_real(cpu, 0x21, &ctx, tb_seg, tb_seg);
        if out.eflags & CF != 0 {
            return if done == 0 {
                Err(w(&out, EAX))
            } else {
                Ok(done)
            };
        }
        let moved = w(&out, EAX) as u32;
        if !write {
            from_tb(cpu, 0, at + done, moved);
        }
        done += moved;
        if moved < piece || done >= len {
            return Ok(done);
        }
    }
}

/// DOS3Call: INT 21h with the registers the caller has.
pub fn dos3call(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    if int21(cpu, &mut ctx) {
        resume(cpu, &ctx);
    }
    None
}

/// Copy `len` bytes from the task's `from` (linear) to the transfer
/// buffer at `to`.
fn to_tb(cpu: &mut Cpu, from: u32, to: u32, len: u32) {
    let data = super::peek(cpu, from, len as usize).unwrap_or_else(|| vec![0; len as usize]);
    let (_, base) = tb(cpu);
    for (i, b) in data.into_iter().enumerate() {
        cpu.bus.write_8((base + to) as usize + i, b);
    }
}

/// Copy `len` bytes of the transfer buffer at `from` to the task's `to`.
fn from_tb(cpu: &mut Cpu, from: u32, to: u32, len: u32) {
    let (_, base) = tb(cpu);
    let data: Vec<u8> = (0..len)
        .map(|i| cpu.bus.read_8((base + from + i) as usize))
        .collect();
    super::poke(cpu, to, &data);
}

/// Copy the task's string at `from` (DS:DX's mostly) into the transfer
/// buffer at `to`, to `end` or at most `NAME_MAX` bytes.
fn string_to_tb(cpu: &mut Cpu, from: u32, to: u32, end: u8) {
    let mut bytes = Vec::new();
    for i in 0..NAME_MAX {
        let b = super::peek(cpu, from + i, 1).map_or(end, |b| b[0]);
        bytes.push(b);
        if b == end {
            break;
        }
    }
    let (_, base) = tb(cpu);
    for (i, b) in bytes.into_iter().enumerate() {
        cpu.bus.write_8((base + to) as usize + i, b);
    }
}

/// INT 21h. False when the task's context doesn't go on (it ended, or
/// went elsewhere).
fn int21(cpu: &mut Cpu, ctx: &mut Context) -> bool {
    let ax = w(ctx, EAX);
    let [al, ah] = ax.to_le_bytes();
    let ds = sel(ctx, Seg::DS);
    let es = sel(ctx, Seg::ES);
    let dx = w(ctx, EDX);
    let (tb_seg, _) = tb(cpu);
    match ah {
        // Ends: the task's.
        0x00 => {
            super::task::exit(cpu, 0);
            return false;
        }
        0x4C | 0x31 => {
            super::task::exit(cpu, al as u16);
            return false;
        }
        // The DTA.
        0x1A => super::task(cpu).dta = (ds as u32) << 16 | dx as u32,
        0x2F => {
            let dta = super::task(cpu).dta;
            ctx.seg[Seg::ES as usize] = (dta >> 16) as u16;
            set_w(ctx, EBX, dta as u16);
        }
        // The vectors.
        0x25 => {
            let handler = (ds as u32) << 16 | dx as u32;
            let task = super::task(cpu);
            task.vectors.retain(|&(v, _)| v != al);
            task.vectors.push((al, handler));
        }
        0x35 => {
            let handler = vector_of(cpu, al).unwrap_or_else(|| {
                super::builtin::stub_address(cpu, 0, "DefaultVector").unwrap_or(0)
            });
            ctx.seg[Seg::ES as usize] = (handler >> 16) as u16;
            set_w(ctx, EBX, handler as u16);
        }
        // The PSP.
        0x51 | 0x62 => {
            let psp = super::task(cpu).psp;
            set_w(ctx, EBX, psp);
        }
        0x50 => {}
        // Memory: blocks of the global heap.
        0x48 => {
            let paras = w(ctx, EBX) as u32;
            let owner = super::task(cpu).handle;
            let h = global::alloc(cpu, global::GMEM_FIXED, paras.max(1) * 16, owner, false);
            if h == 0 {
                fail(ctx, NO_MEMORY);
                set_w(ctx, EBX, 0x1000);
            } else {
                set_w(ctx, EAX, global::handle_to_sel(h));
                set_cf(ctx, false);
            }
        }
        0x49 => {
            let h = global::handle_of(cpu, es);
            if h != 0 && global::free(cpu, h) {
                set_cf(ctx, false);
            } else {
                fail(ctx, BAD_BLOCK);
            }
        }
        0x4A => {
            let h = global::handle_of(cpu, es);
            let paras = w(ctx, EBX) as u32;
            if h != 0 && global::realloc(cpu, h, paras.max(1) * 16, 0) != 0 {
                set_cf(ctx, false);
            } else {
                fail(ctx, NO_MEMORY);
                set_w(ctx, EBX, 0);
            }
        }
        // EXEC: a Windows program becomes a task; DOS programs don't run.
        0x4B => exec(cpu, ctx, al),
        // Long file names: not on Windows 3.1.
        0x71 => fail(ctx, 0x7100),
        // A name at DS:DX.
        0x39 | 0x3A | 0x3B | 0x3C | 0x3D | 0x41 | 0x43 | 0x5B => {
            string_to_tb(cpu, at(cpu, ds, dx), NAME1, 0);
            let mut regs = *ctx;
            set_w(&mut regs, EDX, NAME1 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            *ctx = returned(out, &regs, ctx);
        }
        // A temporary file's name made in the directory at DS:DX.
        0x5A => {
            let target = at(cpu, ds, dx);
            string_to_tb(cpu, target, NAME1, 0);
            let mut regs = *ctx;
            set_w(&mut regs, EDX, NAME1 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            if out.eflags & CF == 0 {
                // The name and its NUL, no more: the buffer is the
                // directory's path and 13 bytes.
                let (_, base) = tb(cpu);
                let len = (0..NAME_MAX)
                    .find(|&i| cpu.bus.read_8((base + NAME1 + i) as usize) == 0)
                    .map_or(NAME_MAX, |i| i + 1);
                from_tb(cpu, NAME1, target, len);
            }
            *ctx = returned(out, &regs, ctx);
        }
        // Rename: DS:DX to ES:DI.
        0x56 => {
            string_to_tb(cpu, at(cpu, ds, dx), NAME1, 0);
            let di = w(ctx, EDI);
            string_to_tb(cpu, at(cpu, es, di), NAME2, 0);
            let mut regs = *ctx;
            set_w(&mut regs, EDX, NAME1 as u16);
            set_w(&mut regs, EDI, NAME2 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            *ctx = returned(out, &regs, ctx);
        }
        // Extended open: the name at DS:SI.
        0x6C => {
            let si = w(ctx, ESI);
            string_to_tb(cpu, at(cpu, ds, si), NAME1, 0);
            let mut regs = *ctx;
            set_w(&mut regs, ESI, NAME1 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            *ctx = returned(out, &regs, ctx);
        }
        // The current directory, to DS:SI.
        0x47 => {
            let si = w(ctx, ESI);
            let mut regs = *ctx;
            set_w(&mut regs, ESI, NAME1 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            if out.eflags & CF == 0 {
                from_tb(cpu, NAME1, at(cpu, ds, si), 64);
            }
            *ctx = returned(out, &regs, ctx);
        }
        // The canonical name of DS:SI, to ES:DI.
        0x60 => {
            let (si, di) = (w(ctx, ESI), w(ctx, EDI));
            string_to_tb(cpu, at(cpu, ds, si), NAME1, 0);
            let mut regs = *ctx;
            set_w(&mut regs, ESI, NAME1 as u16);
            set_w(&mut regs, EDI, NAME2 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            if out.eflags & CF == 0 {
                from_tb(cpu, NAME2, at(cpu, es, di), 128);
            }
            *ctx = returned(out, &regs, ctx);
        }
        // A string to the console, to its '$'.
        0x09 => {
            string_to_tb(cpu, at(cpu, ds, dx), NAME1, b'$');
            let mut regs = *ctx;
            set_w(&mut regs, EDX, NAME1 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            *ctx = returned(out, &regs, ctx);
        }
        // Country information, to DS:DX.
        0x38 if dx != 0xFFFF => {
            let mut regs = *ctx;
            set_w(&mut regs, EDX, OTHER as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            if out.eflags & CF == 0 {
                from_tb(cpu, OTHER, at(cpu, ds, dx), 34);
            }
            *ctx = returned(out, &regs, ctx);
        }
        // Read and write, in pieces.
        0x3F | 0x40 => read_write(cpu, ctx, ah == 0x40),
        // Searches, in the transfer buffer's DTA, copied to the task's.
        0x4E | 0x4F => {
            let dta = super::task(cpu).dta;
            let dta_lin = super::linear(cpu, dta);
            if ah == 0x4E {
                string_to_tb(cpu, at(cpu, ds, dx), NAME1, 0);
            }
            to_tb(cpu, dta_lin, DTA, FIND_SIZE);
            let saved = dos_dta(cpu);
            set_dos_dta(cpu, (tb_seg as u32) << 16 | DTA);
            let mut regs = *ctx;
            set_w(&mut regs, EDX, NAME1 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            set_dos_dta(cpu, saved);
            from_tb(cpu, DTA, dta_lin, FIND_SIZE);
            *ctx = returned(out, &regs, ctx);
        }
        // IOCTL with a buffer at DS:DX.
        0x44 if matches!(al, 0x02..=0x05) => {
            let cx = w(ctx, ECX) as u32;
            let len = cx.min(CHUNK);
            let target = at(cpu, ds, dx);
            to_tb(cpu, target, 0, len);
            let mut regs = *ctx;
            set_w(&mut regs, EDX, 0);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            from_tb(cpu, 0, target, len);
            *ctx = returned(out, &regs, ctx);
        }
        0x44 if matches!(al, 0x0C | 0x0D) => {
            let target = at(cpu, ds, dx);
            to_tb(cpu, target, OTHER, 0x40);
            let mut regs = *ctx;
            set_w(&mut regs, EDX, OTHER as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            from_tb(cpu, OTHER, target, 0x40);
            *ctx = returned(out, &regs, ctx);
        }
        // Parse a file name at DS:SI into the FCB at ES:DI.
        0x29 => {
            let (si, di) = (w(ctx, ESI), w(ctx, EDI));
            string_to_tb(cpu, at(cpu, ds, si), NAME1, 0);
            let fcb = at(cpu, es, di);
            to_tb(cpu, fcb, NAME2, 0x25);
            let mut regs = *ctx;
            set_w(&mut regs, ESI, NAME1 as u16);
            set_w(&mut regs, EDI, NAME2 as u16);
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            from_tb(cpu, NAME2, fcb, 0x25);
            let used = w(&out, ESI).wrapping_sub(NAME1 as u16);
            let mut out = returned(out, &regs, ctx);
            set_w(&mut out, ESI, si.wrapping_add(used));
            set_w(&mut out, EDI, di);
            *ctx = out;
        }
        // Pointers DOS gives into its own data: selectors of them.
        0x1B | 0x1C | 0x1F | 0x32 => {
            let regs = *ctx;
            let (out, rds, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            let mut out = returned(out, &regs, ctx);
            out.seg[Seg::DS as usize] = segment_selector(cpu, rds);
            *ctx = out;
        }
        0x34 | 0x52 => {
            let regs = *ctx;
            let (out, _, res) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            let mut out = returned(out, &regs, ctx);
            out.seg[Seg::ES as usize] = segment_selector(cpu, res);
            *ctx = out;
        }
        // The FCB functions: not for Windows programs.
        0x0F..=0x17 | 0x21..=0x24 | 0x27 | 0x28 => set_al(ctx, 0xFF),
        // Buffered keyboard input: no console.
        0x0A => {}
        // The rest take no pointers.
        _ => {
            let regs = *ctx;
            let (out, _, _) = call_real(cpu, 0x21, &regs, tb_seg, tb_seg);
            *ctx = returned(out, &regs, ctx);
        }
    }
    true
}

/// The registers DOS returned in `out`, called with `passed`, for the
/// task that called with `ctx`: its segments, and its DX, SI and DI
/// where DOS left the transfer buffer's offsets in them.
fn returned(out: Context, passed: &Context, ctx: &Context) -> Context {
    let mut kept = out;
    kept.seg = ctx.seg;
    for reg in [EDX, ESI, EDI] {
        if w(&out, reg) == w(passed, reg) {
            kept.gpr[reg] = ctx.gpr[reg];
        }
    }
    kept
}

/// DOS's DTA, as segment:offset.
fn dos_dta(cpu: &Cpu) -> u32 {
    (cpu.bus.dta_segment as u32) << 16 | cpu.bus.dta_offset as u32
}

fn set_dos_dta(cpu: &mut Cpu, dta: u32) {
    cpu.bus.dta_segment = (dta >> 16) as u16;
    cpu.bus.dta_offset = dta as u16;
}

/// AH=3Fh and 40h: CX bytes of the handle BX, to or from DS:DX.
fn read_write(cpu: &mut Cpu, ctx: &mut Context, write: bool) {
    let target = at(cpu, sel(ctx, Seg::DS), w(ctx, EDX));
    match transfer(cpu, w(ctx, EBX), target, w(ctx, ECX) as u32, write) {
        Ok(done) => {
            set_w(ctx, EAX, done as u16);
            set_cf(ctx, false);
        }
        Err(code) => fail(ctx, code),
    }
}

/// AH=4Bh: EXEC. AL=0 starts the program at DS:DX as a task if it is a
/// Windows program, with the parameter block at ES:BX: the environment's
/// selector, the command tail, and for Windows programs the show
/// command's two words.
fn exec(cpu: &mut Cpu, ctx: &mut Context, al: u8) {
    if al != 0 {
        fail(ctx, INVALID_FUNCTION);
        return;
    }
    let ds = sel(ctx, Seg::DS);
    let path = super::string(cpu, at(cpu, ds, w(ctx, EDX)));
    let block = at(cpu, sel(ctx, Seg::ES), w(ctx, EBX));
    let tail = super::linear(cpu, super::read_32(cpu, block + 2));
    let len = super::peek(cpu, tail, 1).map_or(0, |b| b[0]) as usize;
    let arguments = crate::dosstr::from_bytes(&super::peek(cpu, tail + 1, len).unwrap_or_default());
    let show_at = super::linear(cpu, super::read_32(cpu, block + 6));
    let show = if show_at != 0 && super::read_16(cpu, show_at) == 2 {
        super::read_16(cpu, show_at + 2)
    } else {
        1
    };
    match super::task::exec(cpu, &path, arguments.trim_start(), show) {
        Ok(instance) => {
            set_w(ctx, EAX, instance);
            set_cf(ctx, false);
        }
        Err(code) => fail(ctx, if code == 0 { BAD_FORMAT } else { code }),
    }
}

/// INT 2Fh: what a Windows program asks of the multiplex interrupt.
/// False when the task's context doesn't go on now (it gave up its
/// turn).
fn int2f(cpu: &mut Cpu, ctx: &mut Context) -> bool {
    match w(ctx, EAX) {
        // Windows' enhanced mode: 4.0, as Windows 95's.
        0x1600 => set_w(ctx, EAX, 0x0004),
        // In protected mode, under DPMI.
        0x1686 => set_w(ctx, EAX, 0),
        // Release the time slice: another task goes.
        0x1680 => {
            set_al(ctx, 0);
            super::task::yield_now(cpu, *ctx);
            return false;
        }
        // The MS-DOS extensions' entry point: none.
        0x168A => {}
        // XMS: none for protected-mode programs.
        0x4300 => set_al(ctx, 0),
        _ => {
            let (tb_seg, _) = tb(cpu);
            let regs = *ctx;
            let (out, _, _) = call_real(cpu, 0x2F, &regs, tb_seg, tb_seg);
            *ctx = returned(out, &regs, ctx);
        }
    }
    true
}

/// The BIOS's functions without pointers: the time, the keyboard, the
/// equipment and memory sizes, the video BIOS's queries.
fn bios(cpu: &mut Cpu, vector: u8, ctx: &mut Context) {
    let (tb_seg, _) = tb(cpu);
    let regs = *ctx;
    let (out, _, _) = call_real(cpu, vector, &regs, tb_seg, tb_seg);
    *ctx = returned(out, &regs, ctx);
}

/// INT 34h-3Dh: the floating-point emulator's calls Microsoft's
/// compilers emit, which the code is patched to the instructions they
/// stand for in, as Windows does when there is a coprocessor; then the
/// instruction runs.
fn fpu_fixup(cpu: &mut Cpu, vector: u8, ctx: &mut Context) {
    let cs = sel(ctx, Seg::CS);
    let Some((base, _, _)) = super::selector_base(cpu, cs) else {
        return;
    };
    let ip = ctx.eip as u16;
    let start = base.wrapping_add(ip.wrapping_sub(2) as u32);
    let code: Vec<u8> = match vector {
        // FWAIT, then the escape D8h-DFh.
        0x34..=0x3B => vec![0x9B, 0xD8 + (vector - 0x34)],
        // A standalone FWAIT.
        0x3D => vec![0x90, 0x9B],
        // An instruction with a segment override: CD 3C xy mm, xy's top
        // two bits the segment, its low three the escape's.
        _ => {
            let xy = super::peek(cpu, start + 2, 1).map_or(0, |b| b[0]);
            let prefix = [0x3E, 0x36, 0x2E, 0x26][(xy >> 6) as usize];
            vec![0x9B, prefix, 0xD8 | (xy & 7)]
        }
    };
    // Code segments are read-only to the task: write through the
    // linear address.
    super::poke(cpu, start, &code);
    ctx.eip = ip.wrapping_sub(2) as u32;
}
