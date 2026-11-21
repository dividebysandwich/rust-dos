//! The built-in virtual-8086 monitor: with expanded memory on, the
//! built-in DOS runs in virtual-8086 mode under it, as DOS runs under
//! EMM386 on a 386. Programs that ask the processor see what they see
//! there: SMSW and MOV from CR0 give PE set (and PG, as EMM386 pages),
//! and VCPI (vcpi.rs) lets DOS extenders switch to protected mode and
//! back through the monitor.
//!
//! The monitor runs DOS with IOPL 3, as EMM386 does: CLI, STI, PUSHF,
//! POPF and IRET run as they do in real mode, and so does INT n, which
//! the processor takes to the monitor's IDT. Its I/O permission bitmap
//! leaves every port open. The monitor's level 0 is Rust code: the
//! processor hands it every interrupt and exception from virtual-8086
//! mode where its IDT would take it (`Cpu::deliver_interrupt`), and it
//! reflects them to the real-mode vector table as a real-mode interrupt
//! would go there, or emulates the privileged instruction that raised a
//! #GP. Real-mode code thus runs on the same handlers, with the same
//! frames, as without the monitor; only what tells the modes apart
//! differs.
//!
//! The monitor's tables (GDT, IDT and TSS with its bitmap) are in the
//! BIOS ROM at F000:4000, below the first megabyte, so the A20 gate,
//! which DOS programs keep on closing to have addresses wrap at 1 MB as
//! on an 8086, doesn't move them. Paging stays off: the page frame keeps
//! its copies (ems.rs), and the A20 gate is the machine's own. A VCPI
//! client in protected mode gets the gate open, as the specification
//! wants, and the one DOS had back when it returns.
//!
//! The DPMI host (dpmi/) and the built-in Windows (win32/, win16/) leave
//! virtual-8086 mode for their protected mode as they leave real mode,
//! and come back to it under the monitor (`enter`) where they went back
//! to real mode.

use crate::bus::Bus;
use crate::cpu::{
    CR0_PE, CR0_PG, Cpu, CpuFlags, CpuResult, CpuState, DescTable, IntSource, Seg, SegCache,
};
use iced_x86::{Code, Decoder, DecoderOptions, Mnemonic, OpKind, Register};

/// The monitor's part of the ROM, as linear (and physical) addresses.
const ROM: u32 = 0xF0000;
const GDT: u32 = 0xF4000;
/// The VCPI's protected-mode entry point (vcpi.rs).
pub const VCPI_ENTRY: u16 = 0x4040;
/// Where the IDT's gates lead: an IRETD the processor never reaches, as
/// the monitor takes every event before it reads a gate.
const GATE: u16 = 0x4048;
const IDT: u32 = 0xF4100;
const TSS: u32 = 0xF4900;
/// The TSS, its I/O permission bitmap of 64K ports and the byte of 1s
/// the bitmap ends with.
const IOPB: u32 = 0x68;
const TSS_LIMIT: u32 = IOPB + 0x2000;
const GDT_ENTRIES: usize = 4;
/// The GDT's selectors: the monitor's code (its ROM, 32-bit), a flat
/// data segment for its level 0 stack, and its TSS.
pub const CODE0: u16 = 0x08;
pub const DATA0: u16 = 0x10;
const TSS_SEL: u16 = 0x18;
/// The TSS descriptor's type: a busy 32-bit TSS, as the task register
/// has it.
const TSS_BUSY: u8 = 0x8B;

/// A segment descriptor.
fn descriptor(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
    crate::dpmi::descriptor(base, limit, access, flags)
}

