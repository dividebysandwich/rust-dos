//! VxDCall0 to VxDCall8: how Windows 98's KERNEL32 and the programs that
//! know its ordinals call a VxD's service from level 3. The first
//! argument is the service, the VxD's id in its high word and the
//! service's number in the low one. The service takes its own arguments
//! from the caller's stack, after the service number, and pops them: on
//! Windows 98 the call traps through INT 30h into VMM, and the service
//! works on the caller's registers and stack.
//!
//! The built-in Windows has the VWIN32 services that Win32 programs call
//! this way (`vwin32`): its version, the coprocessor VxD's version, and
//! the DPMI and debugger dispatch. CopyLok's protection uses the DPMI
//! dispatch to call real-mode interrupts with DPMI 0300h. A call of any
//! other service ends the program, as Windows 98 does on a call of a
//! service no VxD provides ("invalid VxD dynamic link call").

use crate::cpu::{Cpu, CpuFlags, Seg};
use crate::dpmi::Context;
use crate::dpmi::realcall::{self, Kind};
use crate::win32::realmode::{self, Frame};

/// VWIN32's device id.
const VWIN32: u32 = 0x002A;

/// The places of the registers in `Context::gpr`.
const EAX: usize = 0;
const ECX: usize = 1;
const EDX: usize = 2;
const EBX: usize = 3;
const ESP: usize = 4;
const ESI: usize = 6;
const EDI: usize = 7;

const CF: u32 = 0x0001;

/// The flags a DPMI function gives back to its caller.
const STATUS_FLAGS: [CpuFlags; 6] = [
    CpuFlags::CF,
    CpuFlags::PF,
    CpuFlags::AF,
    CpuFlags::ZF,
    CpuFlags::SF,
    CpuFlags::OF,
];

/// DPMI's error for a function the host doesn't have.
const UNSUPPORTED: u16 = 0x8001;

/// VxDCallN(service, N arguments).
pub fn vxd_call(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let service = args[0];
    // The stub pops the service and N dwords after it, which are the
    // service's own arguments either way.
    let popped = 4 * (args.len() as u32 - 1);
    if service >> 16 == VWIN32
        && let Some(result) = vwin32(cpu, service & 0xFFFF, popped)
    {
        return result;
    }
    crate::win32::fatal(
        cpu,
        &format!(
            "an invalid VxD call: VxD {:04X}h's service {:04X}h, which the \
             built-in Windows doesn't have",
            service >> 16,
            service & 0xFFFF
        ),
        None,
    );
    None
}

/// The dword `index` of the service's arguments on the caller's stack,
/// after the return address and the service.
fn argument(cpu: &mut Cpu, index: u32) -> u32 {
    let esp = cpu.esp();
    cpu.bus.guest_read_32(esp.wrapping_add(8 + 4 * index))
}

/// Pop the `count` dwords of the service's arguments. The stub pops
/// `popped` bytes of them; for the rest the return address moves up, so
/// that the stub's RET leaves the stack after them.
fn pop_arguments(cpu: &mut Cpu, count: u32, popped: u32) {
    let extra = (4 * count).saturating_sub(popped);
    if extra == 0 {
        return;
    }
    let esp = cpu.esp();
    let ret = cpu.bus.guest_read_32(esp);
    let esp = esp.wrapping_add(extra);
    cpu.bus.guest_write_32(esp, ret);
    if !cpu.bus.guest_faulted() {
        cpu.set_esp(esp);
    }
}

/// VWIN32's service `number`, or None for one it doesn't have. Inside is
/// what the function returns: EAX, or None when the caller doesn't go
/// on yet.
fn vwin32(cpu: &mut Cpu, number: u32, popped: u32) -> Option<Option<u32>> {
    let result = match number {
        // VWIN32's version: GetVersion's, major in the high byte, 4.10
        // on Windows 98.
        0x0000 => {
            let version = crate::win32::version::current(cpu);
            let v = crate::win32::version::info(version).get_version;
            Some((v & 0xFF) << 8 | (v >> 8) & 0xFF)
        }
        // The version of VMCPD, the coprocessor's VxD, which takes one
        // argument: 4.05 on Windows 98.
        0x0020 => {
            pop_arguments(cpu, 1, popped);
            Some(0x0405)
        }
        // Int31Dispatch(function, ECX): DPMI's INT 31h with AX the
        // function and ECX the argument, the caller's other registers as
        // they are.
        0x0029 => {
            let function = argument(cpu, 0);
            let parm = argument(cpu, 1);
            if cpu.bus.guest_faulted() {
                return Some(None);
            }
            int31_dispatch(cpu, function, parm, popped)
        }
        // Int41Dispatch(function): the kernel debugger's INT 41h. There
        // is no debugger, and the function comes back, as when none is
        // loaded.
        0x002A => {
            let function = argument(cpu, 0);
            pop_arguments(cpu, 1, popped);
            Some(function & 0xFFFF)
        }
        _ => return None,
    };
    if cpu.bus.guest_faulted() {
        return Some(None);
    }
    Some(result)
}

