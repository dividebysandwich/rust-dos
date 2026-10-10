//! INT 10h in protected mode: the video BIOS functions that take or give a
//! pointer, translated as Windows and HDPMI do. Borland's RTM programs
//! (Jazz Jackrabbit) set the DAC with AX=1012h from a buffer they address
//! by selector.
//!
//! What the BIOS reads is copied into the transfer buffer (dos.rs's) and
//! the call goes to real mode pointing there; what it wrote is copied back
//! to the client's buffer afterwards (`returned`). Other functions go to the
//! BIOS as they are.

use super::dos::{TLB_SIZE, copy, function, pointer};
use super::int31::{offset, segment_selector};
use super::{Context, EBP, EBX, ECX, EDI, EDX, F_VIDEO, Frame, RESULT_FLAGS, call_real, client, real_mode_handler, reflect, resume};
use crate::cpu::{Cpu, Seg};

/// The buffer of a call: the register with its offset, in ES, and how many
/// bytes the BIOS reads from it and writes to it.
struct Buffer {
    reg: usize,
    reads: u32,
    writes: u32,
}

/// The buffer the call in `ctx` (AH and AL) has, if it is one the host
/// translates.
fn buffer(ctx: &Context, ah: u8, al: u8) -> Option<Buffer> {
    let cx = ctx.reg16(ECX) as u32;
    let (reg, reads, writes) = match (ah, al) {
        // Set all palette registers and the overscan from ES:DX, and read
        // them into it.
        (0x10, 0x02) => (EDX, 17, 0),
        (0x10, 0x09) => (EDX, 0, 17),
        // Set CX DAC registers from ES:DX, and read them into it.
        (0x10, 0x12) => (EDX, cx * 3, 0),
        (0x10, 0x17) => (EDX, 0, cx * 3),
        // Load CX characters of BH bytes each from ES:BP into a font.
        (0x11, 0x00 | 0x10) => (EBP, cx * (ctx.reg16(EBX) >> 8) as u32, 0),
        // Write the CX characters at ES:BP, with an attribute after each
        // when AL bit 1 says.
        (0x13, _) => (EBP, if al & 2 != 0 { cx * 2 } else { cx }, 0),
        // The state and functionality information, 64 bytes into ES:DI.
        (0x1B, 0x00) => (EDI, 0, 64),
        _ => return None,
    };
    (reads.max(writes) <= TLB_SIZE).then_some(Buffer { reg, reads, writes })
}

/// INT 10h from the running client, in its context `ctx`.
pub(super) fn int10(cpu: &mut Cpu, ctx: Context) {
    let [ah, al] = function(&ctx);
    let c = client(cpu);
    let (bits32, seg) = (c.bits32, c.rm_stack);
    let tlb = (seg as u32) << 4;
    let mut rm = ctx;
    rm.seg = [seg, 0, seg, seg, 0, 0];
    let frame = Frame { kind: F_VIDEO, ctx, vector: 0x10, ..Frame::default() };
    match buffer(&ctx, ah, al) {
        Some(b) => {
            let Some(from) = pointer(cpu, &ctx, Seg::ES, b.reg, bits32) else {
                return reflect(cpu, 0x10, ctx);
            };
            copy(&mut cpu.bus, from, tlb, b.reads);
            rm.gpr[b.reg] = 0;
        }
        // The address of a font, in ES:BP: a segment, which becomes a
        // selector afterwards.
        _ if ah == 0x11 && al == 0x30 => {}
        _ => return reflect(cpu, 0x10, ctx),
    }
    call_real(cpu, frame, real_mode_handler(cpu, 0x10), rm);
}

/// The BIOS returned from the call of `frame` with the registers `rm`: what
/// it wrote copied to the client, which goes on with the BIOS's registers
/// but for the one that pointed to the transfer buffer.
pub(super) fn returned(cpu: &mut Cpu, frame: Frame, rm: &Context) {
    let caller = frame.ctx;
    let bits32 = client(cpu).bits32;
    let tlb = (client(cpu).rm_stack as u32) << 4;
    let [ah, al] = function(&caller);
    let mut ctx = caller;
    ctx.gpr = rm.gpr;
    ctx.gpr[super::ESP] = caller.gpr[super::ESP];
    ctx.eflags = (caller.eflags & !RESULT_FLAGS) | (rm.eflags & RESULT_FLAGS);
    if let Some(b) = buffer(&caller, ah, al) {
        ctx.gpr[b.reg] = caller.gpr[b.reg];
        if let Some(to) = pointer(cpu, &caller, Seg::ES, b.reg, bits32) {
            copy(&mut cpu.bus, tlb, to, b.writes);
        }
    } else if ah == 0x11 && al == 0x30 {
        let sel = segment_selector(cpu, rm.sel(Seg::ES)).unwrap_or(0);
        ctx.set_sel(Seg::ES, sel);
        if bits32 {
            ctx.gpr[EBP] = offset(rm, EBP, false);
        }
    }
    resume(cpu, &ctx);
}