/// Write the monitor's tables into the ROM: written each time DOS starts
/// (`load_shell`), which a state saved before the monitor was there needs.
pub fn install_rom(bus: &mut Bus) {
    let gdt: [u64; GDT_ENTRIES] = [
        0,
        descriptor(ROM, 0xFFFF, 0x9B, 0x4),
        descriptor(0, 0xFFFF_FFFF, 0x93, 0x4),
        descriptor(TSS, TSS_LIMIT, TSS_BUSY, 0),
    ];
    let bytes: Vec<u8> = gdt.iter().flat_map(|d| d.to_le_bytes()).collect();
    bus.write_rom(GDT as usize, &bytes);
    bus.write_rom((ROM + GATE as u32) as usize, &[0xCF]);
    let gate = |offset: u16| -> u64 { offset as u64 | (CODE0 as u64) << 16 | 0xEEu64 << 40 };
    let idt: Vec<u8> = (0..256).flat_map(|_| gate(GATE).to_le_bytes()).collect();
    bus.write_rom(IDT as usize, &idt);
    let mut tss = vec![0u8; (TSS_LIMIT + 1) as usize];
    tss[8..10].copy_from_slice(&DATA0.to_le_bytes());
    tss[0x66..0x68].copy_from_slice(&(IOPB as u16).to_le_bytes());
    tss[TSS_LIMIT as usize] = 0xFF;
    bus.write_rom(TSS as usize, &tss);
    crate::vcpi::install_entry(bus);
}

/// Whether DOS runs under the monitor, from the prompt on: with expanded
/// memory, and not on a system booted from a disk, which has its own
/// memory managers.
pub fn installed(bus: &Bus) -> bool {
    bus.ems.is_some() && bus.boot.is_none()
}

/// Whether the processor runs virtual-8086 code under the monitor: its
/// IDT is loaded. A VCPI client's own V86 task has the client's.
#[inline(always)]
pub fn owns(cpu: &Cpu) -> bool {
    cpu.idtr.base == IDT && cpu.v86() && cpu.bus.boot.is_none()
}

/// The CR0 programs read under the monitor: the processor's, with paging
/// on as EMM386 has it.
pub fn cr0(cpu: &Cpu) -> u32 {
    cpu.cr0 | CR0_PG
}

/// Go on in virtual-8086 mode under the monitor, from real mode or from
/// a protected-mode host, with the segment registers' selectors as
/// segments and the other registers as they are.
pub fn enter(cpu: &mut Cpu) {
    let segs = Seg::ALL.map(|seg| cpu.seg_cache(seg).selector);
    enter_with(cpu, segs);
}

/// `enter` with the segment registers `segs` (in `Seg::ALL`'s order).
pub fn enter_with(cpu: &mut Cpu, segs: [u16; 6]) {
    if cpu.cr0 & CR0_PG != 0 {
        cpu.tlb.flush();
    }
    cpu.cr0 = (cpu.cr0 | CR0_PE) & !CR0_PG;
    cpu.gdtr = DescTable {
        base: GDT,
        limit: (GDT_ENTRIES * 8 - 1) as u16,
    };
    cpu.idtr = DescTable {
        base: IDT,
        limit: 0x7FF,
    };
    cpu.ldtr = SegCache::null(0);
    cpu.tr = SegCache::from_descriptor(TSS_SEL, TSS, TSS_LIMIT, TSS_BUSY as u16);
    cpu.set_cpu_flag(CpuFlags::NT | CpuFlags::RF, false);
    cpu.set_cpu_flag(CpuFlags::VM | CpuFlags::IOPL, true);
    cpu.cpl = 3;
    for (seg, selector) in Seg::ALL.into_iter().zip(segs) {
        cpu.load_seg_v86(seg, selector);
    }
}

/// Back to real mode, the segment registers' selectors as segments: for
/// expanded memory turned off at the prompt.
pub fn leave(cpu: &mut Cpu) {
    cpu.cr0 &= !(CR0_PE | CR0_PG);
    cpu.gdtr = DescTable {
        base: 0,
        limit: 0xFFFF,
    };
    cpu.idtr = DescTable {
        base: 0,
        limit: 0x3FF,
    };
    cpu.ldtr = SegCache::null(0);
    cpu.tr = SegCache::null(0);
    cpu.set_cpu_flag(
        CpuFlags::VM | CpuFlags::IOPL | CpuFlags::NT | CpuFlags::RF,
        false,
    );
    cpu.cpl = 0;
    for seg in Seg::ALL {
        let selector = cpu.seg_cache(seg).selector;
        cpu.set_seg_cache(seg, SegCache::real(selector));
    }
}