/// VWIN32's Int31Dispatch: the DPMI function `function` with ECX
/// `parm`, on the caller's other registers. The caller gets AX in EAX,
/// and the registers and flags the function leaves. The real-mode calls
/// (0300h to 0302h) go down to real mode, and the caller goes on once
/// the code returns (`call_returned`). The function's two arguments are
/// popped once nothing can fault any more: a page fault runs the
/// service again.
fn int31_dispatch(cpu: &mut Cpu, function: u32, parm: u32, popped: u32) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    ctx.gpr[EAX] = function;
    ctx.gpr[ECX] = parm;
    // The caller's stack once the call has returned, where 0300h's CX
    // words are: after the return address, the service and the
    // arguments.
    ctx.gpr[ESP] = ctx.gpr[ESP].wrapping_add(8 + popped.max(8));
    let call = Kind::of(function as u16).map(|kind| read_call(cpu, &ctx, kind));
    if cpu.bus.guest_faulted() {
        return None;
    }
    pop_arguments(cpu, 2, popped);
    ctx.gpr[ESP] = cpu.esp();
    let result = match call {
        Some(call) => {
            if call_real_mode(cpu, &ctx, call) {
                return None;
            }
            Ok(())
        }
        None => function_of(cpu, &mut ctx),
    };
    match result {
        Ok(()) => ctx.eflags &= !CF,
        Err(code) => {
            ctx.gpr[EAX] = ctx.gpr[EAX] & 0xFFFF_0000 | code as u32;
            ctx.eflags |= CF;
        }
    }
    give_back(cpu, &ctx);
    Some(ctx.gpr[EAX] & 0xFFFF)
}

/// Put the registers a DPMI function left in `ctx` in the processor:
/// the general registers but EAX, which the stub's return sets, and ESP,
/// and the arithmetic flags.
fn give_back(cpu: &mut Cpu, ctx: &Context) {
    let mut snapshot = cpu.snapshot();
    for reg in [ECX, EDX, EBX, 5, ESI, EDI] {
        snapshot.gpr[reg] = ctx.gpr[reg];
    }
    cpu.restore(&snapshot);
    for flag in STATUS_FLAGS {
        cpu.set_cpu_flag(flag, ctx.eflags & flag.bits() != 0);
    }
}

/// The DPMI functions but the real-mode calls that a Win32 program may
/// call through VWIN32: the real-mode vectors and the version. The others
/// fail as a host fails a function it doesn't have.
fn function_of(cpu: &mut Cpu, ctx: &mut Context) -> Result<(), u16> {
    let ax = ctx.gpr[EAX] as u16;
    let bl = ctx.gpr[EBX] as u8;
    let set_w = |ctx: &mut Context, reg: usize, value: u16| {
        ctx.gpr[reg] = ctx.gpr[reg] & 0xFFFF_0000 | value as u32;
    };
    match ax {
        0x0200 => {
            let v = cpu.bus.read_32(bl as usize * 4);
            set_w(ctx, ECX, (v >> 16) as u16);
            set_w(ctx, EDX, v as u16);
        }
        0x0201 => {
            let v = (ctx.gpr[ECX] & 0xFFFF) << 16 | ctx.gpr[EDX] & 0xFFFF;
            cpu.bus.write_32(bl as usize * 4, v);
        }
        0x0400 => {
            set_w(ctx, EAX, 0x005A);
            // A 32-bit host, real mode in virtual 8086 mode, virtual
            // memory, a Pentium; the PICs' bases.
            set_w(ctx, EBX, 0x0007);
            set_w(ctx, ECX, 0x0005);
            set_w(ctx, EDX, 0x0870);
        }
        _ => {
            cpu.bus.log_string(&format!(
                "[W32] VWIN32's Int31Dispatch: DPMI function {:04X}h isn't there",
                ax
            ));
            return Err(UNSUPPORTED);
        }
    }
    Ok(())
}

/// A real-mode call a Win32 program asked for: what kind, the call
/// structure's flat address and its registers, the words of the caller's
/// stack to copy and the code to call.
struct RealCall {
    kind: Kind,
    at: u32,
    rm: Context,
    params: Vec<u16>,
    target: u32,
}