/// Expanded memory was turned on or off at the prompt: DOS goes on under
/// the monitor, or in real mode, from here. A program that runs keeps
/// the mode it has until it ends.
pub fn sync(cpu: &mut Cpu) {
    if !cpu.shell_idle() {
        return;
    }
    if installed(&cpu.bus) && !cpu.pe() {
        install_rom(&mut cpu.bus);
        enter(cpu);
    } else if !installed(&cpu.bus) && owns(cpu) {
        leave(cpu);
    }
}

/// An interrupt or exception in virtual-8086 mode under the monitor, which
/// the processor hands over where its IDT would take it to level 0.
pub fn event(cpu: &mut Cpu, vector: u8, source: IntSource, error: Option<u32>) -> CpuResult {
    match source {
        IntSource::External | IntSource::Software => reflect(cpu, vector),
        IntSource::Exception if vector == 13 && error == Some(0) => general_protection(cpu),
        // The others go to the program's handlers as in real mode:
        // divide errors, single steps and breakpoints, #UD, the FPU's.
        IntSource::Exception => reflect(cpu, vector),
    }
}

/// Reflect interrupt `vector` to the handler in the real-mode vector
/// table, as a real-mode interrupt goes there: FLAGS, CS and IP on the
/// stack, interrupts and single steps off.
fn reflect(cpu: &mut Cpu, vector: u8) -> CpuResult {
    let entry = vector as u32 * 4;
    let ip = cpu.read_linear_u16(entry);
    let cs = cpu.read_linear_u16(entry + 2);
    let flags = cpu.flags16();
    let (old_cs, old_ip) = (cpu.cs(), cpu.ip());
    cpu.push_sized(2, flags as u32)?;
    cpu.push_sized(2, old_cs as u32)?;
    cpu.push_sized(2, old_ip as u32)?;
    cpu.set_cpu_flag(CpuFlags::IF | CpuFlags::TF | CpuFlags::AC, false);
    cpu.load_seg_v86(Seg::CS, cs);
    cpu.set_ip(ip);
    Ok(())
}