/// Read what DPMI 0300h, 0301h or 0302h from a Win32 program calls: the
/// call structure at the flat address EDI (ES is the flat data
/// selector), and CX words of the caller's stack, as win16/dpmi.rs reads
/// them for a Win16 task. The caller checks for a page fault.
fn read_call(cpu: &mut Cpu, ctx: &Context, kind: Kind) -> RealCall {
    let at = ctx.gpr[EDI];
    let mut bytes = [0u8; realcall::SIZE];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    let params: Vec<u16> = (0..ctx.gpr[ECX] & 0xFFFF)
        .map(|i| cpu.bus.guest_read_16(ctx.gpr[ESP].wrapping_add(2 * i)))
        .collect();
    let rm = realcall::decode(&bytes);
    let target = match kind {
        Kind::Interrupt => cpu.bus.read_32((ctx.gpr[EBX] & 0xFF) as usize * 4),
        Kind::FarCall | Kind::IretCall => (rm.seg[Seg::CS as usize] as u32) << 16 | rm.eip & 0xFFFF,
    };
    RealCall {
        kind,
        at,
        rm,
        params,
        target,
    }
}

/// Call the real-mode code of `call`, with the caller's context `ctx`
/// kept for when it returns. True when the code went down to real mode;
/// false when there was nothing to call, a vector of 0:0.
fn call_real_mode(cpu: &mut Cpu, ctx: &Context, call: RealCall) -> bool {
    let RealCall {
        kind,
        at,
        mut rm,
        params,
        target,
    } = call;
    let bl = ctx.gpr[EBX] as u8;
    if crate::win32::trace::on(cpu) {
        let what = match kind {
            Kind::Interrupt => format!("INT {:02X}h", bl),
            _ => format!("{:04X}:{:04X}", target >> 16, target & 0xFFFF),
        };
        cpu.bus.log_string(&format!(
            "[W32] VWIN32 DPMI {:04X}h {}: AX={:04X} BX={:04X} CX={:04X} DX={:04X} \
             SI={:04X} DI={:04X} DS={:04X} ES={:04X}",
            ctx.gpr[EAX] & 0xFFFF,
            what,
            rm.gpr[EAX] & 0xFFFF,
            rm.gpr[EBX] & 0xFFFF,
            rm.gpr[ECX] & 0xFFFF,
            rm.gpr[EDX] & 0xFFFF,
            rm.gpr[ESI] & 0xFFFF,
            rm.gpr[EDI] & 0xFFFF,
            rm.seg[Seg::DS as usize],
            rm.seg[Seg::ES as usize],
        ));
    }
    if target == 0 {
        return false;
    }
    let (ss, sp) = realmode::host_stack(cpu);
    let (ss, sp) = realcall::stack(&rm, (ss, sp as u32));
    let words = kind.frame(rm.eflags, &params, realmode::RETURN);
    rm.seg[Seg::CS as usize] = (target >> 16) as u16;
    rm.eip = target & 0xFFFF;
    rm.seg[Seg::SS as usize] = ss;
    rm.gpr[ESP] = sp;
    rm.eflags = kind.entry_flags(rm.eflags);
    // The caller goes on after the stub with CF clear and AX the
    // function, as a DPMI host leaves them.
    let mut after = *ctx;
    after.eflags &= !CF;
    after.gpr[EAX] &= 0xFFFF;
    let frame = Frame {
        kind: realmode::Kind::Vxd,
        ctx: after,
        at,
    };
    realmode::call(cpu, frame, &rm, &words);
    true
}

/// Real-mode code a Win32 program called through VWIN32's Int31Dispatch
/// returned, in the real-mode context `rm`: the call structure gets the
/// registers, flags and segments it left, and the program goes on after
/// the stub.
pub fn call_returned(cpu: &mut Cpu, frame: Frame, rm: &Context) {
    // Through the process's memory, which finds the structure's page in
    // the page file too, should it have gone out while the code ran.
    let bytes = crate::win32::process_bytes(cpu, frame.at, realcall::SIZE);
    if let Some(mut bytes) = bytes.and_then(|b| <[u8; realcall::SIZE]>::try_from(b).ok()) {
        realcall::encode(rm, &mut bytes, false);
        crate::win32::write_process_bytes(cpu, frame.at, &bytes);
    }
    crate::win32::system::enter_user(cpu, &frame.ctx);
    if crate::win32::trace::on(cpu) {
        cpu.bus.log_string(&format!(
            "[W32] VWIN32 DPMI returned: AX={:04X} BX={:04X} CX={:04X} DX={:04X} flags={:04X}",
            rm.gpr[EAX] & 0xFFFF,
            rm.gpr[EBX] & 0xFFFF,
            rm.gpr[ECX] & 0xFFFF,
            rm.gpr[EDX] & 0xFFFF,
            rm.eflags & 0xFFFF,
        ));
    }
}