/// What the monitor does about an instruction that raised #GP(0).
enum Emulated {
    /// It ran as the monitor emulates it; the program goes on after it.
    Done,
    /// It is a privileged instruction the monitor doesn't let a program
    /// run, which stops the program.
    Refused(&'static str),
    /// Not a privileged instruction: a segment limit or the like, which
    /// goes to INT 0Dh as it does in real mode.
    Reflect,
}

/// #GP(0) in virtual-8086 mode: the privileged instructions EMM386
/// emulates, or the program's handler.
fn general_protection(cpu: &mut Cpu) -> CpuResult {
    // Every IOPL-sensitive instruction runs at IOPL 3. A program can't
    // lower it (POPF and IRET leave IOPL alone in virtual-8086 mode), so
    // this is a service that left other flags: on again, and the
    // instruction runs again.
    if cpu.iopl() < 3 {
        cpu.set_cpu_flag(CpuFlags::IOPL, true);
        return Ok(());
    }
    let (cs, ip) = (cpu.cs(), cpu.ip());
    let mut bytes = [0u8; 15];
    for (i, b) in bytes.iter_mut().enumerate() {
        let at = ((cs as u32) << 4).wrapping_add(ip.wrapping_add(i as u16) as u32);
        *b = cpu.bus.read_8((at & cpu.bus.a20_mask()) as usize);
    }
    let instr = Decoder::with_ip(16, &bytes, ip as u64, DecoderOptions::NONE).decode();
    match emulate(cpu, &instr) {
        Emulated::Done => {
            cpu.set_ip(ip.wrapping_add(instr.len() as u16));
            Ok(())
        }
        Emulated::Reflect => reflect(cpu, 13),
        Emulated::Refused(what) => {
            refuse(cpu, what, cs, ip);
            Ok(())
        }
    }
}

/// Emulate the privileged instruction `instr`, as EMM386 does.
fn emulate(cpu: &mut Cpu, instr: &iced_x86::Instruction) -> Emulated {
    use crate::cpu::{CR0_AM, CR0_CD, CR0_EM, CR0_MP, CR0_NE, CR0_NW, CR0_TS, CR0_WP};
    // CR0's bits a program may change under the monitor: the FPU's and
    // the cache's. PE and PG stay on.
    const FREE: u32 = CR0_MP | CR0_EM | CR0_TS | CR0_NE | CR0_WP | CR0_AM | CR0_NW | CR0_CD;
    let pentium = cpu.model >= crate::cpu::CpuModel::Pentium;
    match instr.code() {
        Code::Mov_r32_cr => {
            let value = match instr.op1_register() {
                Register::CR0 => cr0(cpu),
                Register::CR2 => cpu.cr2,
                Register::CR3 => cpu.cr3,
                Register::CR4 if pentium => cpu.cr4,
                _ => return Emulated::Refused("a control register it doesn't have"),
            };
            cpu.set_reg(instr.op0_register(), value);
            Emulated::Done
        }
        Code::Mov_cr_r32 => {
            let value = cpu.reg(instr.op1_register());
            match instr.op0_register() {
                Register::CR0 if value & (CR0_PE | CR0_PG) == CR0_PE | CR0_PG => {
                    cpu.cr0 = cpu.cr0 & !FREE | value & FREE;
                    Emulated::Done
                }
                Register::CR0 => Emulated::Refused("a switch out of protected mode"),
                Register::CR2 => {
                    cpu.cr2 = value;
                    Emulated::Done
                }
                _ => Emulated::Refused("a load of a control register"),
            }
        }
        Code::Mov_r32_dr => {
            let n = instr.op1_register() as usize - Register::DR0 as usize;
            let value = cpu.dr[n & 7];
            cpu.set_reg(instr.op0_register(), value);
            Emulated::Done
        }
        Code::Mov_dr_r32 => {
            let n = instr.op0_register() as usize - Register::DR0 as usize;
            cpu.dr[n & 7] = cpu.reg(instr.op1_register());
            Emulated::Done
        }
        _ => match instr.mnemonic() {
            // The processor waits for an interrupt, which the monitor
            // reflects; its HLT idles the machine as real mode's does.
            Mnemonic::Hlt => {
                cpu.state = CpuState::Halted;
                Emulated::Done
            }
            Mnemonic::Clts => {
                cpu.cr0 &= !CR0_TS;
                Emulated::Done
            }
            Mnemonic::Lmsw => {
                let value = if instr.op0_kind() == OpKind::Register {
                    cpu.reg(instr.op0_register())
                } else {
                    match crate::instructions::operand::read_op(cpu, instr, 0, 2) {
                        Ok(value) => value,
                        Err(_) => return Emulated::Reflect,
                    }
                };
                // LMSW can't clear PE; MP, EM and TS are loaded.
                let bits = CR0_MP | CR0_EM | CR0_TS;
                cpu.cr0 = cpu.cr0 & !bits | value & bits;
                Emulated::Done
            }
            Mnemonic::Invd | Mnemonic::Wbinvd | Mnemonic::Invlpg => Emulated::Done,
            Mnemonic::Lgdt | Mnemonic::Lidt | Mnemonic::Lldt | Mnemonic::Ltr => {
                Emulated::Refused("a load of a descriptor table")
            }
            Mnemonic::Mov if matches!(instr.code(), Code::Mov_r32_tr | Code::Mov_tr_r32) => {
                Emulated::Refused("a test register")
            }
            Mnemonic::Rdmsr | Mnemonic::Wrmsr => Emulated::Refused("a model-specific register"),
            _ => Emulated::Reflect,
        },
    }
}

/// A program ran a privileged instruction the monitor doesn't let it run,
/// as one that wants real mode does (a switch to protected mode of its
/// own, "unreal" mode): it ends, with a message, as EMM386 stops it.
fn refuse(cpu: &mut Cpu, what: &str, cs: u16, ip: u16) {
    cpu.bus.log_string(&format!(
        "[V86] {what} at {cs:04X}:{ip:04X} in virtual-8086 mode: the program ends (EMS off runs it in real mode)"
    ));
    crate::video::print_string(
        cpu,
        &format!(
            "\r\nThe program ran a privileged instruction at {cs:04X}:{ip:04X}, which it can't\r\n\
             run in virtual-8086 mode. Turn EMS off (ems=false) to run it in real mode.\r\n"
        ),
    );
    cpu.state = CpuState::RebootShell;
}
